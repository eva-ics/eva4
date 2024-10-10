use clap::Parser;
use eva::Mode;
use eva::{AUTHOR, PRODUCT_NAME, VERSION};
use eva_common::err_logger;
use eva_common::{services, EResult, Error, ErrorKind};
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

#[cfg(target_os = "linux")]
fn apply_thread_params(tid: libc::c_int, params: &services::RealtimeConfig) -> EResult<()> {
    let user_id = unsafe { libc::getuid() };
    if !params.cpu_ids.is_empty() {
        if user_id == 0 {
            unsafe {
                let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
                for cpu in &params.cpu_ids {
                    libc::CPU_SET(*cpu, &mut cpuset);
                }
                let res =
                    libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
                if res != 0 {
                    return Err(Error::failed(format!("CPU affinity set error: {}", res)));
                }
            }
        } else {
            eprintln!("Core CPU affinity is not set, the core is not launched as root");
        }
    }
    if let Some(priority) = params.priority {
        if user_id == 0 {
            let res = unsafe {
                libc::sched_setscheduler(
                    tid,
                    libc::SCHED_FIFO,
                    &libc::sched_param {
                        sched_priority: priority,
                    },
                )
            };
            if res != 0 {
                return Err(Error::failed(format!(
                    "Real-time priority set error: {}",
                    res
                )));
            }
        } else {
            eprintln!("Core real-time priority is not set, the core is not launched as root");
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RealtimeParams {
    priority: Option<i32>,
    cpu_ids: Option<String>,
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
    let tid = unsafe { i32::try_from(libc::syscall(libc::SYS_gettid)).unwrap_or(-200) };
    apply_thread_params(tid, &realtime).expect("Unable to apply real-time core parameters");
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
