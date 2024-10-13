use clap::Parser;
use eva::Mode;
use eva::{AUTHOR, PRODUCT_NAME, VERSION};
use eva_common::err_logger;
use eva_common::{services, ErrorKind};
use serde::Deserialize;
use std::time::Duration;

const SLEEP_NOT_READY: Duration = Duration::from_secs(1);

err_logger!();

#[derive(Parser)]
#[clap(name = PRODUCT_NAME, version = VERSION, author = AUTHOR)]
struct Args {
    #[clap(long = "mode", default_value = "regular")]
    mode: Mode,
    #[clap(long)]
    pid_file: Option<String>,
    #[clap(long)]
    system_name: Option<String>,
    #[clap(short = 'C', long = "connection-path")]
    connection_path: Option<String>,
    #[clap(long)]
    fips: bool,
    #[clap(long)]
    realtime: Option<String>,
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Debug, Deserialize)]
struct RealtimeParams {
    priority: Option<i32>,
    cpu_ids: Option<String>,
    prealloc_heap: Option<usize>,
}

impl From<RealtimeParams> for services::RealtimeConfig {
    fn from(params: RealtimeParams) -> Self {
        services::RealtimeConfig {
            priority: params.priority,
            cpu_ids: params
                .cpu_ids
                .map(|s| {
                    s.split('|')
                        .map(|s| s.parse().expect("Invalid real-time CPU id"))
                        .collect()
                })
                .unwrap_or_default(),
            prealloc_heap: params.prealloc_heap,
        }
    }
}

fn main() {
    eva_common::self_test();
    let args = Args::parse();
    let realtime_str = args
        .realtime
        .as_deref()
        .unwrap_or_default()
        .trim_start_matches('\'')
        .trim_end_matches('\'');
    let realtime_params: RealtimeParams = eva_common::serde_keyvalue::from_key_values(realtime_str)
        .expect("Real-time parameters error");
    eva::launch_sysinfo().expect("Unable to launch sysinfo thread");
    let realtime: services::RealtimeConfig = realtime_params.into();
    eva::apply_current_thread_params(&realtime, false).unwrap();
    if let Err(e) = eva::node::launch(
        args.mode,
        args.system_name.as_deref(),
        args.pid_file.as_deref(),
        args.connection_path.as_deref(),
        args.fips,
        realtime,
    )
    .log_err()
    {
        if e.kind() == ErrorKind::NotReady {
            println!("{}", e);
            std::thread::sleep(SLEEP_NOT_READY);
            std::process::exit(3);
        } else {
            panic!("{}", e);
        }
    }
}
