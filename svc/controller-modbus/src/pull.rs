use crate::common;
use crate::modbus::parse_block_value;
use crate::types::RegisterKind;
use eva_common::events::RawStateEventOwned;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
use eva_sdk::controller::{format_raw_state_topic, RawStateCache, RawStateEventPreparedOwned};
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use log::{error, trace, warn};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eva_sdk::prelude::err_logger;

err_logger!();

#[allow(clippy::too_many_lines)]
async fn pull(
    regs: &[common::PullReg],
    tx: async_channel::Sender<(String, Vec<u8>)>,
    raw_state_cache: Arc<RawStateCache>,
    timeout: Duration,
) -> EResult<()> {
    let mut oids_failed: HashSet<&OID> = HashSet::new();
    let mut states_pulled: HashMap<&OID, RawStateEventPreparedOwned> = HashMap::new();
    macro_rules! save_val {
        ($task: expr, $val: expr) => {
            if let Some(raw_state_prepared) = states_pulled.get_mut($task.oid()) {
                let raw_state = raw_state_prepared.state_mut();
                raw_state.value = ValueOptionOwned::Value($val);
            } else {
                let raw_state = RawStateEventOwned::new(1, $val);
                states_pulled.insert(
                    &$task.oid(),
                    RawStateEventPreparedOwned::from_rse_owned(raw_state, $task.value_delta()),
                );
            }
        };
    }
    for pull_reg in regs {
        trace!("pulling register {:?}", pull_reg.reg);
        let modbus_client = crate::MODBUS_CLIENT.get().unwrap();
        macro_rules! mark_failed {
            ($err: expr) => {{
                if svc_is_terminating() {
                    return Ok(());
                }
                logreducer::error!(
                    format!("pull::{}", pull_reg.reg),
                    "block pull failed for {} / {}: {}",
                    pull_reg.reg,
                    pull_reg.count,
                    $err
                );
                for task in pull_reg.map() {
                    oids_failed.insert(task.oid());
                }
            }};
        }
        macro_rules! mark_task_failed {
            ($task: expr) => {{
                oids_failed.insert($task.oid());
            }};
        }
        match pull_reg.reg.kind() {
            RegisterKind::Coil | RegisterKind::Discrete => {
                match modbus_client
                    .safe_get_bool(pull_reg.unit, pull_reg.reg, pull_reg.count, timeout)
                    .await
                {
                    Ok(data) => {
                        logreducer::clear_error!(format!("pull::{}", pull_reg.reg));
                        for task in pull_reg.map() {
                            if let Some(val) = data.get(task.block_offset() as usize) {
                                let value = Value::U8(u8::from(*val));
                                if task.need_transform() {
                                    if let Ok(val) = TryInto::<f64>::try_into(value).log_err() {
                                        if let Ok(n) = task.transform_value(val).log_err() {
                                            save_val!(task, Value::F64(n));
                                            logreducer::clear_error!(format!(
                                                "pull::{}",
                                                task.oid()
                                            ));
                                        } else {
                                            oids_failed.insert(task.oid());
                                        }
                                    } else {
                                        logreducer::error!(
                                            format!("pull::{}", task.oid()),
                                            "unable to parse value for {}",
                                            task.oid()
                                        );
                                        oids_failed.insert(task.oid());
                                    }
                                } else {
                                    save_val!(task, value);
                                    logreducer::clear_error!(format!("pull::{}", task.oid()));
                                }
                            } else {
                                logreducer::error!(
                                    format!("pull::{}", task.oid()),
                                    "the block does not contain data for {}",
                                    task.oid(),
                                );
                                mark_task_failed!(task);
                            }
                        }
                    }
                    Err(e) => mark_failed!(e),
                }
            }
            RegisterKind::Input | RegisterKind::Holding => {
                match modbus_client
                    .safe_get_u16(pull_reg.unit, pull_reg.reg, pull_reg.count, timeout)
                    .await
                {
                    Ok(data) => {
                        for task in pull_reg.map() {
                            match parse_block_value(
                                &data,
                                task.block_offset(),
                                task.tp(),
                                task.bit(),
                            ) {
                                Ok(value) => {
                                    if task.need_transform() {
                                        if let Ok(val) = TryInto::<f64>::try_into(value).log_err() {
                                            if let Ok(n) = task.transform_value(val).log_err() {
                                                save_val!(task, Value::F64(n));
                                                logreducer::clear_error!(format!(
                                                    "pull::{}",
                                                    task.oid()
                                                ));
                                            }
                                        }
                                    } else {
                                        save_val!(task, value);
                                        logreducer::clear_error!(format!("pull::{}", task.oid()));
                                    }
                                }
                                Err(e) => {
                                    logreducer::error!(
                                        format!("pull::{}", task.oid()),
                                        "parse error for {}: {}",
                                        task.oid(),
                                        e
                                    );
                                    mark_task_failed!(task);
                                }
                            }
                        }
                    }
                    Err(e) => mark_failed!(e),
                }
            }
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
                    logreducer::error!(
                        format!("pull::{}", oid),
                        "state queue error for {}: {}",
                        oid,
                        e
                    );
                } else {
                    logreducer::clear_error!(format!("pull::{}", oid));
                }
            }
            Err(e) => error!("state serialization error for {}: {}", oid, e),
        }
    }
    Ok(())
}

pub async fn launch(
    regs: Vec<common::PullReg>,
    interval: Duration,
    cache_time: Option<Duration>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
    id: usize,
    timeout: Duration,
) {
    log::info!("starting puller #{}, interval: {:?}", id, interval);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_ticked: Option<Instant> = None;
    let raw_state_cache = Arc::new(RawStateCache::new(cache_time));
    loop {
        let t = ticker.tick().await.into_std();
        if let Some(prev) = last_ticked {
            if t - prev > interval {
                warn!("PLC puller timeout");
            }
        }
        if let Err(e) = pull(&regs, tx.clone(), raw_state_cache.clone(), timeout).await {
            logreducer::error!("pull", "PLC error: {}", e);
            poc();
            tokio::time::sleep(crate::SLEEP_ON_ERROR_INTERVAL).await;
        } else {
            logreducer::clear_error!("pull");
        }
        last_ticked.replace(t);
    }
}
