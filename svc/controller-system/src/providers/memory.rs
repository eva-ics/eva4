use crate::metric::Metric;
use crate::tools::calc_usage;
use eva_common::err_logger;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;
use sysinfo::System;

err_logger!();

const REFRESH: Duration = Duration::from_secs(1);

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set MEMORY CONFIG"))
}

#[derive(Default, Deserialize)]
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
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("memory report worker started");
    loop {
        int.tick().await;
        let Ok(sys) = tokio::task::spawn_blocking(move || {
            let mut sys = System::new();
            sys.refresh_memory();
            sys
        })
        .await
        .log_err_with("memory") else {
            continue;
        };
        Metric::new0("ram", "total")
            .report(sys.total_memory())
            .await;
        Metric::new0("ram", "avail")
            .report(sys.available_memory())
            .await;
        Metric::new0("ram", "used").report(sys.used_memory()).await;
        Metric::new0("ram", "usage")
            .report(calc_usage(sys.total_memory(), sys.available_memory()))
            .await;
        Metric::new0("swap", "total").report(sys.total_swap()).await;
        Metric::new0("swap", "avail").report(sys.free_swap()).await;
        Metric::new0("swap", "used").report(sys.used_swap()).await;
        Metric::new0("swap", "usage")
            .report(calc_usage(sys.total_swap(), sys.free_swap()))
            .await;
    }
}
