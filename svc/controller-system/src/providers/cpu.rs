use crate::metric::Metric;
use crate::CPU_REFRESH;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use sysinfo::{CpuRefreshKind, System};

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set CPU CONFIG"))
}

#[derive(Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
}

pub async fn report_worker() {
    if !CONFIG.get().unwrap().enabled {
        return;
    }
    let mut sys = System::new();
    let mut int = tokio::time::interval(CPU_REFRESH);
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
            let load_avg = System::load_average();
            Metric::new("cpu/core", name, "usage")
                .report(cpu.cpu_usage())
                .await;
            Metric::new("cpu/core", name, "freq")
                .report(cpu.frequency())
                .await;
            Metric::new0("load_avg", "1").report(load_avg.one).await;
            Metric::new0("load_avg", "5").report(load_avg.five).await;
            Metric::new0("load_avg", "15")
                .report(load_avg.fifteen)
                .await;
        }
    }
}
