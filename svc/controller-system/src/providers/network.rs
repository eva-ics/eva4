use crate::metric::Metric;
use eva_common::prelude::*;
use log::info;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashSet;
use sysinfo::Networks;

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(config: Config) -> EResult<()> {
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set NETWORK CONFIG"))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    enabled: bool,
    interfaces: Option<HashSet<String>>,
}

pub async fn report_worker() {
    let config = CONFIG.get().unwrap();
    if !config.enabled {
        return;
    }
    if let Some(ref i) = config.interfaces {
        if i.is_empty() {
            return;
        }
    }
    let mut networks = Networks::new();
    let mut int = tokio::time::interval(crate::NETWORK_REFRESH);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!("network report worker started");
    loop {
        int.tick().await;
        networks.refresh_list();
        let mut reported = if config.interfaces.is_some() {
            Some(HashSet::<&str>::new())
        } else {
            None
        };
        for (name, i) in &networks {
            if let Some(ref i) = config.interfaces {
                if !i.contains(name) {
                    continue;
                }
            }
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
            Metric::new("network", name, "rx")
                .report(i.packets_received())
                .await;
            Metric::new("network", name, "tx")
                .report(i.packets_transmitted())
                .await;
            Metric::new("network", name, "rx_total")
                .report(i.total_packets_received())
                .await;
            Metric::new("network", name, "tx_total")
                .report(i.total_packets_transmitted())
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
            if let Some(r) = reported.as_mut() {
                r.insert(name);
            }
        }
        if let Some(r) = reported {
            for c in config.interfaces.as_ref().unwrap() {
                if !r.contains(c.as_str()) {
                    for res in [
                        "rxb",
                        "txb",
                        "rxb_total",
                        "txb_total",
                        "rx",
                        "tx",
                        "rx_total",
                        "tx_total",
                        "rx_err",
                        "tx_err",
                        "rx_err_total",
                        "tx_err_total",
                    ] {
                        Metric::new("network", c, res).failed().report(-1).await;
                    }
                }
            }
        }
    }
}