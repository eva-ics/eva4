use crate::metric::Metric;
use crate::tools::{calc_usage, format_name};
use eva_common::err_logger;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::Disks;

err_logger!();

static CONFIG: OnceCell<Config> = OnceCell::new();
static MOUNTPOINT_NAME_CACHE: Lazy<Mutex<BTreeMap<PathBuf, Arc<String>>>> = Lazy::new(<_>::default);

const REFRESH: Duration = Duration::from_secs(10);

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set DISKS CONFIG"))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
    mount_points: Option<HashSet<PathBuf>>,
}

fn format_metric_mountpoint_name(path: &Path) -> Arc<String> {
    let mount_point_absolute = path.to_string_lossy();
    let mount_point: Cow<str> = if let Some(m) = mount_point_absolute.strip_prefix('/') {
        m.into()
    } else if let Some(m) = mount_point_absolute.strip_suffix(":\\") {
        m.into()
    } else if let Some(m) = mount_point_absolute.strip_suffix(':') {
        m.into()
    } else {
        mount_point_absolute
    };
    if mount_point.is_empty() {
        "SYSTEM_ROOT".to_owned().into()
    } else {
        format_name(&mount_point, false)
    }
}

fn format_mountpoint_name(path: &Path) -> Arc<String> {
    let mut cache = MOUNTPOINT_NAME_CACHE.lock();
    if let Some(name) = cache.get(path) {
        name.clone()
    } else {
        let name = format_metric_mountpoint_name(path);
        cache.insert(path.to_owned(), name.clone());
        name
    }
}

/// # Panics
///
/// will panic if config is not set
pub async fn report_worker() {
    let config = CONFIG.get().unwrap();
    if !config.enabled {
        return;
    }
    if let Some(ref i) = config.mount_points {
        if i.is_empty() {
            return;
        }
    }
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("disks report worker started");
    loop {
        int.tick().await;
        let mut reported = if config.mount_points.is_some() {
            Some(HashSet::<&Path>::new())
        } else {
            None
        };
        let mut disks_s = None;
        if let Ok(disks) = tokio::task::spawn_blocking(move || {
            let mut disks = Disks::new();
            disks.refresh_list();
            disks
        })
        .await
        .log_err_with("disks")
        {
            disks_s.replace(disks);
            for disk in disks_s.as_ref().unwrap() {
                let path = disk.mount_point();
                if let Some(ref i) = config.mount_points {
                    if !i.contains(path) {
                        continue;
                    }
                }
                let name = format_mountpoint_name(path);
                Metric::new("disk", &name, "total")
                    .report(disk.total_space())
                    .await;
                Metric::new("disk", &name, "avail")
                    .report(disk.available_space())
                    .await;
                Metric::new("disk", &name, "used")
                    .report(disk.total_space() - disk.available_space())
                    .await;
                Metric::new("disk", &name, "usage")
                    .report(calc_usage(disk.total_space(), disk.available_space()))
                    .await;
                if let Some(r) = reported.as_mut() {
                    r.insert(path);
                }
            }
        }
        if let Some(r) = reported {
            for c in config.mount_points.as_ref().unwrap() {
                if !r.contains(c.as_path()) {
                    for res in ["total", "avail", "usage"] {
                        Metric::new("disk", &format_mountpoint_name(c), res)
                            .failed()
                            .report(-1)
                            .await;
                    }
                }
            }
        }
    }
}
