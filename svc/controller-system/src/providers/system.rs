use crate::metric::Metric;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;
use sysinfo::System;

const REFRESH: Duration = Duration::from_secs(1);

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set SYSTEM CONFIG"))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
}

pub async fn report_worker() {
    let config = CONFIG.get().unwrap();
    if !config.enabled {
        return;
    }
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("system report worker started");
    Metric::new0("os", "name")
        .report(System::long_os_version())
        .await;
    Metric::new0("os", "version")
        .report(System::os_version())
        .await;
    Metric::new0("os", "kernel")
        .report(System::kernel_version())
        .await;
    Metric::new0("os", "distribution_id")
        .report(System::distribution_id())
        .await;
    Metric::new0("os", "arch").report(System::cpu_arch()).await;
    loop {
        int.tick().await;
        Metric::new0("os", "uptime").report(System::uptime()).await;
    }
}
