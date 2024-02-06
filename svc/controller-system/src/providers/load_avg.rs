use crate::metric::Metric;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;
use sysinfo::System;

static CONFIG: OnceCell<Config> = OnceCell::new();

const REFRESH: Duration = Duration::from_secs(1);

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set LOAD_AVG CONFIG"))
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
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("load average report worker started");
    loop {
        int.tick().await;
        let load_avg = System::load_average();
        Metric::new0("load_avg", "1").report(load_avg.one).await;
        Metric::new0("load_avg", "5").report(load_avg.five).await;
        Metric::new0("load_avg", "15")
            .report(load_avg.fifteen)
            .await;
    }
}
