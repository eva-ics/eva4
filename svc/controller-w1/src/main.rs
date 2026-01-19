use eva_common::events::RAW_STATE_TOPIC;
use eva_common::events::RawStateEvent;
use eva_common::prelude::*;
use eva_sdk::controller::RawStateCache;
use eva_sdk::controller::{Action, actt};
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic;
use std::time::Duration;

err_logger!();

mod actions;
mod common;
mod eapi;
mod pull;
mod w1;

static TIMEOUT: OnceLock<Duration> = OnceLock::new();
static VERIFY_DELAY: OnceLock<Option<Duration>> = OnceLock::new();
static ACTION_QUEUES: OnceLock<HashMap<OID, async_channel::Sender<Action>>> = OnceLock::new();
static ACTT: OnceLock<actt::Actt> = OnceLock::new();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "1-Wire controller";

static ACTIONS_VERIFY: atomic::AtomicBool = atomic::AtomicBool::new(false);
static DEFAULT_RETRIES: atomic::AtomicU8 = atomic::AtomicU8::new(0);

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    #[allow(clippy::cast_possible_truncation)]
    let config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    if initial.is_mode_rtf() {
        println!("marking all mapped items as failed");
        let payload_failed = pack(&RawStateEvent::new0(eva_common::ITEM_STATUS_ERROR))?;
        let mut bus = initial.init_bus_client().await?;
        for pull in config.pull {
            bus.publish(
                &format!("{}{}", RAW_STATE_TOPIC, pull.oid().as_path()),
                payload_failed.as_slice().into(),
                busrt::QoS::No,
            )
            .await?;
        }
        return Ok(());
    }
    ACTIONS_VERIFY.store(config.actions_verify, atomic::Ordering::SeqCst);
    DEFAULT_RETRIES.store(config.retries.unwrap_or(0), atomic::Ordering::SeqCst);
    VERIFY_DELAY
        .set(config.verify_delay)
        .map_err(|_| Error::core("Unable to set VERIFY_DELAY"))?;
    let mut action_oids = Vec::new();
    for oid in config.action_map.keys() {
        action_oids.push(oid);
    }
    ACTT.set(actt::Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("w1.get")
            .required("path")
            .optional("timeout")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("w1.set")
            .required("path")
            .required("value")
            .optional("timeout")
            .optional("verify")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("w1.scan")
            .optional("types")
            .optional("attrs_any")
            .optional("attrs_all")
            .optional("timeout")
            .optional("full"),
    );
    info.add_method(
        ServiceMethod::new("w1.info")
            .required("path")
            .optional("timeout"),
    );
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(config.queue_size);
    let rpc = initial
        .init_rpc(eapi::Handlers::new(info, tx.clone()))
        .await?;
    unsafe {
        owfs::init(&config.path).map_err(Error::failed)?;
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
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    info!("connected to {}", config.path);
    let mut action_queues: HashMap<OID, async_channel::Sender<Action>> = HashMap::new();
    for (oid, map) in config.action_map {
        let (atx, arx) = async_channel::bounded::<Action>(config.action_queue_size);
        debug!("starting action queue for {}", oid);
        actions::start_action_handler(map, arx, tx.clone());
        action_queues.insert(oid, atx);
    }
    ACTION_QUEUES
        .set(action_queues)
        .map_err(|_| Error::core("Unable to set ACTION_QUEUES"))?;
    let raw_state_cache = Arc::new(RawStateCache::new(config.pull_cache_sec));
    for pull in config.pull {
        let c_tx = tx.clone();
        let r_c = raw_state_cache.clone();
        tokio::spawn(async move {
            pull::launch(&pull, c_tx, r_c).await;
        });
    }
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    unsafe {
        owfs::finish();
    }
    Ok(())
}
