use crate::metric::Metric;
use sysinfo::Networks;

pub async fn report_worker() {
    let mut networks = Networks::new();
    let mut int = tokio::time::interval(crate::NETWORK_REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        int.tick().await;
        networks.refresh_list();
        for (name, i) in &networks {
            Metric::new("network", name, "rxb")
                .report(i.received())
                .await;
            Metric::new("network", name, "txb")
                .report(i.transmitted())
                .await;
            Metric::new("network", name, "rxb_total")
                .report(i.total_received())
                .await;
            Metric::new("network", name, "txb_total")
                .report(i.total_transmitted())
                .await;
            Metric::new("network", name, "rx_err")
                .report(i.errors_on_received())
                .await;
            Metric::new("network", name, "tx_err")
                .report(i.errors_on_transmitted())
                .await;
            Metric::new("network", name, "rx_err_total")
                .report(i.total_errors_on_received())
                .await;
            Metric::new("network", name, "tx_err_total")
                .report(i.total_errors_on_transmitted())
                .await;
        }
    }
}
