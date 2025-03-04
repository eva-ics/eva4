/// The core module
use crate::actmgr;
use crate::inventory_db;
use crate::items::{self, Filter, Inventory, InventoryStats, Item, ItemConfigData, NodeFilter};
use crate::spoint;
use crate::svcmgr;
use crate::Mode;
use crate::{EResult, Error};
use atty::Stream;
use busrt::client::AsyncClient;
use busrt::rpc::{Rpc, RpcClient};
use busrt::QoS;
use eva_common::acl::{OIDMask, OIDMaskList};
use eva_common::common_payloads::NodeData;
use eva_common::dobj::{DataObject, Endianess, ObjectMap};
use eva_common::err_logger;
use eva_common::events;
use eva_common::events::Force;
use eva_common::events::OnModifiedOwned;
use eva_common::events::ReplicationStateEventExtended;
use eva_common::events::{
    DbState, LocalStateEvent, NodeInfo, RawStateBulkEventOwned, RawStateEventOwned,
    RemoteStateEvent, ReplicationInventoryItem, ReplicationState, LOCAL_STATE_TOPIC,
    LOG_INPUT_TOPIC, RAW_STATE_BULK_TOPIC, RAW_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC,
    REMOTE_STATE_TOPIC, REPLICATION_INVENTORY_TOPIC, REPLICATION_NODE_STATE_TOPIC,
    REPLICATION_STATE_TOPIC, SERVICE_STATUS_TOPIC,
};
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use eva_common::registry;
use eva_common::time::monotonic_ns;
use eva_common::time::Time;
use eva_common::tools::format_path;
use eva_common::ITEM_STATUS_ERROR;
use eva_common::SLEEP_STEP;
use log::{debug, error, info, trace, warn};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use sysinfo::{ProcessExt, SystemExt};
use tokio::io::AsyncWriteExt;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Default)]
struct AccountingEvent<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    subj: Option<&'a str>,
    #[serde(default)]
    data: Value,
}

err_logger!();

const ACCOUNTING_TOPIC: &str = "AAA/REPORT";
const NODE_CHECKER_INTERVAL: Duration = Duration::from_secs(1);

/// Lvar operations
pub enum LvarOp {
    Set(Option<ItemStatus>, Option<Value>),
    Reset,
    Clear,
    Toggle,
    Increment,
    Decrement,
}

// clippy, this is not a file path!
#[allow(clippy::case_sensitive_file_extension_comparisons)]
#[inline]
pub fn sender_allowed_auto_create(sender: &str) -> bool {
    sender.contains(".controller.")
        || sender.contains(".plc.")
        || sender.ends_with(".plc")
        || sender.ends_with(".program")
}

pub enum ActionLaunchResult {
    State(bool),
    Local(Vec<u8>),
    Remote(Box<actmgr::ActionInfoOwned>),
}

#[derive(Deserialize)]
pub struct BusMessage {
    topic: String,
    message: Value,
}

#[allow(clippy::option_option)]
#[derive(Serialize)]
struct LvarRemoteOp<'a> {
    i: &'a OID,
    node: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<ItemStatus>,
    #[serde(skip_serializing_if = "ValueOptionOwned::is_none")]
    value: ValueOptionOwned,
}
impl<'a> LvarRemoteOp<'a> {
    fn from_op(op: LvarOp, oid: &'a OID, node: &'a str) -> Self {
        match op {
            LvarOp::Set(s, v) => Self {
                i: oid,
                node,
                status: s,
                value: v.into(),
            },
            _ => Self {
                i: oid,
                node,
                status: None,
                value: ValueOptionOwned::No,
            },
        }
    }
}

const ERR_MSG_STATE_LMACRO: &str = "can not update state for lmacro";

/// Announces states for local items
#[inline]
async fn announce_local_state<C>(
    oid: &OID,
    state: &LocalStateEvent,
    client: &Arc<Mutex<C>>,
) -> EResult<()>
where
    C: busrt::client::AsyncClient + ?Sized,
{
    client
        .lock()
        .await
        .publish(
            &format!("{}{}", LOCAL_STATE_TOPIC, oid.as_path()),
            pack(&state)?.into(),
            QoS::No,
        )
        .await?;
    Ok(())
}

/// Announces states for local items for a specific client
#[inline]
async fn announce_local_state_for<C>(
    oid: &OID,
    state: &LocalStateEvent,
    receiver: &str,
    client: &Arc<Mutex<C>>,
) -> EResult<()>
where
    C: busrt::client::AsyncClient + ?Sized,
{
    client
        .lock()
        .await
        .publish_for(
            &format!("{}{}", LOCAL_STATE_TOPIC, oid.as_path()),
            receiver,
            pack(&state)?.into(),
            QoS::No,
        )
        .await?;
    Ok(())
}

/// Announces states for remote items
#[inline]
async fn announce_remote_state<C>(
    oid: &OID,
    state: &RemoteStateEvent,
    current: bool,
    client: &Arc<Mutex<C>>,
) -> EResult<()>
where
    C: busrt::client::AsyncClient + ?Sized,
{
    client
        .lock()
        .await
        .publish(
            &format!(
                "{}{}",
                if current {
                    REMOTE_STATE_TOPIC
                } else {
                    REMOTE_ARCHIVE_STATE_TOPIC
                },
                oid.as_path()
            ),
            pack(&state)?.into(),
            QoS::No,
        )
        .await?;
    Ok(())
}

/// Announces states for remote items for a specific client
#[inline]
async fn announce_remote_state_for<C>(
    oid: &OID,
    state: &RemoteStateEvent,
    current: bool,
    receiver: &str,
    client: &Arc<Mutex<C>>,
) -> EResult<()>
where
    C: busrt::client::AsyncClient + ?Sized,
{
    client
        .lock()
        .await
        .publish_for(
            &format!(
                "{}{}",
                if current {
                    REMOTE_STATE_TOPIC
                } else {
                    REMOTE_ARCHIVE_STATE_TOPIC
                },
                oid.as_path()
            ),
            receiver,
            pack(&state)?.into(),
            QoS::No,
        )
        .await?;
    Ok(())
}

/// Stores local states into the registry
#[inline]
async fn save_item_state(oid: OID, state: DbState, rpc: &RpcClient) {
    trace!("saving state key for {}", oid);
    if inventory_db::is_initialized() {
        inventory_db::push_state(oid, state);
    } else {
        registry::key_set(registry::R_STATE, oid.as_path(), state, rpc)
            .await
            .log_ef_with("unable to save state");
    }
}

async fn handle_save(rx: async_channel::Receiver<(OID, DbState)>, rpc: &RpcClient) {
    while let Ok((oid, db_state)) = rx.recv().await {
        save_item_state(oid, db_state, rpc).await;
    }
}

/// Stores local item configs into the registry
#[inline]
async fn save_item_config(oid: &OID, config: Value, rpc: &RpcClient) -> EResult<()> {
    if inventory_db::is_initialized() {
        inventory_db::save_item(oid.clone(), config).await?;
    } else {
        registry::key_set(registry::R_INVENTORY, oid.as_path(), config, rpc).await?;
    }
    Ok(())
}

/// Destroys registry items (both state and config)
#[inline]
async fn destroy_inventory_item(oid: &OID, rpc: &RpcClient) -> EResult<()> {
    let oid_path = oid.as_path();
    if inventory_db::is_initialized() {
        inventory_db::destroy_item(oid.clone()).await?;
    } else {
        registry::key_delete(registry::R_STATE, oid_path, rpc).await?;
        registry::key_delete(registry::R_INVENTORY, oid_path, rpc).await?;
    }
    Ok(())
}

/// Serializes item state for announce plus db state if instant save is on
macro_rules! prepare_state_data {
    ($item: expr, $state: expr, $instant_save: expr) => {{
        let s_st: LocalStateEvent = Into::<LocalStateEvent>::into($state);
        let db_st = if $instant_save.matches($item.oid()) {
            Some(Into::<DbState>::into($state))
        } else {
            None
        };
        (s_st, db_st)
    }};
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
enum InstantSave {
    Strict(bool),
    ByMask(OIDMaskList),
}

impl InstantSave {
    #[inline]
    fn matches(&self, oid: &OID) -> bool {
        match self {
            InstantSave::Strict(v) => *v,
            InstantSave::ByMask(mask) => mask.matches(oid),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Core {
    #[serde(skip_deserializing, default = "crate::get_version_owned")]
    version: String,
    #[serde(skip_deserializing, default = "crate::get_build")]
    build: u64,
    #[serde(skip_deserializing, default = "crate::eapi::get_version")]
    eapi_version: u16,
    #[serde(skip_deserializing)]
    boot_id: u64,
    #[serde(skip)]
    dir_eva: String,
    #[serde(skip_deserializing)]
    system_arch: Option<&'static str>,
    #[serde(skip_deserializing)]
    mode: Mode,
    #[serde(skip_serializing)]
    inventory_db: Option<String>,
    #[serde(skip_deserializing, default)]
    system_name: String,
    #[serde(skip)]
    pid_file: String,
    instant_save: InstantSave,
    #[serde(default)]
    auto_create: bool,
    #[serde(default)]
    source_sensors: bool,
    #[serde(
        skip_serializing,
        deserialize_with = "eva_common::tools::de_float_as_duration"
    )]
    suicide_timeout: Duration,
    #[serde(
        skip_serializing,
        deserialize_with = "eva_common::tools::de_float_as_duration"
    )]
    timeout: Duration,
    #[serde(
        skip_serializing,
        deserialize_with = "eva_common::tools::de_float_as_duration"
    )]
    keep_action_history: Duration,
    workers: u32,
    #[serde(skip_deserializing, default = "std::process::id")]
    pid: u32,
    #[serde(
        skip_deserializing,
        serialize_with = "eva_common::time::serialize_time_now"
    )]
    time: (),
    #[serde(
        skip_deserializing,
        default = "Instant::now",
        serialize_with = "eva_common::time::serialize_uptime"
    )]
    uptime: Instant,
    #[serde(
        skip_deserializing,
        serialize_with = "eva_common::tools::serialize_atomic_bool"
    )]
    active: Arc<atomic::AtomicBool>,
    #[serde(skip)]
    running: Arc<atomic::AtomicBool>,
    #[serde(skip)]
    files_to_remove: Vec<String>, // pid files, sockets, etc
    #[serde(skip)]
    inventory: Arc<RwLock<Inventory>>,
    #[serde(skip)]
    rpc: OnceCell<Arc<RpcClient>>,
    #[serde(skip)]
    state_db_tx: OnceCell<async_channel::Sender<(OID, DbState)>>,
    #[serde(skip)]
    inv_process: std::sync::Mutex<HashSet<String>>, // locked sources
    #[serde(skip)]
    scheduled_saves: parking_lot::Mutex<HashSet<OID>>,
    #[serde(skip)]
    initial_announced: Mutex<bool>,
    #[serde(skip)]
    action_manager: Arc<actmgr::Manager>,
    #[serde(skip)]
    service_manager: svcmgr::Manager,
    #[serde(skip_deserializing, default = "crate::get_product_code")]
    product_code: String,
    #[serde(skip_deserializing, default = "crate::get_product_name")]
    product_name: String,
    #[serde(skip)]
    nodes: std::sync::Mutex<HashMap<String, NodeData>>,
    mem_warn: Option<u64>,
    #[serde(skip)]
    node_checker_fut: std::sync::Mutex<Option<JoinHandle<()>>>,
    #[serde(skip)]
    state_processor_lock: Arc<tokio::sync::Mutex<()>>,
    #[serde(skip_deserializing, default = "num_cpus::get")]
    num_cpus: usize,
    #[serde(skip)]
    object_map: parking_lot::Mutex<ObjectMap>,
}

/// # Panics
///
/// will panic (checker fut) if any mutex is poisoned
pub fn start_node_checker(core: Arc<Core>) {
    let timeout = core.timeout;
    let c = core.clone();
    let node_checker_fut = tokio::spawn(async move {
        loop {
            let mut ntc: HashMap<String, Vec<(String, Option<Duration>)>> = HashMap::new();
            // insert owned to release mutex asap
            for (k, v) in &*c.nodes.lock().unwrap() {
                let node_name = k.clone();
                if let Some(s) = ntc.get_mut(v.svc().unwrap()) {
                    s.push((node_name, v.timeout()));
                } else {
                    ntc.insert(v.svc().unwrap().to_owned(), vec![(node_name, v.timeout())]);
                }
            }
            for (s, nodes) in ntc {
                if let Ok(v) = c.service_manager.is_service_online(&s, timeout).await {
                    if !v {
                        for (node, timeout) in nodes {
                            c.mark_source_online(&node, &s, false, None, timeout).await;
                        }
                    }
                }
            }
            tokio::time::sleep(NODE_CHECKER_INTERVAL).await;
        }
    });
    core.node_checker_fut
        .lock()
        .unwrap()
        .replace(node_checker_fut);
}

macro_rules! handle_term_signal {
    ($kind: expr, $running: expr, $can_log: expr) => {
        tokio::spawn(async move {
            trace!("starting handler for {:?}", $kind);
            loop {
                match signal($kind) {
                    Ok(mut v) => {
                        v.recv().await;
                    }
                    Err(e) => {
                        error!("Unable to bind to signal {:?}: {}", $kind, e);
                        break;
                    }
                }
                if $can_log {
                    debug!("got termination signal");
                } else {
                    crate::logs::disable_console_log();
                }
                $running.store(false, atomic::Ordering::Relaxed);
            }
        });
    };
}

macro_rules! ignore_term_signal {
    ($kind: expr) => {
        tokio::spawn(async move {
            trace!("starting empty handler for {:?}", $kind);
            loop {
                match signal($kind) {
                    Ok(mut v) => {
                        v.recv().await;
                    }
                    Err(e) => {
                        error!("Unable to bind to signal {:?}: {}", $kind, e);
                        break;
                    }
                }
                trace!("got termination signal, ignoring");
            }
        });
    };
}

/// NOTE
/// the local state can be changed with 3 methods only:
/// create (sets blank)
/// update_state_from_raw (sets the state from the raw event)
/// deploy_local_items (may copy the old state if replace is allowed and the item exists)
///
/// Each of the methods (except create) MUST apply the item logic (if defined). If a new method is
/// added, list it here
///
/// Remote state updates do not need to apply the logic as remote items do not have it
impl Core {
    pub fn new_from_db(
        db: &mut yedb::Database,
        dir_eva: &str,
        system_name: Option<&str>,
        pid_file: Option<&str>,
    ) -> EResult<Self> {
        trace!("loading core config");
        let mut core: Core =
            serde_json::from_value(db.key_get(&registry::format_config_key("core"))?)?;
        core.update_paths(dir_eva, pid_file);
        core.system_name = if let Some(sname) = system_name {
            sname.to_owned()
        } else {
            get_hostname()?
        };
        core.action_manager.set_keep_for(core.keep_action_history);
        core.system_arch.replace(crate::ARCH_SFX);
        Ok(core)
    }
    pub fn new_spoint(
        dir_eva: &str,
        system_name: &str,
        pid_file: Option<&str>,
        suicide_timeout: Duration,
        timeout: Duration,
    ) -> Self {
        let mut core = Self {
            version: crate::VERSION.to_owned(),
            build: crate::BUILD,
            eapi_version: crate::eapi::EAPI_VERSION,
            boot_id: 0,
            dir_eva: <_>::default(),
            system_arch: Some(crate::ARCH_SFX),
            mode: Mode::SPoint,
            inventory_db: None,
            system_name: system_name.to_owned(),
            pid_file: <_>::default(),
            auto_create: false,
            source_sensors: false,
            instant_save: InstantSave::Strict(false),
            suicide_timeout,
            timeout,
            keep_action_history: Duration::from_secs(0),
            workers: crate::spoint::SPOINT_WORKERS,
            pid: std::process::id(),
            time: (),
            uptime: Instant::now(),
            active: Arc::new(atomic::AtomicBool::new(false)),
            running: Arc::new(atomic::AtomicBool::new(false)),
            files_to_remove: <_>::default(),
            inventory: <_>::default(),
            product_code: crate::PRODUCT_CODE.to_owned(),
            product_name: crate::PRODUCT_NAME.to_owned(),
            nodes: <_>::default(),
            node_checker_fut: <_>::default(),
            rpc: <_>::default(),
            state_db_tx: <_>::default(),
            inv_process: <_>::default(),
            scheduled_saves: <_>::default(),
            initial_announced: <_>::default(),
            action_manager: <_>::default(),
            service_manager: <_>::default(),
            state_processor_lock: <_>::default(),
            num_cpus: num_cpus::get(),
            mem_warn: None,
            object_map: <_>::default(),
        };
        core.update_paths(dir_eva, pid_file);
        core
    }
    pub async fn init_inventory_db(&self) -> EResult<()> {
        if let Some(ref idb) = self.inventory_db {
            info!("connecting to {}", idb);
            inventory_db::init(idb, self.workers, self.timeout).await?;
        }
        Ok(())
    }
    pub async fn load_inventory_db(&self) -> EResult<()> {
        if inventory_db::is_initialized() {
            info!("loading inventory from the database");
            let mut inv = self.inventory.write().await;
            inventory_db::load(&mut inv, self.boot_id).await?;
        }
        Ok(())
    }
    pub fn load_inventory(&self, db: &mut yedb::Database) -> EResult<()> {
        if self.inventory_db.is_none() {
            info!("loading inventory from the registry");
            let inv_key = registry::format_top_key(registry::R_INVENTORY);
            let inv_offs = inv_key.len() + 1;
            let inv = db.key_get_recursive(&inv_key)?;
            let st_key = registry::format_top_key(registry::R_STATE);
            let st_offs = st_key.len() + 1;
            info!("loading states");
            let st = db.key_get_recursive(&st_key)?;
            let mut states: HashMap<String, serde_json::Value> = HashMap::new();
            for (p, i) in st {
                states.insert(p[st_offs..].to_owned(), i);
            }
            info!("creating inventory");
            let mut inventory = self.inventory.try_write()?;
            for (p, i) in inv {
                let s = &p[inv_offs..];
                let oid: OID = OID::from_path(s)?;
                debug!("loading {oid}");
                let state = if let Some(st) = states.remove(s) {
                    Some(serde_json::from_value(st)?)
                } else {
                    None
                };
                inventory.append_item_from_value(&oid, i, state, self.boot_id)?;
            }
        }
        Ok(())
    }
    async fn announce_startup(&self) {
        #[derive(Serialize)]
        struct Info<'a> {
            version: &'a str,
            build: u64,
            boot_id: u64,
            mode: Mode,
        }
        let Ok(data) = to_value(Info {
            version: &self.version,
            build: self.build,
            boot_id: self.boot_id,
            mode: self.mode,
        }) else {
            return;
        };
        let rpc = self.rpc.get().unwrap();
        let Ok(payload) = pack(&AccountingEvent {
            subj: Some("started"),
            data,
        }) else {
            return;
        };
        let _ = rpc
            .client()
            .lock()
            .await
            .publish(ACCOUNTING_TOPIC, payload.into(), QoS::No)
            .await;
    }
    async fn announce_terminating(&self) {
        let rpc = self.rpc.get().unwrap();
        let Ok(payload) = pack(&AccountingEvent {
            subj: Some("terminating"),
            ..Default::default()
        }) else {
            return;
        };
        let _ = rpc
            .client()
            .lock()
            .await
            .publish(ACCOUNTING_TOPIC, payload.into(), QoS::Processed)
            .await;
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
    #[inline]
    fn generate_ieid(&self) -> IEID {
        IEID::new(self.boot_id, monotonic_ns())
    }
    #[inline]
    pub fn boot_id(&self) -> u64 {
        self.boot_id
    }
    #[inline]
    pub fn nodes(&self) -> &std::sync::Mutex<HashMap<String, NodeData>> {
        &self.nodes
    }
    #[inline]
    pub async fn inventory_stats(&self) -> InventoryStats {
        self.inventory.read().await.stats()
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub fn lock_source(&self, source_id: &str) -> bool {
        let mut inv_process = self.inv_process.lock().unwrap();
        if inv_process.contains(source_id) {
            false
        } else {
            inv_process.insert(source_id.to_owned());
            true
        }
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn unlock_source(&self, source_id: &str) {
        self.inv_process.lock().unwrap().remove(source_id);
    }
    /// # Panics
    ///
    /// Will panic if the core rpc is not set or the mutex is poisoned
    pub async fn save(&self) -> EResult<()> {
        trace!("saving scheduled item states");
        let oids: HashSet<OID> = mem::take(&mut self.scheduled_saves.lock());
        if oids.is_empty() {
            return Ok(());
        }
        let rpc = self.rpc.get().unwrap();
        for oid in oids {
            let r = self.inventory.read().await.get_item(&oid);
            if let Some(s_state) = r.and_then(|item| item.db_state()) {
                save_item_state(oid, s_state, rpc).await;
            }
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the core rpc is not set
    pub async fn create_local_item(&self, oid: OID) -> EResult<Item> {
        let tp = oid.kind();
        let ieid = if tp == ItemKind::Lmacro {
            None
        } else {
            Some(self.generate_ieid())
        };
        trace!("creating local item: {}", oid);
        let result = self.inventory.write().await.create_item(oid, ieid, None);
        match result {
            Ok(item) => {
                let rpc = self.rpc.get().unwrap();
                save_item_config(item.oid(), item.config()?, rpc).await?;
                if let Some(stc) = item.state() {
                    let (s_state, db_st) =
                        prepare_state_data!(item, &*stc.lock(), self.instant_save);
                    self.process_new_state(item.oid(), s_state, db_st, rpc)
                        .await?;
                }
                info!("local item created: {}", item.oid());
                Ok(item)
            }
            Err(e) if e.kind() == ErrorKind::ResourceAlreadyExists => Err(e),
            Err(e) => {
                error!("{}", e);
                Err(e)
            }
        }
    }
    /// Proceses state for a local item
    async fn process_new_state(
        &self,
        oid: &OID,
        s_state: LocalStateEvent,
        db_st: Option<DbState>,
        rpc: &RpcClient,
    ) -> EResult<()> {
        trace!("announcing state for {}", oid);
        announce_local_state(oid, &s_state, &rpc.client())
            .await
            .log_ef();
        if let Some(db_state) = db_st {
            if let Some(state_db_tx) = self.state_db_tx.get() {
                state_db_tx.send((oid.clone(), db_state)).await.log_ef();
            }
        } else {
            trace!("scheduling state save for {}", oid);
            self.schedule_save(oid);
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the core rpc is not set
    ///
    /// Destroys local items by OID or mask
    #[inline]
    pub async fn destroy_local_items(&self, mask: &OIDMask) {
        let rpc = self.rpc.get().unwrap();
        let items = self.inventory.read().await.get_items_by_mask(
            mask,
            &Filter::default().node(NodeFilter::Local),
            true,
        );
        for item in items {
            trace!("destroying local item: {}", item.oid());
            let _r = self.inventory.write().await.remove_item(item.oid());
            self.unschedule_save(item.oid());
            destroy_inventory_item(item.oid(), rpc).await.log_ef();
            info!("local item destroyed: {}", item.oid());
        }
    }
    /// Creates/recreates local items from Vec of values
    ///
    /// # Panics
    ///
    /// Will panic if the core rpc is not set
    #[inline]
    pub async fn deploy_local_items(
        &self,
        configs: Vec<ItemConfigData>,
        lock: bool,
    ) -> EResult<()> {
        let rpc = self.rpc.get().unwrap();
        if inventory_db::is_initialized() {
            let mut configs_to_save: Vec<(OID, Value)> = Vec::with_capacity(configs.len());
            let mut oids = Vec::with_capacity(configs.len());
            for val in configs {
                let item = self
                    .inventory
                    .write()
                    .await
                    .append_item_from_config_data(val, self.boot_id)?;
                let oid = item.oid();
                oids.push(oid.clone());
                info!("local item created: {}", oid);
                trace!("saving config for {}", oid);
                configs_to_save.push((oid.clone(), item.config()?));
            }
            inventory_db::save_items_bulk(configs_to_save).await?;
            for oid in oids {
                if let Some(item) = self.inventory.read().await.get_item(&oid) {
                    if let Some(stc) = item.state() {
                        let _stp_lock = if lock {
                            Some(self.state_processor_lock.lock().await)
                        } else {
                            None
                        };
                        let (s_state, db_st) =
                            prepare_state_data!(item, &*stc.lock(), self.instant_save);
                        //process new state without db_st, manually save
                        self.process_new_state(item.oid(), s_state, None, rpc)
                            .await?;
                        if let Some(db_state) = db_st {
                            inventory_db::push_state(oid, db_state);
                        }
                    }
                }
            }
        } else {
            for val in configs {
                let item = self
                    .inventory
                    .write()
                    .await
                    .append_item_from_config_data(val, self.boot_id)?;
                let oid = item.oid();
                info!("local item created: {}", oid);
                trace!("saving config for {}", oid);
                save_item_config(oid, item.config()?, rpc).await?;
                if let Some(stc) = item.state() {
                    let _stp_lock = if lock {
                        Some(self.state_processor_lock.lock().await)
                    } else {
                        None
                    };
                    let (s_state, db_st) =
                        prepare_state_data!(item, &*stc.lock(), self.instant_save);
                    self.process_new_state(item.oid(), s_state, db_st, rpc)
                        .await?;
                }
            }
        }
        Ok(())
    }

    #[inline]
    pub async fn terminate_action(&self, uuid: &Uuid) -> EResult<()> {
        warn!("terminating action {}", uuid);
        self.action_manager
            .terminate_action(uuid, self.timeout)
            .await
    }

    pub async fn kill_actions(&self, oid: &OID) -> EResult<()> {
        warn!("killing actions for {}", oid);
        let target = {
            let inv = self.inventory.read().await;
            if let Some(unit) = inv.get_item(oid) {
                if let Some(action_params) = unit.action() {
                    action_params.svc().to_owned()
                } else {
                    return Err(Error::failed("unit action not configured"));
                }
            } else {
                return Err(Error::not_found(format!("unit not found: {}", oid)));
            }
        };
        self.action_manager
            .kill_actions(oid, &target, self.timeout)
            .await
    }

    /// Returns the result already serialized to avoid value copying
    #[inline]
    pub async fn action_result_serialized(&self, uuid: &Uuid) -> EResult<Vec<u8>> {
        if let Some(info) = self
            .action_manager
            .get_action_serialized(uuid, &self.system_name, self.timeout)
            .await?
        {
            Ok(info)
        } else {
            Err(Error::not_found("action not found"))
        }
    }
    /// # Panics
    ///
    /// Will panic if state mutex is poisoned
    ///
    /// If state only result is requested, the action is automatically terminated on failure / wait
    /// timeout
    #[allow(clippy::too_many_lines)]
    pub async fn action(
        &self,
        u: Option<Uuid>,
        oid: &OID,
        // None for unit toggle
        action_params: Option<eva_common::actions::Params>,
        priority: u8,
        wait: Option<Duration>,
        state_only_result: bool,
    ) -> EResult<ActionLaunchResult> {
        debug!("launching action for {}", oid);
        let params = if let Some(p) = action_params {
            match p {
                eva_common::actions::Params::Unit(_) => {
                    if oid.kind() != ItemKind::Unit {
                        return Err(Error::invalid_data("item is not a unit"));
                    }
                }
                eva_common::actions::Params::Lmacro(_) => {
                    if oid.kind() != ItemKind::Lmacro {
                        return Err(Error::invalid_data("item is not a macro"));
                    }
                }
            }
            Some(p)
        } else {
            if oid.kind() != ItemKind::Unit {
                return Err(Error::invalid_data("item is not a unit"));
            }
            None
        };
        let (action, listener, core_listener, action_timeout) = {
            let inv = self.inventory.read().await;
            if let Some(item) = inv.get_item(oid) {
                if !item.enabled() {
                    return Err(Error::access(format!("{oid} is disabled")));
                }
                let a_params = if let Some(p) = params {
                    p
                } else if let Some(state) = item.state() {
                    let val = if let Ok(n) = i64::try_from(state.lock().value()) {
                        Value::U8(u8::from(n <= 0))
                    } else {
                        Value::U8(1)
                    };
                    eva_common::actions::Params::new_unit(val)
                } else {
                    return Err(Error::access(format!("{oid} has no state to toggle")));
                };
                if let Some(source) = item.source() {
                    let (action, listener, core_listener) =
                        actmgr::Action::create(actmgr::ActionArgs {
                            uuid: u,
                            oid,
                            params: a_params,
                            timeout: None,
                            priority,
                            config: None,
                            node: Some(source.node().to_owned()),
                            target: source.svc().to_owned(),
                            wait,
                        });
                    (
                        action,
                        listener,
                        core_listener,
                        if let Some(n) = self.nodes.lock().unwrap().get(source.node()) {
                            n.timeout()
                        } else {
                            None
                        },
                    )
                } else if let Some(action_params) = item.action() {
                    let (action, listener, core_listener) =
                        actmgr::Action::create(actmgr::ActionArgs {
                            uuid: u,
                            oid,
                            params: a_params,
                            timeout: Some(action_params.timeout().unwrap_or(self.timeout)),
                            priority,
                            config: action_params.config().cloned(),
                            node: None,
                            target: action_params.svc().to_owned(),
                            wait: None,
                        });
                    let _stp_lock = self.state_processor_lock.lock().await;
                    let s_st = if let Some(stc) = item.state() {
                        let mut state = stc.lock();
                        state.act_incr(self.generate_ieid());
                        Some(Into::<LocalStateEvent>::into(&*state))
                    } else {
                        None
                    };
                    if let Some(s_state) = s_st {
                        let rpc = self.rpc.get().unwrap();
                        announce_local_state(item.oid(), &s_state, &rpc.client()).await?;
                    }
                    (action, listener, core_listener, None)
                } else {
                    return Err(Error::failed(format!("{oid} action not configured")));
                }
            } else {
                return Err(Error::not_found(format!("{oid} not found")));
            }
        };
        if action.node().is_none() {
            let uuid = *action
                .uuid()
                .ok_or_else(|| Error::core(actmgr::ERR_NO_UUID))?;
            let (t_accepted, l_accepted) = triggered::trigger();
            let action_manager = self.action_manager.clone();
            let inventory = self.inventory.clone();
            let oid = oid.clone();
            let boot_id = self.boot_id;
            let default_timeout = self.timeout;
            let rpc = self.rpc.get().unwrap().clone();
            let state_processor_lock = self.state_processor_lock.clone();
            tokio::spawn(async move {
                let timeout = action.timeout().unwrap_or(default_timeout);
                action_manager
                    .launch_action(action, Some(t_accepted), timeout)
                    .await
                    .log_ef();
                if tokio::time::timeout(timeout, core_listener).await.is_err() {
                    action_manager.mark_action_timed_out(&uuid);
                }
                if oid.kind() == ItemKind::Unit {
                    let inv = inventory.read().await;
                    if let Some(unit) = inv.get_item(&oid) {
                        if unit.source().is_none() {
                            let _stp_lock = state_processor_lock.lock().await;
                            let s_st = if let Some(stc) = unit.state() {
                                let mut state = stc.lock();
                                let ieid = IEID::new(boot_id, monotonic_ns());
                                state.act_decr(ieid);
                                Some(Into::<LocalStateEvent>::into(&*state))
                            } else {
                                None
                            };
                            if let Some(s_state) = s_st {
                                announce_local_state(unit.oid(), &s_state, &rpc.client())
                                    .await
                                    .log_ef();
                            }
                        }
                    }
                }
            });
            l_accepted.await;
            if let Some(w) = wait {
                let _r = tokio::time::timeout(w, listener).await;
            }
            if state_only_result {
                Ok(ActionLaunchResult::State(
                    self.action_manager
                        .must_be_completed(&uuid, self.timeout)
                        .await,
                ))
            } else {
                let info = self.action_result_serialized(&uuid).await?;
                Ok(ActionLaunchResult::Local(info))
            }
        } else {
            let timeout = action_timeout.unwrap_or(self.timeout);
            let res = self
                .action_manager
                .launch_action(action, None, timeout)
                .await
                .log_err()?
                .ok_or_else(|| Error::invalid_data("no action info from svc"))?;
            if state_only_result {
                let s = res.check_completed();
                if !s {
                    self.action_manager
                        .terminate_action(&res.uuid, self.timeout)
                        .await
                        .log_ef();
                }
                Ok(ActionLaunchResult::State(s))
            } else {
                Ok(ActionLaunchResult::Remote(Box::new(res)))
            }
        }
    }
    /// # Panics
    ///
    /// Will panic if the core rpc is not set
    ///
    /// Destroys local items by vec of OIDs or item configs
    #[inline]
    pub async fn undeploy_local_items(&self, configs: Vec<Value>) -> EResult<()> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ItemOrOid {
            Item(Box<items::ItemConfigData>),
            Oid(Arc<OID>),
        }
        let rpc = self.rpc.get().unwrap();
        let mut oids: Vec<Arc<OID>> = Vec::new();
        for c in configs {
            let d = ItemOrOid::deserialize(c)?;
            oids.push(match d {
                ItemOrOid::Item(i) => i.oid,
                ItemOrOid::Oid(oid) => oid,
            });
        }
        if inventory_db::is_initialized() {
            let mut oids_to_destroy: Vec<OID> = Vec::new();
            for oid in oids {
                let result = self.inventory.write().await.remove_item(&oid);
                if result.is_some() {
                    info!("local item destroyed: {}", oid);
                    self.unschedule_save(&oid);
                    oids_to_destroy.push(oid.as_ref().clone());
                }
            }
            inventory_db::destroy_items_bulk(oids_to_destroy)
                .await
                .log_ef_with("unable to destroy items in database");
        } else {
            for oid in oids {
                let result = self.inventory.write().await.remove_item(&oid);
                if result.is_some() {
                    destroy_inventory_item(&oid, rpc).await.log_ef();
                    info!("local item destroyed: {}", oid);
                }
                self.unschedule_save(&oid);
            }
        }
        Ok(())
    }
    fn mark_core_node_online(
        &self,
        source_id: &str,
        svc: &str,
        online: bool,
        info: Option<NodeInfo>,
        timeout: Option<Duration>,
    ) -> bool {
        macro_rules! log_node_online {
            () => {
                info!(
                    "marking the source {} {}",
                    source_id,
                    if online { "online" } else { "offline" }
                );
            };
        }
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(node) = nodes.get_mut(source_id) {
            if node.online() != online {
                log_node_online!();
                node.set_online(online);
            }
            if svc != node.svc().unwrap() {
                warn!(
                    "node {} handler moved from {} to {}, destroying",
                    source_id,
                    node.svc().unwrap(),
                    source_id
                );
                return false;
            }
            if let Some(i) = info {
                node.update_info(i);
            }
            node.update_timeout(timeout);
        } else {
            log_node_online!();
            nodes.insert(
                source_id.to_owned(),
                NodeData::new(Some(svc), online, info, timeout),
            );
        }
        true
    }
    async fn set_source_sensor(&self, source_id: &str, online: bool) -> EResult<()> {
        let mut inventory = self.inventory.write().await;
        let source_sensor_oid: OID =
            format!("sensor:{}/node/{}", self.system_name, source_id).parse()?;
        let mut source_sensor = inventory.get_item(&source_sensor_oid);
        let ieid = self.generate_ieid();
        if source_sensor.is_none() {
            source_sensor = Some(inventory.create_item(source_sensor_oid, Some(ieid), None)?);
        }
        let item = source_sensor.unwrap();
        let state = item
            .state()
            .ok_or_else(|| Error::core("no state for src sensor"))?;
        let (s_state, db_st) = {
            let mut stc = state.lock();
            stc.force_set_state(Some(1), Some(Value::U8(u8::from(online))), ieid);
            prepare_state_data!(item, &*stc, self.instant_save)
        };
        let rpc = self.rpc.get().unwrap();
        self.process_new_state(item.oid(), s_state, db_st, rpc)
            .await?;
        Ok(())
    }
    #[inline]
    pub async fn mark_source_online(
        &self,
        source_id: &str,
        svc: &str,
        online: bool,
        info: Option<NodeInfo>,
        timeout: Option<Duration>,
    ) {
        if self.mark_core_node_online(source_id, svc, online, info, timeout) {
            self.inventory
                .read()
                .await
                .mark_source_online(source_id, online);
            if self.source_sensors {
                self.set_source_sensor(source_id, online).await.log_ef();
            }
        } else {
            self.destroy_source(source_id).await;
        }
    }
    /// # Panics
    ///
    /// will panic if the node mutex is poisoned
    pub async fn destroy_source(&self, source_id: &str) {
        info!("destroying elements from the source: {}", source_id);
        let source = self
            .inventory
            .read()
            .await
            .get_or_create_source(source_id, "");
        source.mark_destroyed();
        while !self.lock_source(source_id) {
            // wait until inventory processor abort
            sleep(SLEEP_STEP).await;
        }
        self.nodes.lock().unwrap().remove(source_id);
        let i = self.inventory.read().await.get_items_by_source(source_id);
        if let Some(items) = i {
            for (oid, _item) in items {
                let _r = self.inventory.write().await.remove_item(&oid);
            }
        }
        info!("source destroyed: {}", source_id);
        self.unlock_source(source_id);
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    ///
    /// Sets lvar state from RPC call
    ///
    /// LVar state is always announced
    pub async fn lvar_op(
        &self,
        oid: &OID,
        op: LvarOp,
        method_orig: &str,
    ) -> EResult<Option<Value>> {
        if oid.kind() != ItemKind::Lvar {
            return Err(Error::not_implemented(
                "Lvar ops can be applied to Lvars only",
            ));
        }
        let lvar = self
            .inventory
            .read()
            .await
            .get_item(oid)
            .ok_or_else(|| Error::not_found(oid))?;
        if !lvar.enabled() {
            return Err(Error::access(format!("Lvar {} is disabled", oid)));
        }
        if let Some(source) = lvar.source() {
            let rpc = self.rpc.get().unwrap();
            return unpack(
                tokio::time::timeout(
                    self.timeout,
                    rpc.call(
                        source.svc(),
                        method_orig,
                        pack(&LvarRemoteOp::from_op(op, oid, source.node()))?.into(),
                        QoS::Processed,
                    ),
                )
                .await??
                .payload(),
            )
            .map_err(Into::into);
        }
        if let Some(st) = lvar.state() {
            let rpc = self.rpc.get().unwrap();
            let (s_state, db_st, value) = {
                let mut state = st.lock();
                let ieid = self.generate_ieid();
                let value = match op {
                    LvarOp::Set(status, value) => {
                        trace!("setting lvar {} state to {:?} {:?}", oid, status, value);
                        state.force_set_state(status, value, ieid);
                        None
                    }
                    LvarOp::Reset => {
                        trace!("resetting lvar {} state", oid);
                        state.force_set_state(Some(1), None, ieid);
                        None
                    }
                    LvarOp::Clear => {
                        trace!("clearing lvar {} state", oid);
                        state.force_set_state(Some(0), None, ieid);
                        None
                    }
                    LvarOp::Toggle => {
                        trace!("toggling lvar {} state", oid);
                        let st = state.status();
                        state.force_set_state(Some(ItemStatus::from(st == 0)), None, ieid);
                        None
                    }
                    LvarOp::Increment => {
                        trace!("incrementing lvar {} value", oid);
                        let val: i64 = state.value().try_into().unwrap_or_default();
                        if val == i64::MAX {
                            return Err(Error::invalid_data("value too big"));
                        }
                        let value: Value = (val + 1).into();
                        state.force_set_value(value.clone(), ieid);
                        Some(value)
                    }
                    LvarOp::Decrement => {
                        trace!("decrementing lvar {} value", oid);
                        let val: i64 = state.value().try_into().unwrap_or_default();
                        if val == i64::MIN {
                            return Err(Error::invalid_data("value too small"));
                        }
                        let value: Value = (val - 1).into();
                        state.force_set_value(value.clone(), ieid);
                        Some(value)
                    }
                };
                let (s_state, db_st) = prepare_state_data!(lvar, &*state, self.instant_save);
                (s_state, db_st, value)
            };
            self.process_new_state(oid, s_state, db_st, rpc).await?;
            Ok(value)
        } else {
            Err(Error::core(format!("Lvar {} has no state", oid)))
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn process_modified(
        &self,
        om: events::OnModifiedOwned,
        really_modified: bool,
        force: Force,
        delta: Option<f64>,
        t: Option<f64>,
        sender: &str,
    ) -> EResult<()> {
        match om {
            events::OnModifiedOwned::SetOther(o) => {
                if really_modified {
                    for oid in o.oid {
                        for item in self.inventory.read().await.get_items_by_mask(
                            &oid,
                            &Filter::default().node(NodeFilter::Local),
                            false,
                        ) {
                            let rw = RawStateEventOwned {
                                status: o.status,
                                value: o.value.clone(),
                                force,
                                t,
                                ..RawStateEventOwned::default()
                            };
                            self.process_raw_state(item, rw, false, sender)
                                .await
                                .log_ef();
                        }
                    }
                }
            }
            events::OnModifiedOwned::SetOtherValueDelta(o) => {
                macro_rules! ev {
                    ($delta: expr) => {
                        RawStateEventOwned {
                            status: 1,
                            value: ValueOptionOwned::Value(Value::F64($delta)),
                            force,
                            t,
                            ..RawStateEventOwned::default()
                        }
                    };
                }
                macro_rules! ev_reset {
                    () => {
                        RawStateEventOwned {
                            status: 1,
                            value: ValueOptionOwned::Value(Value::F64(0.0)),
                            force,
                            t,
                            ..RawStateEventOwned::default()
                        }
                    };
                }
                if let Some(delta) = delta {
                    let mut rw = None;
                    macro_rules! apply_delta {
                        () => {
                            if delta < 0.0 {
                                match o.on_negative {
                                    events::OnNegativeDelta::Skip => {}
                                    events::OnNegativeDelta::Reset => {
                                        rw = Some(ev_reset!());
                                    }
                                    // overflow is already respected
                                    events::OnNegativeDelta::Process
                                    | events::OnNegativeDelta::Overflow { floor: _, ceil: _ } => {
                                        rw = Some(ev!(delta));
                                    }
                                }
                            } else {
                                rw = Some(ev!(delta));
                            }
                        };
                    }
                    if let Some(item) = self.inventory.read().await.get_item(&o.oid) {
                        if let Some(state) = item.state() {
                            if state.lock().status() <= ITEM_STATUS_ERROR {
                                match o.on_error {
                                    events::OnModifiedError::Skip => {}
                                    events::OnModifiedError::Reset => {
                                        rw = Some(ev_reset!());
                                    }
                                    events::OnModifiedError::Process => {
                                        apply_delta!();
                                    }
                                }
                            } else {
                                apply_delta!();
                            }
                        }
                        if let Some(rw) = rw {
                            self.process_raw_state(item, rw, false, sender)
                                .await
                                .log_ef();
                        }
                        return Ok(());
                    }
                    if self.auto_create && sender_allowed_auto_create(sender) {
                        apply_delta!();
                        if let Some(rw) = rw {
                            self.auto_create_item_from_raw(&o.oid, rw, sender).await;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    ///
    /// Note: LVar state is not updated when the status is 0
    pub async fn update_state_from_raw_bulk(&self, raw: Vec<RawStateBulkEventOwned>, sender: &str) {
        let mut item_events_to_process = Vec::with_capacity(raw.len());
        let mut item_events_to_create = Vec::new();
        let mut allow_auto_create = None;
        {
            let inv = self.inventory.read().await;
            for r in raw {
                let tp = r.oid.kind();
                if tp == ItemKind::Lmacro {
                    Err::<(), Error>(Error::not_implemented(ERR_MSG_STATE_LMACRO)).log_ef();
                    continue;
                }
                if let Some(item) = inv.get_item(&r.oid) {
                    item_events_to_process.push((item, RawStateEventOwned::from(r)));
                } else {
                    if allow_auto_create.is_none() {
                        allow_auto_create.replace(sender_allowed_auto_create(sender));
                    }
                    if self.auto_create && allow_auto_create.unwrap() {
                        item_events_to_create.push(r);
                    }
                }
            }
        }
        let _stp_lock = self.state_processor_lock.lock().await;
        for (item, r) in item_events_to_process {
            self.process_raw_state(item, r, false, sender)
                .await
                .log_efd();
        }
        if !item_events_to_create.is_empty() {
            self.auto_create_item_from_raw_bulk(item_events_to_create, sender, false)
                .await;
        }
    }

    #[async_recursion::async_recursion]
    async fn process_raw_state(
        &self,
        item: Item,
        mut raw: RawStateEventOwned,
        lock: bool,
        sender: &str,
    ) -> EResult<()> {
        if item.source().is_some() {
            return Err(Error::busy(format!(
                "unable to update item {} from raw event: remote",
                item.oid()
            )));
        }
        let force = raw.force;
        if !item.enabled() && force != Force::Full {
            debug!(
                "ignoring state from raw event for {} - disabled",
                item.oid()
            );
            return Ok(());
        }
        let Some(state) = item.state() else {
            warn!("no state property in {}", item.oid());
            return Ok(());
        };
        debug!(
            "setting state from raw event for {}, status: {}, value: {:?}",
            item.oid(),
            raw.status,
            raw.value
        );
        let stp_lock = if lock {
            Some(self.state_processor_lock.lock().await)
        } else {
            None
        };
        let on_modified = raw.on_modified.take();
        let mut delta = None;
        let t = raw.t;
        let now = Time::now().timestamp();
        let ((s_state, db_st), really_modified) = {
            let mut state = state.lock();
            if item.oid().kind() == ItemKind::Lvar && state.status() == 0 && force == Force::None {
                // lvars with status 0 are not set from RAW
                return Ok(());
            }
            if let Some(OnModifiedOwned::SetOtherValueDelta(ref om)) = on_modified {
                if let ValueOptionOwned::Value(ref value) = raw.value {
                    let prev_value = state.value();
                    if *prev_value != Value::Unit {
                        let current: f64 = value.try_into().unwrap_or_default();
                        let previous: f64 = prev_value.try_into().unwrap_or_default();
                        if let Some(period) = om.period {
                            let elapsed = now - state.t();
                            if elapsed > 0.0 {
                                delta = Some(
                                    calc_delta(current, previous, om.on_negative) / period
                                        * elapsed,
                                );
                            } else {
                                warn!(
                                    "ignoring delta for {} - non-positive time delta",
                                    item.oid()
                                );
                            }
                        } else {
                            // no period
                            delta = Some(calc_delta(current, previous, om.on_negative));
                        }
                    }
                }
            }
            let really_modified =
                state.set_from_raw(raw, item.logic(), item.oid(), self.boot_id, now);
            // status/value have been really modified or forced to report
            if really_modified || force != Force::None {
                (
                    prepare_state_data!(item, &*state, self.instant_save),
                    really_modified,
                )
            } else {
                // not really_modified
                return Ok(());
            }
        };
        let rpc = self.rpc.get().unwrap();
        self.process_new_state(item.oid(), s_state, db_st, rpc)
            .await?;
        drop(stp_lock);
        if let Some(om) = on_modified {
            self.process_modified(om, really_modified, force, delta, t, sender)
                .await?;
        };
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    ///
    /// Note: LVar state is not updated when the status is 0
    pub async fn update_state_from_raw(
        &self,
        oid: &OID,
        raw: RawStateEventOwned,
        sender: &str,
    ) -> EResult<()> {
        let tp = oid.kind();
        if tp == ItemKind::Lmacro {
            return Err(Error::not_implemented(ERR_MSG_STATE_LMACRO));
        }
        let maybe_item = self.inventory.read().await.get_item(oid);
        if let Some(item) = maybe_item {
            self.process_raw_state(item, raw, true, sender).await
        } else if self.auto_create && sender_allowed_auto_create(sender) {
            self.auto_create_item_from_raw(oid, raw, sender).await;
            Ok(())
        } else {
            Err(Error::not_found(oid))
        }
    }

    pub async fn auto_create_item_from_raw_empty(&self, oid: &OID, sender: &str) {
        if self.auto_create && sender_allowed_auto_create(sender) {
            let item_config = ItemConfigData::blank(oid, sender);
            info!(
                "auto-creating local item {} (blank) source: {}",
                oid, sender
            );
            if let Err(e) = self.deploy_local_items(vec![item_config], true).await {
                error!("auto-creation failed for {}: {}", oid, e);
            }
        }
    }

    async fn auto_create_item_from_raw(&self, oid: &OID, raw: RawStateEventOwned, sender: &str) {
        // do not auto-create items if event time is 0 (not modified)
        if let Some(t) = raw.t {
            if t == 0.0 {
                return;
            }
        }
        let item_config = ItemConfigData::from_raw_event(oid, raw, sender);
        info!("auto-creating local item {} source: {}", oid, sender);
        if let Err(e) = self.deploy_local_items(vec![item_config], true).await {
            error!("auto-creation failed for {}: {}", oid, e);
        }
    }

    async fn auto_create_item_from_raw_bulk(
        &self,
        raw: Vec<RawStateBulkEventOwned>,
        sender: &str,
        lock: bool,
    ) {
        let item_configs = raw
            .into_iter()
            .map(|r| {
                let (oid, rseo) = r.split_into_oid_and_rseo();
                info!("auto-creating local item {} source: {}", oid, sender);
                ItemConfigData::from_raw_event(&oid, rseo, sender)
            })
            .collect();
        if let Err(e) = self.deploy_local_items(item_configs, lock).await {
            error!("auto-creation failed for bulk bus frame: {}", e);
        }
    }

    /// # Panics
    ///
    /// Will panic if the core rpc is not set
    // if refactoring, do not write-lock the inventory for long, as the process may take a very
    // long time!
    #[allow(clippy::too_many_lines)]
    pub async fn process_remote_inventory(
        &self,
        remote_inv: HashMap<OID, ReplicationInventoryItem>,
        source_id: &str,
        sender: &str,
        // do not remove missing items, do not lock the source
        quick: bool,
    ) -> EResult<()> {
        #[inline]
        fn check_state(item: &ReplicationInventoryItem) -> bool {
            let tp = item.oid.kind();
            match tp {
                ItemKind::Lvar | ItemKind::Sensor => {
                    if item.act.is_some() || item.ieid.is_none() || item.t.is_none() {
                        warn!("invalid repl item {} state", item.oid);
                        return false;
                    }
                }
                ItemKind::Lmacro => {
                    if item.status.is_some()
                        || item.value.is_some()
                        || item.act.is_some()
                        || item.ieid.is_some()
                        || item.t.is_some()
                    {
                        warn!("invalid repl item {} state", item.oid);
                        return false;
                    }
                }
                ItemKind::Unit => {
                    if item.ieid.is_none() || item.t.is_none() {
                        warn!("invalid repl item {} state", item.oid);
                        return false;
                    }
                }
            }
            true
        }
        if source_id.starts_with('.') {
            return Err(Error::invalid_params(
                "source ids starting with dots are reserved, ignoring incoming payload",
            ));
        }
        if !quick && !self.lock_source(source_id) {
            warn!(
                "source {} inventory processor is busy. ignoring incoming payload",
                source_id
            );
            return Ok(());
        }
        let (existing_items, source) = {
            let inv = self.inventory.read().await;
            (
                inv.get_items_by_source(source_id),
                inv.get_or_create_source(source_id, sender),
            )
        };
        let online = source.online();
        let rpc = self.rpc.get().unwrap();
        if let Some(existing) = existing_items {
            // remove deleted items
            if !quick {
                for oid in existing.keys() {
                    if source.is_destroyed() {
                        break;
                    }
                    if !remote_inv.contains_key(oid.as_ref()) {
                        debug!(
                            "removing remote item {}, node: {} from rpl {}",
                            oid, source_id, sender
                        );
                        let _r = self.inventory.write().await.remove_item(oid);
                    }
                }
            }
            // append new and modified items, for non-modified - update state only
            for remote in remote_inv.into_values() {
                if source.is_destroyed() {
                    break;
                }
                if let Some(ex) = existing.get(&remote.oid) {
                    if ex.source().is_none() {
                        warn!(
                            "attempt to modify local item {} from rpl by {}, ignored",
                            ex.oid(),
                            sender
                        );
                        continue;
                    }
                    if ex.meta() == remote.meta.as_ref() {
                        debug!("setting state for {} from rpl inv {}", remote.oid, sender);
                        if ex.enabled() != remote.enabled {
                            ex.set_enabled(remote.enabled);
                        }
                        if check_state(&remote) && remote.oid.kind() != ItemKind::Lmacro {
                            if let Some(st) = ex.state() {
                                if let Some(s_state) = {
                                    let mut state = st.lock();
                                    let rs: ReplicationState = match remote.try_into() {
                                        Ok(v) => v,
                                        Err(e) => {
                                            error!("unable to process remote item state: {}", e);
                                            continue;
                                        }
                                    };
                                    if &rs.ieid > state.ieid() {
                                        state.set_from_rs(rs);
                                        let ev: LocalStateEvent = (&*state).into();
                                        Some(RemoteStateEvent::from_local_state_event(
                                            ev,
                                            source.node(),
                                            online,
                                        ))
                                    } else {
                                        if !rs.ieid.is_phantom()
                                            && !state.ieid().is_phantom()
                                            && rs.ieid.boot_id() < state.ieid().boot_id()
                                        {
                                            warn!("fatal replication problem for {}, node {} boot_id went backward", ex.oid(), source_id);
                                        }
                                        None
                                    }
                                } {
                                    announce_remote_state(ex.oid(), &s_state, true, &rpc.client())
                                        .await
                                        .log_ef();
                                }
                            }
                        }
                        continue;
                    }
                };
                debug!(
                    "creating remote item {}, node: {} from rpl {}",
                    remote.oid, source_id, sender
                );
                if let Ok(item) = self
                    .inventory
                    .write()
                    .await
                    .append_remote_item(remote, source.clone())
                    .log_err()
                {
                    if item.oid().kind() != ItemKind::Lmacro {
                        if let Some(s_state) = item.local_state_event() {
                            announce_remote_state(
                                item.oid(),
                                &RemoteStateEvent::from_local_state_event(
                                    s_state,
                                    source.node(),
                                    online,
                                ),
                                true,
                                &rpc.client(),
                            )
                            .await
                            .log_ef();
                        } else {
                            warn!("no state property in {}", item.oid());
                        }
                    }
                }
            }
        } else {
            // no source yet, add all items
            for remote in remote_inv.into_values() {
                if source.is_destroyed() {
                    break;
                }
                debug!(
                    "creating remote item {}, node: {} from rpl {}",
                    remote.oid, source_id, sender
                );
                if check_state(&remote) {
                    if let Ok(item) = self
                        .inventory
                        .write()
                        .await
                        .append_remote_item(remote, source.clone())
                        .log_err()
                    {
                        if item.oid().kind() != ItemKind::Lmacro {
                            if let Some(s_state) = item.local_state_event() {
                                announce_remote_state(
                                    item.oid(),
                                    &RemoteStateEvent::from_local_state_event(
                                        s_state,
                                        source.node(),
                                        online,
                                    ),
                                    true,
                                    &rpc.client(),
                                )
                                .await
                                .log_ef();
                            } else {
                                warn!("no state property in {}", item.oid());
                            }
                        }
                    }
                }
            }
        }
        debug!("source inventory processed: {}", source_id);
        if !quick {
            self.unlock_source(source_id);
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub async fn update_state_from_repl(
        &self,
        oid: &OID,
        rsex: ReplicationStateEventExtended,
        sender: &str,
    ) -> EResult<()> {
        let rse = match rsex {
            ReplicationStateEventExtended::Basic(rse) => rse,
            ReplicationStateEventExtended::Inventory(i) => {
                let mut map = HashMap::new();
                map.insert(oid.clone(), i.item);
                return self
                    .process_remote_inventory(map, &i.node, sender, true)
                    .await;
            }
        };
        let tp = oid.kind();
        match tp {
            ItemKind::Lmacro => return Err(Error::not_implemented(ERR_MSG_STATE_LMACRO)),
            ItemKind::Lvar | ItemKind::Sensor => {
                if rse.act.is_some() {
                    return Err(Error::invalid_data(format!(
                        "invalid state payload for {}",
                        oid
                    )));
                }
            }
            ItemKind::Unit => {}
        }
        let maybe_item = self.inventory.read().await.get_item(oid);
        let item = if rse.force_accept {
            if let Some(item) = maybe_item {
                item
            } else {
                let mut inventory = self.inventory.write().await;
                let source = inventory.get_or_create_source(&rse.node, sender);
                inventory.append_remote_item(
                    ReplicationInventoryItem {
                        oid: oid.clone(),
                        act: rse.act,
                        enabled: true,
                        ieid: Some(IEID::new(0, 0)),
                        meta: None,
                        status: None,
                        value: ValueOptionOwned::No,
                        t: Some(0.),
                    },
                    source,
                )?
            }
        } else {
            maybe_item.ok_or_else(|| Error::not_found(oid))?
        };
        if let Some(state) = item.state() {
            debug!("setting state from repl event for {}, from {}", oid, sender);
            if let Some(source) = item.source() {
                if rse.node != source.node() {
                    return Err(Error::busy(format!(
                        "unable to set item state {} from rpl event from {}: node differs",
                        oid, sender
                    )));
                }
                let (s_state, current) = {
                    let mut state = state.lock();
                    if &rse.ieid > state.ieid() {
                        state.set_from_rs(rse.into());
                        let ev = RemoteStateEvent::from_local_state_event(
                            (&*state).into(),
                            source.node(),
                            source.online(),
                        );
                        (ev, true)
                    } else {
                        let ev: RemoteStateEvent = rse.into();
                        (ev, false)
                    }
                };
                let rpc = self.rpc.get().unwrap();
                announce_remote_state(oid, &s_state, current, &rpc.client())
                    .await
                    .log_ef();
            } else {
                warn!("attempting to update local item from repl {}", oid);
            }
        } else {
            warn!("no state property in {}", oid);
        }
        Ok(())
    }
    #[inline]
    fn schedule_save(&self, oid: &OID) {
        self.scheduled_saves.lock().insert(oid.clone());
    }
    #[inline]
    fn unschedule_save(&self, oid: &OID) {
        self.scheduled_saves.lock().remove(oid);
    }
    /// # Panics
    ///
    /// Will panic if the core rpc is not set or the mutex is poisoned
    pub async fn set_local_items_enabled(&self, mask: &OIDMask, value: bool) -> EResult<()> {
        let configs: Vec<(OID, Value)> = self
            .inventory
            .read()
            .await
            .get_items_by_mask(mask, &Filter::default(), true)
            .iter()
            .filter(|item| item.source().is_none())
            .filter_map(|item| {
                item.set_enabled(value);
                if let Ok(config) = item.config() {
                    Some((item.oid().clone(), config))
                } else {
                    error!("unable to serialize config for {}", item.oid());
                    None
                }
            })
            .collect();
        let rpc = self.rpc.get().unwrap();
        for (oid, config) in configs {
            save_item_config(&oid, config, rpc).await.log_ef();
        }
        Ok(())
    }
    /// Announces states for all local items, called during startup
    ///
    /// # Panics
    ///
    /// Will panic if the core rpc is not set or the mutex is poisoned
    pub async fn announce_local_initial(&self) {
        let mut initial_announced = self.initial_announced.lock().await;
        let rpc = self.rpc.get().unwrap();
        info!("announcing local states");
        let inventory = self.inventory.read().await;
        let items = inventory.list_local_items();
        for item in items {
            if !self.is_active() {
                break;
            }
            if let Some(s_state) = item.local_state_event() {
                announce_local_state(item.oid(), &s_state, &rpc.client())
                    .await
                    .log_ef();
            }
        }
        *initial_announced = true;
    }
    /// # Panics
    ///
    /// Will panic if RPC is not set
    pub async fn force_announce_state<'a>(
        &self,
        mask_list: &OIDMaskList,
        source_id: Option<NodeFilter<'a>>,
    ) -> EResult<()> {
        // skip if initial announce hasn't been done yet
        if !*self.initial_announced.lock().await {
            return Ok(());
        }
        let rpc = self.rpc.get().unwrap();
        let inventory = self.inventory.read().await;
        for item in inventory.list_items_with_states(mask_list, source_id) {
            if let Some(state) = item.local_state_event() {
                if let Some(src) = item.source() {
                    announce_remote_state(
                        item.oid(),
                        &RemoteStateEvent::from_local_state_event(state, src.node(), src.online()),
                        true,
                        &rpc.client(),
                    )
                    .await?;
                } else {
                    announce_local_state(item.oid(), &state, &rpc.client()).await?;
                }
            }
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if RPC is not set
    pub async fn force_announce_state_for<'a>(
        &self,
        mask_list: &OIDMaskList,
        source_id: Option<NodeFilter<'a>>,
        receiver: &str,
    ) -> EResult<()> {
        // skip if initial announce hasn't been done yet
        if !*self.initial_announced.lock().await {
            return Ok(());
        }
        let rpc = self.rpc.get().unwrap();
        let inventory = self.inventory.read().await;
        for item in inventory.list_items_with_states(mask_list, source_id) {
            if let Some(state) = item.local_state_event() {
                if let Some(src) = item.source() {
                    announce_remote_state_for(
                        item.oid(),
                        &RemoteStateEvent::from_local_state_event(state, src.node(), src.online()),
                        true,
                        receiver,
                        &rpc.client(),
                    )
                    .await?;
                } else {
                    announce_local_state_for(item.oid(), &state, receiver, &rpc.client()).await?;
                }
            }
        }
        Ok(())
    }
    #[inline]
    pub async fn list_items<'a>(
        &self,
        mask_list: &OIDMaskList,
        include: Option<&OIDMaskList>,
        exclude: Option<&OIDMaskList>,
        source_id: Option<NodeFilter<'a>>,
        include_stateless: bool,
    ) -> Vec<Item> {
        let mut filter = Filter::default();
        if let Some(v) = include {
            filter.set_include(v);
        }
        if let Some(v) = exclude {
            filter.set_exclude(v);
        }
        if let Some(v) = source_id {
            filter.set_node(v);
        }
        #[allow(clippy::mutable_key_type)]
        let mut h: HashSet<Item> = HashSet::default();
        for mask in mask_list.oid_masks() {
            let items =
                self.inventory
                    .read()
                    .await
                    .get_items_by_mask(mask, &filter, include_stateless);
            for item in items {
                h.insert(item);
            }
        }
        let mut result: Vec<Item> = h.into_iter().collect();
        result.sort();
        result
    }
    #[inline]
    fn update_paths(&mut self, dir_eva: &str, pid_file: Option<&str>) {
        dir_eva.clone_into(&mut self.dir_eva);
        self.pid_file = format_path(dir_eva, pid_file, Some("var/eva.pid"));
    }
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.load(atomic::Ordering::Relaxed)
    }
    pub fn set_rpc(&self, rpc: Arc<RpcClient>) -> EResult<()> {
        self.rpc
            .set(rpc.clone())
            .map_err(|_| Error::core("unable to set RPC"))?;
        self.action_manager.set_rpc(rpc.clone())?;
        self.service_manager.set_rpc(rpc.clone())?;
        crate::logs::set_rpc(rpc)?;
        Ok(())
    }
    pub fn set_components(&self) -> EResult<()> {
        self.service_manager
            .set_core_active_beacon(self.active.clone())?;
        Ok(())
    }
    pub fn log_summary(&self) {
        debug!("core.boot_id = {}", self.boot_id);
        debug!("core.dir_eva = {}", self.dir_eva);
        debug!("core.system_name = {}", self.system_name);
        debug!("core.instant_save = {:?}", self.instant_save);
        debug!("core.pid_file = {}", self.pid_file);
        debug!("core.suicide_timeout = {:?}", self.suicide_timeout);
        debug!("core.timeout = {:?}", self.timeout);
        debug!("core.workers = {}", self.workers);
    }
    #[allow(clippy::cast_sign_loss)]
    #[inline]
    pub fn set_boot_id(&mut self, db: &mut yedb::Database) -> EResult<()> {
        self.boot_id = db.key_increment(&registry::format_data_key("boot-id"))? as u64;
        Ok(())
    }
    #[inline]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
    #[inline]
    pub fn service_manager(&self) -> &svcmgr::Manager {
        &self.service_manager
    }
    #[inline]
    pub fn action_manager(&self) -> &actmgr::Manager {
        &self.action_manager
    }
    #[inline]
    pub fn system_name(&self) -> &str {
        &self.system_name
    }
    #[inline]
    pub fn dir_eva(&self) -> &str {
        &self.dir_eva
    }
    pub fn inventory(&self) -> &RwLock<Inventory> {
        &self.inventory
    }
    #[inline]
    pub fn workers(&self) -> u32 {
        self.workers
    }
    pub async fn write_pid_file(&self) -> EResult<()> {
        tokio::fs::write(&self.pid_file, self.pid.to_string())
            .await
            .map_err(Into::into)
    }
    pub fn register_signals(&self) {
        let mut handle_cc = match std::env::var_os("EVA_ENABLE_CC") {
            Some(v) => v == "1",
            None => false,
        };
        // always handle cc if run with cargo
        if let Some(v) = std::env::var_os("CARGO_PKG_NAME") {
            if !v.is_empty() {
                handle_cc = true;
                trace!("running under cargo");
            }
        };
        if handle_cc {
            let running = self.running.clone();
            handle_term_signal!(
                SignalKind::interrupt(),
                running,
                atty::is(Stream::Stdout) && atty::is(Stream::Stderr)
            );
        } else {
            ignore_term_signal!(SignalKind::interrupt());
        }
        let running = self.running.clone();
        handle_term_signal!(SignalKind::terminate(), running, true);
    }
    #[inline]
    pub fn add_file_to_remove(&mut self, fname: &str) {
        self.files_to_remove.push(fname.to_owned());
    }
    async fn announce_core_state(
        &self,
        state: &eva_common::services::ServiceStatusBroadcastEvent,
    ) -> EResult<()> {
        if self.mode == Mode::Regular {
            if let Some(rpc) = self.rpc.get() {
                rpc.client()
                    .lock()
                    .await
                    .publish(SERVICE_STATUS_TOPIC, pack(state)?.into(), QoS::No)
                    .await?;
            }
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if RPC is not set
    #[inline]
    pub async fn start(&self, queue_size: usize) -> EResult<()> {
        let (tx, rx) = async_channel::bounded(queue_size);
        self.state_db_tx
            .set(tx)
            .map_err(|_| Error::core("Unable to set db tx"))?;
        let rpc = self.rpc.get().unwrap().clone();
        tokio::spawn(async move {
            handle_save(rx, &rpc).await;
        });
        spawn_mem_checker(
            self.mem_warn
                .unwrap_or(crate::MEMORY_WARN_DEFAULT * u64::from(self.workers)),
        );
        self.action_manager.start().await
    }
    #[inline]
    pub async fn mark_loaded(&self) {
        debug!("marking the core loaded");
        self.running.store(true, atomic::Ordering::Relaxed);
        self.active.store(true, atomic::Ordering::Relaxed);
        self.announce_core_state(&eva_common::services::ServiceStatusBroadcastEvent::ready())
            .await
            .log_ef();
        self.announce_startup().await;
    }
    pub async fn set_reload_flag(&self) -> EResult<()> {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&format!("{}/var/eva.reload", self.dir_eva))
            .await?;
        let _r = f.write(&[0x00]).await?;
        Ok(())
    }
    pub async fn publish_bus_messge(&self, msg: BusMessage) -> EResult<()> {
        let rpc = self
            .rpc
            .get()
            .ok_or_else(|| Error::not_ready("core RPC not ready"))?;
        rpc.client()
            .lock()
            .await
            .publish(&msg.topic, pack(&msg.message)?.into(), QoS::Processed)
            .await?;
        Ok(())
    }
    #[inline]
    pub fn shutdown(&self) {
        self.running.store(false, atomic::Ordering::Relaxed);
    }
    /// # Panics
    ///
    /// will panic if any mutex is poisoned
    pub async fn list_spoints(&self) -> EResult<Vec<spoint::Info>> {
        spoint::list_remote(self.rpc.get().unwrap().clone(), self.timeout()).await
    }
    /// # Panics
    ///
    /// will panic if any mutex is poisoned
    pub async fn block(&self, full: bool) {
        trace!("blocking until terminated");
        info!("{} ready ({} mode)", self.system_name, self.mode);
        while self.running.load(atomic::Ordering::Relaxed) {
            sleep(SLEEP_STEP).await;
        }
        self.announce_terminating().await;
        self.active.store(false, atomic::Ordering::Relaxed);
        if let Some(fut) = self.node_checker_fut.lock().unwrap().as_ref() {
            fut.abort();
        }
        bmart::process::suicide(self.suicide_timeout, false);
        if full {
            crate::seq::shutdown().await;
            let _r = self
                .announce_core_state(
                    &eva_common::services::ServiceStatusBroadcastEvent::terminating(),
                )
                .await;
            self.save().await.log_ef_with("save");
            self.service_manager
                .stop(&self.system_name, self.timeout, false)
                .await;
            bmart::process::kill_pstree(
                std::process::id(),
                Some(Duration::from_millis(100)),
                false,
            )
            .await;
        }
        for f in &self.files_to_remove {
            let _r = tokio::fs::remove_file(f).await;
        }
        if let Some(tx) = self.state_db_tx.get() {
            while !tx.is_empty() {
                tokio::time::sleep(SLEEP_STEP).await;
            }
            // give 100ms more to make sure states are saved
            tokio::time::sleep(SLEEP_STEP).await;
        }
        inventory_db::shutdown().await;
        let _r = tokio::fs::remove_file(&self.pid_file).await;
        info!("the core shutted down");
    }
    pub fn dobj_list(&self) -> Vec<(String, usize)> {
        let map = self.object_map.lock();
        let mut result = Vec::with_capacity(map.objects.len());
        for n in map.objects.keys() {
            result.push((n.to_string(), map.size_of(n).unwrap_or_default()));
        }
        result
    }
    pub fn dobj_get(&self, name: &str) -> Option<DataObject> {
        self.object_map.lock().objects.get(name).cloned()
    }
    pub fn dobj_validate(&self) -> EResult<()> {
        self.object_map.lock().validate()
    }
    /// # Panics
    ///
    /// Will panic if registry is not set
    pub async fn dobj_insert(&self, dobj: Vec<DataObject>) -> EResult<()> {
        self.object_map.lock().extend(dobj.clone());
        for o in dobj {
            let name = o.name.clone();
            registry::key_set(registry::R_DATA_OBJECT, &name, o, self.rpc.get().unwrap()).await?;
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if registry is not set
    pub async fn dobj_remove(&self, names: &[&str]) -> EResult<()> {
        self.object_map.lock().remove_bulk(names);
        for n in names {
            registry::key_delete(registry::R_DATA_OBJECT, n, self.rpc.get().unwrap()).await?;
        }
        Ok(())
    }
    pub fn load_dobj(&self, db: &mut yedb::Database) -> EResult<()> {
        info!("loading data objects");
        let s_key = registry::format_top_key(registry::R_DATA_OBJECT);
        let s_offs = s_key.len() + 1;
        let mut objects = Vec::new();
        for (n, v) in db.key_get_recursive(&s_key)? {
            let name = &n[s_offs..];
            debug!("loading dobj {}", name);
            let dobj: DataObject = serde_json::from_value(v)?;
            objects.push(dobj);
        }
        let mut map = self.object_map.lock();
        map.extend(objects);
        if let Err(e) = map.validate() {
            error!("invalid data objects found: {}", e);
        }
        Ok(())
    }
    pub async fn dobj_push(
        &self,
        name: String,
        buf: Vec<u8>,
        endianess: Endianess,
        sender: &str,
    ) -> EResult<()> {
        let vals = self
            .object_map
            .lock()
            .parse_values(&name.try_into()?, &buf, endianess)?;
        let raw: Vec<RawStateBulkEventOwned> = vals
            .into_iter()
            .map(|(oid, value)| RawStateBulkEventOwned::new(oid, RawStateEventOwned::new(1, value)))
            .collect();
        self.update_state_from_raw_bulk(raw, sender).await;
        Ok(())
    }
    pub async fn dobj_error(
        &self,
        name: String,
        status: Option<ItemStatus>,
        sender: &str,
    ) -> EResult<()> {
        let status = status.unwrap_or(ITEM_STATUS_ERROR);
        if status >= 0 {
            return Err(Error::invalid_params(
                "error status must be a negative integer",
            ));
        }
        let oids = self.object_map.lock().mapped_oids(&name.try_into()?);
        let raw: Vec<RawStateBulkEventOwned> = oids
            .into_iter()
            .map(|oid| RawStateBulkEventOwned::new(oid, RawStateEventOwned::new0(status)))
            .collect();
        self.update_state_from_raw_bulk(raw, sender).await;
        Ok(())
    }
}

pub async fn init_core_client<C>(client: &mut C) -> EResult<()>
where
    C: AsyncClient,
{
    let lvl = crate::logs::get_min_log_level();
    let mut topics: Vec<String> = vec![
        RAW_STATE_BULK_TOPIC.to_owned(),
        format!("{RAW_STATE_TOPIC}#"),
        format!("{}#", eva_common::actions::ACTION_TOPIC),
        format!("{REPLICATION_STATE_TOPIC}#"),
        format!("{REPLICATION_INVENTORY_TOPIC}#"),
        format!("{REPLICATION_NODE_STATE_TOPIC}#"),
        format!("{LOG_INPUT_TOPIC}error"),
    ];
    if lvl.0 == eva_common::LOG_LEVEL_TRACE {
        topics.push(format!("{LOG_INPUT_TOPIC}trace"));
    }
    if lvl.0 <= eva_common::LOG_LEVEL_DEBUG {
        topics.push(format!("{LOG_INPUT_TOPIC}debug"));
    }
    if lvl.0 <= eva_common::LOG_LEVEL_INFO {
        topics.push(format!("{LOG_INPUT_TOPIC}info"));
    }
    if lvl.0 <= eva_common::LOG_LEVEL_WARN {
        topics.push(format!("{LOG_INPUT_TOPIC}warn"));
    }
    client
        .subscribe_bulk(
            &topics.iter().map(|item| &**item).collect::<Vec<&str>>(),
            QoS::No,
        )
        .await?;
    Ok(())
}

pub fn get_hostname() -> EResult<String> {
    Ok(hostname::get()
        .map_err(|e| Error::failed(format!("unable to get host name: {e}")))?
        .to_string_lossy()
        .to_string())
}

fn spawn_mem_checker(mem_warn: u64) {
    let mut int = tokio::time::interval(crate::SYSINFO_CHECK_INTERVAL);
    tokio::spawn(async move {
        let pid = sysinfo::Pid::from(std::process::id() as usize);
        loop {
            int.tick().await;
            // safe lock
            let system = loop {
                let Some(system) = crate::SYSTEM_INFO.try_lock() else {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    continue;
                };
                break system;
            };
            let Some(total_memory) = system.process(pid).map(sysinfo::Process::memory) else {
                continue;
            };
            drop(system);
            crate::check_memory_usage("core process", total_memory, mem_warn);
        }
    });
}

fn calc_delta(current: f64, prev: f64, on_negative: events::OnNegativeDelta) -> f64 {
    if let events::OnNegativeDelta::Overflow { floor, ceil } = on_negative {
        if current < prev {
            return (ceil - prev) + (current - floor);
        }
    }
    // other negative variants are processed later
    current - prev
}
