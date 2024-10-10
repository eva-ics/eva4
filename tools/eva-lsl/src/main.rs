use clap::Parser;
use eva_common::payload::pack;
use eva_common::{common_payloads::ParamsId, prelude::*};
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;

#[derive(Parser)]
struct Args {
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
    #[clap(short = 'd', long)]
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
}

#[tokio::main]
async fn main() -> EResult<()> {
    let args = Args::parse();
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
    buffer.push(0x01);
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
    let mut process = tokio::process::Command::new(cmd)
        .stdin(std::process::Stdio::piped())
        .args(cmd_args)
        .env("EVA_SVC_DEBUG", "1")
        .spawn()?;
    let mut stdin = process
        .stdin
        .take()
        .ok_or_else(|| Error::io("stdin is not available"))?;
    stdin.write_all(&buffer).await?;
    let exitcode = process.wait().await?;
    std::process::exit(exitcode.code().unwrap_or(0));
}
