use crate::metric::Metric;
use eva_common::err_logger;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;
use sysinfo::System;

err_logger!();

const REFRESH: Duration = Duration::from_secs(1);
const REPORT_OS_INFO_EVERY: usize = 10;

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

struct OsInfo {
    long_os_version: Option<String>,
    os_version: Option<String>,
    kernel_version: Option<String>,
    distribution_id: String,
    arch: Option<String>,
}

/// # Panics
///
/// will panic if config is not set
pub async fn report_worker() {
    let config = CONFIG.get().unwrap();
    if !config.enabled {
        return;
    }
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("system report worker started");
    let mut c = 0;
    loop {
        int.tick().await;
        Metric::new0("os", "uptime").report(System::uptime()).await;
        if c == 0 {
            if let Ok(info) = tokio::task::spawn_blocking(move || OsInfo {
                long_os_version: System::long_os_version(),
                os_version: System::os_version(),
                kernel_version: System::kernel_version(),
                distribution_id: System::distribution_id(),
                arch: System::cpu_arch(),
            })
            .await
            .log_err_with("os info")
            {
                Metric::new0("os", "name")
                    .report(info.long_os_version)
                    .await;
                Metric::new0("os", "version").report(info.os_version).await;
                Metric::new0("os", "kernel")
                    .report(info.kernel_version)
                    .await;
                Metric::new0("os", "distribution_id")
                    .report(info.distribution_id)
                    .await;
                Metric::new0("os", "arch").report(info.arch).await;
            }
            c = REPORT_OS_INFO_EVERY;
        }
        c -= 1;
    }
}
