use crate::common;
use crate::enip;
use crate::types::EipType;
use eva_common::events::RawStateEventOwned;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
use eva_sdk::controller::{format_raw_state_topic, RawStateCache, RawStateEventPreparedOwned};
use eva_sdk::service::poc;
use log::{error, trace, warn};
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eva_sdk::prelude::err_logger;

err_logger!();

#[allow(clippy::too_many_lines)]
#[allow(clippy::cast_possible_wrap)]
fn pull_plc(
    tags: &[common::PullTag],
    tx: async_channel::Sender<(String, Vec<u8>)>,
    raw_state_cache: Arc<RawStateCache>,
) {
    let mut oids_failed: HashSet<&OID> = HashSet::new();
    let mut states_pulled: HashMap<&OID, RawStateEventPreparedOwned> = HashMap::new();
    let timeout = *crate::TIMEOUT.get().unwrap();
    let retries = crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst);
    for tag in tags {
        trace!("pulling tag {}", tag.path());
        if let Err(e) = enip::pull_tag(tag.path(), tag.id(), timeout, retries) {
            logreducer::error!("pull", "Unable to pull: {}", e);
            poc();
            for task in tag.map() {
                oids_failed.insert(task.oid());
            }
        } else {
            logreducer::clear_error!("pull");
            for task in tag.map() {
                macro_rules! save_val {
                    ($val: expr) => {{
                        let raw_state = RawStateEventOwned::new(1, $val);
                        states_pulled.insert(
                            &task.oid(),
                            RawStateEventPreparedOwned::from_rse_owned(
                                raw_state,
                                task.value_delta(),
                            ),
                        );
                    }};
                }
                macro_rules! process_tag {
                    ($fn:path, $vt: expr) => {
                        let val = $fn(tag.id(), task.offset() as i32);
                        if task.need_transform() {
                            if let Ok(n) = task.transform_value(val).log_err() {
                                save_val!(Value::F64(n));
                            } else {
                                oids_failed.insert(task.oid());
                            }
                        } else {
                            save_val!($vt(val))
                        }
                    };
                }
                match task.tp() {
                    EipType::Bit => {
                        // bit task offset is already bit offset
                        let bit_no = task.offset() + task.bit();
                        if bit_no > i32::MAX as u32 {
                            logreducer::error!(
                                format!("pull::{}", tag.path()),
                                "bit index overflow for {}",
                                tag.path()
                            );
                            poc();
                        } else {
                            match enip::safe_get_bit(tag.id(), bit_no as i32) {
                                Ok(v) => {
                                    logreducer::clear_error!(format!("pull::{}", tag.path()));
                                    let value = Value::U8(v);
                                    save_val!(value);
                                }
                                Err(e) => {
                                    logreducer::error!(
                                        format!("pull::{}", tag.path()),
                                        "Unable to process PLC tag {}, bit get error: {}",
                                        tag.path(),
                                        e
                                    );
                                    poc();
                                    oids_failed.insert(task.oid());
                                }
                            }
                        }
                    }
                    EipType::Uint8 => unsafe {
                        process_tag!(plctag::plc_tag_get_uint8, Value::U8);
                    },
                    EipType::Int8 => unsafe {
                        process_tag!(plctag::plc_tag_get_int8, Value::I8);
                    },
                    EipType::Uint16 => unsafe {
                        process_tag!(plctag::plc_tag_get_uint16, Value::U16);
                    },
                    EipType::Int16 => unsafe {
                        process_tag!(plctag::plc_tag_get_int16, Value::I16);
                    },
                    EipType::Uint32 => unsafe {
                        process_tag!(plctag::plc_tag_get_uint32, Value::U32);
                    },
                    EipType::Int32 => unsafe {
                        process_tag!(plctag::plc_tag_get_int32, Value::I32);
                    },
                    EipType::Uint64 => unsafe {
                        process_tag!(plctag::plc_tag_get_uint64, Value::U64);
                    },
                    EipType::Int64 => unsafe {
                        process_tag!(plctag::plc_tag_get_int64, Value::I64);
                    },
                    EipType::Real32 => unsafe {
                        process_tag!(plctag::plc_tag_get_float32, Value::F32);
                    },
                    EipType::Real64 => unsafe {
                        process_tag!(plctag::plc_tag_get_float64, Value::F64);
                    },
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
            Err(e) => logreducer::error!(
                format!("pull::{}", oid),
                "state serialization error for {}: {}",
                oid,
                e
            ),
        }
    }
}

async fn pull(
    tags: Arc<Vec<common::PullTag>>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
    raw_state_cache: Arc<RawStateCache>,
) -> EResult<()> {
    tokio::task::spawn_blocking(move || pull_plc(&tags, tx, raw_state_cache)).await?;
    Ok(())
}

pub async fn launch(
    tags: Vec<common::PullTag>,
    interval: Duration,
    cache_time: Option<Duration>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
    id: usize,
) {
    log::info!("starting puller #{}, interval: {:?}", id, interval);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_ticked: Option<Instant> = None;
    let tags_c = Arc::new(tags);
    let raw_state_cache = Arc::new(RawStateCache::new(cache_time));
    loop {
        let t = ticker.tick().await.into_std();
        if let Some(prev) = last_ticked {
            if t - prev > interval {
                warn!("PLC puller timeout");
            }
        }
        if let Err(e) = pull(tags_c.clone(), tx.clone(), raw_state_cache.clone()).await {
            error!("PLC error: {}", e);
            poc();
            tokio::time::sleep(crate::SLEEP_ON_ERROR_INTERVAL).await;
        }
        last_ticked.replace(t);
    }
}
