use clap::Parser;
use eva::Mode;
use eva::{AUTHOR, PRODUCT_NAME, VERSION};
use eva_common::err_logger;
use eva_common::{services, ErrorKind};
use serde::Deserialize;
use std::sync::Arc;
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
    #[clap(long, default_value = "1000000")]
    direct_alloc_limit: usize,
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    let realtime: services::RealtimeConfig = realtime_params.into();
    eva::launch_sysinfo(realtime.priority.unwrap_or_default() == 0)
        .expect("Unable to launch sysinfo thread");
    let aa = if realtime.priority.is_some() {
        let thread_async_allocator = Arc::new(eva::bus::ThreadAsyncAllocator::new());
        let t = thread_async_allocator.clone();
        std::thread::Builder::new()
            .name("EvaBusAlloc".to_owned())
            .spawn(move || {
                t.run();
            })
            .expect("Unable to start async allocator thread");
        Some((args.direct_alloc_limit, thread_async_allocator))
    } else {
        None
    };
    eva::apply_current_thread_params(&realtime, false).unwrap();
    if let Err(e) = eva::node::launch(
        args.mode,
        args.system_name.as_deref(),
        args.pid_file.as_deref(),
        args.connection_path.as_deref(),
        args.fips,
        realtime,
        aa,
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
