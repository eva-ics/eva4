use libc::TIOCSCTTY;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::fd::AsRawFd as _;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    Pid(u32),
    Stdout(Vec<u8>),
    Error(String),
    Terminated(Option<i32>),
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
pub enum Input {
    Data(Vec<u8>),
    Resize((usize, usize)),
    Terminate,
}

const BUF_SIZE: usize = 8192;

fn set_term_size(
    fd: i32,
    term_size: (usize, usize),
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let ws = libc::winsize {
        ws_row: u16::try_from(term_size.1)?,
        ws_col: u16::try_from(term_size.0)?,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) } != 0 {
        return Err("ioctl".into());
    }

    Ok(())
}

#[derive(Default)]
pub struct Process {
    pid: Option<u32>,
}

impl Process {
    #[allow(clippy::too_many_arguments)]
    pub async fn run<C, A, D, S, ENV, EnvK, EnvV>(
        &mut self,
        command: C,
        args: A,
        env: ENV,
        work_dir: D,
        terminal_name: &str,
        terminal_size: (usize, usize),
        out_tx: async_channel::Sender<Output>,
        in_rx: async_channel::Receiver<Input>,
    ) where
        C: AsRef<OsStr>,
        D: AsRef<Path>,
        A: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
        ENV: IntoIterator<Item = (EnvK, EnvV)>,
        EnvK: AsRef<OsStr>,
        EnvV: AsRef<OsStr>,
    {
        match self
            .run_subprocess(
                command,
                args,
                env,
                work_dir,
                terminal_name,
                terminal_size,
                out_tx.clone(),
                in_rx,
            )
            .await
        {
            Ok(v) => {
                out_tx.send(Output::Terminated(v)).await.ok();
            }
            Err(e) => {
                out_tx.send(Output::Error(e.to_string())).await.ok();
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_subprocess<C, A, D, S, ENV, EnvK, EnvV>(
        &mut self,
        command: C,
        args: A,
        env: ENV,
        work_dir: D,
        terminal_name: &str,
        terminal_size: (usize, usize),
        out_tx: async_channel::Sender<Output>,
        in_rx: async_channel::Receiver<Input>,
    ) -> Result<Option<i32>, Box<dyn std::error::Error + Send + Sync + 'static>>
    where
        C: AsRef<OsStr>,
        D: AsRef<Path>,
        A: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
        ENV: IntoIterator<Item = (EnvK, EnvV)>,
        EnvK: AsRef<OsStr>,
        EnvV: AsRef<OsStr>,
    {
        let win_size = rustix::termios::Winsize {
            ws_col: terminal_size.0.try_into()?,
            ws_row: terminal_size.1.try_into()?,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = rustix_openpty::openpty(None, Some(&win_size))?;
        let (master, slave) = (pty.controller, pty.user);

        let master_fd = master.as_raw_fd();
        let slave_fd = slave.as_raw_fd();

        if let Ok(mut termios) = rustix::termios::tcgetattr(&master) {
            // Set character encoding to UTF-8.
            termios
                .input_modes
                .set(rustix::termios::InputModes::IUTF8, true);
            let _ = rustix::termios::tcsetattr(
                &master,
                rustix::termios::OptionalActions::Now,
                &termios,
            );
        }

        let mut builder = Command::new(command);

        builder
            .current_dir(work_dir)
            .args(args)
            .envs(env)
            .env("COLUMNS", terminal_size.0.to_string())
            .env("LINES", terminal_size.1.to_string())
            .env("TERM", terminal_name);

        builder.stdin(slave.try_clone()?);
        builder.stderr(slave.try_clone()?);
        builder.stdout(slave);

        unsafe {
            builder.pre_exec(move || {
                let err = libc::setsid();
                if err == -1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Failed to set session id",
                    ));
                }

                let res = libc::ioctl(slave_fd, TIOCSCTTY as _, 0);
                if res == -1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to set controlling terminal: {}", res),
                    ));
                }

                libc::close(slave_fd);
                libc::close(master_fd);

                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                libc::signal(libc::SIGALRM, libc::SIG_DFL);

                Ok(())
            });
        }

        let mut child = builder.spawn()?;

        //unsafe {
        //set_nonblocking(master_fd);
        //}

        let pid = child.id().ok_or("unable to get child pid")?;

        out_tx.send(Output::Pid(pid)).await?;
        self.pid = Some(pid);

        let mut stdout = File::from_std(std::fs::File::from(master));

        let mut stdin = stdout.try_clone().await?;

        let tx_stdout = out_tx.clone();
        //let tx_stderr = out_tx.clone();

        //let tx_echo = out_tx.clone();

        let fut_out = tokio::spawn(async move {
            let mut buf = [0u8; BUF_SIZE];
            while let Ok(b) = stdout.read(&mut buf).await {
                if b == 0 {
                    break;
                }
                if tx_stdout
                    .send(Output::Stdout(buf[..b].to_vec()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let fut_in = tokio::spawn(async move {
            while let Ok(input) = in_rx.recv().await {
                let mut data = match input {
                    Input::Data(d) => d,
                    Input::Resize(size) => {
                        set_term_size(stdin.as_raw_fd(), size).ok();
                        bmart::process::kill_pstree_with_signal(
                            pid,
                            bmart::process::Signal::SIGWINCH,
                            true,
                        );
                        continue;
                    }
                    Input::Terminate => {
                        break;
                    }
                };
                if data == [0x0a] {
                    data[0] = 0x0d;
                }
                if stdin.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        let result = child.wait().await?;

        fut_out.abort();
        fut_in.abort();

        let exit_code = result.code();

        Ok(exit_code)
    }
}
