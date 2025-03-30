use crate::metric::Metric;
use crate::tools::format_name;
use eva_common::prelude::*;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::fs;

static CONFIG: OnceCell<Config> = OnceCell::new();

const REFRESH: Duration = Duration::from_secs(1);

struct Stat {
    sectors_read: u64,
    sectors_written: u64,
    time_in_progress: u64,
    reads: u64,
    writes: u64,
}

impl Stat {
    #[cfg(target_os = "linux")]
    fn from_procfs_diskstat(d: procfs::DiskStat) -> (String, Self) {
        (
            d.name,
            Self {
                sectors_read: d.sectors_read,
                sectors_written: d.sectors_written,
                reads: d.reads,
                writes: d.writes,
                time_in_progress: d.time_in_progress,
            },
        )
    }
}

trait Diff {
    fn counter_diff(self, other: Self) -> Self;
}

impl Diff for u64 {
    fn counter_diff(self, other: Self) -> Self {
        if self >= other {
            self - other
        } else {
            u64::MAX - other + self + 1
        }
    }
}

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set BLK CONFIG"))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
    devices: Option<HashSet<String>>,
}

#[allow(clippy::unused_async)]
#[cfg(not(target_os = "linux"))]
pub async fn report_worker() {
    log::warn!("blk worker exited, platform not supported");
}

async fn get_sector_size(dev: &str) -> Option<u64> {
    fs::read_to_string(format!("/sys/block/{}/queue/hw_sector_size", dev))
        .await
        .ok()
        .and_then(|v| v.trim_end().parse().ok())
}

#[allow(clippy::cast_precision_loss)]
async fn report_device(name: &str, current: &Stat, prev: &Stat) -> bool {
    if let Some(sector_size) = get_sector_size(name).await {
        let read_bytes = current.sectors_read.counter_diff(prev.sectors_read) * sector_size;
        let written_bytes =
            current.sectors_written.counter_diff(prev.sectors_written) * sector_size;
        let reads = current.reads.counter_diff(prev.reads);
        let writes = current.writes.counter_diff(prev.writes);
        let mut util = current.time_in_progress.counter_diff(prev.time_in_progress) as f64 * 100.0
            / REFRESH.as_millis() as f64;
        if util > 100.0 {
            util = 100.0;
        }
        Metric::new("blk", &format_name(name, false), "r")
            .report(reads)
            .await;
        Metric::new("blk", &format_name(name, false), "w")
            .report(writes)
            .await;
        Metric::new("blk", &format_name(name, false), "rb")
            .report(read_bytes)
            .await;
        Metric::new("blk", &format_name(name, false), "wb")
            .report(written_bytes)
            .await;
        Metric::new("blk", &format_name(name, false), "util")
            .report(util)
            .await;
        true
    } else {
        false
    }
}

#[cfg(target_os = "linux")]
async fn diskstats() -> EResult<Vec<procfs::DiskStat>> {
    let mut result = Vec::new();
    for line in fs::read_to_string("/proc/diskstats").await?.split('\n') {
        if !line.is_empty() {
            result.push(procfs::DiskStat::from_line(line).map_err(Error::invalid_data)?);
        }
    }
    Ok(result)
}

/// # Panics
///
/// will panic if config is not set
#[allow(clippy::unused_async)]
#[cfg(target_os = "linux")]
pub async fn report_worker() {
    use std::collections::BTreeMap;
    let config = CONFIG.get().unwrap();
    if !config.enabled {
        return;
    }
    if let Some(ref i) = config.devices {
        if i.is_empty() {
            return;
        }
    }
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    log::info!("blk report worker started");
    let mut prev: Option<BTreeMap<String, Stat>> = None;
    loop {
        int.tick().await;
        let mut reported = if config.devices.is_some() {
            Some(HashSet::<String>::new())
        } else {
            None
        };
        macro_rules! mark_reported {
            ($name: expr) => {
                if let Some(ref mut r) = reported {
                    r.insert($name.clone());
                }
            };
        }
        if let Ok(stats_vec) = diskstats().await {
            let stats: BTreeMap<String, Stat> = stats_vec
                .into_iter()
                .filter_map(|v| {
                    if let Some(ref devices) = config.devices {
                        if devices.contains(&v.name) {
                            Some(Stat::from_procfs_diskstat(v))
                        } else {
                            None
                        }
                    } else if v.name.starts_with("loop") {
                        None
                    } else {
                        Some(Stat::from_procfs_diskstat(v))
                    }
                })
                .collect();
            if let Some(ref prev_stats) = prev {
                for (name, device_stats) in &stats {
                    if let Some(prev_drive_stats) = prev_stats.get(name) {
                        if report_device(name, device_stats, prev_drive_stats).await {
                            mark_reported!(name);
                        }
                    } else {
                        // do not mark device as failed if no prev
                        mark_reported!(name);
                    }
                }
            }
            let prev_empty = prev.is_none();
            prev.replace(stats);
            if prev_empty {
                // first loop, skip reported check
                continue;
            }
        }
        if let Some(r) = reported {
            for c in config.devices.as_ref().unwrap() {
                if !r.contains(c) {
                    for res in ["r", "w", "rb", "wb", "util"] {
                        Metric::new("blk", &format_name(c, false), res)
                            .failed()
                            .report(-1)
                            .await;
                    }
                }
            }
        }
    }
}
