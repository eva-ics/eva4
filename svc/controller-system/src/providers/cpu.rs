use crate::metric::Metric;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, System};

const REFRESH: Duration = Duration::from_secs(1);

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set CPU CONFIG"))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
}

/// # Panics
///
/// will panic if config is not set
pub async fn report_worker() {
    if !CONFIG.get().unwrap().enabled {
        return;
    }
    let mut sys = System::new();
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("cpu report worker started");
    loop {
        int.tick().await;
        sys.refresh_cpu_specifics(CpuRefreshKind::everything());
        for cpu in sys.cpus() {
            let mut name = cpu.name();
            if let Some(n) = name.strip_prefix("cpu") {
                name = n;
            }
            Metric::new("cpu/core", name, "usage")
                .report(cpu.cpu_usage())
                .await;
            Metric::new("cpu/core", name, "freq")
                .report(cpu.frequency())
                .await;
        }
    }
}
