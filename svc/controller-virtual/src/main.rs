use eva_common::common_payloads::ParamsOID;
use eva_common::events::RawStateEvent;
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
use eva_sdk::controller::{format_action_topic, format_raw_state_topic, Action};
use eva_sdk::prelude::*;
use once_cell::sync::{Lazy, OnceCell};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;
use tagmap::TagMap;
use tokio::sync::Mutex;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Virtual controller";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static ITEMS: Lazy<Mutex<BTreeMap<OID, VirtualItem>>> = Lazy::new(<_>::default);
static VARS: Lazy<Mutex<TagMap>> = Lazy::new(<_>::default);

static AUTO_CREATE: atomic::AtomicBool = atomic::AtomicBool::new(false);
static TX: OnceCell<async_channel::Sender<(String, Vec<u8>)>> = OnceCell::new();

async fn puller(pull: Vec<PullTask>, pull_interval: Option<Duration>) -> EResult<()> {
    let mut int = tokio::time::interval(pull_interval.unwrap_or_else(|| Duration::from_secs(1)));
    loop {
        for task in &pull {
            let tag = task.var_tag.as_ref().unwrap();
            match VARS.lock().await.get(tag) {
                Ok(val) => {
                    for m in &task.map {
                        if let Some(idx) = m.idx {
                            if let Value::Seq(ref s) = val {
                                if let Some(v) = s.get(idx) {
                                    set_item_state(&m.oid, Some(1), Some(v.clone())).await?;
                                } else {
                                    error!("value seq index out of range: {}[{}]", tag, idx);
                                    set_item_state(&m.oid, Some(ITEM_STATUS_ERROR), None).await?;
                                }
                            } else {
                                error!("value is not a seq: {}", tag);
                                set_item_state(&m.oid, Some(ITEM_STATUS_ERROR), None).await?;
                            }
                        } else {
                            set_item_state(&m.oid, Some(1), Some(val.clone())).await?;
                        }
                    }
                }
                Err(e) => {
                    error!("unable to query {}: {}", tag, e);
                    for m in &task.map {
                        set_item_state(&m.oid, Some(ITEM_STATUS_ERROR), None).await?;
                    }
                }
            }
        }
        int.tick().await;
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    items: Option<HashSet<OID>>,
    #[serde(default)]
    auto_create: bool,
    #[serde(default)]
    action_map: BTreeMap<OID, ActionProp>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pull_interval: Option<Duration>,
    #[serde(default)]
    pull: Vec<PullTask>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PullTask {
    var: String,
    #[serde(default)]
    map: Vec<PullMap>,
    #[serde(skip)]
    var_tag: Option<tagmap::Tag>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PullMap {
    idx: Option<usize>,
    oid: OID,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionProp {
    var: String,
    #[serde(skip)]
    var_tag: Option<tagmap::Tag>,
}

struct VirtualItem {
    oid: OID,
    status: ItemStatus,
    value: Value,
}

#[derive(Serialize)]
struct VirtualItemInfo<'a> {
    oid: &'a OID,
    status: ItemStatus,
    value: &'a Value,
}

#[derive(Serialize)]
struct VarInfo<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a Value>,
}

impl<'a> From<&'a VirtualItem> for VirtualItemInfo<'a> {
    fn from(item: &'a VirtualItem) -> Self {
        Self {
            oid: &item.oid,
            status: item.status,
            value: &item.value,
        }
    }
}

impl VirtualItem {
    fn new(oid: OID, status: Option<ItemStatus>, value: Option<Value>) -> Self {
        Self {
            oid,
            status: status.unwrap_or(1),
            value: value.unwrap_or_default(),
        }
    }
    #[inline]
    fn new0(oid: OID) -> Self {
        Self::new(oid, None, None)
    }
}

struct Handlers {
    info: ServiceInfo,
    action_map: BTreeMap<OID, ActionProp>,
}

fn safe_set(vars: &mut TagMap, i: &str, value: Value) -> EResult<()> {
    vars.set(i.parse()?, value)?;
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
            "list" => {
                if !payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let items = ITEMS.lock().await;
                let item_list: Vec<VirtualItemInfo> =
                    items.values().map(Into::<VirtualItemInfo>::into).collect();
                Ok(Some(pack(&item_list)?))
            }
            "var.get" => {
                #[derive(Deserialize)]
                struct Params {
                    i: String,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: Params = unpack(payload)?;
                let val = VARS.lock().await.get(&p.i.parse()?)?;
                Ok(Some(pack(&val)?))
            }
            "var.set" => {
                #[derive(Deserialize)]
                struct Params {
                    i: String,
                    value: Value,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: Params = unpack(payload)?;
                VARS.lock().await.set(p.i.parse()?, p.value)?;
                Ok(None)
            }
            "var.set_bulk" => {
                #[derive(Deserialize)]
                struct Params {
                    i: Vec<String>,
                    values: Vec<Value>,
                }
                #[derive(Serialize)]
                struct Res {
                    #[serde(skip_serializing_if = "Vec::is_empty")]
                    failed: Vec<String>,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: Params = unpack(payload)?;
                if p.i.len() != p.values.len() {
                    return Err(Error::invalid_params("var id and value len mismatch").into());
                }
                let mut vars = VARS.lock().await;
                let mut failed = Vec::new();
                for (id, value) in p.i.into_iter().zip(p.values) {
                    if let Err(e) = safe_set(&mut vars, &id, value) {
                        error!("unable to set {}: {}", id, e);
                        failed.push(id);
                    }
                }
                Ok(Some(pack(&Res { failed })?))
            }
            "var.destroy" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    i: String,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: Params = unpack(payload)?;
                VARS.lock().await.delete(p.i.parse()?)?;
                Ok(None)
            }
            "var.list" => {
                #[derive(Deserialize)]
                struct Params {
                    #[serde(default)]
                    full: bool,
                }
                let full = if payload.is_empty() {
                    false
                } else {
                    let p: Params = unpack(payload)?;
                    p.full
                };
                let vars = VARS.lock().await;
                let result: Vec<VarInfo> = vars
                    .tags()
                    .iter()
                    .map(|(id, value)| VarInfo {
                        id: id.as_str().unwrap_or_default(),
                        value: if full { Some(value) } else { None },
                    })
                    .collect();
                Ok(Some(pack(&result)?))
            }
            "set" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsSet {
                    i: OID,
                    status: Option<ItemStatus>,
                    #[serde(default)]
                    value: ValueOptionOwned,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsSet = unpack(payload)?;
                set_item_state(&p.i, p.status, p.value.into()).await?;
                Ok(None)
            }
            "get" => {
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsOID = unpack(payload)?;
                let items = ITEMS.lock().await;
                if let Some(item) = items.get(&p.i) {
                    Ok(Some(pack(&Into::<VirtualItemInfo>::into(item))?))
                } else {
                    Err(Error::not_found(p.i).into())
                }
            }
            "action" => {
                if payload.is_empty() {
                    return Err(Error::invalid_params("payload not specified").into());
                }
                let mut action: Action = unpack(payload)?;
                let params = action.take_unit_params()?;
                set_item_state(action.oid(), Some(1), Some(params.value.clone())).await?;
                if let Some(m) = self.action_map.get(action.oid()) {
                    VARS.lock()
                        .await
                        .set(m.var_tag.as_ref().unwrap().clone(), params.value)?;
                }
                let payload = pack(&action.event_completed(None))?;
                let topic = format_action_topic(action.oid());
                TX.get()
                    .unwrap()
                    .send((topic, payload))
                    .await
                    .map_err(Error::io)?;
                Ok(None)
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

async fn set_item_state(
    oid: &OID,
    status: Option<ItemStatus>,
    value: Option<Value>,
) -> EResult<()> {
    let payload = {
        let mut items = ITEMS.lock().await;
        if let Some(item) = items.get_mut(oid) {
            if let Some(s) = status {
                item.status = s;
            }
            if let Some(v) = value {
                item.value = v;
            }
            let event = RawStateEvent::new(item.status, &item.value);
            pack(&event)?
        } else if AUTO_CREATE.load(atomic::Ordering::Relaxed) {
            let item = VirtualItem::new(oid.clone(), status, value);
            let event = RawStateEvent::new(item.status, &item.value);
            let payload = pack(&event)?;
            items.insert(oid.clone(), item);
            payload
        } else {
            return Err(Error::not_found(oid));
        }
    };
    TX.get()
        .unwrap()
        .send((format_raw_state_topic(oid), payload))
        .await
        .log_ef();
    Ok(())
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let mut config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    for task in &mut config.pull {
        task.var_tag.replace(task.var.parse()?);
    }
    for v in config.action_map.values_mut() {
        v.var_tag.replace(v.var.parse()?);
    }
    let mut items = BTreeMap::new();
    if let Some(citems) = config.items {
        for oid in citems {
            items.insert(oid.clone(), VirtualItem::new0(oid));
        }
    }
    let (tx, rx) = async_channel::bounded(1024);
    TX.set(tx).map_err(|_| Error::core("unable to set TX"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("list").description("list oid registers"));
    info.add_method(
        ServiceMethod::new("get")
            .description("get oid register")
            .required("i"),
    );
    info.add_method(
        ServiceMethod::new("set")
            .description("set oid register")
            .required("i")
            .optional("status")
            .optional("value"),
    );
    info.add_method(
        ServiceMethod::new("var.list")
            .description("list variable values")
            .optional("full"),
    );
    info.add_method(
        ServiceMethod::new("var.get")
            .description("get variable value")
            .required("i"),
    );
    info.add_method(
        ServiceMethod::new("var.set")
            .description("set variable value")
            .required("i")
            .required("value"),
    );
    info.add_method(
        ServiceMethod::new("var.destroy")
            .description("destroy variable")
            .required("i"),
    );
    AUTO_CREATE.store(config.auto_create, atomic::Ordering::Relaxed);
    let handlers = Handlers {
        info,
        action_map: config.action_map,
    };
    let rpc: Arc<RpcClient> = initial.init_rpc(handlers).await?;
    let registry = initial.init_registry(&rpc);
    if let Ok(val) = registry.key_get("vars").await {
        *VARS.lock().await = TagMap::deserialize(val)?;
    }
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    let cl = client.clone();
    tokio::spawn(async move {
        while let Ok((topic, payload)) = rx.recv().await {
            cl.lock()
                .await
                .publish(&topic, payload.into(), QoS::No)
                .await
                .log_ef();
        }
    });
    if !config.pull.is_empty() {
        tokio::spawn(async move {
            puller(config.pull, config.pull_interval).await.log_ef();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
    }
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    registry
        .key_set("vars", to_value(&*VARS.lock().await)?)
        .await?;
    Ok(())
}
