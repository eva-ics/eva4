use atty::Stream;
use colored::Colorize;
use eva_client::VersionInfo;
use eva_common::prelude::*;
use serde::Deserialize;
use std::process::Stdio;
use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    repository_url: String,
}

/// # Panics
///
/// Will panic on stdin problems
pub async fn read_stdin() -> Vec<u8> {
    let mut stdin = tokio::io::stdin();
    let mut buf: Vec<u8> = Vec::new();
    if atty::is(Stream::Stdin) {
        println!("Reading stdin, Ctrl-D to finish...");
    }
    stdin.read_to_end(&mut buf).await.unwrap();
    buf
}

pub async fn svc_version_info() -> EResult<VersionInfo> {
    let mut node_exc = crate::EVA_DIR.clone();
    node_exc.push("svc/eva-node");
    let p = tokio::process::Command::new(&node_exc)
        .args(["--mode", "info"])
        .stdout(Stdio::piped())
        .spawn()?;
    let res = p.wait_with_output().await?;
    serde_json::from_slice(&res.stdout).map_err(Into::into)
}

pub async fn load_config() -> EResult<()> {
    let mut reg_cli = crate::EVA_DIR.clone();
    reg_cli.push("sbin/eva-registry-cli");
    let p = tokio::process::Command::new(&reg_cli)
        .args(["get", "eva/config/cloud-manager"])
        .stdout(Stdio::piped())
        .spawn()?;
    let res = p.wait_with_output().await?;
    let config: Config = serde_json::from_slice(&res.stdout)?;
    crate::REPOSITORY_URL
        .set(config.repository_url)
        .map_err(|_| Error::core("unable to set config vars"))?;
    Ok(())
}

pub async fn set_config_field(field: &str, value: &Value) -> EResult<()> {
    let mut reg_cli = crate::EVA_DIR.clone();
    reg_cli.push("sbin/eva-registry-cli");
    let mut p = tokio::process::Command::new(&reg_cli)
        .args([
            "set-field",
            "eva/config/cloud-manager",
            field,
            "-",
            "-p",
            "json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdin = p
        .stdin
        .as_mut()
        .ok_or_else(|| Error::failed("Unable to take the process stdin"))?;
    stdin.write_all(&serde_json::to_vec(value)?).await?;
    let out = p.wait_with_output().await?;
    if out.status.code().unwrap_or(-1) == 0 {
        Ok(())
    } else {
        Err(Error::failed(format!(
            "the process exited with code {}",
            out.status
        )))
    }
}

pub async fn confirm(act: &str) -> EResult<()> {
    let ps = format!(
        "Type {} (uppercase) to {} or press {} to abort > ",
        "YES".red().bold(),
        act,
        "ENTER".white()
    );
    let mut stdin = io::BufReader::new(tokio::io::stdin());
    let mut buf = String::new();
    let mut stdout = io::stdout();
    stdout.write_all(ps.as_bytes()).await?;
    stdout.flush().await?;
    stdin
        .read_line(&mut buf)
        .await
        .map_err(|_| Error::aborted())?;
    println!();
    if buf.trim() == "YES" {
        Ok(())
    } else {
        Err(Error::aborted())
    }
}

pub fn ctable(titles: &[&str]) -> prettytable::Table {
    let mut table = prettytable::Table::new();
    let format = prettytable::format::FormatBuilder::new()
        .column_separator(' ')
        .borders(' ')
        .separators(
            &[prettytable::format::LinePosition::Title],
            prettytable::format::LineSeparator::new('-', '-', '-', '-'),
        )
        .padding(0, 1)
        .build();
    table.set_format(format);
    let mut titlevec: Vec<prettytable::Cell> = Vec::new();
    for t in titles {
        titlevec.push(prettytable::Cell::new(t).style_spec("Fb"));
    }
    table.set_titles(prettytable::Row::new(titlevec));
    table
}
