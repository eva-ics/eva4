use busrt::QoS;
use eva_common::acl::OIDMaskList;
use eva_common::events::{
    AAA_ACL_TOPIC, AAA_KEY_TOPIC, NodeInfo, ReplicationInventoryItem, ReplicationNodeInventoryItem,
    ReplicationStateEventExtended,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::pubsub::{PS_ITEM_BULK_STATE_TOPIC, PS_NODE_STATE_TOPIC};
use eva_sdk::types::FullItemState;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic};
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum ReplicationData {
    Inventory(ReplicationInventoryItem),
    State(FullItemState),
}

impl ReplicationData {
    pub fn oid(&self) -> &OID {
        match self {
            ReplicationData::State(v) => &v.oid,
            ReplicationData::Inventory(v) => &v.oid,
        }
    }
    pub fn into_replication_state_event_extended(
        self,
        system_name: &str,
    ) -> ReplicationStateEventExtended {
        match self {
            ReplicationData::State(v) => {
                ReplicationStateEventExtended::Basic(v.into_replication_state_event(system_name))
            }
            ReplicationData::Inventory(v) => {
                ReplicationStateEventExtended::Inventory(ReplicationNodeInventoryItem {
                    node: system_name.to_owned(),
                    item: v,
                })
            }
        }
    }
}

impl From<FullItemState> for ReplicationData {
    fn from(v: FullItemState) -> Self {
        ReplicationData::State(v)
    }
}

impl From<ReplicationInventoryItem> for ReplicationData {
    fn from(v: ReplicationInventoryItem) -> Self {
        ReplicationData::Inventory(v)
    }
}

mod aaa;
mod eapi;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "uni-directional replication service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static BULK_STATE_TOPIC: OnceCell<String> = OnceCell::new();
static BULK_SEND_CONFIG: OnceCell<BulkSendConfig> = OnceCell::new();
static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static KEY_SVC: OnceCell<String> = OnceCell::new();
static SYSTEM_NAME: OnceCell<String> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();

static OIDS: OnceCell<OIDMaskList> = OnceCell::new();
static OIDS_EXCLUDE: OnceCell<Vec<String>> = OnceCell::new();

static PUBSUB_CLIENT: OnceCell<psrt::client::UdpClient> = OnceCell::new();

static MTU: atomic::AtomicUsize = atomic::AtomicUsize::new(1200);

fn get_mtu() -> usize {
    MTU.load(atomic::Ordering::Relaxed)
}

async fn pubsub_publish(topic: &str, data: &[u8]) -> EResult<()> {
    let mtu = get_mtu();
    if data.len() > mtu {
        warn!(
            "data size exceeds MTU, the packet may be dropped, size: {}, mtu: {}",
            data.len(),
            mtu
        );
    }
    //info!(
    //"publishing to topic: {}, data length: {}",
    //topic,
    //data.len()
    //);
    PUBSUB_CLIENT
        .get()
        .unwrap()
        .publish(topic, data)
        .await
        .map_err(Error::io)
}

#[inline]
fn default_queue_size() -> usize {
    1024
}

#[inline]
fn default_mtu_size() -> usize {
    1200
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PubSubConfig {
    host: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default = "default_queue_size")]
    queue_size: usize,
    #[serde(default = "default_mtu_size")]
    mtu: usize,
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
struct Config {
    pubsub: PubSubConfig,
    key_svc: String,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    announce_interval: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    interval: Option<Duration>,
    #[serde(default)]
    send: Option<BulkSendConfig>,
    #[serde(default)]
    oids: OIDMaskList,
    #[serde(default)]
    oids_exclude: OIDMaskList,
    #[serde(default)]
    replicate_remote: bool,
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
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
    MTU.store(config.pubsub.mtu, atomic::Ordering::Relaxed);
    OIDS.set(config.oids.clone())
        .map_err(|_| Error::core("unable to set OIDS"))?;
    let oids_exclude_s: Vec<String> = config
        .oids_exclude
        .oid_masks()
        .iter()
        .map(ToString::to_string)
        .collect();
    OIDS_EXCLUDE
        .set(oids_exclude_s)
        .map_err(|_| Error::core("unable to set OIDS_EXCLUDE"))?;
    let (sender_tx, sender_rx) = async_channel::bounded(config.pubsub.queue_size);
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let handlers = eapi::Handlers::new(sender_tx, info, config.replicate_remote);
    let mut ps_client = psrt::client::UdpClient::connect(&config.pubsub.host)
        .await
        .map_err(Error::io)?;
    if let Some(ref username) = config.pubsub.username {
        if let Some(ref pubsub_key) = config.pubsub.key.as_ref() {
            ps_client = ps_client.with_encryption_auth(
                username,
                &hex::decode(pubsub_key)
                    .map_err(|e| Error::invalid_data(format!("invalid pub/sub key: {}", e)))?,
            );
        }
    }
    PUBSUB_CLIENT
        .set(ps_client)
        .map_err(|_| Error::core("unable to set PUBSUB_CLIENT"))?;
    if config.replicate_remote {
        warn!(
            "Remote item replication is on. Use a proper cloud structure only to avoid event loops"
        );
    }
    let rpc = initial.init_rpc(handlers).await?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("unable to set RPC"))?;
    eva_sdk::service::exclude_oids(
        rpc.as_ref(),
        &config.oids_exclude,
        if config.replicate_remote {
            eva_sdk::service::EventKind::Actual
        } else {
            eva_sdk::service::EventKind::Local
        },
    )
    .await?;
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
    if config.pubsub.username.is_some() {
        if config.pubsub.key.is_none() {
            warn!("pub/sub username is set, but the key is not set");
        }
    } else {
        warn!("pub/sub username is not set, the connection is not encrypted");
    }
    svc_start_signal_handlers();
    initial.drop_privileges()?;
    let buf_ttl = if let Some(bulk_send) = config.send {
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
    eva_sdk::service::set_poc(Some(timeout));
    svc_mark_ready(&client).await?;
    svc_wait_core(&rpc, initial.startup_timeout(), true)
        .await
        .log_ef();
    if let Some(interval) = config.interval {
        let oids = config.oids.clone();
        let oids_exclude = config.oids_exclude.clone();
        let replicate_remote = config.replicate_remote;
        tokio::spawn(async move {
            loop {
                if let Err(e) =
                    eapi::submit_periodic(interval, &oids, &oids_exclude, replicate_remote).await
                {
                    error!("eapi submit periodic error: {}", e);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }
    tokio::spawn(async move {
        eapi::sender(sender_rx, buf_ttl).await;
    });
    let announce_fut = if let Some(announce_interval) = config.announce_interval {
        if announce_interval.as_nanos() == 0 {
            None
        } else {
            let announce_topic = format!("{}{}", PS_NODE_STATE_TOPIC, initial.system_name());
            let node_info = NodeInfo {
                build: initial.eva_build(),
                version: initial.eva_version().to_owned(),
            };
            let status = eva_sdk::pubsub::PsNodeStatus::new_running()
                .with_info(node_info)
                .with_api_disabled();
            let serialized_status = serde_json::to_vec(&status)?;
            let announce_fut = tokio::spawn(async move {
                let mut interval = tokio::time::interval(announce_interval);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    trace!("announcing node state");
                    pubsub_publish(&announce_topic, &serialized_status)
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
    if let Some(fut) = announce_fut {
        fut.abort();
    }
    if config.announce_interval.is_some() {
        let announce_topic = format!("{}{}", PS_NODE_STATE_TOPIC, initial.system_name());
        pubsub_publish(
            &announce_topic,
            &serde_json::to_vec(&eva_sdk::pubsub::PsNodeStatus::new_terminating()).unwrap(),
        )
        .await
        .log_ef();
    }
    svc_mark_terminating(&client).await?;
    Ok(())
}
