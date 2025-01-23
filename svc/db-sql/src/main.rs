use busrt::QoS;
use eva_common::acl::{OIDMask, OIDMaskList};
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use eva_sdk::service::set_poc;
use eva_sdk::types::{Fill, StateProp};
use eva_sdk::types::{ItemState, ShortItemStateConnected, State};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{atomic, Arc};
use std::time::Duration;

err_logger!();

mod common;
mod db;
mod timescale;

use common::TsExtension;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "SQL database service";

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static TS_EXTENSION: OnceCell<Option<TsExtension>> = OnceCell::new();
static SKIP_DISCONNECTED: atomic::AtomicBool = atomic::AtomicBool::new(false);

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
            "state_history" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct StateHistoryParams {
                    i: OID,
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
                        let ts_ext = TS_EXTENSION.get().unwrap();
                        if let Some(x) = ts_ext {
                            match x {
                                TsExtension::Timescale => timescale::state_history_filled(
                                    p.i,
                                    p.t_start.unwrap_or_else(|| {
                                        eva_common::time::now_ns_float() - 86400.0
                                    }),
                                    p.t_end,
                                    fill,
                                    p.precision,
                                    p.limit,
                                    p.prop,
                                    p.xopts,
                                    p.compact,
                                )
                                .await
                                .log_err()?,
                            }
                        } else {
                            return Err(Error::unsupported("no ts extension set").into());
                        }
                    } else {
                        db::state_history(
                            p.i,
                            p.t_start
                                .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                            p.t_end,
                            p.precision,
                            p.limit,
                            p.prop,
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
                    xopts: HashMap<String, Value>,
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
                    notify(p).await?;
                    Ok(None)
                }
            }
            m => svc_handle_default_rpc(m, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
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
            let res = db::submit_bulk(v).await;
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
    let mut int = tokio::time::interval(interval);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let rpc = RPC.get().unwrap();
    while !svc_is_terminating() {
        int.tick().await;
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
        states.retain(|s| !skip_disconnected || s.connected);
        if !states.is_empty() {
            let t = eva_common::time::now_ns_float();
            tx.send(Event::BulkState(
                states
                    .into_iter()
                    .map(|s| ItemState {
                        oid: s.oid,
                        status: s.status,
                        value: s.value,
                        set_time: t,
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
    set_poc(config.panic_in);
    TS_EXTENSION
        .set(config.ts_extension)
        .map_err(|_| Error::core("unable to set TS_EXTENSION"))?;
    SKIP_DISCONNECTED.store(config.skip_disconnected, atomic::Ordering::Relaxed);
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
    let rpc: Arc<RpcClient> = initial
        .init_rpc(Handlers {
            tx: tx.clone(),
            info,
        })
        .await?;
    initial.drop_privileges()?;
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
    let client = rpc.client().clone();
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
    let simple_cleaning = config.simple_cleaning;
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
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    if let Some(keep) = keep {
        eva_common::cleaner!(
            "records",
            db::cleanup,
            CLEANUP_INTERVAL,
            keep,
            simple_cleaning
        );
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
