use crate::metric::Metric;
use crate::tools::{calc_usage, format_name};
use std::borrow::Cow;
use sysinfo::Disks;

pub async fn report_worker() {
    let mut disks = Disks::new();
    let mut int = tokio::time::interval(crate::DISK_REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        int.tick().await;
        disks.refresh_list();
        for disk in &disks {
            let mount_point_absolute = disk.mount_point().to_string_lossy();
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
