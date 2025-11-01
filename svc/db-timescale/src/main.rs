use busrt::QoS;
use eva_common::acl::{OIDMask, OIDMaskList};
use eva_common::common_payloads::ValueOrList;
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::prelude::*;
use eva_internal::RtcSyncedInterval;
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use eva_sdk::service::set_poc;
use eva_sdk::types::{Fill, StateProp};
use eva_sdk::types::{ItemState, ShortItemStateConnected, State};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{atomic, Arc};
use std::time::Duration;

err_logger!();

mod common;
mod db;
mod timescale;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Timescale database service";

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const OID_CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static SKIP_DISCONNECTED: atomic::AtomicBool = atomic::AtomicBool::new(false);

static EVA_PG_ENABLED: atomic::AtomicBool = atomic::AtomicBool::new(false);

pub fn history_table_name(xopts: &BTreeMap<String, Value>) -> EResult<String> {
    if let Some(t) = xopts.get("rp") {
        let name = format!("rp_{}", t);
        check_sql_safe(&name, "rp")?;
        Ok(name)
    } else {
        Ok("state_history_events".to_owned())
    }
}

pub fn check_sql_safe(s: &str, param: &str) -> EResult<()> {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        Ok(())
    } else {
        Err(Error::invalid_data(format!(
            "invalid characters in the parameter {}",
            param
        )))
    }
}

pub fn eva_pg_enabled() -> bool {
    EVA_PG_ENABLED.load(atomic::Ordering::Relaxed)
}

struct Handlers {
    tx: async_channel::Sender<Event>,
    info: ServiceInfo,
}

fn process_state(
    topic: &str,
    path: &str,
    payload: &[u8],
    tx: &async_channel::Sender<Event>,
) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<State>(payload) {
            Ok(v) => {
                // TODO move to task pool (remove try_send)
                let res = tx
                    .try_send(Event::State(ItemState::from_state(v, oid)))
                    .map_err(Error::core);
                if res.is_err() {
                    poc();
                }
                res?;
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "state_history_combined" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct StateHistoryCombinedParams {
                    i: ValueOrList<String>,
                    #[serde(alias = "s")]
                    t_start: Option<f64>,
                    #[serde(alias = "e")]
                    t_end: Option<f64>,
                    #[serde(alias = "w")]
                    fill: Fill,
                    #[serde(alias = "p")]
                    precision: Option<u32>,
                    #[serde(alias = "o", rename = "xopts", default)]
                    xopts: BTreeMap<String, Value>,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: StateHistoryCombinedParams = unpack(payload)?;
                let data = timescale::state_history_combined(
                    &p.i.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    p.t_start
                        .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                    p.t_end,
                    p.fill,
                    p.precision,
                    p.xopts,
                )
                .await
                .log_err()?;
                Ok(Some(pack(&data)?))
            }
            "state_history" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct StateHistoryParams {
                    i: String,
                    #[serde(alias = "s")]
                    t_start: Option<f64>,
                    #[serde(alias = "e")]
                    t_end: Option<f64>,
                    #[serde(alias = "w")]
                    fill: Option<Fill>,
                    #[serde(alias = "p")]
                    precision: Option<u32>,
                    #[serde(alias = "n")]
                    limit: Option<usize>,
                    #[serde(alias = "x")]
                    prop: Option<StateProp>,
                    #[serde(alias = "o", rename = "xopts", default)]
                    xopts: BTreeMap<String, Value>,
                    #[serde(default)]
                    compact: bool,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: StateHistoryParams = unpack(payload)?;
                    let data = if let Some(fill) = p.fill {
                        timescale::state_history_filled(
                            &p.i,
                            p.t_start
                                .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                            p.t_end,
                            fill,
                            p.precision,
                            p.limit,
                            p.prop,
                            p.xopts,
                            p.compact,
                        )
                        .await
                        .log_err()?
                    } else {
                        db::state_history(
                            &p.i,
                            p.t_start
                                .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                            p.t_end,
                            p.precision,
                            p.limit,
                            p.prop,
                            p.xopts,
                            p.compact,
                        )
                        .await
                        .log_err()?
                    };
                    Ok(Some(pack(&data)?))
                }
            }
            "state_log" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct StateLogParams {
                    i: OIDMask,
                    #[serde(alias = "s")]
                    t_start: Option<f64>,
                    #[serde(alias = "e")]
                    t_end: Option<f64>,
                    #[serde(alias = "n")]
                    limit: Option<usize>,
                    #[serde(alias = "o", default)]
                    xopts: BTreeMap<String, Value>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let mut p: StateLogParams = unpack(payload)?;
                    let offset: Option<usize> = if let Some(o) = p.xopts.remove("offset") {
                        Some(TryInto::<u32>::try_into(o)? as usize)
                    } else {
                        None
                    };
                    let data = db::state_log(
                        p.i.to_wildcard_oid()?,
                        p.t_start
                            .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                        p.t_end,
                        p.limit,
                        offset,
                        p.xopts,
                    )
                    .await
                    .log_err()?;
                    Ok(Some(pack(&data)?))
                }
            }
            "state_push" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: Event = unpack(payload)?;
                    match p {
                        Event::State(v) => db::submit(v).await?,
                        Event::BulkState(v) => db::submit_bulk(v, false).await?,
                    };
                    Ok(None)
                }
            }
            m => svc_handle_default_rpc(m, &self.info),
        }
    }
    async fn handle_frame(&self, frame: Frame) {
        svc_need_ready!();
        if frame.kind() == busrt::FrameKind::Publish {
            if let Some(topic) = frame.topic() {
                if let Some(o) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), &self.tx).log_ef();
                } else if let Some(o) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), &self.tx).log_ef();
                } else if let Some(o) = topic.strip_prefix(REMOTE_ARCHIVE_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), &self.tx).log_ef();
                }
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Event {
    State(ItemState),
    BulkState(Vec<ItemState>),
}

async fn notify(event: Event) -> EResult<()> {
    match event {
        Event::State(v) => {
            let res = db::submit(v).await;
            if res.is_err() {
                poc();
            }
            res
        }
        Event::BulkState(v) => {
            let res = db::submit_bulk(v, true).await;
            if res.is_err() {
                poc();
            }
            res
        }
    }
}

async fn sender(rx: async_channel::Receiver<Event>, buf_ttl: Option<Duration>) {
    if let Some(bttl) = buf_ttl {
        let mut buf_interval = tokio::time::interval(bttl);
        let mut data_buf: Vec<ItemState> = Vec::new();
        loop {
            tokio::select! {
                f = rx.recv() => {
                    if let Ok(event) = f {
                        match event {
                            Event::State(state) => {
                                data_buf.push(state);
                            },
                            Event::BulkState(_) => {
                                    notify(event).await.log_ef();
                                    },
                        }
                    } else {
                        break;
                    }
                }

                _ = buf_interval.tick() => {
                    if !data_buf.is_empty() {
                        let data = std::mem::take(&mut data_buf);
                        notify(Event::BulkState(data)).await.log_ef();
                    }
                }
            }
        }
    } else {
        while let Ok(event) = rx.recv().await {
            notify(event).await.log_ef();
        }
    }
}

async fn collect_periodic(
    oids: &OIDMaskList,
    oids_exclude: &OIDMaskList,
    interval: Duration,
    tx: &async_channel::Sender<Event>,
) -> EResult<()> {
    #[derive(Serialize)]
    struct ParamsState {
        i: Vec<String>,
        exclude: Vec<String>,
    }
    let i: Vec<String> = oids.oid_masks().iter().map(ToString::to_string).collect();
    let exclude: Vec<String> = oids_exclude
        .oid_masks()
        .iter()
        .map(ToString::to_string)
        .collect();
    let p = ParamsState { i, exclude };
    let payload = pack(&p)?;
    let mut int = RtcSyncedInterval::new(interval);
    let rpc = RPC.get().unwrap();
    while !svc_is_terminating() {
        let t = int.tick().await;
        let data = rpc
            .call(
                "eva.core",
                "item.state",
                (payload.as_slice()).into(),
                QoS::Processed,
            )
            .await?;
        let mut states: Vec<ShortItemStateConnected> = unpack(data.payload())?;
        let skip_disconnected = SKIP_DISCONNECTED.load(atomic::Ordering::Relaxed);
        states.retain(|s| {
            (!skip_disconnected || s.connected) && s.value.as_ref().map_or(true, Value::is_numeric)
        });
        if !states.is_empty() {
            tx.send(Event::BulkState(
                states
                    .into_iter()
                    .map(|s| ItemState {
                        oid: s.oid,
                        status: s.status,
                        value: s.value,
                        set_time: t.timestamp(),
                    })
                    .collect(),
            ))
            .await
            .map_err(Error::core)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    EVA_PG_ENABLED.store(config.eva_pg, atomic::Ordering::Relaxed);
    SKIP_DISCONNECTED.store(config.skip_disconnected, atomic::Ordering::Relaxed);
    set_poc(config.panic_in);
    let timeout = initial.timeout();
    let (tx, rx) = async_channel::bounded(config.queue_size);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("state_history")
            .required("i")
            .optional("t_start")
            .optional("t_end")
            .optional("fill")
            .optional("precision")
            .optional("limit")
            .optional("prop")
            .optional("compact"),
    );
    info.add_method(
        ServiceMethod::new("state_log")
            .required("i")
            .optional("t_start")
            .optional("t_end")
            .optional("limit"),
    );
    info.add_method(
        ServiceMethod::new("state_history_combined")
            .required("i")
            .optional("t_start")
            .optional("t_end")
            .optional("fill")
            .optional("precision"),
    );
    let rpc: Arc<RpcClient> = initial
        .init_rpc(Handlers {
            tx: tx.clone(),
            info,
        })
        .await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    db::init(
        &config.db,
        config.pool_size.unwrap_or_else(|| initial.workers() * 5),
        timeout,
    )
    .await?;
    tokio::spawn(async move {
        sender(rx, config.buf_ttl_sec).await;
    });
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("unable to set RPC"))?;
    if !config.ignore_events {
        eva_sdk::service::exclude_oids(
            rpc.as_ref(),
            &config.oids_exclude,
            eva_sdk::service::EventKind::Any,
        )
        .await?;
        eva_sdk::service::subscribe_oids(
            rpc.as_ref(),
            &config.oids,
            eva_sdk::service::EventKind::Any,
        )
        .await?;
    }
    let keep = config.keep;
    let collect_fut = if let Some(interval) = config.interval {
        let rpc_c = rpc.clone();
        let startup_timeout = initial.startup_timeout();
        let fut = tokio::spawn(async move {
            let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
            while !svc_is_terminating() {
                collect_periodic(&config.oids, &config.oids_exclude, interval, &tx)
                    .await
                    .log_ef();
            }
        });
        Some(fut)
    } else {
        None
    };
    svc_start_signal_handlers();
    if let Some(keep) = keep {
        eva_common::cleaner!("records", db::cleanup_events, CLEANUP_INTERVAL, keep);
    }
    if config.cleanup_oids {
        eva_common::cleaner!("oids", db::cleanup_oids, OID_CLEANUP_INTERVAL);
    }
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    if let Some(f) = collect_fut {
        f.abort();
    }
    svc_mark_terminating(&client).await?;
    Ok(())
}
