use eva_common::events::{
    NodeInfo, NodeStateEvent, NodeStatus, RAW_STATE_TOPIC, REPLICATION_NODE_STATE_TOPIC,
    RawStateEventOwned, ReplicationInventoryItem,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::aaa;

err_logger!();

const STATE_CHECKER_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Deserialize, Serialize, Clone)]
pub struct PullData {
    pub info: NodeInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ReplicationInventoryItem>>,
}

pub static NODES: LazyLock<RwLock<HashMap<String, Node>>> = LazyLock::new(<_>::default);
pub static RELOAD_TRIGGERS: LazyLock<RwLock<HashMap<String, async_channel::Sender<bool>>>> =
    LazyLock::new(<_>::default);

#[derive(Serialize)]
struct NodeStateLvar {
    oid: OID,
}

pub async fn deploy_undeploy_node_state_sensors(name: &str, deploy: bool) -> EResult<()> {
    let Some(lvar_prefix) = crate::STATE_LVAR.get() else {
        return Ok(());
    };
    let inv = vec![NodeStateLvar {
        oid: format!("{}/{}/connected", lvar_prefix, name).parse()?,
    }];
    if deploy {
        eapi_bus::deploy_items(&inv).await?;
    } else {
        eapi_bus::undeploy_items(&inv).await?;
    }
    Ok(())
}

pub async fn publish_node_state(name: &str, svc_ok: bool, online: bool) -> EResult<()> {
    let Some(lvar_prefix) = crate::STATE_LVAR.get() else {
        return Ok(());
    };
    let topic = format!(
        "{}{}/{}/connected",
        RAW_STATE_TOPIC,
        lvar_prefix.as_path(),
        name
    );
    let ev = RawStateEventOwned::new(if svc_ok { 1 } else { -1 }, Value::U8(online.into()));
    eapi_bus::publish(&topic, pack(&ev)?.into()).await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn reload_node(
    name: &str,
    key_id: &str,
    compress: bool,
    timeout: Duration,
    trusted: bool,
) -> EResult<()> {
    debug!("reloading node {}", name);
    let ps_rpc = crate::PUBSUB_RPC.get().unwrap();
    let mut opts = aaa::get_enc_opts(key_id).await?;
    opts = opts.compression(if compress {
        psrpc::options::Compression::Bzip2
    } else {
        psrpc::options::Compression::No
    });
    let mut res = PullData::deserialize(
        tokio::time::timeout(timeout, ps_rpc.call(name, "pull", None, &opts)).await??,
    )?;
    if !trusted {
        let acl = aaa::get_acl(key_id).await?;
        if let Some(items) = res.items.take() {
            let mut allowed_items = Vec::with_capacity(items.len());
            for item in items {
                if acl.check_item_write(&item.oid) {
                    allowed_items.push(item);
                } else {
                    warn!("node {} is not allowed to replicate {}", name, item.oid);
                }
            }
            res.items.replace(allowed_items);
        }
    }
    {
        let mut nodes = NODES.write().await;
        let node = nodes
            .get_mut(name)
            .ok_or_else(|| Error::core("failed to reload node: no such object"))?;
        node.last_reload = Some(Instant::now());
        node.info.replace(res.info.clone());
        if let Some(ref items) = res.items
            && crate::SUBSCRIBE_EACH.load(atomic::Ordering::Relaxed)
        {
            let mut oids: HashSet<&OID> = HashSet::new();
            for item in items {
                oids.insert(&item.oid);
            }
            let mut to_subscribe: Vec<String> = Vec::new();
            let mut to_unsubscribe: Vec<String> = Vec::new();
            let mut to_remove: HashSet<OID> = HashSet::new();
            for oid in &node.oids {
                if !oids.contains(oid) {
                    to_remove.insert(oid.clone());
                    to_unsubscribe.push(format!("{}{}", crate::PS_ITEM_STATE_TOPIC, oid.as_path()));
                }
            }
            node.oids.retain(|v| !to_remove.contains(v));
            for oid in oids {
                if !node.oids.contains(oid) {
                    to_subscribe.push(format!("{}{}", crate::PS_ITEM_STATE_TOPIC, oid.as_path()));
                    node.oids.insert(oid.clone());
                }
            }
            let ps_rpc = crate::PUBSUB_RPC.get().unwrap();
            let ps_client = ps_rpc.client();
            let qos = ps_rpc.qos();
            trace!("new topics {:?}", to_subscribe);
            trace!("removed {:?}", to_unsubscribe);
            if !to_subscribe.is_empty() {
                ps_client
                    .subscribe_bulk(
                        to_subscribe
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<&str>>()
                            .as_slice(),
                        qos,
                    )
                    .await?;
            }
            if !to_unsubscribe.is_empty() {
                ps_client
                    .unsubscribe_bulk(
                        to_unsubscribe
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<&str>>()
                            .as_slice(),
                    )
                    .await?;
            }
        }
    }
    eapi_bus::client()
        .lock()
        .await
        .publish(
            &format!("{}{}", crate::REPLICATION_INVENTORY_TOPIC, name),
            pack(&res.items)?.into(),
            QoS::Processed,
        )
        .await?;
    mark_node(name, true, Some(res.info), false, Some(timeout)).await?;
    Ok(())
}

async fn ping_node(name: &str, key_id: &str, compress: bool, timeout: Duration) -> EResult<()> {
    trace!("pinging node {}", name);
    let ps_rpc = crate::PUBSUB_RPC.get().unwrap();
    let mut opts = aaa::get_enc_opts(key_id).await?;
    opts = opts.compression(if compress {
        psrpc::options::Compression::Bzip2
    } else {
        psrpc::options::Compression::No
    });
    tokio::time::timeout(timeout, ps_rpc.call(name, "ping", None, &opts)).await??;
    Ok(())
}

async fn reloader(
    name: &str,
    rx: async_channel::Receiver<bool>,
    ready: triggered::Listener,
    trusted: bool,
) -> EResult<()> {
    ready.await;
    debug!("node reloader started for {}", name);
    let (reload_interval, compress, key_id, node_timeout, reloader_active) =
        if let Some(node) = NODES.read().await.get(name) {
            (
                node.reload_interval,
                node.compress,
                node.key_id.clone(),
                node.timeout,
                node.reloader_active.clone(),
            )
        } else {
            return Err(Error::core("reloader failed to start: no such node object"));
        };
    let mut int = tokio::time::interval(reload_interval);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let mut force: bool = false;
        let from_trig = tokio::select! {
            res = rx.recv() => if let Ok(r) = res {
                force = r;
                while let Ok(v) = rx.try_recv() {
                    if v {
                        force = true;
                    }
                }
                true
            } else {
                break;
            },
            _r = int.tick() => false
        };
        let last_reload = NODES
            .read()
            .await
            .get(name)
            .ok_or_else(|| Error::core("failed to get node data: no such object"))?
            .last_reload;
        if !force
            && let Some(last) = last_reload
            && last.elapsed() > reload_interval / 2
        {
            let lvl = if from_trig {
                log::Level::Warn
            } else {
                log::Level::Debug
            };
            log::log!(lvl, "{} reload triggered too often, skipping", name);
        }
        reloader_active.store(true, atomic::Ordering::SeqCst);
        if let Err(e) = reload_node(name, &key_id, compress, node_timeout, trusted).await {
            mark_node(name, false, None, false, None).await?;
            error!("failed to reload the node {}: {}", name, e);
        }
        reloader_active.store(false, atomic::Ordering::SeqCst);
    }
    debug!("node reloader stopped for {}", name);
    Ok(())
}

async fn pinger(name: &str, ready: triggered::Listener) -> EResult<()> {
    ready.await;
    debug!("node pinger started for {}", name);
    let (ping_interval, compress, key_id, node_timeout, online_beacon, reloader_active) =
        if let Some(node) = NODES.read().await.get(name) {
            (
                node.ping_interval,
                node.compress,
                node.key_id.clone(),
                node.timeout,
                node.online.clone(),
                node.reloader_active.clone(),
            )
        } else {
            return Err(Error::core("pinger failed to start: no such node object"));
        };
    let mut int = tokio::time::interval(ping_interval);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        int.tick().await;
        if online_beacon.load(atomic::Ordering::SeqCst)
            && !reloader_active.load(atomic::Ordering::SeqCst)
            && let Err(e) = ping_node(name, &key_id, compress, node_timeout).await
        {
            mark_node(name, false, None, false, None).await?;
            error!("failed to ping the node {}: {}", name, e);
        }
    }
}

#[derive(Deserialize, Serialize, bmart::tools::Sorting)]
#[serde(deny_unknown_fields)]
#[sorting(id = "name")]
#[allow(clippy::struct_excessive_bools)]
pub struct Node {
    name: String,
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration",
        serialize_with = "eva_common::tools::serialize_duration_as_f64"
    )]
    ping_interval: Duration,
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration",
        serialize_with = "eva_common::tools::serialize_duration_as_f64"
    )]
    reload_interval: Duration,
    compress: bool,
    pub(crate) enabled: bool,
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration",
        serialize_with = "eva_common::tools::serialize_duration_as_f64"
    )]
    timeout: Duration,
    #[serde(default = "eva_common::tools::default_true")]
    trusted: bool,
    #[serde(skip)]
    sttc: bool,
    #[serde(skip)]
    online: Arc<atomic::AtomicBool>,
    #[serde(skip)]
    link_uptime: Option<Instant>,
    #[serde(skip)]
    reloader_active: Arc<atomic::AtomicBool>,
    #[serde(skip)]
    last_reload: Option<Instant>,
    #[serde(skip)]
    oids: HashSet<OID>,
    key_id: String,
    admin_key_id: Option<String>,
    #[serde(skip)]
    info: Option<NodeInfo>,
    #[serde(skip)]
    reloader_fut: Option<JoinHandle<()>>,
    #[serde(skip)]
    pinger_fut: Option<JoinHandle<()>>,
    #[serde(default = "eva_common::tools::default_true")]
    pub(crate) api_enabled: bool,
    #[serde(skip)]
    last_event: atomic::AtomicU64,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize, bmart::tools::Sorting)]
#[sorting(id = "name")]
pub struct NodeI<'a> {
    name: &'a str,
    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
    timeout: Duration,
    compress: bool,
    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
    ping_interval: Duration,
    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
    reload_interval: Duration,
    #[serde(rename = "static")]
    sttc: bool,
    enabled: bool,
    api_enabled: bool,
    managed: bool,
    trusted: bool,
    online: bool,
    #[serde(serialize_with = "eva_common::tools::serialize_opt_duration_as_f64")]
    link_uptime: Option<Duration>,
    version: Option<&'a str>,
    build: Option<u64>,
}

impl Node {
    pub fn new(name: &str, sttc: bool, trusted: bool, api_enabled: bool) -> Self {
        Self {
            name: name.to_owned(),
            ping_interval: crate::DEFAULT_PING_INTERVAL,
            reload_interval: crate::DEFAULT_RELOAD_INTERVAL,
            compress: false,
            enabled: true,
            timeout: *crate::TIMEOUT.get().unwrap(),
            trusted,
            sttc,
            online: Arc::new(atomic::AtomicBool::new(false)),
            link_uptime: None,
            reloader_active: Arc::new(atomic::AtomicBool::new(false)),
            last_reload: None,
            oids: HashSet::new(),
            key_id: crate::DEFAULT_KEY_ID.get().unwrap().clone(),
            admin_key_id: None,
            info: None,
            reloader_fut: None,
            pinger_fut: None,
            api_enabled,
            last_event: <_>::default(),
        }
    }
    fn stop_tasks(&self) {
        if let Some(ref fut) = self.reloader_fut {
            fut.abort();
        }
        if let Some(ref fut) = self.pinger_fut {
            fut.abort();
        }
    }
    pub fn info(&self) -> NodeI<'_> {
        NodeI {
            name: &self.name,
            timeout: self.timeout,
            compress: self.compress,
            ping_interval: self.ping_interval,
            reload_interval: self.reload_interval,
            trusted: self.trusted,
            sttc: self.sttc,
            enabled: self.enabled,
            api_enabled: self.api_enabled,
            managed: self.admin_key_id.is_some(),
            online: self.online(),
            link_uptime: self.link_uptime.as_ref().map(Instant::elapsed),
            version: self.info.as_ref().map(|i| i.version.as_str()),
            build: self.info.as_ref().map(|i| i.build),
        }
    }
    /// used by nodes with API disabled only
    #[inline]
    pub fn update_last_event(&self) {
        let now = eva_common::time::monotonic_ns();
        self.last_event.store(now, atomic::Ordering::Relaxed);
    }
    #[inline]
    pub fn online(&self) -> bool {
        self.online.load(atomic::Ordering::Relaxed)
    }
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[inline]
    pub fn set_static(&mut self) {
        self.sttc = true;
    }
    #[inline]
    pub fn is_static(&self) -> bool {
        self.sttc
    }
    #[inline]
    pub fn compress(&self) -> bool {
        self.compress
    }
    #[inline]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
    #[inline]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    #[inline]
    pub fn admin_key_id(&self) -> Option<&str> {
        self.admin_key_id.as_deref()
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop_tasks();
    }
}

async fn append_node(mut node: Node, nodes: &mut HashMap<String, Node>) -> EResult<()> {
    debug!("appending node {}", node.name);
    let (tx, trig, p_trig) = if node.enabled && node.api_enabled {
        let (trig, ready) = triggered::trigger();
        let (tx, rx) = async_channel::bounded::<bool>(1024);
        let name = node.name.clone();
        let reloader_fut = tokio::spawn(async move {
            reloader(&name, rx, ready, node.trusted).await.log_ef();
        });
        let (p_trig, p_ready) = triggered::trigger();
        let name = node.name.clone();
        let pinger_fut = tokio::spawn(async move {
            pinger(&name, p_ready).await.log_ef();
        });
        deploy_undeploy_node_state_sensors(&node.name, true).await?;
        node.reloader_fut.replace(reloader_fut);
        node.pinger_fut.replace(pinger_fut);
        (Some(tx), Some(trig), Some(p_trig))
    } else {
        eapi_bus::publish(
            &format!("{}{}", REPLICATION_NODE_STATE_TOPIC, node.name),
            pack(&NodeStateEvent {
                status: NodeStatus::Removed,
                info: None,
                timeout: None,
            })?
            .into(),
        )
        .await?;
        deploy_undeploy_node_state_sensors(&node.name, false).await?;
        (None, None, None)
    };
    let name = node.name.clone();
    nodes.insert(name.clone(), node);
    if let Some(txch) = tx {
        RELOAD_TRIGGERS.write().await.insert(name.clone(), txch);
    }
    eapi_bus::publish(
        &format!("{}{}", REPLICATION_NODE_STATE_TOPIC, name),
        pack(&NodeStateEvent {
            status: NodeStatus::Offline,
            info: None,
            timeout: None,
        })?
        .into(),
    )
    .await?;
    if let Some(t) = trig {
        t.trigger();
    }
    if let Some(t) = p_trig {
        t.trigger();
    }
    Ok(())
}

pub async fn append_discovered_node(
    name: &str,
    info: Option<NodeInfo>,
    api_enabled: bool,
) -> EResult<()> {
    if name == crate::SYSTEM_NAME.get().unwrap() {
        return Ok(());
    }
    trace!("starting append for discovered node {}", name);
    let mut nodes = NODES.write().await;
    let mut recreate = false;
    if let Some(n) = nodes.get_mut(name) {
        if n.api_enabled == api_enabled {
            if crate::discovery_enabled()
                && let Some(node_info) = info
            {
                n.info.replace(node_info);
            }
            if n.enabled && n.api_enabled && !n.online() {
                trace!(
                    "node {} already exists and is offline, triggering reload",
                    name
                );
                if let Some(tx) = RELOAD_TRIGGERS.read().await.get(name) {
                    tx.send(false).await.log_ef();
                } else {
                    error!("core error: no reload trigger for {}", name);
                }
            } else {
                trace!("node {} already exists and is online, ignoring", name);
            }
            return Ok(());
        }
        recreate = true;
    }
    if crate::discovery_enabled() {
        if recreate {
            remove_node(name, &mut nodes).await?;
        }
        info!("appending discovered node: {}", name);
        let mut node = Node::new(name, false, true, api_enabled);
        node.info = info;
        append_node(node, &mut nodes).await?;
    }
    Ok(())
}

pub async fn append_static_node(node: Node, nodes: &mut HashMap<String, Node>) -> EResult<()> {
    if nodes.contains_key(&node.name) {
        remove_node(&node.name, nodes).await?;
    }
    info!("appending static node: {}", node.name);
    append_node(node, nodes).await?;
    Ok(())
}

pub async fn remove_node(name: &str, nodes: &mut HashMap<String, Node>) -> EResult<()> {
    let node = nodes.remove(name);
    if let Some(n) = node {
        info!("removing node: {}", name);
        RELOAD_TRIGGERS.write().await.remove(name);
        // stop manually to make sure futs are stopped
        n.stop_tasks();
        let topics: Vec<String> = n
            .oids
            .iter()
            .map(|oid| format!("{}{}", crate::PS_ITEM_STATE_TOPIC, oid.as_path()))
            .collect();
        if !topics.is_empty() {
            crate::PUBSUB_RPC
                .get()
                .unwrap()
                .client()
                .unsubscribe_bulk(
                    topics
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<&str>>()
                        .as_slice(),
                )
                .await?;
        }
    } else {
        return Err(Error::not_found(format!("no such node: {}", name)));
    }
    eapi_bus::publish(
        &format!("{}{}", REPLICATION_NODE_STATE_TOPIC, name),
        pack(&NodeStateEvent {
            status: NodeStatus::Removed,
            info: None,
            timeout: None,
        })?
        .into(),
    )
    .await?;
    deploy_undeploy_node_state_sensors(name, false).await?;
    Ok(())
}

pub async fn mark_node(
    name: &str,
    online: bool,
    info: Option<NodeInfo>,
    force: bool,
    timeout: Option<Duration>,
) -> EResult<()> {
    {
        let mut nodes = NODES.write().await;
        if let Some(node) = nodes.get_mut(name) {
            if node.online() != online {
                if node.sttc || online {
                    info!("marking node: {} online={}", name, online);
                    node.online.store(online, atomic::Ordering::SeqCst);
                    if online {
                        node.link_uptime.replace(Instant::now());
                    } else {
                        node.link_uptime.take();
                    }
                } else if !online {
                    return remove_node(name, &mut nodes).await;
                }
            }
        } else if !force {
            return Ok(());
        }
    };
    eapi_bus::publish(
        &format!("{}{}", REPLICATION_NODE_STATE_TOPIC, name),
        pack(&NodeStateEvent {
            status: if online {
                NodeStatus::Online
            } else {
                NodeStatus::Offline
            },
            info,
            timeout,
        })?
        .into(),
    )
    .await?;
    publish_node_state(name, true, online).await?;
    Ok(())
}

pub async fn state_checker() {
    let mut int = tokio::time::interval(STATE_CHECKER_INTERVAL);
    loop {
        int.tick().await;
        let mut nodes_set_offline = Vec::new();
        let now = eva_common::time::monotonic_ns();
        for (name, node) in &*NODES.read().await {
            if !node.api_enabled
                && node.online()
                && node.last_event.load(atomic::Ordering::Relaxed)
                    + u64::try_from(node.timeout.as_nanos()).unwrap()
                    < now
            {
                nodes_set_offline.push(name.clone());
            }
        }
        for node in nodes_set_offline {
            mark_node(&node, false, None, false, None).await.log_ef();
        }
    }
}
