use eva_common::common_payloads::ParamsOID;
use eva_common::events::RawStateEvent;
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, format_raw_state_topic, Action};
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Virtual controller";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    items: Option<HashSet<OID>>,
    #[serde(default)]
    auto_create: bool,
}

struct VirtualItem {
    oid: OID,
    status: ItemStatus,
    value: Value,
}

#[derive(Serialize, bmart::tools::Sorting)]
#[sorting(id = "oid")]
struct VirtualItemInfo<'a> {
    oid: &'a OID,
    status: ItemStatus,
    value: &'a Value,
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
    fn new(oid: OID, status: Option<ItemStatus>, value: Option<Value>) -> EResult<Self> {
        let item_status = match oid.kind() {
            ItemKind::Unit => status.unwrap_or(0),
            ItemKind::Sensor => status.unwrap_or(1),
            _ => {
                return Err(Error::not_implemented(format!(
                    "{} items are not supported",
                    oid.kind()
                )));
            }
        };
        Ok(Self {
            oid,
            status: item_status,
            value: value.unwrap_or_default(),
        })
    }
    #[inline]
    fn new0(oid: OID) -> EResult<Self> {
        Self::new(oid, None, None)
    }
}

struct Handlers {
    info: ServiceInfo,
    items: Mutex<HashMap<OID, VirtualItem>>,
    auto_create: bool,
    tx: async_channel::Sender<(String, Vec<u8>)>,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "list" => {
                if !payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let items = self.items.lock().unwrap();
                let mut item_list: Vec<VirtualItemInfo> =
                    items.values().map(Into::<VirtualItemInfo>::into).collect();
                item_list.sort();
                Ok(Some(pack(&item_list)?))
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
                self.set_item_state(&p.i, p.status, p.value.into()).await?;
                Ok(None)
            }
            "get" => {
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsOID = unpack(payload)?;
                let items = self.items.lock().unwrap();
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
                self.set_item_state(action.oid(), Some(params.status), params.value.into())
                    .await?;
                let payload = pack(&action.event_completed(None))?;
                let topic = format_action_topic(action.oid());
                self.tx.send((topic, payload)).await.map_err(Error::io)?;
                Ok(None)
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

impl Handlers {
    async fn set_item_state(
        &self,
        oid: &OID,
        status: Option<ItemStatus>,
        value: Option<Value>,
    ) -> EResult<()> {
        let payload = {
            let mut items = self.items.lock().unwrap();
            if let Some(item) = items.get_mut(oid) {
                if let Some(s) = status {
                    item.status = s;
                }
                if let Some(v) = value {
                    item.value = v;
                }
                let event = RawStateEvent::new(item.status, &item.value);
                pack(&event)?
            } else if self.auto_create {
                let item = VirtualItem::new(oid.clone(), status, value)?;
                let event = RawStateEvent::new(item.status, &item.value);
                let payload = pack(&event)?;
                items.insert(oid.clone(), item);
                payload
            } else {
                return Err(Error::not_found(oid));
            }
        };
        self.tx
            .send((format_raw_state_topic(oid), payload))
            .await
            .log_ef();
        Ok(())
    }
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let mut items = HashMap::new();
    if let Some(citems) = config.items {
        for oid in citems {
            items.insert(oid.clone(), VirtualItem::new0(oid)?);
        }
    }
    let (tx, rx) = async_channel::bounded(1024);
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
    let handlers = Handlers {
        info,
        items: Mutex::new(items),
        auto_create: config.auto_create,
        tx,
    };
    let rpc: Arc<RpcClient> = initial.init_rpc(handlers).await?;
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
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
