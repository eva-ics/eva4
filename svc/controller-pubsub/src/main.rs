use eva_common::acl::OIDMaskList;
use eva_common::common_payloads::{ParamsId, ValueOrList};
use eva_common::events::{
    LocalStateEvent, RawStateEvent, RawStateEventOwned, RemoteStateEvent, LOCAL_STATE_TOPIC,
    RAW_STATE_TOPIC, REMOTE_STATE_TOPIC,
};
use eva_common::prelude::*;
use eva_sdk::controller::{
    format_action_topic, transform, Action, RawStateCache, RawStateEventPreparedOwned,
};
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use eva_sdk::types::State;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use psrpc::pubsub;
use serde::{Deserialize, Serialize};
use std::collections::{hash_map, BTreeMap, HashMap};
use std::sync::{atomic, Arc};
use std::time::Duration;
use tokio::sync::oneshot;
use ttl_cache::TtlCache;

mod common;

use common::safe_run_macro;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "PubSub controller gateway service";

const CACHE_SIZE: usize = 100_000;
const CACHE_TTL: Duration = Duration::from_secs(60);

struct PubSubTask {
    topic: Arc<String>,
    payload: Vec<u8>,
    qos: i32,
    processed: Option<oneshot::Sender<()>>,
}

static SYSTEM_NAME: OnceCell<String> = OnceCell::new();
static BUS_TX: OnceCell<async_channel::Sender<(String, Vec<u8>)>> = OnceCell::new();
static PUBSUB_TX: OnceCell<async_channel::Sender<PubSubTask>> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();
static DEFAULT_QOS: atomic::AtomicI32 = atomic::AtomicI32::new(1);
static STATE_CACHE: Lazy<Mutex<TtlCache<OID, Arc<State>>>> =
    Lazy::new(|| Mutex::new(TtlCache::new(CACHE_SIZE)));

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
    output: BTreeMap<OID, Vec<Arc<OutputMapping>>>,
    action_map: BTreeMap<OID, OutputMapping>,
}

fn format_output_string(s: &str) -> String {
    s.replace("${system_name}", SYSTEM_NAME.get().unwrap())
}

async fn get_state(oid: &OID) -> EResult<Arc<State>> {
    if let Some(state) = STATE_CACHE.lock().get(oid) {
        return Ok(state.clone());
    }
    let payload = ParamsId { i: oid.as_str() };
    let res = eapi_bus::call("eva.core", "item.state", pack(&payload)?.into()).await?;
    let state = Arc::new(
        unpack::<Vec<State>>(res.payload())?
            .into_iter()
            .next()
            .ok_or_else(|| Error::not_found(format!("no such item: {}", oid)))?,
    );
    STATE_CACHE
        .lock()
        .insert(oid.clone(), state.clone(), CACHE_TTL);
    Ok(state)
}

fn format_output_value(mut value: Value, m: &OutputMappingEntry, oid: &OID) -> EResult<Value> {
    if !m.transform.is_empty() {
        let f = f64::try_from(value)?;
        value = Value::F64(transform::transform(&m.transform, oid, f)?);
    }
    if !m.value_map.is_empty() {
        if let Some(val) = m.value_map.get(&value.to_string()) {
            value = val.clone();
        }
    }
    Ok(value)
}

async fn send_output(o: &OutputMapping, mut unit_action: Option<(OID, Value)>) -> EResult<()> {
    let mut value = Value::Unit;
    for m in &o.map {
        if let Some(ref payload) = m.payload {
            if let Value::String(s) = payload {
                value.jp_insert(&m.path, Value::String(format_output_string(s)))?;
            } else {
                value.jp_insert(&m.path, payload.clone())?;
            }
        } else if let Some(ref oid) = m.oid {
            let state = get_state(oid).await?;
            let val = match m.prop {
                SourceProp::Status => Value::I16(state.status),
                SourceProp::Value => state.value.clone().unwrap_or_default(),
                SourceProp::Time => Value::F64(state.set_time),
                SourceProp::TimeSec =>
                {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    Value::U64(state.set_time as u64)
                }
                SourceProp::TimeMillis => Value::U64(
                    eva_common::time::Time::from_timestamp(state.set_time).timestamp_ms(),
                ),
                SourceProp::TimeMicros => Value::U64(
                    eva_common::time::Time::from_timestamp(state.set_time).timestamp_us(),
                ),
                SourceProp::TimeNanos => Value::U64(
                    eva_common::time::Time::from_timestamp(state.set_time).timestamp_ns(),
                ),
            };
            let val = format_output_value(val, m, oid)?;
            value.jp_insert(&m.path, val)?;
        } else if let Some((oid, av)) = unit_action.take() {
            let val = format_output_value(av, m, &oid)?;
            value.jp_insert(&m.path, val)?;
        }
    }
    let payload = o.packer.pack(value)?;
    PUBSUB_TX
        .get()
        .unwrap()
        .send(PubSubTask {
            topic: o.topic.clone(),
            payload,
            qos: o
                .qos
                .unwrap_or_else(|| DEFAULT_QOS.load(atomic::Ordering::Relaxed)),
            processed: None,
        })
        .await
        .map_err(Error::failed)?;
    Ok(())
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "pubsub.publish" => {
                #[derive(Deserialize)]
                struct Task {
                    topic: String,
                    payload: Value,
                    qos: Option<i32>,
                    #[serde(default)]
                    packer: Packer,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let task: Task = unpack(payload)?;
                    let qos = task
                        .qos
                        .unwrap_or_else(|| DEFAULT_QOS.load(atomic::Ordering::Relaxed));
                    let (tx, rx) = if qos > 0 {
                        let (t, r) = oneshot::channel();
                        (Some(t), Some(r))
                    } else {
                        (None, None)
                    };
                    PUBSUB_TX
                        .get()
                        .unwrap()
                        .send(PubSubTask {
                            topic: Arc::new(task.topic),
                            payload: task.packer.pack(task.payload)?,
                            qos,
                            processed: tx,
                        })
                        .await
                        .map_err(Error::failed)?;
                    if let Some(r) = rx {
                        tokio::time::timeout(*TIMEOUT.get().unwrap(), r)
                            .await
                            .map_err(|_| Error::timeout())?
                            .map_err(Error::failed)?;
                    }
                    Ok(None)
                }
            }
            "action" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let mut action: Action = unpack(payload)?;
                    let action_topic = format_action_topic(action.oid());
                    if let Some(o) = self.action_map.get(action.oid()) {
                        // TODO run in background and move to task pool
                        let params = action.take_unit_params()?;
                        let payload_pending = pack(&action.event_pending())?;
                        BUS_TX
                            .get()
                            .unwrap()
                            .send((action_topic.clone(), payload_pending))
                            .await
                            .map_err(Error::failed)?;
                        send_output(o, Some((action.oid().clone(), params.value))).await?;
                        let payload_completed = pack(&action.event_completed(None))?;
                        BUS_TX
                            .get()
                            .unwrap()
                            .send((action_topic.clone(), payload_completed))
                            .await
                            .map_err(Error::failed)?;
                        Ok(None)
                    } else {
                        Err(Error::not_found(format!("{} has no action map", action.oid())).into())
                    }
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_frame(&self, frame: Frame) {
        svc_need_ready!();
        let Some(topic) = frame.topic() else {
            return;
        };
        macro_rules! process {
            ($oid_str: expr, $pt: ty) => {
                let Ok(pt_state) = eva_common::payload::unpack::<$pt>(frame.payload()).log_err()
                else {
                    return;
                };
                let Ok(oid) = OID::from_path($oid_str).log_err() else {
                    return;
                };
                let state = pt_state.into();
                STATE_CACHE
                    .lock()
                    .insert(oid.clone(), Arc::new(state), CACHE_TTL);
                if let Some(o_v) = self.output.get(&oid) {
                    for o in o_v {
                        if !o.ignore_events {
                            send_output(o, None).await.log_ef();
                        }
                    }
                }
            };
        }
        if let Some(i) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
            process!(i, LocalStateEvent);
        } else if let Some(i) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
            process!(i, RemoteStateEvent);
        }
    }
}

#[inline]
fn default_queue_size() -> usize {
    1024
}

#[inline]
fn default_qos() -> i32 {
    1
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PsProto {
    Psrt,
    Mqtt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PubSubConfig {
    proto: PsProto,
    #[serde(default)]
    ca_certs: Option<String>,
    host: ValueOrList<String>,
    #[serde(default)]
    cluster_hosts_randomize: bool,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
    ping_interval: Duration,
    #[serde(default = "default_queue_size")]
    queue_size: usize,
    #[serde(default = "default_qos")]
    qos: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    input_cache_sec: Option<Duration>,
    pubsub: PubSubConfig,
    extra: Option<ConfigExtra>,
    #[serde(default)]
    input: Vec<InputMapping>,
    #[serde(default)]
    output: Vec<Arc<OutputMapping>>,
    #[serde(default)]
    action_map: BTreeMap<OID, OutputMapping>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigExtra {
    topics: Vec<String>,
    process: OID,
}

fn default_jp() -> String {
    "$.".to_owned()
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct InputMapping {
    topic: String,
    #[serde(default)]
    packer: Packer,
    #[serde(default)]
    map: Vec<InputMappingEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct InputMappingEntry {
    #[serde(default = "default_jp")]
    path: String,
    oid: OID,
    #[serde(default)]
    process: Processing,
    #[serde(default)]
    value_map: BTreeMap<String, Value>,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
enum Processing {
    Status,
    #[default]
    Value,
    Action,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputMapping {
    topic: Arc<String>,
    #[serde(default)]
    packer: Packer,
    #[serde(default)]
    map: Vec<OutputMappingEntry>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    interval: Option<Duration>,
    #[serde(default)]
    ignore_events: bool,
    #[serde(default)]
    qos: Option<i32>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct OutputMappingEntry {
    #[serde(default = "default_jp")]
    path: String,
    payload: Option<Value>,
    oid: Option<OID>,
    #[serde(default)]
    prop: SourceProp,
    #[serde(default)]
    value_map: BTreeMap<String, Value>,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
enum SourceProp {
    Status,
    #[default]
    Value,
    Time,
    TimeSec,
    TimeMillis,
    TimeMicros,
    TimeNanos,
}

#[derive(Deserialize, Default, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
enum Packer {
    #[default]
    No,
    Json,
    MsgPack,
}

impl Packer {
    fn unpack(self, data: &[u8]) -> EResult<Value> {
        match self {
            Packer::No => {
                let s = std::str::from_utf8(data)?;
                if let Ok(v) = s.parse::<u64>() {
                    Ok(Value::U64(v))
                } else if let Ok(v) = s.parse::<i64>() {
                    Ok(Value::I64(v))
                } else if let Ok(v) = s.parse::<f64>() {
                    Ok(Value::F64(v))
                } else {
                    Ok(Value::String(s.to_owned()))
                }
            }
            Packer::Json => serde_json::from_slice(data).map_err(Into::into),
            Packer::MsgPack => eva_common::payload::unpack(data),
        }
    }
    fn pack(self, value: Value) -> EResult<Vec<u8>> {
        match self {
            Packer::No => Ok(value.to_string().as_bytes().to_vec()),
            Packer::Json => serde_json::to_vec(&value).map_err(Into::into),
            Packer::MsgPack => eva_common::payload::pack(&value),
        }
    }
}

async fn execute_action(oid: &OID, value: &Value) -> EResult<()> {
    #[derive(Serialize)]
    struct ActionParams<'a> {
        value: &'a Value,
    }
    #[derive(Serialize)]
    struct ActionPayload<'a> {
        i: &'a OID,
        params: ActionParams<'a>,
    }
    debug!("executing action for {}, value: {}", oid, value);
    let action_payload = ActionPayload {
        i: oid,
        params: ActionParams { value },
    };
    eapi_bus::call(
        "eva.core",
        "action",
        eva_common::payload::pack(&action_payload)?.into(),
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_input_payload(
    topic: &str,
    data: &[u8],
    mapping: &InputMapping,
    cache: &RawStateCache,
) {
    debug!(
        "processing topic {}, payload: {}",
        topic,
        std::str::from_utf8(data).unwrap_or_default()
    );
    match mapping.packer.unpack(data) {
        Ok(value) => {
            let mut states = HashMap::<&OID, RawStateEventPreparedOwned>::new();
            for m in &mapping.map {
                debug!("looking up for {}", m.path);
                match value.jp_lookup(&m.path) {
                    Ok(Some(mut v)) => {
                        let mut value_transformed = None;
                        if !m.value_map.is_empty() {
                            if let Some(v_mapped) = m.value_map.get(&v.to_string()) {
                                v = v_mapped;
                            }
                        }
                        if !m.transform.is_empty() {
                            let f = match f64::try_from(v) {
                                Ok(n) => n,
                                Err(e) => {
                                    error!(
                                        "{} unable to parse value for transform ({}): {}",
                                        topic, value, e
                                    );
                                    continue;
                                }
                            };
                            match transform::transform(&m.transform, &m.oid, f) {
                                Ok(n) => {
                                    value_transformed.replace(Value::F64(n));
                                    v = value_transformed.as_ref().unwrap();
                                }
                                Err(e) => {
                                    error!(
                                        "{} unable to transform the value ({}): {}",
                                        topic, value, e
                                    );
                                    continue;
                                }
                            }
                        }
                        match m.process {
                            Processing::Action => {
                                execute_action(&m.oid, v).await.log_ef();
                            }
                            Processing::Status => {
                                let item_status = v
                                    .clone()
                                    .try_into()
                                    .unwrap_or(eva_common::ITEM_STATUS_ERROR);
                                match states.entry(&m.oid) {
                                    hash_map::Entry::Vacant(e) => {
                                        e.insert(RawStateEventPreparedOwned::from_rse_owned(
                                            RawStateEventOwned::new0(item_status),
                                            None,
                                        ));
                                    }
                                    hash_map::Entry::Occupied(mut o) => {
                                        o.get_mut().state_mut().status = item_status;
                                    }
                                }
                            }
                            Processing::Value => {
                                let val = v.clone();
                                match states.entry(&m.oid) {
                                    hash_map::Entry::Vacant(e) => {
                                        e.insert(RawStateEventPreparedOwned::from_rse_owned(
                                            RawStateEventOwned::new(1, val),
                                            None,
                                        ));
                                    }
                                    hash_map::Entry::Occupied(mut o) => {
                                        o.get_mut().state_mut().value =
                                            ValueOptionOwned::Value(val);
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => error!("{} value process error: {}", topic, e),
                }
            }
            let tx = BUS_TX.get().unwrap();
            cache.retain_map_modified(&mut states);
            for (oid, raw) in states {
                let topic = format!("{}{}", RAW_STATE_TOPIC, oid.as_path());
                let Ok(payload) = eva_common::payload::pack(raw.state()).log_err() else {
                    return;
                };
                if let Err(e) = tx.send((topic, payload)).await {
                    error!("{}", e);
                    poc();
                }
            }
        }
        Err(e) => error!("{} unable to unpack the value: {}", topic, e),
    }
}

async fn handle_input(
    rx: async_channel::Receiver<Box<dyn pubsub::Message + Send + Sync>>,
    mapping_vec: Vec<InputMapping>,
    cache: RawStateCache,
    tester: Arc<psrpc::Tester>,
    process_extra: Option<OID>,
) {
    let mapping: BTreeMap<String, InputMapping> = mapping_vec
        .into_iter()
        .map(|v| (v.topic.clone(), v))
        .collect();
    while let Ok(message) = rx.recv().await {
        let topic = message.topic();
        let payload = message.data();
        if !tester.fire(topic, payload) {
            if let Some(m) = mapping.get(topic) {
                process_input_payload(topic, payload, m, &cache).await;
            } else if let Some(ref oid) = process_extra {
                let pubsub_payload = std::str::from_utf8(payload).map_or_else(
                    |_| Value::Bytes(payload.to_vec()),
                    |v| Value::String(v.to_owned()),
                );
                let kwargs = common::PubSubData {
                    pubsub_topic: topic.to_owned(),
                    pubsub_payload,
                };
                let params = common::ParamsRun {
                    i: oid,
                    params: common::LParams { kwargs },
                    wait: eapi_bus::timeout(),
                };
                safe_run_macro(params).await.log_ef();
            }
        }
    }
}

fn spawn_bus_client_worker(
    bus_client: Arc<tokio::sync::Mutex<(dyn AsyncClient + 'static)>>,
    rx: async_channel::Receiver<(String, Vec<u8>)>,
) {
    tokio::spawn(async move {
        while let Ok((topic, payload)) = rx.recv().await {
            bus_client
                .lock()
                .await
                .publish(&topic, payload.into(), QoS::No)
                .await
                .log_ef();
        }
    });
}

fn spawn_pubsub_worker(
    pubsub_client: Arc<dyn pubsub::Client + std::marker::Send + Sync + 'static>,
    pubsub_rx: async_channel::Receiver<PubSubTask>,
) {
    tokio::spawn(async move {
        while let Ok(task) = pubsub_rx.recv().await {
            pubsub_client
                .publish(&task.topic, task.payload, task.qos)
                .await
                .log_ef();
            if let Some(t) = task.processed {
                let _ = t.send(());
            }
        }
    });
}

fn spawn_pubsub_tester(
    pubsub_client: Arc<dyn pubsub::Client + std::marker::Send + Sync + 'static>,
    tester: Arc<psrpc::Tester>,
    interval: Duration,
    timeout: Duration,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = psrpc::pstest(tester.clone(), pubsub_client.clone(), timeout).await {
                error!("PubSub server test failed: {}", e);
                poc();
            } else {
                debug!("PubSub server test passed");
            }
        }
    });
}

fn spawn_interval_output(o: Arc<OutputMapping>, interval: Duration) {
    tokio::spawn(async move {
        let mut int = tokio::time::interval(interval);
        int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            int.tick().await;
            send_output(&o, None).await.log_ef();
        }
    });
}

async fn connect(
    pubsub_config: &PubSubConfig,
    timeout: Duration,
) -> EResult<(
    Arc<dyn pubsub::Client + Send + Sync + 'static>,
    async_channel::Receiver<Box<dyn pubsub::Message + Send + Sync>>,
)> {
    match pubsub_config.proto {
        PsProto::Psrt => {
            let mut psrt_config = psrt::client::Config::new("")
                .set_timeout(timeout)
                .set_queue_size(pubsub_config.queue_size);
            if let Some(ref username) = pubsub_config.username {
                let password = pubsub_config.password.as_deref().unwrap_or_default();
                psrt_config = psrt_config.set_auth(username, password);
            }
            if let Some(ref ca_certs) = pubsub_config.ca_certs {
                let certs = tokio::fs::read_to_string(ca_certs).await?;
                psrt_config = psrt_config.set_tls(true).set_tls_ca(Some(certs));
            }
            let mut client = None;
            for host in pubsub_config.host.iter() {
                psrt_config.update_path(host);
                psrt_config = psrt_config.build();
                match psrt::client::Client::connect(&psrt_config).await {
                    Ok(v) => {
                        client = Some(v);
                        info!("connected to PSRT server {}", host);
                        break;
                    }
                    Err(e) => warn!("Unable to connect to {}: {}", host, e),
                }
            }
            if let Some(mut c) = client.take() {
                let rx =
                    pubsub::Client::take_data_channel(&mut c, pubsub_config.queue_size).unwrap();
                let p: Arc<dyn pubsub::Client + Send + Sync + 'static> = Arc::new(c);
                Ok((p, rx))
            } else {
                Err(Error::failed("Unable to find working psrt host"))
            }
        }
        PsProto::Mqtt => {
            let mut builder = paho_mqtt::ConnectOptionsBuilder::new();
            let mut mqtt_config = builder
                .keep_alive_interval(pubsub_config.ping_interval * 2)
                .connect_timeout(timeout);
            if let Some(ref username) = pubsub_config.username {
                let password = pubsub_config.password.as_deref().unwrap_or_default();
                mqtt_config = mqtt_config.user_name(username).password(password);
            }
            if let Some(ref ca_certs) = pubsub_config.ca_certs {
                let mut b = paho_mqtt::ssl_options::SslOptionsBuilder::new();
                let ssl = b.trust_store(ca_certs).map_err(Error::failed)?;
                mqtt_config = mqtt_config.ssl_options(ssl.finalize());
            }
            let mut client = None;
            let cfg = mqtt_config.finalize();
            for host in pubsub_config.host.iter() {
                let c = paho_mqtt::async_client::AsyncClient::new(format!("tcp://{}", host))
                    .map_err(Error::failed)?;
                match c.connect(cfg.clone()).await {
                    Ok(_) => {
                        client = Some(c);
                        info!("connected to MQTT server {}", host);
                        break;
                    }
                    Err(e) => warn!("Unable to connect to {}: {}", host, e),
                }
            }
            if let Some(mut c) = client.take() {
                let rx =
                    pubsub::Client::take_data_channel(&mut c, pubsub_config.queue_size).unwrap();
                let p: Arc<dyn pubsub::Client + Send + Sync + 'static> = Arc::new(c);
                Ok((p, rx))
            } else {
                Err(Error::failed("Unable to find working mqtt host"))
            }
        }
    }
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let mut config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    if initial.is_mode_rtf() {
        println!("marking all mapped items as failed");
        let payload_failed = pack(&RawStateEvent::new0(eva_common::ITEM_STATUS_ERROR))?;
        let mut bus = initial.init_bus_client().await?;
        for i in config.input {
            for m in i.map {
                bus.publish(
                    &format!("{}{}", RAW_STATE_TOPIC, m.oid.as_path()),
                    payload_failed.as_slice().into(),
                    busrt::QoS::No,
                )
                .await?;
            }
        }
        return Ok(());
    }
    eva_sdk::service::set_bus_error_suicide_timeout(initial.shutdown_timeout())?;
    if config.pubsub.cluster_hosts_randomize {
        config.pubsub.host.shuffle();
    }
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("unable to set TIMEOUT"))?;
    SYSTEM_NAME
        .set(initial.system_name().to_owned())
        .map_err(|_| Error::core("unable to set SYSTEM_NAME"))?;
    DEFAULT_QOS.store(config.pubsub.qos, atomic::Ordering::Relaxed);
    eva_sdk::service::set_poc(Some(timeout));
    let mut bus_oids: Vec<&OID> = Vec::new();
    let mut omap: BTreeMap<OID, Vec<Arc<OutputMapping>>> = <_>::default();
    for o in &config.output {
        for m in &o.map {
            if let Some(ref oid) = m.oid {
                bus_oids.push(oid);
                omap.entry(oid.clone()).or_default().push(o.clone());
            }
        }
    }
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("pubsub.publish")
            .required("topic")
            .required("payload")
            .optional("qos")
            .optional("packer"),
    );
    eapi_bus::init_blocking(
        &initial,
        Handlers {
            info,
            output: omap,
            action_map: config.action_map,
        },
    )
    .await?;
    initial.drop_privileges()?;
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(initial.bus_queue_size());
    BUS_TX
        .set(tx)
        .map_err(|_| Error::core("unable to set BUS_TX"))?;
    let (pubsub_tx, pubsub_rx) = async_channel::bounded::<PubSubTask>(config.pubsub.queue_size);
    PUBSUB_TX
        .set(pubsub_tx)
        .map_err(|_| Error::core("unable to set PUBSUB_TX"))?;
    spawn_bus_client_worker(eapi_bus::client(), rx);
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    let (pubsub_client, pubsub_input_rx) = connect(&config.pubsub, timeout).await?;
    let topics: Vec<&str> = config.input.iter().map(|v| v.topic.as_str()).collect();
    let tester: Arc<psrpc::Tester> = <_>::default();
    if !topics.is_empty() {
        pubsub_client
            .subscribe_bulk(&topics, config.pubsub.qos)
            .await?;
    }
    let process_extra = if let Some(extra) = config.extra {
        if !extra.topics.is_empty() {
            pubsub_client
                .subscribe_bulk(
                    &extra
                        .topics
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<&str>>(),
                    config.pubsub.qos,
                )
                .await?;
        }
        Some(extra.process)
    } else {
        None
    };
    let raw_state_cache = RawStateCache::new(config.input_cache_sec);
    let tester_c = tester.clone();
    tokio::spawn(async move {
        handle_input(
            pubsub_input_rx,
            config.input,
            raw_state_cache,
            tester_c,
            process_extra,
        )
        .await;
        poc();
    });
    let masks: OIDMaskList =
        OIDMaskList::new(bus_oids.into_iter().cloned().map(Into::into).collect());
    eapi_bus::subscribe_oids(&masks, eva_sdk::service::EventKind::Local).await?;
    eapi_bus::subscribe_oids(&masks, eva_sdk::service::EventKind::Remote).await?;
    tokio::spawn(async move {
        spawn_pubsub_worker(pubsub_client.clone(), pubsub_rx);
        spawn_pubsub_tester(pubsub_client, tester, config.pubsub.ping_interval, timeout);
        eapi_bus::wait_core(true).await.log_ef();
        for o in config.output {
            if let Some(interval) = o.interval {
                spawn_interval_output(o, interval);
            }
        }
    });
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    while !PUBSUB_TX.get().unwrap().is_empty() {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
    Ok(())
}
