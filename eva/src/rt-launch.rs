use std::io::Read as _;

use clap::Parser;
use eva_common::payload::{pack, unpack};
use eva_common::services::RealtimeConfig;
use eva_common::services::SERVICE_PAYLOAD_INITIAL;
use eva_common::{prelude::*, services};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Parser)]
struct Args {
    target: String,
}

pub fn read_initial_sync() -> EResult<services::Initial> {
    let mut buf = [0_u8; 1];
    std::io::stdin().read_exact(&mut buf)?;
    if buf[0] != SERVICE_PAYLOAD_INITIAL {
        return Err(Error::invalid_data("invalid payload"));
    }
    let mut buf = [0_u8; 4];
    std::io::stdin().read_exact(&mut buf)?;
    let len: usize = u32::from_le_bytes(buf).try_into().map_err(Error::failed)?;
    let mut buf = vec![0_u8; len];
    std::io::stdin().read_exact(&mut buf)?;
    unpack(&buf)
}

fn apply_current_thread_params(params: &services::RealtimeConfig) -> EResult<()> {
    let mut rt_params = rtsc::thread_rt::Params::new().with_cpu_ids(&params.cpu_ids);
    if let Some(priority) = params.priority {
        rt_params = rt_params.with_priority(Some(priority));
        if priority > 0 {
            rt_params = rt_params.with_scheduling(rtsc::thread_rt::Scheduling::FIFO);
        }
    }
    if let Err(e) = rtsc::thread_rt::apply_for_current(&rt_params) {
        if e == rtsc::Error::AccessDenied {
            eprintln!("Real-time parameters are not set, the service is not launched as root");
        } else {
            return Err(Error::failed(format!(
                "Real-time priority set error: {}",
                e
            )));
        }
    }
    if let Some(prealloc_heap) = params.prealloc_heap {
        if prealloc_heap > 0 {
            eprintln!("Heap preallocation is not available");
        }
    }
    Ok(())
}

fn main() -> EResult<()> {
    let args = Args::parse();
    let mut initial = read_initial_sync()?;
    apply_current_thread_params(initial.realtime())?;
    initial = initial.with_realtime(RealtimeConfig::default());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run_svc(&args.target, initial))
}

async fn run_svc(target: &str, initial: services::Initial) -> EResult<()> {
    let mut process = tokio::process::Command::new(target)
        .stdin(std::process::Stdio::piped())
        .env("EVA_RT_LAUNCHER", "1")
        .spawn()?;
    let packed = pack(&initial)?;
    let mut buffer = Vec::with_capacity(1 + 4 + packed.len());
    buffer.push(eva_common::services::SERVICE_PAYLOAD_INITIAL);
    buffer.extend_from_slice(&u32::try_from(packed.len())?.to_le_bytes());
    buffer.extend_from_slice(&packed);
    let mut pipe_stdin = process
        .stdin
        .take()
        .ok_or_else(|| Error::io("stdin is not available"))?;
    pipe_stdin.write_all(&buffer).await?;
    let initial_proxy = tokio::spawn(async move {
        let buf = &mut [0_u8; 1];
        let mut sys_stdin = tokio::io::stdin();
        while sys_stdin.read_exact(buf).await.is_ok() {
            pipe_stdin.write_all(buf).await?;
        }
        Ok(()) as EResult<()>
    });
    tokio::select! {
        _ = initial_proxy => {
            process.kill().await.ok();
            std::process::exit(-1);
        }
        v = process.wait() => {
            let exitcode = v.unwrap();
            std::process::exit(exitcode.code().unwrap_or(0));
        }
    }
}
