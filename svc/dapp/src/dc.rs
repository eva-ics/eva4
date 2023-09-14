use bmart::process::CommandResult;
use eva_common::{EResult, Error};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinHandle;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default, Deserialize, Serialize)]
pub enum Version {
    V1,
    #[default]
    V2,
}

impl Version {
    pub fn cmd_up(self) -> &'static str {
        match self {
            Version::V1 => "docker-compose up",
            Version::V2 => "docker compose up --remove-orphans --quiet-pull",
        }
    }
    pub fn cmd_down(self) -> &'static str {
        match self {
            Version::V1 => "docker-compose down",
            Version::V2 => "docker compose down",
        }
    }
}

impl TryFrom<u16> for Version {
    type Error = Error;
    fn try_from(v: u16) -> EResult<Self> {
        match v {
            1 => Ok(Version::V1),
            2 => Ok(Version::V2),
            _ => Err(Error::unsupported("unsupported docker compose version")),
        }
    }
}

pub type IoReceiver = async_channel::Receiver<String>;

pub struct App {
    path: String,
    version: Version,
    timeout: Duration,
    pid: Option<u32>,
    fut_stdout: Option<JoinHandle<()>>,
    fut_stderr: Option<JoinHandle<()>>,
}

impl App {
    pub fn new(path: &str, version: Version, timeout: Duration) -> Self {
        Self {
            path: path.to_owned(),
            version,
            timeout,
            pid: None,
            fut_stdout: None,
            fut_stderr: None,
        }
    }
    pub async fn start(&mut self) -> EResult<(IoReceiver, IoReceiver)> {
        if self.pid.is_some() {
            return Err(Error::busy("app is already started"));
        }
        let _ = self.compose_down().await;
        let cmd = format!("chdir \"{}\" && {}", self.path, self.version.cmd_up());
        let mut child = tokio::process::Command::new("sh")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .args(["-c", &cmd])
            .spawn()?;
        let Some(stdout) = child.stdout.take() else {
            return Err(Error::io("Unable to create stdout reader"));
        };
        let mut stdout_reader = BufReader::new(stdout).lines();
        let Some(stderr) = child.stderr.take() else {
            return Err(Error::io("Unable to create stderr reader"));
        };
        let Some(ppid) = child.id() else {
            return Err(Error::io("Unable to take process id"));
        };
        let mut stderr_reader = BufReader::new(stderr).lines();
        let (tx_out, rx_out) = async_channel::bounded(1024);
        let (tx_err, rx_err) = async_channel::bounded(1024);
        let fut_stdout = tokio::spawn(async move {
            while let Some(line) = match stdout_reader.next_line().await {
                Ok(v) => v,
                Err(e) => {
                    let _r = tx_out.send(e.to_string()).await;
                    return;
                }
            } {
                let _r = tx_out.send(line).await;
            }
        });
        let fut_stderr = tokio::spawn(async move {
            while let Some(line) = match stderr_reader.next_line().await {
                Ok(v) => v,
                Err(e) => {
                    let _r = tx_err.send(e.to_string()).await;
                    return;
                }
            } {
                let _r = tx_err.send(line).await;
            }
        });
        self.fut_stdout.replace(fut_stdout);
        self.fut_stderr.replace(fut_stderr);
        self.pid.replace(ppid);
        Ok((rx_out, rx_err))
    }
    async fn compose_down(&self) -> EResult<CommandResult> {
        let cmd = format!("chdir \"{}\" && {}", self.path, self.version.cmd_down());
        bmart::process::command(
            "sh",
            ["-c", &cmd],
            self.timeout,
            bmart::process::Options::default(),
        )
        .await
        .map_err(Into::into)
    }
    pub async fn stop(&mut self) -> EResult<Option<CommandResult>> {
        if let Some(pid) = self.pid.take() {
            bmart::process::kill_pstree(pid, Some(self.timeout), true).await;
            self.stop_futs();
            let result = self.compose_down().await?;
            Ok(Some(result))
        } else {
            self.stop_futs();
            Ok(None)
        }
    }
    fn stop_futs(&mut self) {
        macro_rules! stop {
            ($f: expr) => {
                if let Some(fut) = $f.take() {
                    fut.abort();
                }
            };
        }
        stop!(self.fut_stdout);
        stop!(self.fut_stderr);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            bmart::process::kill_pstree_sync(pid, true);
        }
        self.stop_futs();
    }
}
