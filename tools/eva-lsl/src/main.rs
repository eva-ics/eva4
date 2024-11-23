use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use colored::Colorize;
use eva_common::payload::pack;
use eva_common::{common_payloads::ParamsId, prelude::*};
use notify::Watcher as _;
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;

const SVC_TPL: &str = include_str!("../tpl/svc_main.rs");

#[derive(Parser)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser)]
enum Command {
    New(CommandNew),
    Run(CommandRun),
}

#[derive(Parser)]
struct CommandNew {
    name: String,
}

#[derive(Parser)]
struct CommandRun {
    #[clap()]
    service_id: String,
    #[clap(
        short = 'i',
        long,
        help = "Override service ID (default: SVC_ID.HOSTNAME.PID)"
    )]
    id_override: Option<String>,
    #[clap(short = 'b', long, default_value = "/opt/eva4/var/bus.ipc")]
    bus: String,
    #[clap(
        short = 'd',
        long,
        help = "Override the service data path (use a local directory)"
    )]
    data_path: Option<String>,
    #[clap(
        short = 'u',
        long,
        help = "Run under a specific user (default: no privilege drop)"
    )]
    user: Option<String>,
    #[clap(long, help = "Build in release mode")]
    release: bool,
    #[clap(long, help = "Do not use cargo, run the service binary directly")]
    binary: Option<String>,
    #[clap(short = 'w', long, help = "Automatically restart on source changes")]
    watch: bool,
}

async fn add_dependency(
    name: &str,
    version: &str,
    features: &[&str],
    path: Option<String>,
    disable_defaults: bool,
) -> EResult<()> {
    let dep = if path.is_some() {
        name.to_owned()
    } else {
        format!("{}@{}", name, version)
    };
    println!("Adding dependency: {}", dep.green().bold());
    let mut cmd = tokio::process::Command::new("cargo");
    cmd.arg("-q").arg("add").arg(dep);
    if let Some(path) = path {
        cmd.arg("--path").arg(path);
    }
    for feature in features {
        cmd.arg("--features").arg(feature);
    }
    if disable_defaults {
        cmd.arg("--no-default-features");
    }
    let result = cmd.status().await?;
    if !result.success() {
        return Err(Error::failed(format!("Unable to add dependency {}", name)));
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> EResult<()> {
    #[cfg(target_os = "windows")]
    let _ansi_enabled = ansi_term::enable_ansi_support();
    let args = Args::parse();
    match args.command {
        Command::New(c) => new(c).await,
        Command::Run(c) => run(c).await,
    }
}

async fn new(args: CommandNew) -> EResult<()> {
    let result = tokio::process::Command::new("cargo")
        .args(["new", &args.name])
        .status()
        .await?;
    if !result.success() {
        return Err(Error::io("failed to create a new project"));
    }
    std::env::set_current_dir(&args.name)?;
    add_dependency(
        "eva-common",
        "0.3.75",
        &[
            "events",
            "common-payloads",
            "payload",
            "acl",
            "openssl-no-fips",
        ],
        None,
        false,
    )
    .await?;
    add_dependency("eva-sdk", "0.3", &["controller"], None, false).await?;
    add_dependency("tokio", "1.36", &["full"], None, false).await?;
    add_dependency("async-trait", "0.1", &[], None, false).await?;
    add_dependency("serde", "1.0", &["derive", "rc"], None, false).await?;
    add_dependency("log", "0.4", &[], None, false).await?;
    add_dependency("jemallocator", "0.5", &[], None, false).await?;
    add_dependency("once_cell", "1.20", &[], None, false).await?;
    tokio::fs::write("src/main.rs", SVC_TPL).await?;
    tokio::fs::OpenOptions::new()
        .append(true)
        .open("Cargo.toml")
        .await?
        .write_all(
            "
[features]
std-alloc = []
"
            .as_bytes(),
        )
        .await?;
    Ok(())
}

async fn run(args: CommandRun) -> EResult<()> {
    let hostname = hostname::get()?.to_string_lossy().to_string();
    let pid = std::process::id();
    let client = eva_client::EvaClient::connect(
        &args.bus,
        &format!("eva-svc-launcher.{}.{}", hostname, pid),
        eva_client::Config::default(),
    )
    .await?;
    let result: Vec<Value> = client
        .call(
            "eva.core",
            "svc.get_init",
            ParamsId {
                i: &args.service_id,
            },
        )
        .await?;
    drop(client);
    let mut initial = eva_common::services::Initial::deserialize(
        result
            .first()
            .ok_or_else(|| Error::invalid_data("the core has returned an invalid initial payload"))?
            .clone(),
    )?;
    initial.set_bus_path(&args.bus);
    if let Some(ref data_path) = args.data_path {
        initial.set_data_path(data_path);
    }
    let svc_id = args
        .id_override
        .unwrap_or_else(|| format!("{}.{}.{}", args.service_id, hostname, pid));
    initial.set_id(&svc_id);
    initial.set_user(args.user.as_deref());
    let packed = pack(&initial)?;
    let mut buffer = Vec::with_capacity(1 + 4 + packed.len());
    buffer.push(eva_common::services::SERVICE_PAYLOAD_INITIAL);
    buffer.extend_from_slice(&u32::try_from(packed.len())?.to_le_bytes());
    buffer.extend_from_slice(&packed);
    let (cmd, cmd_args) = if let Some(ref binary) = args.binary {
        (binary.as_str(), vec![])
    } else {
        let mut a = vec!["run"];
        if args.release {
            a.push("--release");
        }
        ("cargo", a)
    };
    loop {
        let current_dir: PathBuf = std::env::current_dir()?;
        let mut cargo_toml = current_dir.clone();
        cargo_toml.push("Cargo.toml");
        if cargo_toml.exists() {
            break;
        }
        let parent_dir = current_dir
            .parent()
            .ok_or_else(|| Error::invalid_data("no Cargo.toml found"))?;
        std::env::set_current_dir(parent_dir)?;
    }
    let (watch_tx, watch_rx) = async_channel::unbounded();

    let mut watcher: notify::PollWatcher = notify::Watcher::new(
        move |ev: Result<notify::Event, notify::Error>| {
            let Ok(event) = ev else {
                return;
            };
            if args.watch
                && matches!(
                    event.kind,
                    notify::EventKind::Create(_)
                        | notify::EventKind::Modify(_)
                        | notify::EventKind::Remove(_)
                )
            {
                let _ = watch_tx.try_send(());
            }
        },
        notify::Config::default().with_poll_interval(Duration::from_secs(1)),
    )
    .map_err(Error::io)?;
    let mut src_dir = std::env::current_dir()?;
    src_dir.push("src");
    watcher
        .watch(&src_dir, notify::RecursiveMode::Recursive)
        .map_err(Error::io)?;

    let exitcode = loop {
        let mut process = tokio::process::Command::new(cmd)
            .stdin(std::process::Stdio::piped())
            .args(&cmd_args)
            .env("EVA_SVC_DEBUG", "1")
            .spawn()?;
        let mut stdin = process
            .stdin
            .take()
            .ok_or_else(|| Error::io("stdin is not available"))?;
        stdin.write_all(&buffer).await?;
        tokio::select! {
            _ = watch_rx.recv() => {
                let _ = process.kill().await;
                let _ = process.wait().await;
            }
            exitcode = process.wait() => {
                break exitcode?;
            }
        }
    };
    std::process::exit(exitcode.code().unwrap_or(0));
}
