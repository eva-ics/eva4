use busrt::QoS;
use eva_common::acl::OIDMaskList;
use eva_common::common_payloads::ValueOrList;
use eva_common::events::{
    NodeInfo, NodeStateEvent, NodeStatus, AAA_ACL_TOPIC, AAA_KEY_TOPIC,
    REPLICATION_INVENTORY_TOPIC, REPLICATION_NODE_STATE_TOPIC,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::pubsub::{PS_ITEM_BULK_STATE_TOPIC, PS_ITEM_STATE_TOPIC, PS_NODE_STATE_TOPIC};
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;

mod aaa;
mod eapi;
mod nodes;
mod pubsub;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "v4 replication service";

const DEFAULT_RELOAD_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(10);

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

lazy_static::lazy_static! {
    static ref BULK_SEND_CONFIG: OnceCell<BulkSendConfig> = <_>::default();
    static ref BULK_STATE_TOPIC: OnceCell<String> = <_>::default();
    static ref PUBSUB_RPC: OnceCell<Arc<psrpc::RpcClient>> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
    static ref KEY_SVC: OnceCell<String> = <_>::default();
    static ref SYSTEM_NAME: OnceCell<String> = <_>::default();
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref DEFAULT_KEY_ID: OnceCell<String> = <_>::default();
    static ref REG: OnceCell<Registry> = <_>::default();
    static ref HTTP_CLIENT: OnceCell<eva_sdk::http::Client> = <_>::default();
    static ref PULL_DATA: OnceCell<nodes::PullData> = <_>::default();
    static ref BULK_SECURE_TOPICS: OnceCell<HashSet<String>> = <_>::default();

}

static DISCOVERY_ENABLED: atomic::AtomicBool = atomic::AtomicBool::new(false);
static SUBSCRIBE_EACH: atomic::AtomicBool = atomic::AtomicBool::new(false);

static NODES_LOADED: atomic::AtomicBool = atomic::AtomicBool::new(false);

#[derive(Deserialize, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SubscribeKind {
    Each,
    All,
    BulkOnly,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PsProto {
    Psrt,
    Mqtt,
}

#[inline]
fn default_queue_size() -> usize {
    1024
}

#[inline]
fn default_qos() -> i32 {
    1
}

#[inline]
fn default_cloud_key() -> String {
    "default".to_owned()
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
struct BulkReceiveConfig {
    #[serde(default)]
    topics: HashSet<String>,
    #[serde(default)]
    secure_topics: HashSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkSendConfig {
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
    buf_ttl_sec: Duration,
    topic: String,
    #[serde(default)]
    compress: bool,
    #[serde(default)]
    encryption_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkConfig {
    #[serde(default)]
    send: Option<BulkSendConfig>,
    #[serde(default)]
    receive: Option<BulkReceiveConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    pubsub: PubSubConfig,
    key_svc: String,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    announce_interval: Option<Duration>,
    #[serde(default)]
    api_enabled: bool,
    #[serde(default)]
    discovery_enabled: bool,
    #[serde(default = "default_cloud_key")]
    default_key_id: String,
    subscribe: SubscribeKind,
    #[serde(default)]
    bulk: Option<BulkConfig>,
    oids: OIDMaskList,
    #[serde(default)]
    replicate_remote: bool,
}

async fn mark_all_offline() -> EResult<()> {
    let client = RPC.get().unwrap().client();
    let node_topics: Vec<(String, bool)> = nodes::NODES
        .read()
        .await
        .iter()
        .map(|(k, v)| (format!("{REPLICATION_NODE_STATE_TOPIC}{k}"), v.is_static()))
        .collect();
    let offline_payload: Vec<u8> = pack(&NodeStateEvent {
        status: NodeStatus::Offline,
        info: None,
        timeout: None,
    })?;
    let remove_payload: Vec<u8> = pack(&NodeStateEvent {
        status: NodeStatus::Removed,
        info: None,
        timeout: None,
    })?;
    for t in node_topics {
        client
            .lock()
            .await
            .publish(
                &t.0,
                if t.1 {
                    offline_payload.as_slice()
                } else {
                    remove_payload.as_slice()
                }
                .into(),
                QoS::No,
            )
            .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let mut config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    if config.pubsub.cluster_hosts_randomize {
        config.pubsub.host.shuffle();
    }
    SYSTEM_NAME
        .set(initial.system_name().to_owned())
        .map_err(|_| Error::core("unable to set SYSTEM_NAME"))?;
    KEY_SVC
        .set(config.key_svc)
        .map_err(|_| Error::core("unable to set KEY_SVC"))?;
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("unable to set TIMEOUT"))?;
    HTTP_CLIENT
        .set(eva_sdk::http::Client::new(
            (initial.workers() * 100)
                .try_into()
                .map_err(Error::failed)?,
            timeout,
        ))
        .map_err(|_| Error::core("Unable to set HTTP_CLIENT"))?;
    DEFAULT_KEY_ID
        .set(config.default_key_id)
        .map_err(|_| Error::core("unable to set DEFAULT_KEY_ID"))?;
    let pull_data = nodes::PullData {
        info: NodeInfo {
            build: initial.eva_build(),
            version: initial.eva_version().to_owned(),
        },
        items: None,
    };
    PULL_DATA
        .set(pull_data)
        .map_err(|_| Error::core("unable to set PULL_DATA"))?;
    DISCOVERY_ENABLED.store(config.discovery_enabled, atomic::Ordering::SeqCst);
    SUBSCRIBE_EACH.store(
        config.subscribe == SubscribeKind::Each,
        atomic::Ordering::SeqCst,
    );
    let qos = config.pubsub.qos;
    let (sender_tx, sender_rx) = async_channel::bounded(config.pubsub.queue_size);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("node.list"));
    info.add_method(ServiceMethod::new("node.append").required("i"));
    info.add_method(ServiceMethod::new("node.deploy").required("nodes"));
    info.add_method(ServiceMethod::new("node.undeploy").required("nodes"));
    info.add_method(ServiceMethod::new("node.export").required("i"));
    info.add_method(ServiceMethod::new("node.get_config").required("i"));
    info.add_method(ServiceMethod::new("node.get").required("i"));
    info.add_method(ServiceMethod::new("node.reload").required("i"));
    info.add_method(ServiceMethod::new("node.test").required("i"));
    info.add_method(ServiceMethod::new("node.mtest").required("i"));
    info.add_method(ServiceMethod::new("node.remove").required("i"));
    let handlers = eapi::Handlers::new(sender_tx, info, config.replicate_remote);
    let rpc = initial.init_rpc(handlers).await?;
    initial.drop_privileges()?;
    let registry = initial.init_registry(&rpc);
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("unable to set RPC"))?;
    eva_sdk::service::subscribe_oids(
        rpc.as_ref(),
        &config.oids,
        if config.replicate_remote {
            eva_sdk::service::EventKind::Actual
        } else {
            eva_sdk::service::EventKind::Local
        },
    )
    .await?;
    let static_nodes = {
        let mut n = Vec::new();
        let reg_nodes = registry.key_get_recursive("node").await?;
        for (k, v) in reg_nodes {
            debug!("node loaded: {}", k);
            let mut node = nodes::Node::deserialize(v)?;
            node.set_static();
            assert!(
                (k == node.name()),
                "node name mismatch: {} != {}",
                k,
                node.name()
            );
            nodes::mark_node(node.name(), false, None, true, None).await?;
            n.push(node);
        }
        n
    };
    REG.set(registry)
        .map_err(|_| Error::core("unable to set registry object"))?;
    let client = rpc.client().clone();
    client
        .lock()
        .await
        .subscribe_bulk(
            &[&format!("{AAA_KEY_TOPIC}#"), &format!("{AAA_ACL_TOPIC}#")],
            QoS::No,
        )
        .await?;
    svc_init_logs(&initial, client.clone())?;
    if config.replicate_remote {
        warn!(
            "Remote item replication is on. Use a proper cloud structure only to avoid event loops"
        );
    }
    let mut bulk_recv_topics: HashSet<String> = HashSet::new();
    let mut bulk_secure_topics: HashSet<String> = HashSet::new();
    if let Some(ref mut bulk) = config.bulk {
        if let Some(bulk_recv) = bulk.receive.take() {
            if let Some(ref bulk_send) = bulk.send {
                if bulk_recv.topics.contains(&bulk_send.topic)
                    || bulk_recv.secure_topics.contains(&bulk_send.topic)
                {
                    warn!(
                        "bulk.receive.topics contain bulk.send.topic. \
                        This may slow down network operations. Consider using \
                        a dedicated topic to send/receive"
                    );
                }
            }
            for topic in bulk_recv.topics {
                bulk_recv_topics.insert(topic);
            }
            for topic in bulk_recv.secure_topics {
                bulk_secure_topics.insert(topic);
            }
        }
    }
    svc_start_signal_handlers();
    let mut pubsub_rpc_config = psrpc::Config::new(initial.system_name())
        .timeout(timeout)
        .ping_interval(config.pubsub.ping_interval)
        .queue_size(config.pubsub.queue_size)
        .qos(qos);
    let pubsub_fatal = pubsub_rpc_config.arm_failure_trigger();
    let mut topic_broker = psrpc::tools::TopicBroker::new();
    let (_, rx) =
        topic_broker.register_prefix(PS_ITEM_BULK_STATE_TOPIC, config.pubsub.queue_size)?;
    tokio::spawn(async move {
        pubsub::ps_bulk_state_handler(rx).await.log_ef();
    });
    if config.subscribe != SubscribeKind::BulkOnly {
        let (_, rx) =
            topic_broker.register_prefix(PS_ITEM_STATE_TOPIC, config.pubsub.queue_size)?;
        tokio::spawn(async move {
            pubsub::ps_state_handler(rx).await.log_ef();
        });
    }
    let mut pubsub_rpc_handlers =
        pubsub::PubSubHandlers::new(config.api_enabled, topic_broker, config.replicate_remote);
    let ps_rpc = Arc::new(match config.pubsub.proto {
        PsProto::Psrt => {
            let mut psrt_config = psrt::client::Config::new("")
                .set_timeout(timeout)
                .set_queue_size(config.pubsub.queue_size);
            if let Some(ref username) = config.pubsub.username {
                let password = if let Some(ref password) = config.pubsub.password {
                    password
                } else {
                    ""
                };
                psrt_config = psrt_config.set_auth(username, password);
            }
            if let Some(ref ca_certs) = config.pubsub.ca_certs {
                let certs = tokio::fs::read_to_string(ca_certs).await?;
                psrt_config = psrt_config.set_tls(true).set_tls_ca(Some(certs));
            }
            let mut client = None;
            for host in config.pubsub.host.iter() {
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
            if let Some(c) = client.take() {
                pubsub_rpc_handlers.start(config.pubsub.queue_size, initial.startup_timeout())?;
                psrpc::RpcClient::create(c, pubsub_rpc_handlers, pubsub_rpc_config).await?
            } else {
                return Err(Error::failed("Unable to find working psrt host"));
            }
        }
        PsProto::Mqtt => {
            let mut builder = paho_mqtt::ConnectOptionsBuilder::new();
            let mut mqtt_config = builder
                .keep_alive_interval(config.pubsub.ping_interval * 2)
                .connect_timeout(timeout);
            if let Some(ref username) = config.pubsub.username {
                let password = if let Some(ref password) = config.pubsub.password {
                    password
                } else {
                    ""
                };
                mqtt_config = mqtt_config.user_name(username).password(password);
            }
            if let Some(ca_certs) = config.pubsub.ca_certs {
                let mut b = paho_mqtt::ssl_options::SslOptionsBuilder::new();
                let ssl = b.trust_store(ca_certs).map_err(Error::failed)?;
                mqtt_config = mqtt_config.ssl_options(ssl.finalize());
            }
            let mut client = None;
            let cfg = mqtt_config.finalize();
            for host in config.pubsub.host.iter() {
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
            if let Some(c) = client.take() {
                pubsub_rpc_handlers.start(config.pubsub.queue_size, initial.startup_timeout())?;
                psrpc::RpcClient::create(c, pubsub_rpc_handlers, pubsub_rpc_config).await?
            } else {
                return Err(Error::failed("Unable to find working mqtt host"));
            }
        }
    });
    PUBSUB_RPC
        .set(ps_rpc.clone())
        .map_err(|_| Error::core("unable to set PUBSUB_RPC"))?;
    let buf_ttl = if let Some(Some(bulk_send)) = config.bulk.map(|v| v.send) {
        let t = bulk_send.buf_ttl_sec;
        let topic = format!("{}{}", PS_ITEM_BULK_STATE_TOPIC, bulk_send.topic);
        BULK_STATE_TOPIC
            .set(topic)
            .map_err(|_| Error::core("unable to set BULK_STATE_TOPIC"))?;
        BULK_SEND_CONFIG
            .set(bulk_send)
            .map_err(|_| Error::core("unable to set BULK_SEND_CONFIG"))?;
        Some(t)
    } else {
        None
    };
    let mut topics = vec![format!("{PS_NODE_STATE_TOPIC}+")];
    for topic in &bulk_recv_topics {
        topics.push(format!("{PS_ITEM_BULK_STATE_TOPIC}{topic}"));
    }
    for topic in &bulk_secure_topics {
        topics.push(format!("{PS_ITEM_BULK_STATE_TOPIC}{topic}"));
    }
    if !bulk_secure_topics.is_empty() {
        BULK_SECURE_TOPICS
            .set(bulk_secure_topics)
            .map_err(|_| Error::core("Unable to set BULK_SECURE_TOPICS"))?;
    }
    if config.subscribe == SubscribeKind::All {
        for kind in [ItemKind::Unit, ItemKind::Sensor, ItemKind::Lvar] {
            topics.push(format!("{}{}/#", PS_ITEM_STATE_TOPIC, kind));
        }
    }
    if !topics.is_empty() {
        ps_rpc
            .client()
            .subscribe_bulk(
                topics
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>()
                    .as_slice(),
                qos,
            )
            .await?;
    }
    eva_sdk::service::set_poc(Some(timeout));
    tokio::spawn(async move {
        pubsub_fatal.await;
        eva_sdk::service::poc();
    });
    svc_mark_ready(&client).await?;
    svc_wait_core(&rpc, initial.startup_timeout(), true)
        .await
        .log_ef();
    {
        let mut nodes = nodes::NODES.write().await;
        for node in static_nodes {
            nodes::append_static_node(node, &mut nodes).await?;
        }
    }
    NODES_LOADED.store(true, atomic::Ordering::SeqCst);
    tokio::spawn(async move {
        eapi::sender(sender_rx, buf_ttl).await;
    });
    let announce_fut = if let Some(announce_interval) = config.announce_interval {
        if announce_interval.as_nanos() == 0 {
            None
        } else {
            let client = ps_rpc.client();
            let announce_topic = format!("{}{}", PS_NODE_STATE_TOPIC, initial.system_name());
            let announce_fut = tokio::spawn(async move {
                let mut interval = tokio::time::interval(announce_interval);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    trace!("announcing node state");
                    client
                        .publish(
                            &announce_topic,
                            serde_json::to_vec(&eva_sdk::pubsub::PsNodeStatus::new_running())
                                .unwrap(),
                            qos,
                        )
                        .await
                        .log_ef();
                }
            });
            Some(announce_fut)
        }
    } else {
        None
    };
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    mark_all_offline().await?;
    if let Some(fut) = announce_fut {
        fut.abort();
    }
    if config.announce_interval.is_some() {
        let announce_topic = format!("{}{}", PS_NODE_STATE_TOPIC, initial.system_name());
        ps_rpc
            .client()
            .publish(
                &announce_topic,
                serde_json::to_vec(&eva_sdk::pubsub::PsNodeStatus::new_terminating()).unwrap(),
                qos,
            )
            .await
            .log_ef();
    }
    svc_mark_terminating(&client).await?;
    ps_rpc.client().bye().await?;
    Ok(())
}
