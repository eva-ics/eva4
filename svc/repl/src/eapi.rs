use eva_common::acl::OIDMaskList;
use eva_common::common_payloads::ParamsId;
use eva_common::events::{
    AAA_ACL_TOPIC, AAA_KEY_TOPIC, FullItemStateAndInfoOwned, LOCAL_STATE_TOPIC, LocalStateEvent,
    REMOTE_STATE_TOPIC, RemoteStateEvent,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::FullItemState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, atomic};
use std::time::Duration;

use crate::nodes;
use crate::{ReplicationData, aaa};

err_logger!();

pub struct Handlers {
    tx: async_channel::Sender<FullItemState>,
    info: ServiceInfo,
    replicate_remote: bool,
}

impl Handlers {
    pub fn new(
        tx: async_channel::Sender<FullItemState>,
        info: ServiceInfo,
        replicate_remote: bool,
    ) -> Self {
        Self {
            tx,
            info,
            replicate_remote,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NodeOrId {
    Node(Box<nodes::Node>),
    Id(String),
}

impl NodeOrId {
    #[inline]
    pub fn name(&self) -> &str {
        match self {
            NodeOrId::Node(n) => n.name(),
            NodeOrId::Id(i) => i,
        }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        macro_rules! not_local {
            ($name: expr) => {
                if $name == crate::SYSTEM_NAME.get().unwrap() {
                    return Err(Error::failed(format!("{} is the local node", $name)).into());
                }
            };
        }
        match method {
            "action" | "action.toggle" | "run" | "lvar.set" | "lvar.reset" | "lvar.clear"
            | "lvar.toggle" | "lvar.incr" | "lvar.decr" | "action.result" | "rpvt" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let params = unpack::<BTreeMap<String, Value>>(payload)?;
                    let res = call_remote_method(method, params, false, false).await?;
                    Ok(Some(pack(&res)?))
                }
            }
            "node.list" => {
                if payload.is_empty() {
                    let nodes = nodes::NODES.read().await;
                    let mut result: Vec<nodes::NodeI> =
                        nodes.values().map(nodes::Node::info).collect();
                    result.sort();
                    Ok(Some(pack(&result)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "node.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    let nodes = nodes::NODES.read().await;
                    if let Some(node) = nodes.get(p.i) {
                        Ok(Some(pack(&node.info())?))
                    } else {
                        Err(Error::not_found("node not found").into())
                    }
                }
            }
            "node.append" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsAppend {
                    i: String,
                    #[serde(default = "eva_common::tools::default_true")]
                    trusted: bool,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsAppend = unpack(payload)?;
                    not_local!(&p.i);
                    let node = nodes::Node::new(&p.i, true, p.trusted, true);
                    let nval = to_value(&node)?;
                    nodes::append_static_node(node, &mut *nodes::NODES.write().await).await?;
                    eapi_bus::registry()
                        .key_set(&format!("node/{}", p.i), nval)
                        .await?;
                    Ok(None)
                }
            }
            "node.deploy" => {
                #[derive(Deserialize)]
                struct ParamsNodes {
                    #[serde(default)]
                    nodes: Vec<nodes::Node>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsNodes = unpack(payload)?;
                    for mut node in p.nodes {
                        node.set_static();
                        let name = node.name().to_owned();
                        let nval = to_value(&node)?;
                        nodes::append_static_node(node, &mut *nodes::NODES.write().await).await?;
                        eapi_bus::registry()
                            .key_set(&format!("node/{}", name), nval)
                            .await?;
                    }
                    Ok(None)
                }
            }
            "node.undeploy" => {
                #[derive(Deserialize)]
                struct ParamsNodes {
                    #[serde(default)]
                    nodes: Vec<NodeOrId>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsNodes = unpack(payload)?;
                    for node in p.nodes {
                        let name = node.name();
                        nodes::remove_node(name, &mut *nodes::NODES.write().await).await?;
                        eapi_bus::registry()
                            .key_delete(&format!("node/{}", name))
                            .await?;
                    }
                    Ok(None)
                }
            }
            "node.export" => {
                #[derive(Serialize)]
                struct NodesExportRes<'a> {
                    nodes: Vec<&'a nodes::Node>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    let nodes = nodes::NODES.read().await;
                    if p.i == "*" || p.i == "#" {
                        let mut res = NodesExportRes {
                            nodes: nodes.values().collect::<Vec<&nodes::Node>>(),
                        };
                        res.nodes.sort();
                        Ok(Some(pack(&res)?))
                    } else if let Some(n) = nodes.get(p.i) {
                        let res = NodesExportRes { nodes: vec![n] };
                        Ok(Some(pack(&res)?))
                    } else {
                        Err(Error::not_found("node not found").into())
                    }
                }
            }
            "node.get_config" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    let nodes = nodes::NODES.read().await;
                    if let Some(n) = nodes.get(p.i) {
                        if n.is_static() {
                            Ok(Some(pack(n)?))
                        } else {
                            Err(Error::failed(
                                "the node is not static, append a static node with the same name",
                            )
                            .into())
                        }
                    } else {
                        Err(Error::not_found("node not found").into())
                    }
                }
            }
            "node.remove" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    nodes::remove_node(p.i, &mut *nodes::NODES.write().await).await?;
                    eapi_bus::registry()
                        .key_delete(&format!("node/{}", p.i))
                        .await?;
                    Ok(None)
                }
            }
            "node.reload" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    if let Some(tx) = nodes::RELOAD_TRIGGERS.read().await.get(p.i) {
                        tx.send(true).await.map_err(Error::core)?;
                        Ok(None)
                    } else {
                        Err(Error::failed("no such node or the node is disabled").into())
                    }
                }
            }
            "node.test" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    call_remote_method("ping", call_remote_params(p.i), false, true).await?;
                    Ok(None)
                }
            }
            "node.mtest" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    not_local!(p.i);
                    call_remote_method("bus::eva.core::test", call_remote_params(p.i), true, true)
                        .await?;
                    Ok(None)
                }
            }
            _ => {
                if method.starts_with("bus::") {
                    if payload.is_empty() {
                        Err(RpcError::params(None))
                    } else {
                        let params = unpack::<BTreeMap<String, Value>>(payload)?;
                        let res = call_remote_method(method, params, true, false).await?;
                        Ok(Some(pack(&res)?))
                    }
                } else {
                    svc_handle_default_rpc(method, &self.info)
                }
            }
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
        svc_need_ready!();
        if !crate::NODES_LOADED.load(atomic::Ordering::SeqCst) {
            return;
        }
        if frame.kind() == busrt::FrameKind::Publish
            && let Some(topic) = frame.topic()
        {
            if let Some(o) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                process_local_state(topic, o, frame.payload(), &self.tx)
                    .await
                    .log_ef();
            } else if let Some(o) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
                if self.replicate_remote {
                    process_remote_state(topic, o, frame.payload(), &self.tx)
                        .await
                        .log_ef();
                }
            } else if let Some(key_id) = topic.strip_prefix(AAA_KEY_TOPIC) {
                aaa::KEYS.lock().unwrap().remove(key_id);
                aaa::ENC_OPTS.lock().unwrap().remove(key_id);
                aaa::ACLS.lock().unwrap().remove(key_id);
            } else if let Some(acl_id) = topic.strip_prefix(AAA_ACL_TOPIC) {
                aaa::ACLS
                    .lock()
                    .unwrap()
                    .retain(|_, v| !v.contains_acl(acl_id));
            }
        }
    }
}

async fn notify(data: Data<'_>) -> EResult<()> {
    let ps_rpc = crate::PUBSUB_RPC.get().unwrap();
    let client = ps_rpc.client();
    let qos = ps_rpc.qos();
    match data {
        Data::Single(state) => {
            client
                .publish(
                    &format!("{}{}", crate::PS_ITEM_STATE_TOPIC, state.oid.as_path()),
                    serde_json::to_vec(
                        &state.into_replication_state_event(crate::SYSTEM_NAME.get().unwrap()),
                    )?,
                    qos,
                )
                .await?;
        }
        Data::Bulk(state) => {
            let cfg = crate::BULK_SEND_CONFIG.get().unwrap();
            let mut opts = if let Some(ref key_id) = cfg.encryption_key {
                aaa::get_enc_opts(key_id).await?
            } else {
                psrpc::options::Options::new()
            };
            opts = opts.compression(if cfg.compress {
                psrpc::options::Compression::Bzip2
            } else {
                psrpc::options::Compression::No
            });
            let mut frame = vec![0x00, psrpc::PROTO_VERSION, opts.flags(), 0x00, 0x00];
            frame.extend(crate::SYSTEM_NAME.get().unwrap().as_bytes());
            frame.push(0x00);
            if let Some(ref key_id) = cfg.encryption_key {
                frame.extend(key_id.as_bytes().to_vec());
            }
            frame.push(0x00);
            frame.extend(opts.pack_payload(pack(state)?).await?);
            client
                .publish(crate::BULK_STATE_TOPIC.get().unwrap(), frame, qos)
                .await?;
        }
    }
    Ok(())
}

pub async fn sender(rx: async_channel::Receiver<FullItemState>, buf_ttl: Option<Duration>) {
    if let Some(bttl) = buf_ttl {
        let mut buf_interval = tokio::time::interval(bttl);
        let mut data_buf: Vec<ReplicationData> = Vec::new();
        loop {
            tokio::select! {
                f = rx.recv() => {
                    if let Ok(state) = f {
                        data_buf.push(state.into());
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
        while let Ok(state) = rx.recv().await {
            notify(Data::Single(state)).await.log_ef();
        }
    }
}

enum Data<'a> {
    Single(FullItemState),
    Bulk(&'a Vec<ReplicationData>),
}

async fn process_local_state(
    topic: &str,
    path: &str,
    payload: &[u8],
    tx: &async_channel::Sender<FullItemState>,
) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<LocalStateEvent>(payload) {
            Ok(v) => {
                tx.send(FullItemState::from_local_state_event(v, oid))
                    .await
                    .map_err(Error::core)?;
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

async fn process_remote_state(
    topic: &str,
    path: &str,
    payload: &[u8],
    tx: &async_channel::Sender<FullItemState>,
) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<RemoteStateEvent>(payload) {
            Ok(v) => {
                tx.send(FullItemState::from_remote_state_event(v, oid))
                    .await
                    .map_err(Error::core)?;
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

#[inline]
fn call_remote_params(name: &str) -> BTreeMap<String, Value> {
    let mut p = BTreeMap::new();
    p.insert("node".to_owned(), Value::String(name.to_owned()));
    p
}

async fn call_remote_method(
    method: &str,
    mut params: BTreeMap<String, Value>,
    admin: bool,
    allow_offline: bool,
) -> EResult<Value> {
    let ps_rpc = crate::PUBSUB_RPC.get().unwrap();
    let i: Value = params
        .remove("node")
        .ok_or_else(|| Error::invalid_params("node not specified"))?;
    let (node_name, timeout, compress, key_id) = {
        if let Value::String(s) = i {
            let nodes = nodes::NODES.read().await;
            let node = nodes
                .get(&s)
                .ok_or_else(|| Error::failed("node not connected"))?;
            if !node.api_enabled {
                return Err(Error::access("Pub/Sub API is disabled"));
            }
            let key_id = if admin {
                if let Some(key) = node.admin_key_id() {
                    key.to_owned()
                } else {
                    return Err(Error::access("node is not managed, admin key is not set"));
                }
            } else {
                node.key_id().to_owned()
            };
            if !allow_offline && !node.online() {
                return Err(Error::failed("node is offline"));
            }
            (s, node.timeout(), node.compress(), key_id)
        } else {
            return Err(Error::invalid_params("node is not a string"));
        }
    };
    let mut opts = aaa::get_enc_opts(&key_id).await?;
    opts = opts.compression(if compress {
        psrpc::options::Compression::Bzip2
    } else {
        psrpc::options::Compression::No
    });
    macro_rules! call {
        ($params: expr) => {
            tokio::time::timeout(timeout, ps_rpc.call(&node_name, method, $params, &opts))
                .await
                .map_err(Into::<Error>::into)?
        };
    }
    if params.is_empty() {
        call!(None)
    } else {
        call!(Some(&to_value(params)?))
    }
}

pub async fn submit_periodic(
    interval: Duration,
    oids: &OIDMaskList,
    oids_exclude: &OIDMaskList,
    replicate_remote: bool,
) -> EResult<()> {
    #[derive(Serialize)]
    struct Request<'a> {
        i: &'a OIDMaskList,
        exclude: &'a OIDMaskList,
        #[serde(skip_serializing_if = "Option::is_none")]
        node: Option<&'a str>,
        include_binary_values: bool,
    }
    let payload = busrt::borrow::Cow::Referenced(Arc::new(pack(&Request {
        i: oids,
        exclude: oids_exclude,
        include_binary_values: true,
        node: if replicate_remote {
            None
        } else {
            Some(".local")
        },
    })?));
    let mut int = tokio::time::interval(interval);
    loop {
        int.tick().await;
        let res: Vec<FullItemStateAndInfoOwned> = unpack(
            eapi_bus::call("eva.core", "item.list", payload.clone())
                .await?
                .payload(),
        )?;
        notify(Data::Bulk(
            &res.into_iter()
                .map(|v| ReplicationData::Inventory(v.into()))
                .collect::<Vec<ReplicationData>>(),
        ))
        .await?;
    }
}
