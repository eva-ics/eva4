use clap::Parser;
use eva_common::prelude::*;
use eva_system_common::{
    common::{self, ReportConfig},
    metric::client,
};
use is_terminal::IsTerminal;
use log::info;
use serde::Deserialize;
use syslog::{BasicLogger, Facility, Formatter3164};
use tokio::fs;

const AUTHOR: &str = "Bohemia Automation";
//const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "EVA ICS System Controller Linux agent";

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " build ",
    include_str!("../../build.number")
);

#[cfg(debug_assertions)]
const CONFIG_PATH: &str = "./dev/agent-config.yml";
#[cfg(not(debug_assertions))]
const CONFIG_PATH: &str = "/etc/eva-cs-agent/config.yml";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    report: ReportConfig,
    client: client::Config,
}

#[derive(Parser)]
#[command(author = AUTHOR, version = LONG_VERSION, about = DESCRIPTION, long_about = None)]
struct Args {}

#[tokio::main(worker_threads = 1)]
async fn main() -> EResult<()> {
    let _ = Args::parse();
    let config: Config = serde_yaml::from_str(
        &fs::read_to_string(CONFIG_PATH)
            .await
            .map_err(|e| Error::io(format!("unable to load {}: {}", CONFIG_PATH, e)))?,
    )
    .map_err(Error::invalid_params)?;
    let log_level_filter = log::LevelFilter::Info;
    if std::io::stdout().is_terminal() {
        eva_common::console_logger::configure_env_logger(false);
    } else {
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: "eva-cs-agent".into(),
            pid: 0,
        };
        let logger = syslog::unix(formatter).map_err(Error::failed)?;
        log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
            .map(|()| log::set_max_level(log_level_filter))
            .map_err(Error::failed)?;
    }
    if config.client.fips {
        eva_common::services::enable_fips()?;
        info!("FIPS: enabled");
    }
    config.report.set()?;
    common::spawn_workers();
    client::spawn_worker(config.client);
    info!("{} {} started", DESCRIPTION, LONG_VERSION);
    loop {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
}
