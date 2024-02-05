use crate::metric::Metric;
use crate::tools::calc_usage;
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
        .map_err(|_| Error::core("Unable to set MEMORY CONFIG"))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
}

pub async fn report_worker() {
    if !CONFIG.get().unwrap().enabled {
        return;
    }
    let mut sys = System::new();
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("memory report worker started");
    loop {
        int.tick().await;
        sys.refresh_memory();
        for (total, free, id) in &[
            (sys.total_memory(), sys.free_memory(), "ram"),
            (sys.total_swap(), sys.free_swap(), "swap"),
        ] {
            Metric::new0(id, "total").report(total).await;
            Metric::new0(id, "free").report(free).await;
            Metric::new0(id, "usage")
                .report(calc_usage(*total, *free))
                .await;
        }
    }
}
