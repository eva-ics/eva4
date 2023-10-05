use crate::adsbr::Var;
use crate::common;
use eva_common::events::RawStateEventOwned;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
use eva_sdk::controller::{format_raw_state_topic, RawStateCache, RawStateEventPreparedOwned};
use eva_sdk::service::svc_is_terminating;
use log::{error, trace, warn};
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eva_sdk::prelude::err_logger;

err_logger!();

#[allow(clippy::too_many_lines)]
#[allow(clippy::cast_possible_wrap)]
async fn pull(
    symbols: &[common::PullSymbol],
    tx: async_channel::Sender<(String, Vec<u8>)>,
    raw_state_cache: Arc<RawStateCache>,
) {
    let mut oids_failed: HashSet<&OID> = HashSet::new();
    let mut states_pulled: HashMap<&OID, RawStateEventPreparedOwned> = HashMap::new();
    let timeout = crate::TIMEOUT.get().unwrap();
    let retries = crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst);
    let vars = symbols
        .iter()
        .map(|v| v.var().unwrap())
        .collect::<Vec<Arc<Var>>>();
    match crate::adsbr::read_multi(
        &vars.iter().map(AsRef::as_ref).collect::<Vec<&Var>>(),
        *timeout,
        retries,
    )
    .await
    {
        Ok(result) => {
            for (res, symbol) in result.into_iter().zip(symbols.iter()) {
                match res {
                    Ok(val) => {
                        for task in symbol.map() {
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
                                            if let Ok(n) = task.transform_value(val).log_err() {
                                                save_value!(Value::F64(n));
                                            } else {
                                                oids_failed.insert(task.oid());
                                            }
                                        } else {
                                            error!("{} value parse error", task.oid());
                                            oids_failed.insert(task.oid());
                                        }
                                    } else {
                                        save_value!($val.clone());
                                    }
                                };
                            }
                            if let Some(idx) = task.idx() {
                                if let Value::Seq(ref vals) = val {
                                    if let Some(value) = vals.get(idx) {
                                        process!(value);
                                    } else {
                                        warn!(
                                            "{} pull error: array does not contain the required index",
                                            task.oid()
                                        );
                                        oids_failed.insert(task.oid());
                                    }
                                } else if idx == 0 {
                                    process!(val.clone());
                                } else {
                                    error!(
                                        "{} pull error: {} value is not an array",
                                        task.oid(),
                                        symbol.symbol()
                                    );
                                    oids_failed.insert(task.oid());
                                }
                            } else {
                                process!(val.clone());
                            }
                        }
                    }
                    Err(e) => {
                        error!("{} pull error: {}", symbol.symbol(), e);
                        for task in symbol.map() {
                            oids_failed.insert(task.oid());
                        }
                    }
                }
            }
        }
        Err(e) => {
            error!("pull error: {}", e);
            crate::poc().await;
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
    symbols: Vec<common::PullSymbol>,
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
        pull(&symbols, tx.clone(), raw_state_cache.clone()).await;
        last_ticked.replace(t);
    }
}
