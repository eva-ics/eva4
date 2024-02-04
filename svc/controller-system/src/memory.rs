use crate::metric::Metric;
use crate::tools::calc_usage;
use sysinfo::System;

pub async fn report_worker() {
    let mut sys = System::new();
    let mut int = tokio::time::interval(crate::MEMORY_REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
