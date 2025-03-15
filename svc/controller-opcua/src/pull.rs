use crate::common;
use crate::conv::ValueConv;
use eva_common::events::RawStateEventOwned;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
use eva_sdk::controller::{format_raw_state_topic, RawStateCache, RawStateEventPreparedOwned};
use eva_sdk::service::poc;
use eva_sdk::service::svc_is_terminating;
use log::{error, trace, warn};
use opcua::client::prelude::NodeId;
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eva_sdk::prelude::err_logger;

err_logger!();

#[allow(clippy::too_many_lines)]
#[allow(clippy::cast_possible_wrap)]
async fn pull(
    nodes: &[common::PullNode],
    tx: async_channel::Sender<(String, Vec<u8>)>,
    raw_state_cache: Arc<RawStateCache>,
) {
    let mut oids_failed: HashSet<&OID> = HashSet::new();
    let mut states_pulled: HashMap<&OID, RawStateEventPreparedOwned> = HashMap::new();
    let timeout = *crate::TIMEOUT.get().unwrap();
    let retries = crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst);
    let vars = nodes
        .iter()
        .map(|v| v.node().clone())
        .collect::<Vec<NodeId>>();
    let ranges = nodes
        .iter()
        .map(common::PullNode::range)
        .collect::<Vec<Option<&str>>>();
    match crate::comm::read_multi(vars, ranges, timeout, retries).await {
        Ok(result) => {
            logreducer::clear_error!("pull");
            for (res, node) in result.into_iter().zip(nodes.iter()) {
                if let Some(val) = res.value.and_then(|v| {
                    v.into_eva_value()
                        .map_err(|e| node.report_error("value conversion", Some(e)))
                        .ok()
                }) {
                    node.set_online();
                    for task in node.map() {
                        macro_rules! save_value {
                            ($val: expr) => {
                                let raw_state = RawStateEventOwned::new(1, $val);
                                states_pulled.insert(
                                    &task.oid(),
                                    RawStateEventPreparedOwned::from_rse_owned(
                                        raw_state,
                                        task.value_delta(),
                                    ),
                                );
                            };
                        }
                        macro_rules! process {
                            ($val: expr) => {
                                if task.need_transform() {
                                    if let Ok(val) = TryInto::<f64>::try_into($val) {
                                        match task.transform_value(val) {
                                            Ok(n) => {
                                                task.set_online();
                                                save_value!(Value::F64(n));
                                            }
                                            Err(e) => {
                                                task.report_error("transform", Some(e));
                                                oids_failed.insert(task.oid());
                                            }
                                        }
                                    } else {
                                        task.report_error(
                                            &format!("{} value parse error", task.oid()),
                                            None,
                                        );
                                        oids_failed.insert(task.oid());
                                    }
                                } else {
                                    task.set_online();
                                    save_value!($val.clone());
                                }
                            };
                        }
                        if let Some(idx) = task.idx() {
                            if let Some(v) = val.get_by_index(idx) {
                                process!(v);
                            } else {
                                task.report_error(&format!(
                                    "{} pull error: {} value is not an array or index is out of bounds",
                                    task.oid(),
                                    node.node()
                                ), None);
                                oids_failed.insert(task.oid());
                            }
                        } else {
                            process!(val.clone());
                        }
                    }
                } else {
                    node.report_error("pull", None);
                    for task in node.map() {
                        oids_failed.insert(task.oid());
                    }
                }
            }
        }
        Err(e) => {
            logreducer::error!("pull", "pull error: {}", e);
            poc();
        }
    }
    for oid in oids_failed {
        if let Some(raw_state) = states_pulled.get_mut(oid) {
            raw_state.state_mut().status = ITEM_STATUS_ERROR;
        } else {
            states_pulled.insert(
                oid,
                RawStateEventPreparedOwned::from_rse_owned(
                    RawStateEventOwned::new0(ITEM_STATUS_ERROR),
                    None,
                ),
            );
        }
    }
    raw_state_cache.retain_map_modified(&mut states_pulled);
    for (oid, raw_state_prepared) in states_pulled {
        let raw_state = raw_state_prepared.state();
        trace!("{}: {:?}", oid, raw_state);
        match pack(&raw_state) {
            Ok(payload) => {
                if let Err(e) = tx.try_send((format_raw_state_topic(oid), payload)) {
                    error!("state queue error for {}: {}", oid, e);
                }
            }
            Err(e) => error!("state serialization error for {}: {}", oid, e),
        }
    }
}

pub async fn launch(
    nodes: Vec<common::PullNode>,
    interval: Duration,
    cache_time: Option<Duration>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
) {
    log::info!("starting puller, interval: {:?}", interval);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_ticked: Option<Instant> = None;
    let raw_state_cache = Arc::new(RawStateCache::new(cache_time));
    while !svc_is_terminating() {
        let t = ticker.tick().await.into_std();
        if let Some(prev) = last_ticked {
            if t - prev > interval {
                warn!("PLC puller timeout");
            }
        }
        pull(&nodes, tx.clone(), raw_state_cache.clone()).await;
        last_ticked.replace(t);
    }
}
