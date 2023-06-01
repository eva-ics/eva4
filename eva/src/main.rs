use clap::Parser;
use eva::Mode;
use eva::{AUTHOR, PRODUCT_NAME, VERSION};
use eva_common::err_logger;
use eva_common::ErrorKind;
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
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn main() {
    eva_common::self_test();
    let args = Args::parse();
    if let Err(e) = eva::node::launch(
        args.mode,
        args.system_name.as_deref(),
        args.pid_file.as_deref(),
        args.connection_path.as_deref(),
        args.fips,
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
