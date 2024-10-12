use crate::actmgr;
use crate::core::{ActionLaunchResult, Core, LvarOp};
use crate::items::{ItemConfigData, NodeFilter};
use crate::logs::LogLevel;
use crate::seq;
use crate::svc::emit;
use crate::svcmgr;
use crate::Error;
use busrt::tools::pubsub;
use busrt::{
    rpc::{rpc_err_str, RpcError, RpcEvent, RpcHandlers, RpcResult},
    Frame, FrameKind,
};
use eva_common::acl::{OIDMask, OIDMaskList};
use eva_common::common_payloads::{ParamsId, ParamsUuid};
use eva_common::dobj::{DataObject, Endianess};
use eva_common::err_logger;
use eva_common::events::LOG_INPUT_TOPIC;
use eva_common::events::{
    FullItemStateAndInfo, ItemStateAndInfo, NodeInfo, NodeStateEvent, NodeStatus,
    RawStateBulkEventOwned, RawStateEventOwned, ReplicationInventoryItem, ReplicationStateEvent,
    RAW_STATE_BULK_TOPIC, RAW_STATE_TOPIC, REPLICATION_INVENTORY_TOPIC,
    REPLICATION_NODE_STATE_TOPIC, REPLICATION_STATE_TOPIC,
};
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use log::{trace, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{atomic, Arc};
use std::time::Duration;
use sysinfo::{DiskExt, SystemExt};

const HANDLER_ID_RAW_STATE: usize = 1;
const HANDLER_ID_RAW_STATE_BULK: usize = 2;
const HANDLER_ID_ACTION: usize = 10;

static CRASH_SIMULATED: atomic::AtomicU8 = atomic::AtomicU8::new(0);

#[derive(Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
enum CrashSimulatedKind {
    No = 0,
    Error = 1,
    Freeze = 2,
    MemoryOverflow = 3,
    Crash = 0xFF,
}

err_logger!();

pub const EAPI_VERSION: u16 = 1;

#[inline]
pub fn get_version() -> u16 {
    EAPI_VERSION
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceDeploy<'a> {
    id: &'a str,
    params: svcmgr::Params,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamsSvcDeploy<'a> {
    #[serde(borrow, default)]
    svcs: Vec<ServiceDeploy<'a>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SvcOrId<'a> {
    #[serde(borrow)]
    Id(&'a str),
    Svc(Box<ServiceDeploy<'a>>),
}

impl<'a> SvcOrId<'a> {
    fn as_str(&self) -> &str {
        match self {
            Self::Id(i) => i,
            Self::Svc(svc) => svc.id,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamsSvcOrId<'a> {
    #[serde(borrow)]
    svcs: Vec<SvcOrId<'a>>,
}

pub struct BusApi {
    core: Arc<Core>,
    topic_broker: pubsub::TopicBroker,
    channel_size: usize,
}

impl BusApi {
    pub fn new(core: Arc<Core>, channel_size: usize) -> Self {
        Self {
            core,
            topic_broker: <_>::default(),
            channel_size,
        }
    }
    pub fn start(&mut self) -> EResult<()> {
        let (tx, rx) = self.topic_broker.register_prefix_with_handler_id(
            RAW_STATE_TOPIC,
            HANDLER_ID_RAW_STATE,
            self.channel_size,
        )?;
        self.topic_broker.register_prefix_tx_with_handler_id(
            eva_common::actions::ACTION_TOPIC,
            HANDLER_ID_ACTION,
            tx.clone(),
        )?;
        self.topic_broker.register_topic_tx_with_handler_id(
            RAW_STATE_BULK_TOPIC,
            HANDLER_ID_RAW_STATE_BULK,
            tx,
        )?;
        let core = self.core.clone();
        tokio::spawn(async move {
            raw_state_and_action_handler(&core, rx).await;
        });
        let (_, rx) = self
            .topic_broker
            .register_prefix(REPLICATION_STATE_TOPIC, self.channel_size)?;
        let core = self.core.clone();
        tokio::spawn(async move {
            replication_state_handler(&core, rx).await;
        });
        let (_, rx) = self
            .topic_broker
            .register_prefix(REPLICATION_INVENTORY_TOPIC, self.channel_size)?;
        let core = self.core.clone();
        tokio::spawn(async move {
            replication_inventory_handler(core, rx).await;
        });
        let (_, rx) = self
            .topic_broker
            .register_prefix(REPLICATION_NODE_STATE_TOPIC, self.channel_size)?;
        let core = self.core.clone();
        tokio::spawn(async move {
            replication_node_state_handler(&core, rx).await;
        });
        let (_, rx) = self
            .topic_broker
            .register_prefix(LOG_INPUT_TOPIC, self.channel_size)?;
        tokio::spawn(async move {
            log_emit_handler(rx).await;
        });
        Ok(())
    }
}

async fn raw_state_and_action_handler(
    core: &Core,
    rx: async_channel::Receiver<pubsub::Publication>,
) {
    while let Ok(frame) = rx.recv().await {
        match frame.handler_id() {
            HANDLER_ID_RAW_STATE => {
                handle_raw_state_event(core, frame).await;
            }
            HANDLER_ID_RAW_STATE_BULK => {
                handle_raw_state_event_bulk(core, frame).await;
            }
            HANDLER_ID_ACTION => {
                handle_action_state(core, frame);
            }
            v => warn!(
                "core raw state and action handler, orphaned handler id: {}",
                v
            ),
        }
    }
}

async fn handle_raw_state_event(core: &Core, frame: pubsub::Publication) {
    if core.is_active() {
        match OID::from_path(frame.subtopic()) {
            Ok(oid) => {
                let payload = frame.payload();
                if payload.is_empty() {
                    core.auto_create_item_from_raw_empty(&oid, frame.primary_sender())
                        .await;
                } else {
                    match unpack::<RawStateEventOwned>(frame.payload()) {
                        Ok(raw) => {
                            core.update_state_from_raw(&oid, raw, frame.primary_sender())
                                .await
                                .log_efd();
                        }
                        Err(e) => warn!("invalid payload in raw event {}: {}", frame.topic(), e),
                    }
                }
            }
            Err(e) => warn!("invalid OID in raw event {}: {}", frame.topic(), e),
        }
    }
}

async fn handle_raw_state_event_bulk(core: &Core, frame: pubsub::Publication) {
    if core.is_active() {
        match unpack::<Vec<RawStateBulkEventOwned>>(frame.payload()) {
            Ok(raw) => {
                core.update_state_from_raw_bulk(raw, frame.primary_sender())
                    .await;
            }
            Err(e) => warn!("invalid payload in raw bulk event {}: {}", frame.topic(), e),
        }
    }
}

fn handle_action_state(core: &Core, frame: pubsub::Publication) {
    if core.is_active() {
        if let Err(e) = OID::from_path(frame.subtopic()) {
            warn!("invalid OID in action event {}: {}", frame.topic(), e);
        } else {
            match unpack::<eva_common::actions::ActionEvent>(frame.payload()) {
                Ok(action_event) => {
                    core.action_manager().process_event(action_event).log_ef();
                }
                Err(e) => {
                    warn!(
                        "invalid payload in action event, topic {}: {}",
                        frame.topic(),
                        e
                    );
                }
            }
        }
    }
}

async fn replication_state_handler(core: &Core, rx: async_channel::Receiver<pubsub::Publication>) {
    while let Ok(frame) = rx.recv().await {
        if core.is_active() {
            match OID::from_path(frame.subtopic()) {
                Ok(oid) => match unpack::<ReplicationStateEvent>(frame.payload()) {
                    Ok(rpl) => {
                        core.update_state_from_repl(&oid, rpl, frame.primary_sender())
                            .await
                            .log_efd();
                    }
                    Err(e) => {
                        warn!("invalid payload in raw event {}: {}", frame.topic(), e);
                    }
                },
                Err(e) => warn!("invalid OID in raw event {}: {}", frame.topic(), e),
            }
        }
    }
}

async fn replication_inventory_handler(
    core: Arc<Core>,
    rx: async_channel::Receiver<pubsub::Publication>,
) {
    while let Ok(frame) = rx.recv().await {
        if core.is_active() {
            let core = core.clone();
            // TODO move to task pool
            tokio::spawn(async move {
                match unpack::<Vec<ReplicationInventoryItem>>(frame.payload()) {
                    Ok(remote_items) => {
                        let mut remote_inv = HashMap::new();
                        for item in remote_items {
                            remote_inv.insert(item.oid.clone(), item);
                        }
                        core.process_remote_inventory(
                            remote_inv,
                            frame.subtopic(),
                            frame.primary_sender(),
                        )
                        .await
                        .log_ef();
                    }
                    Err(e) => warn!(
                        "invalid payload in inventory event {}: {}",
                        frame.topic(),
                        e
                    ),
                }
            });
        }
    }
}

async fn replication_node_state_handler(
    core: &Core,
    rx: async_channel::Receiver<pubsub::Publication>,
) {
    while let Ok(frame) = rx.recv().await {
        if core.is_active() {
            match unpack::<NodeStateEvent>(frame.payload()) {
                Ok(nse) => match nse.status {
                    NodeStatus::Online => {
                        core.mark_source_online(
                            frame.subtopic(),
                            frame.primary_sender(),
                            true,
                            nse.info,
                            nse.timeout,
                        )
                        .await;
                    }
                    NodeStatus::Offline => {
                        core.mark_source_online(
                            frame.subtopic(),
                            frame.primary_sender(),
                            false,
                            None,
                            nse.timeout,
                        )
                        .await;
                    }
                    NodeStatus::Removed => core.destroy_source(frame.subtopic()).await,
                },
                Err(e) => warn!(
                    "invalid payload in node state event {}: {}",
                    frame.topic(),
                    e
                ),
            }
        }
    }
}

async fn log_emit_handler(rx: async_channel::Receiver<pubsub::Publication>) {
    while let Ok(frame) = rx.recv().await {
        if let Ok(lvl) = frame.subtopic().parse::<LogLevel>() {
            if let Ok(msg) = std::str::from_utf8(frame.payload()) {
                emit(lvl, frame.primary_sender(), msg);
            }
        }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for BusApi {
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
        if frame.kind() == FrameKind::Publish {
            self.topic_broker.process(frame).await.log_ef();
        }
    }
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::similar_names)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        macro_rules! need_ready {
            () => {
                if !self.core.is_active() {
                    warn!(
                        "ignoring rpc call from {}: {}, the core is not ready",
                        event.primary_sender(),
                        method
                    );
                    return Err(RpcError::internal(rpc_err_str("not ready")));
                }
            };
        }
        let payload = event.payload();
        macro_rules! set_enabled {
            ($value: expr) => {{
                if payload.is_empty() {
                    Err(RpcError::params(rpc_err_str("oid/mask not specified")))
                } else {
                    let p: ParamsId = unpack(payload).log_err()?;
                    let mask = p.i.parse().map_err(Into::<Error>::into)?;
                    self.core.set_local_items_enabled(&mask, $value).await?;
                    Ok(None)
                }
            }};
        }
        macro_rules! lvar_op {
            ($op: expr) => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(event.payload()).log_err()?;
                    let oid: OID = p.i.parse().map_err(Into::<Error>::into)?;
                    if let Some(value) = self.core.lvar_op(&oid, $op, method).await? {
                        Ok(Some(pack(&value)?))
                    } else {
                        Ok(None)
                    }
                }
            };
        }
        macro_rules! action_op {
            ($p: expr, $params: expr) => {{
                let oid: OID = $p.i.parse().map_err(Into::<Error>::into)?;
                let res = self
                    .core
                    .action(
                        $p.u,
                        &oid,
                        $params,
                        $p.priority
                            .unwrap_or(eva_common::actions::DEFAULT_ACTION_PRIORITY),
                        $p.wait.map(Duration::from_secs_f64),
                        false,
                    )
                    .await?;
                match res {
                    ActionLaunchResult::Local(ser_info) => Ok(Some(ser_info)),
                    ActionLaunchResult::Remote(info) => Ok(Some(pack(&info)?)),
                    ActionLaunchResult::State(v) => Ok(Some(pack(&v)?)),
                }
            }};
        }
        trace!("rpc call from {}: {}", event.primary_sender(), method);
        match method {
            "test" => {
                if payload.is_empty() {
                    match CRASH_SIMULATED.load(atomic::Ordering::Relaxed) {
                        v if v == CrashSimulatedKind::Error as u8 => {
                            Err(Error::failed("simulated").into())
                        }
                        v if v == CrashSimulatedKind::Freeze as u8 => loop {
                            tokio::time::sleep(eva_common::SLEEP_STEP).await;
                        },
                        _ => Ok(Some(pack(&self.core)?)),
                    }
                } else {
                    Err(RpcError::params(None))
                }
            }
            "save" => {
                need_ready!();
                if payload.is_empty() {
                    self.core.save().await.log_ef();
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "log.purge" => {
                need_ready!();
                if payload.is_empty() {
                    crate::logs::purge_log_records();
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "log.get" => {
                #[derive(Deserialize, Default)]
                #[serde(deny_unknown_fields)]
                struct LogGetParams<'a> {
                    level: Option<Value>,
                    time: Option<u32>,
                    limit: Option<u32>,
                    #[serde(alias = "mod")]
                    module: Option<&'a str>,
                    #[serde(borrow)]
                    msg: Option<&'a str>,
                    #[serde(borrow)]
                    rx: Option<&'a str>,
                }
                need_ready!();
                let p: LogGetParams = if payload.is_empty() {
                    LogGetParams::default()
                } else {
                    unpack(payload).log_err()?
                };
                let log_level = if let Some(lvl) = p.level {
                    Some(lvl.try_into().log_err()?)
                } else {
                    None
                };
                let x: Option<Regex> = if let Some(rx) = p.rx {
                    Some(
                        Regex::new(&rx.to_lowercase())
                            .map_err(Into::<Error>::into)
                            .log_err()?,
                    )
                } else {
                    None
                };
                let filter = crate::logs::RecordFilter::new(
                    log_level,
                    p.module,
                    p.msg,
                    x.as_ref(),
                    p.time,
                    p.limit,
                );
                Ok(Some(pack(&crate::logs::get_log_records(filter))?))
            }
            "node.get" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Serialize)]
                    struct LocalNodeData {
                        svc: Option<()>,
                        online: bool,
                        info: NodeInfo,
                    }
                    let p: ParamsId = unpack(event.payload()).log_err()?;
                    if p.i == self.core.system_name() {
                        let node_data = LocalNodeData {
                            svc: None,
                            online: true,
                            info: crate::local_node_info(),
                        };
                        Ok(Some(pack(&node_data)?))
                    } else {
                        let nodes = self.core.nodes().lock().unwrap();
                        let node_data = nodes.get(p.i).ok_or_else(|| Error::not_found(p.i))?;
                        Ok(Some(pack(node_data)?))
                    }
                }
            }
            "dobj.push" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    struct Payload {
                        #[serde(alias = "i")]
                        name: String,
                        #[serde(alias = "d")]
                        data: Vec<u8>,
                        #[serde(default, alias = "e")]
                        endianess: Endianess,
                    }
                    let p: Payload = unpack(event.payload()).log_err()?;
                    self.core
                        .dobj_push(p.name, p.data, p.endianess, event.primary_sender())
                        .await?;
                    Ok(None)
                }
            }
            "dobj.error" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    struct Payload {
                        #[serde(alias = "i")]
                        name: String,
                        #[serde(alias = "s")]
                        status: Option<ItemStatus>,
                    }
                    let p: Payload = unpack(event.payload()).log_err()?;
                    self.core
                        .dobj_error(p.name, p.status, event.primary_sender())
                        .await?;
                    Ok(None)
                }
            }
            "dobj.list" => {
                if payload.is_empty() {
                    #[derive(Serialize)]
                    struct DataObjectName {
                        name: String,
                        size: usize,
                    }
                    let result: Vec<DataObjectName> = self
                        .core
                        .dobj_list()
                        .into_iter()
                        .map(|(name, size)| DataObjectName { name, size })
                        .collect();
                    Ok(Some(pack(&result)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "dobj.get_config" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(event.payload()).log_err()?;
                    let dobj = self
                        .core
                        .dobj_get(p.i)
                        .ok_or_else(|| Error::not_found(p.i))?;
                    Ok(Some(pack(&dobj)?))
                }
            }
            "dobj.validate" => {
                if payload.is_empty() {
                    self.core.dobj_validate()?;
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "dobj.deploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    struct Payload {
                        #[serde(default)]
                        data_objects: Vec<DataObject>,
                    }
                    let p: Payload = unpack(event.payload()).log_err()?;
                    self.core.dobj_insert(p.data_objects).await?;
                    Ok(None)
                }
            }
            "dobj.undeploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(untagged)]
                    enum DataObjectOrName {
                        Name(String),
                        Object(DataObject),
                    }
                    #[derive(Deserialize)]
                    struct Payload {
                        #[serde(default)]
                        data_objects: Vec<DataObjectOrName>,
                    }
                    let p: Payload = unpack(event.payload()).log_err()?;
                    let objects_to_remove: Vec<String> = p
                        .data_objects
                        .into_iter()
                        .map(|v| match v {
                            DataObjectOrName::Name(n) => n,
                            DataObjectOrName::Object(o) => o.name.into(),
                        })
                        .collect();
                    self.core
                        .dobj_remove(
                            &objects_to_remove
                                .iter()
                                .map(String::as_str)
                                .collect::<Vec<&str>>(),
                        )
                        .await?;
                    Ok(None)
                }
            }
            "node.list" => {
                need_ready!();
                if payload.is_empty() {
                    #[derive(Serialize, bmart::tools::Sorting)]
                    #[sorting(id = "name")]
                    struct NodeData<'a> {
                        name: &'a str,
                        svc: Option<&'a str>,
                        remote: bool,
                        online: bool,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        info: Option<&'a NodeInfo>,
                        #[serde(
                            serialize_with = "eva_common::tools::serialize_opt_duration_as_f64"
                        )]
                        timeout: Option<Duration>,
                    }
                    let nodes = self.core.nodes().lock().unwrap();
                    let mut info: Vec<NodeData> = nodes
                        .iter()
                        .map(|(i, n)| NodeData {
                            name: i,
                            svc: n.svc(),
                            online: n.online(),
                            remote: true,
                            info: n.info(),
                            timeout: n.timeout(),
                        })
                        .collect();
                    let li = crate::local_node_info();
                    info.push(NodeData {
                        name: self.core.system_name(),
                        svc: None,
                        online: true,
                        remote: false,
                        info: Some(&li),
                        timeout: Some(self.core.timeout()),
                    });
                    info.sort();
                    Ok(Some(pack(&info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "spoint.list" => {
                need_ready!();
                if payload.is_empty() {
                    let info = self.core.list_spoints().await?;
                    Ok(Some(pack(&info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "item.summary" => {
                need_ready!();
                if payload.is_empty() {
                    Ok(Some(pack(&self.core.inventory_stats().await)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "item.create" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(event.payload()).log_err()?;
                    let oid: OID = p.i.parse().map_err(Into::<Error>::into)?;
                    self.core.create_local_item(oid).await?;
                    Ok(None)
                }
            }
            "item.destroy" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(event.payload()).log_err()?;
                    let mask: OIDMask = p.i.parse().map_err(Into::<Error>::into)?;
                    self.core.destroy_local_items(&mask).await;
                    Ok(None)
                }
            }
            "item.deploy" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsDeploy {
                        #[serde(default)]
                        items: Vec<ItemConfigData>,
                    }
                    let params: ParamsDeploy = unpack(event.payload()).log_err()?;
                    self.core.deploy_local_items(params.items).await?;
                    Ok(None)
                }
            }
            "item.undeploy" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsUndeploy {
                        #[serde(default)]
                        items: Vec<Value>,
                    }
                    let params: ParamsUndeploy = unpack(event.payload()).log_err()?;
                    self.core.undeploy_local_items(params.items).await?;
                    Ok(None)
                }
            }
            "item.get_config" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(event.payload()).log_err()?;
                    let oid: OID = p.i.parse().map_err(Into::<Error>::into)?;
                    let items = self
                        .core
                        .list_items(
                            &oid.clone().into(),
                            None,
                            None,
                            Some(NodeFilter::Local),
                            true,
                        )
                        .await;
                    if items.is_empty() {
                        Err(Error::not_found(format!("item not found: {}", oid)).into())
                    } else {
                        Ok(Some(pack(&items[0].config()?)?))
                    }
                }
            }
            "item.list" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsList<'a> {
                    i: Option<Value>, // OID or OID mask (parse later)
                    #[serde(default, alias = "src")]
                    node: Option<&'a str>, // source node (.local for local items only)
                    #[serde(default)]
                    include: Option<OIDMaskList>,
                    #[serde(default)]
                    exclude: Option<OIDMaskList>,
                }
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let mut p: ParamsList = unpack(payload).log_err()?;
                let mask_list = if let Some(v) = p.i.take() {
                    v.try_into()?
                } else {
                    OIDMaskList::new_any()
                };
                let node_filter = p.node.map(|v| {
                    if v == crate::LOCAL_NODE_ALIAS || v == self.core.system_name() {
                        NodeFilter::Local
                    } else {
                        NodeFilter::Remote(v)
                    }
                });
                let items = self
                    .core
                    .list_items(
                        &mask_list,
                        p.include.as_ref(),
                        p.exclude.as_ref(),
                        node_filter,
                        true,
                    )
                    .await;
                let system_name = self.core.system_name();
                let result: Vec<FullItemStateAndInfo> = items
                    .iter()
                    .map(|v| v.full_state_and_info(system_name))
                    .collect();
                Ok(Some(pack(&result)?))
            }
            "item.announce" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsList<'a> {
                    #[serde(borrow)]
                    i: Option<&'a str>, // OID or OID mask (parse later)
                    #[serde(default, alias = "src")]
                    node: Option<&'a str>, // source node (.local for local items only)
                }
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsList = unpack(payload).log_err()?;
                let mask = if let Some(i) = p.i {
                    i.parse().map_err(Into::<Error>::into)?
                } else {
                    OIDMask::new_any()
                };
                let node_filter = p.node.map(|v| {
                    if v == crate::LOCAL_NODE_ALIAS || v == self.core.system_name() {
                        NodeFilter::Local
                    } else {
                        NodeFilter::Remote(v)
                    }
                });
                self.core
                    .force_announce_state(&mask.into(), node_filter)
                    .await?;
                Ok(None)
            }
            "item.enable" => {
                need_ready!();
                set_enabled!(true)
            }
            "item.disable" => {
                need_ready!();
                set_enabled!(false)
            }
            "item.state" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsState {
                    i: Option<Value>,
                    #[serde(default)]
                    include: Option<OIDMaskList>,
                    #[serde(default)]
                    exclude: Option<OIDMaskList>,
                    #[serde(default)]
                    full: bool,
                }
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let mut p: ParamsState = unpack(payload).log_err()?;
                #[allow(clippy::redundant_closure)]
                let mask_list = if let Some(v) = p.i.take() {
                    //let masks = HashSet::<String>::deserialize(v)?;
                    if matches!(v, Value::Seq(_)) {
                        let masks = HashSet::<String>::deserialize(v)?;
                        let mut oid_masks: HashSet<OIDMask> = HashSet::with_capacity(masks.len());
                        for mask in masks {
                            let Ok(oid_mask) = mask.parse::<OIDMask>() else {
                                warn!("invalid OID mask: {}", mask);
                                continue;
                            };
                            oid_masks.insert(oid_mask);
                        }
                        OIDMaskList::new(oid_masks)
                    } else {
                        v.try_into()?
                    }
                } else {
                    OIDMaskList::new_any()
                };
                let items = self
                    .core
                    .list_items(
                        &mask_list,
                        p.include.as_ref(),
                        p.exclude.as_ref(),
                        None,
                        false,
                    )
                    .await;
                let system_name = self.core.system_name();
                let result = if p.full {
                    let res: Vec<FullItemStateAndInfo> = items
                        .iter()
                        .map(|v| v.full_state_and_info(system_name))
                        .collect();
                    pack(&res)?
                } else {
                    let res: Vec<ItemStateAndInfo> = items
                        .iter()
                        .map(|v| v.state_and_info(system_name))
                        .collect();
                    pack(&res)?
                };
                Ok(Some(result))
            }
            "action" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsAction<'a> {
                    #[serde(alias = "uuid")]
                    u: Option<uuid::Uuid>,
                    #[serde(borrow)]
                    i: &'a str,
                    params: eva_common::actions::Params,
                    #[serde(default)]
                    priority: Option<u8>,
                    #[serde(default)]
                    wait: Option<f64>,
                }
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsAction = unpack(payload).log_err()?;
                action_op!(p, Some(p.params))
            }
            "action.toggle" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsActionToggle<'a> {
                    #[serde(alias = "uuid")]
                    u: Option<uuid::Uuid>,
                    #[serde(borrow)]
                    i: &'a str,
                    #[serde(default)]
                    priority: Option<u8>,
                    #[serde(default)]
                    wait: Option<f64>,
                }
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsActionToggle = unpack(payload).log_err()?;
                action_op!(p, None)
            }
            "run" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsRun<'a> {
                    #[serde(alias = "uuid")]
                    u: Option<uuid::Uuid>,
                    #[serde(borrow)]
                    i: &'a str,
                    params: Option<eva_common::actions::Params>,
                    #[serde(default)]
                    priority: Option<u8>,
                    #[serde(default)]
                    wait: Option<f64>,
                }
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsRun = unpack(payload).log_err()?;
                action_op!(
                    p,
                    Some(
                        p.params
                            .unwrap_or_else(|| eva_common::actions::Params::new_lmacro(None, None))
                    )
                )
            }
            "seq" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: eva_robots::SequenceOwned = unpack(payload)?;
                seq::execute(self.core.clone(), p).await?;
                Ok(None)
            }
            "seq.terminate" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let val: Value = unpack(payload)?;
                let p: ParamsUuid = ParamsUuid::deserialize(val).log_err()?;
                seq::terminate(&p.u)?;
                Ok(None)
            }
            "action.result" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let val: Value = unpack(payload)?;
                let p: ParamsUuid = ParamsUuid::deserialize(val).log_err()?;
                let info = self.core.action_result_serialized(&p.u).await?;
                Ok(Some(info))
            }
            "action.terminate" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let val: Value = unpack(payload)?;
                let p: ParamsUuid = ParamsUuid::deserialize(val).log_err()?;
                self.core.terminate_action(&p.u).await?;
                Ok(None)
            }
            "action.kill" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsId = unpack(payload).log_err()?;
                let oid: OID = p.i.parse().map_err(Into::<Error>::into)?;
                self.core.kill_actions(&oid).await?;
                Ok(None)
            }
            "action.list" => {
                need_ready!();
                let f: actmgr::Filter = if payload.is_empty() {
                    actmgr::Filter::default()
                } else {
                    unpack(payload).log_err()?
                };
                Ok(Some(
                    self.core
                        .action_manager()
                        .get_actions_filtered_serialized(&f, self.core.system_name())?,
                ))
            }
            "lvar.set" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsSet<'a> {
                    #[serde(borrow)]
                    i: &'a str, // OID or OID mask (parse later)
                    #[serde(default, alias = "s")]
                    status: Option<ItemStatus>,
                    #[serde(default, alias = "v")]
                    value: ValueOptionOwned,
                }
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsSet = unpack(event.payload()).log_err()?;
                    let oid: OID = p.i.parse().map_err(Into::<Error>::into)?;
                    self.core
                        .lvar_op(&oid, LvarOp::Set(p.status, p.value.into()), method)
                        .await?;
                    Ok(None)
                }
            }
            "lvar.reset" => {
                need_ready!();
                lvar_op!(LvarOp::Reset)
            }
            "lvar.incr" => {
                need_ready!();
                lvar_op!(LvarOp::Increment)
            }
            "lvar.decr" => {
                need_ready!();
                lvar_op!(LvarOp::Decrement)
            }
            "lvar.clear" => {
                need_ready!();
                lvar_op!(LvarOp::Clear)
            }
            "lvar.toggle" => {
                need_ready!();
                lvar_op!(LvarOp::Toggle)
            }
            "svc.deploy" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsSvcDeploy = unpack(payload).log_err()?;
                for svc in p.svcs {
                    self.core
                        .service_manager()
                        .deploy_service(
                            svc.id,
                            svc.params,
                            self.core.system_name(),
                            self.core.timeout(),
                        )
                        .await?;
                }
                Ok(None)
            }
            "svc.undeploy" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsSvcOrId = unpack(payload).log_err()?;
                for svc in p.svcs {
                    if let Err(e) = self
                        .core
                        .service_manager()
                        .undeploy_service(
                            svc.as_str(),
                            self.core.system_name(),
                            self.core.timeout(),
                        )
                        .await
                    {
                        if e.kind() != ErrorKind::ResourceNotFound {
                            return Err(e.into());
                        }
                    }
                }
                Ok(None)
            }
            "svc.restart" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsId = unpack(payload).log_err()?;
                self.core
                    .service_manager()
                    .restart_service(p.i, self.core.system_name(), self.core.timeout())
                    .await?;
                Ok(None)
            }
            "svc.purge" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsSvcOrId = unpack(payload).log_err()?;
                for svc in p.svcs {
                    self.core
                        .service_manager()
                        .purge_service(svc.as_str(), self.core.system_name(), self.core.timeout())
                        .await?;
                }
                Ok(None)
            }
            "svc.get_params" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsId = unpack(payload).log_err()?;
                Ok(Some(pack(
                    &self.core.service_manager().get_service_params(p.i)?,
                )?))
            }
            "svc.get_init" => {
                need_ready!();
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsId = unpack(payload).log_err()?;
                Ok(Some(pack(&self.core.service_manager().get_service_init(
                    p.i,
                    self.core.system_name(),
                    self.core.timeout(),
                    true,
                )?)?))
            }
            "svc.list" => {
                #[derive(Deserialize)]
                struct Params {
                    filter: Option<String>,
                }
                need_ready!();
                let params: Option<Params> = if payload.is_empty() {
                    None
                } else {
                    Some(unpack(payload)?)
                };
                let re: Option<regex::Regex> = if let Some(p) = params {
                    if let Some(filter) = p.filter {
                        Some(regex::Regex::new(&filter)?)
                    } else {
                        None
                    }
                } else {
                    None
                };
                Ok(Some(pack(
                    &self
                        .core
                        .service_manager()
                        .list_services(self.core.timeout(), true)
                        .await
                        .into_iter()
                        .filter(|v| {
                            if let Some(ref r) = re {
                                r.is_match(v.id())
                            } else {
                                true
                            }
                        })
                        .collect::<Vec<svcmgr::Info>>(),
                )?))
            }
            "svc.get" => {
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload).log_err()?;
                    Ok(Some(pack(
                        &self
                            .core
                            .service_manager()
                            .get_service_info(p.i, self.core.timeout())
                            .await?,
                    )?))
                }
            }
            // internal method, do not use
            "svc.start_by_launcher" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload).log_err()?;
                    self.core
                        .service_manager()
                        .start_by_launcher(p.i, self.core.system_name(), self.core.timeout())
                        .await;
                    Ok(None)
                }
            }
            "core.shutdown" => {
                need_ready!();
                if payload.is_empty() {
                    self.core.set_reload_flag().await.log_ef();
                    self.core.shutdown();
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            #[allow(clippy::cast_precision_loss)]
            "core.sysinfo" => {
                #[derive(Serialize)]
                struct Info {
                    ram_usage: Option<f64>,
                    disk_usage: Option<f64>,
                    la1: f64,
                    la5: f64,
                    la15: f64,
                }
                if payload.is_empty() {
                    let system = crate::SYSTEM_INFO.lock();
                    let d = eva_common::tools::get_eva_dir();
                    let eva_dir = Path::new(&d);
                    let mut disk_usage: Option<f64> = None;
                    for disk in system.disks() {
                        if eva_dir.starts_with(disk.mount_point()) {
                            let total = disk.total_space();
                            if total > 0 {
                                disk_usage = Some(
                                    (1.0 - disk.available_space() as f64 / total as f64) * 100.0,
                                );
                                break;
                            }
                        }
                    }
                    let ram_usage: Option<f64> = {
                        let total = system.total_memory();
                        if (total) > 0 {
                            Some(system.used_memory() as f64 / total as f64 * 100.0)
                        } else {
                            None
                        }
                    };
                    let la = system.load_average();
                    let la1 = la.one;
                    let la5 = la.five;
                    let la15 = la.fifteen;
                    let info = Info {
                        ram_usage,
                        disk_usage,
                        la1,
                        la5,
                        la15,
                    };
                    drop(system);
                    Ok(Some(pack(&info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "bus.publish" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let message: crate::core::BusMessage = unpack(payload)?;
                    self.core.publish_bus_messge(message).await?;
                    Ok(None)
                }
            }
            "simulate.crash" => {
                #[derive(Deserialize)]
                struct Params {
                    kind: CrashSimulatedKind,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: Params = unpack(payload)?;
                    match p.kind {
                        CrashSimulatedKind::Crash => {
                            #[allow(clippy::cast_possible_wrap)]
                            let pid = nix::unistd::Pid::from_raw(std::process::id() as i32);
                            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL)
                                .map_err(Error::failed);
                            // should exit before, but exit gacefully with error code if SIGKILL
                            // failed
                            std::process::exit(-1);
                        }
                        CrashSimulatedKind::MemoryOverflow => {
                            let workers = self.core.workers();
                            tokio::task::spawn_blocking(move || {
                                for _ in 0..workers {
                                    tokio::task::spawn_blocking(move || {
                                        let mut buf = Vec::new();
                                        loop {
                                            buf.push(Vec::<u8>::with_capacity(1_000_000));
                                        }
                                    });
                                }
                            });
                        }
                        v => CRASH_SIMULATED.store(v as u8, atomic::Ordering::Relaxed),
                    }
                    Ok(None)
                }
            }
            "update" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsUpdate {
                    version: String,
                    build: u64,
                    #[serde(default)]
                    yes: bool,
                    #[serde(default)]
                    url: Option<String>,
                    #[serde(default)]
                    test: bool,
                }
                need_ready!();
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsUpdate = unpack(payload).log_err()?;
                    let dir_eva = Path::new(&eva_common::tools::get_eva_dir()).to_path_buf();
                    if p.yes {
                        warn!("NODE UPDATE REQUESTED TO: {} ({})", p.build, p.version);
                        if p.test {
                            warn!("THE MACHINE WILL BE UPDATED TO THE TEST BUILD!");
                        }
                        let mut ecm = dir_eva.clone();
                        ecm.push("bin/eva-cloud-manager");
                        let mut log_file = dir_eva.clone();
                        log_file.push("log/update.log");
                        let force_ver =
                            format!("EVA_UPDATE_FORCE_VERSION={}:{}", p.version, p.build);
                        let mut update_command = format!(
                            r#"(sleep 1 && env "{}" "{}" node update --YES"#,
                            force_ver,
                            ecm.to_string_lossy()
                        );
                        if let Some(ref url) = p.url {
                            write!(update_command, r#" --repository-url "{}""#, url)
                                .map_err(Error::failed)?;
                        }
                        if p.test {
                            write!(update_command, " --test").map_err(Error::failed)?;
                        }
                        write!(update_command, " >> {} 2>&1)&", log_file.to_string_lossy())
                            .map_err(Error::failed)?;
                        let _r = tokio::process::Command::new("sh")
                            .args(["-c", &update_command])
                            .spawn()?
                            .wait()
                            .await?;
                        Ok(None)
                    } else {
                        Err(Error::failed("not confirmed").into())
                    }
                }
            }
            _ => Err(RpcError::method(None)),
        }
    }
}
