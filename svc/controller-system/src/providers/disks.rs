use crate::metric::Metric;
use crate::tools::{calc_usage, format_name};
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::Disks;

static CONFIG: OnceCell<Config> = OnceCell::new();

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

fn format_mountpoint_name(path: &Path) -> Cow<str> {
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
        "SYSTEM_ROOT".into()
    } else {
        format_name(&mount_point).into()
    }
}

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
    let mut disks = Disks::new();
    let mut int = tokio::time::interval(REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("disks report worker started");
    loop {
        int.tick().await;
        disks.refresh_list();
        let mut reported = if config.mount_points.is_some() {
            Some(HashSet::<&Path>::new())
        } else {
            None
        };
        for disk in &disks {
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
            Metric::new("disk", &name, "usage")
                .report(calc_usage(disk.total_space(), disk.available_space()))
                .await;
            if let Some(r) = reported.as_mut() {
                r.insert(path);
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
