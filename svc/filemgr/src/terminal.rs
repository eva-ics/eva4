use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::fd::FromRawFd as _;
use std::path::Path;
use std::process::Stdio;
use termios::{Termios, TCSANOW};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tracing::{trace, warn};

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    Pid(u32),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
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

fn ansi_to_readable(input: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(input.len());
    for b in input {
        match b {
            0x1b => result.push(b'^'),
            0x7f => result.extend("\x1b[1D \x1b[1D".as_bytes()),
            0x15 => result.extend("\x1b[2K\x1b[0G".as_bytes()),
            b'\r' => result.extend([b'\r', b'\n']),
            _ => result.push(*b),
        }
    }
    result
}

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

fn create_pty(
    term_size: (usize, usize),
) -> Result<(File, Stdio, i32), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pty_master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };

    if pty_master < 0 {
        return Err("pty posix_openpt".into());
    }
    if unsafe { libc::grantpt(pty_master) } != 0 {
        return Err("pty grantpt".into());
    }
    if unsafe { libc::unlockpt(pty_master) } != 0 {
        return Err("pty unlockpt".into());
    }

    let pty_slave_name = unsafe { libc::ptsname(pty_master) };

    if pty_slave_name.is_null() {
        return Err("pty ptsname".into());
    }

    let pty_slave = unsafe { libc::open(pty_slave_name, libc::O_RDWR) };

    if pty_slave < 0 {
        return Err("pty open".into());
    }

    set_term_size(pty_master, term_size)?;

    let mut termios = Termios::from_fd(pty_master)?;
    termios.c_lflag |= termios::ICANON;
    termios::tcsetattr(pty_master, TCSANOW, &termios)?;

    let mut termios = Termios::from_fd(pty_slave)?;
    termios.c_lflag |= termios::ICANON;
    termios::tcsetattr(pty_slave, TCSANOW, &termios)?;

    let f = unsafe { File::from_raw_fd(pty_master) };
    let stdio = unsafe { Stdio::from_raw_fd(pty_slave) };

    Ok((f, stdio, pty_master))
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
        let (mut stdin, stdin_stdio, stdin_master) = create_pty(terminal_size)?;
        let (mut stdout, stdout_stdio, stdout_master) = create_pty(terminal_size)?;
        let (mut stderr, stderr_stdio, _stderr_master) = create_pty(terminal_size)?;

        let mut child = Command::new(command)
            .current_dir(work_dir)
            .args(args)
            .envs(env)
            .env("COLUMNS", terminal_size.0.to_string())
            .env("LINES", terminal_size.1.to_string())
            .env("TERM", terminal_name)
            .stdin(stdin_stdio)
            .stdout(stdout_stdio)
            .stderr(stderr_stdio)
            .spawn()?;

        let pid = child.id().ok_or("unable to get child pid")?;
        out_tx.send(Output::Pid(pid)).await?;
        self.pid = Some(pid);

        let tx_stdout = out_tx.clone();
        let tx_stderr = out_tx.clone();

        let tx_echo = out_tx.clone();

        let out = tokio::spawn(async move {
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

        let err = tokio::spawn(async move {
            let mut buf = [0u8; BUF_SIZE];
            while let Ok(b) = stderr.read(&mut buf).await {
                if b == 0 {
                    break;
                }
                if tx_stderr
                    .send(Output::Stderr(buf[..b].to_vec()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            while let Ok(input) = in_rx.recv().await {
                let mut data = match input {
                    Input::Data(d) => d,
                    Input::Resize(size) => {
                        set_term_size(stdin_master, size).ok();
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
                let Ok(mut termios_stdin) = Termios::from_fd(stdin_master) else {
                    warn!(fd = stdin_master, "unable to get stdin termios");
                    continue;
                };
                let Ok(termios_stdout) = Termios::from_fd(stdout_master) else {
                    warn!(fd = stdout_master, "unable to get stdout termios");
                    continue;
                };
                let input_mode = termios_stdin.c_lflag & termios::ECHO != 0
                    && termios_stdout.c_lflag & termios::ECHO != 0;
                if input_mode {
                    // set canonical mode
                    termios_stdin.c_lflag |= termios::ICANON;
                    termios_stdin.c_lflag |= termios::ECHO;
                    termios_stdin.c_iflag |= termios::ICRNL;
                    if let Err(error) = termios::tcsetattr(stdin_master, TCSANOW, &termios_stdin) {
                        warn!(fd = stdin_master, ?error, "unable to set stdin termios");
                        continue;
                    }
                    trace!("input mode");
                } else {
                    // set non-canonical mode
                    termios_stdin.c_lflag &= !termios::ICANON;
                    termios_stdin.c_iflag &= !termios::ICRNL;
                    if data == [0x0a] {
                        data[0] = 0x0d;
                    }
                    if let Err(e) = termios::tcsetattr(stdin_master, TCSANOW, &termios_stdin) {
                        warn!(fd = stdin_master, ?e, "unable to set stdin termios");
                        continue;
                    }
                    trace!("non-input mode");
                }
                if stdin.write_all(&data).await.is_err() {
                    break;
                }
                if input_mode {
                    let data = ansi_to_readable(&data);
                    tx_echo.send(Output::Stdout(data)).await.ok();
                }
            }
        });

        let result = child.wait().await?;
        tokio::try_join!(out, err).ok();

        let exit_code = result.code();

        Ok(exit_code)
    }
}
