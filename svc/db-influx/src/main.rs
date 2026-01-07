use busrt::QoS;
use eva_common::acl::OIDMaskList;
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::prelude::*;
use eva_common::time::ts_to_ns;
use eva_internal::RtcSyncedInterval;
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use eva_sdk::service::set_poc;
use eva_sdk::types::{Fill, ItemState, ShortItemStateConnected, State, StateProp};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use simple_pool::ResourcePool;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, atomic};
use std::time::Duration;

err_logger!();

mod common;
mod influx;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "InfluxDB database service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static CLIENT_POOL: OnceCell<ResourcePool<influx::InfluxClient>> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();
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
                if v.value.as_ref().map_or(true, Value::is_numeric) {
                    // TODO move to task pool (remove try_send)
                    let res = tx
                        .try_send(Event::State(ItemState::from_state(v, oid)))
                        .map_err(Error::core);
                    if res.is_err() {
                        poc();
                    }
                    res?;
                }
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
                    #[serde(alias = "o", default)]
                    xopts: BTreeMap<String, Value>,
                    #[serde(default)]
                    compact: bool,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: StateHistoryParams = unpack(payload)?;
                    let influx_client = tokio::time::timeout(
                        *TIMEOUT.get().unwrap(),
                        CLIENT_POOL.get().unwrap().get(),
                    )
                    .await
                    .map_err(Into::<Error>::into)?;
                    let data = influx_client
                        .state_history(
                            p.i,
                            p.t_start
                                .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                            p.t_end,
                            p.fill,
                            p.precision,
                            p.limit,
                            p.prop,
                            p.xopts,
                            p.compact,
                        )
                        .await
                        .log_err()?;
                    Ok(Some(pack(&data)?))
                }
            }
            "state_log" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct StateLogParams {
                    i: OID,
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
                    let p: StateLogParams = unpack(payload)?;
                    let influx_client = tokio::time::timeout(
                        *TIMEOUT.get().unwrap(),
                        CLIENT_POOL.get().unwrap().get(),
                    )
                    .await
                    .map_err(Into::<Error>::into)?;
                    let data = influx_client
                        .state_log(
                            p.i,
                            p.t_start
                                .unwrap_or_else(|| eva_common::time::now_ns_float() - 86400.0),
                            p.t_end,
                            p.limit,
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
                    notify(p.as_data()).await?;
                    Ok(None)
                }
            }
            m => svc_handle_default_rpc(m, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
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

enum Data<'a> {
    Single(&'a ItemState),
    Bulk(&'a Vec<ItemState>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Event {
    State(ItemState),
    BulkState(Vec<ItemState>),
}

impl Event {
    fn as_data(&self) -> Data<'_> {
        match self {
            Event::State(v) => Data::Single(v),
            Event::BulkState(v) => Data::Bulk(v),
        }
    }
}

async fn notify(data: Data<'_>) -> EResult<()> {
    let mut batch_q = String::new();
    macro_rules! append_state {
        ($state: expr) => {
            if !batch_q.is_empty() {
                batch_q += "\n";
            }
            let t = ts_to_ns($state.set_time);
            write!(batch_q, "{} status={}i", $state.oid, $state.status).map_err(Error::failed)?;
            if let Some(ref val) = $state.value {
                if let Ok(v) = TryInto::<f64>::try_into(val) {
                    write!(batch_q, ",value={}", v).map_err(Error::failed)?;
                }
            }
            write!(batch_q, " {}", t).map_err(Error::failed)?;
        };
    }
    match data {
        Data::Single(state) => {
            append_state!(state);
        }
        Data::Bulk(states) => {
            for state in states {
                append_state!(state);
            }
        }
    }
    let influx_client =
        tokio::time::timeout(*TIMEOUT.get().unwrap(), CLIENT_POOL.get().unwrap().get()).await?;
    let res = influx_client.submit(batch_q).await;
    if res.is_err() {
        poc();
    }
    res
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
                            Event::BulkState(data) => {
                                    notify(Data::Bulk(&data)).await.log_ef();
                                    },
                        }
                    } else {
                        break;
                    }
                }

                _ = buf_interval.tick() => {
                    if !data_buf.is_empty() {
                        notify(Data::Bulk(&data_buf)).await.log_ef();
                        data_buf.clear();
                    }
                }
            }
        }
    } else {
        while let Ok(event) = rx.recv().await {
            match event {
                Event::State(state) => notify(Data::Single(&state)).await.log_ef(),
                Event::BulkState(data) => notify(Data::Bulk(&data)).await.log_ef(),
            };
        }
    }
}

async fn collect_periodic(
    oids: &OIDMaskList,
    oids_exclude: &OIDMaskList,
    oids_exclude_null: &OIDMaskList,
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
            (!skip_disconnected || s.connected)
                && s.value
                    .as_ref()
                    .map_or_else(|| !oids_exclude_null.matches(&s.oid), Value::is_numeric)
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
    set_poc(config.panic_in);
    let (tx, rx) = async_channel::bounded(config.queue_size);
    tokio::spawn(async move {
        sender(rx, config.buf_ttl_sec).await;
    });
    let client_pool = ResourcePool::new();
    let timeout = initial.timeout();
    let influx_client = influx::InfluxClient::create(&config, timeout).await?;
    for _ in 0..config.clients.unwrap_or_else(|| initial.workers()) {
        client_pool.append(influx_client.clone());
    }
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("unable to set timeout"))?;
    CLIENT_POOL
        .set(client_pool)
        .map_err(|_| Error::core("unable to set client pool"))?;
    SKIP_DISCONNECTED.store(config.skip_disconnected, atomic::Ordering::Relaxed);
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
    let client = rpc.client();
    let collect_fut = if let Some(interval) = config.interval {
        let rpc_c = rpc.clone();
        let startup_timeout = initial.startup_timeout();
        let fut = tokio::spawn(async move {
            let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
            while !svc_is_terminating() {
                collect_periodic(
                    &config.oids,
                    &config.oids_exclude,
                    &config.oids_exclude_null,
                    interval,
                    &tx,
                )
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
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    if let Some(f) = collect_fut {
        f.abort();
    }
    svc_mark_terminating(&client).await?;
    Ok(())
}
