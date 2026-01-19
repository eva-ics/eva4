use crate::metric::Metric;
use crate::tools::format_name;
use eva_common::err_logger;
use eva_common::prelude::*;
use log::info;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, System};
use tokio::sync::Mutex;

err_logger!();

const REFRESH: Duration = Duration::from_secs(1);
const REPORT_CPU_INFO_EVERY: usize = 10;

static CONFIG: OnceLock<Config> = OnceLock::new();

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
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("cpu report worker started");
    let mut c = 0;
    let sys = Arc::new(Mutex::new(System::new()));
    loop {
        int.tick().await;
        let s = sys.clone();
        if tokio::task::spawn_blocking(move || {
            let _ = s
                .try_lock()
                .map(|mut s| s.refresh_cpu_specifics(CpuRefreshKind::everything()));
        })
        .await
        .log_err_with("cpu stats")
        .is_ok()
        {
            for cpu in sys.lock().await.cpus() {
                let mut cpu_name = cpu.name();
                if let Some(n) = cpu_name.strip_prefix("cpu") {
                    cpu_name = n;
                } else if let Some(n) = cpu_name.strip_prefix("CPU ") {
                    cpu_name = n;
                }
                let name = format_name(cpu_name, false);
                Metric::new("cpu/core", &name, "usage")
                    .report(cpu.cpu_usage())
                    .await;
                Metric::new("cpu/core", &name, "freq")
                    .report(cpu.frequency())
                    .await;
            }
            if c == 0 {
                Metric::new0("cpu", "avail").report(num_cpus::get()).await;
                c = REPORT_CPU_INFO_EVERY;
            }
            c -= 1;
        }
    }
}
