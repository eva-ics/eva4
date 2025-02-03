use serde::Serialize;
use std::ffi::OsStr;
use std::os::fd::FromRawFd as _;
use std::path::Path;
use std::process::Stdio;
use termios::{Termios, TCSANOW};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    Pid(u32),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Error(String),
    Terminated(Option<i32>),
}

const BUF_SIZE: usize = 8192;

fn create_pty(
    term_size: (usize, usize),
    // todo fix certain program (mc) after maybe remove
    ce: bool,
) -> Result<(File, Stdio), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let ws = libc::winsize {
        ws_row: u16::try_from(term_size.1)?,
        ws_col: u16::try_from(term_size.0)?,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

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

    if unsafe { libc::ioctl(pty_master, libc::TIOCSWINSZ, &ws) } != 0 {
        return Err("pty ioctl".into());
    }

    if !ce {
        let mut termios = Termios::from_fd(pty_master)?;
        termios.c_lflag &= !(termios::ICANON | termios::ECHO);
        termios::tcsetattr(pty_master, TCSANOW, &termios)?;

        let mut termios = Termios::from_fd(pty_slave)?;
        termios.c_lflag &= !(termios::ICANON | termios::ECHO);
        termios::tcsetattr(pty_slave, TCSANOW, &termios)?;
    }

    let f = unsafe { File::from_raw_fd(pty_master) };
    let stdio = unsafe { Stdio::from_raw_fd(pty_slave) };

    Ok((f, stdio))
}

#[derive(Default)]
pub struct Process {
    pid: Option<u32>,
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            #[allow(clippy::cast_possible_wrap)]
            bmart::process::kill_pstree_sync(pid, true);
        }
    }
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
        stdin_rx: async_channel::Receiver<Option<Vec<u8>>>,
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
                stdin_rx,
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

    #[allow(clippy::too_many_arguments)]
    async fn run_subprocess<C, A, D, S, ENV, EnvK, EnvV>(
        &mut self,
        command: C,
        args: A,
        env: ENV,
        work_dir: D,
        terminal_name: &str,
        terminal_size: (usize, usize),
        out_tx: async_channel::Sender<Output>,
        stdin_rx: async_channel::Receiver<Option<Vec<u8>>>,
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
        let (mut stdin, stdin_stdio) = create_pty(terminal_size, true)?;
        let (mut stdout, stdout_stdio) = create_pty(terminal_size, true)?;
        let (mut stderr, stderr_stdio) = create_pty(terminal_size, true)?;

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
            while let Ok(data) = stdin_rx.recv().await {
                let Some(data) = data else {
                    break;
                };
                if stdin.write_all(&data).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        let result = child.wait().await?;
        tokio::try_join!(out, err).ok();

        let exit_code = result.code();

        Ok(exit_code)
    }
}
