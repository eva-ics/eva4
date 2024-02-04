use crate::metric::Metric;
use crate::CPU_REFRESH;
use sysinfo::{CpuRefreshKind, System};

pub async fn report_worker() {
    let mut sys = System::new();
    let mut int = tokio::time::interval(CPU_REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
