use crate::metric::Metric;
use crate::tools::{calc_usage, format_name};
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashSet;
use sysinfo::Disks;

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set NETWORK CONFIG"))
}

#[derive(Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
    partitions: Option<HashSet<String>>,
}

pub async fn report_worker() {
    let config = CONFIG.get().unwrap();
    if !config.enabled {
        return;
    }
    if let Some(ref i) = config.partitions {
        if i.is_empty() {
            return;
        }
    }
    let mut disks = Disks::new();
    let mut int = tokio::time::interval(crate::DISK_REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("disks report worker started");
    loop {
        int.tick().await;
        disks.refresh_list();
        for disk in &disks {
            let mount_point_absolute = disk.mount_point().to_string_lossy();
            if let Some(ref i) = config.partitions {
                if !i.contains(mount_point_absolute.as_ref()) {
                    continue;
                }
            }
            let mount_point: Cow<str> = if let Some(m) = mount_point_absolute.strip_prefix('/') {
                m.into()
            } else {
                mount_point_absolute
            };
            let name: Cow<str> = if mount_point.is_empty() {
                "SYSTEM_ROOT".into()
            } else {
                format_name(&mount_point).into()
            };
            Metric::new("disk", &name, "total")
                .report(disk.total_space())
                .await;
            Metric::new("disk", &name, "avail")
                .report(disk.available_space())
                .await;
            Metric::new("disk", &name, "usage")
                .report(calc_usage(disk.total_space(), disk.available_space()))
                .await;
        }
    }
}
