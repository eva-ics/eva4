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

//const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "System service Linux agent";

#[cfg(debug_assertions)]
const CONFIG_PATH: &str = "./dev/agent-config.yml";
#[cfg(not(debug_assertions))]
const CONFIG_PATH: &str = "/etc/eva-cs-agent/config.yml";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    report: ReportConfig,
    client: client::Config,
}

#[cfg(feature = "openssl-vendored")]
pub fn enable_fips() -> EResult<()> {
    Err(Error::failed(
        "FIPS can not be enabled, consider using a native OS distribution",
    ))
}

#[cfg(not(feature = "openssl-vendored"))]
pub fn enable_fips() -> EResult<()> {
    #[cfg(feature = "openssl3")]
    {
        FIPS_LOADED
            .set(())
            .map_err(|_| Error::core("FIPS provided already loaded"))?;
        std::mem::forget(openssl::provider::Provider::load(None, "fips").map_err(Error::failed)?);
    }
    #[cfg(not(feature = "openssl3"))]
    openssl::fips::enable(true).map_err(Error::failed)?;
    Ok(())
}

#[tokio::main(worker_threads = 1)]
async fn main() -> EResult<()> {
    let config: Config = serde_yaml::from_str(
        &fs::read_to_string(CONFIG_PATH)
            .await
            .map_err(|e| Error::io(format!("unable to load {}: {}", CONFIG_PATH, e)))?,
    )
    .map_err(Error::invalid_params)?;
    let log_level_filter = log::LevelFilter::Info;
    if std::io::stdout().is_terminal() {
        env_logger::Builder::new()
            .target(env_logger::Target::Stdout)
            .filter_level(log_level_filter)
            .init();
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
        enable_fips()?;
        info!("FIPS: enabled");
    }
    config.report.set()?;
    common::spawn_workers();
    client::spawn_worker(config.client);
    info!("{} {} started", DESCRIPTION, VERSION);
    loop {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
}
