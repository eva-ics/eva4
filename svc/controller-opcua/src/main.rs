use eva_common::events::RawStateEvent;
use eva_common::events::RAW_STATE_TOPIC;
use eva_common::prelude::*;
use eva_sdk::controller::{actt, Action};
use eva_sdk::prelude::*;
use eva_sdk::service::set_poc;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;

err_logger!();

mod actions;
mod comm;
mod common;
mod conv;
mod eapi;
mod pull;

lazy_static! {
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref ACTION_QUEUES: OnceCell<HashMap<OID, async_channel::Sender<Action>>> =
        <_>::default();
    static ref ACTT: OnceCell<actt::Actt> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "OPC-UA client controller";

static DEFAULT_RETRIES: atomic::AtomicU8 = atomic::AtomicU8::new(0);

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    let config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let payload_failed = pack(&RawStateEvent::new0(eva_common::ITEM_STATUS_ERROR))?;
    if initial.is_mode_rtf() {
        println!("marking all mapped items as failed");
        let mut bus = initial.init_bus_client().await?;
        for pull in config.pull {
            for task in pull.map() {
                bus.publish(
                    &format!("{}{}", RAW_STATE_TOPIC, task.oid().as_path()),
                    payload_failed.as_slice().into(),
                    busrt::QoS::No,
                )
                .await?;
            }
        }
        return Ok(());
    }
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(config.queue_size);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("var.get")
            .required("i")
            .optional("range")
            .optional("timeout")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("var.set")
            .required("i")
            .required("value")
            .required("type")
            .optional("range")
            .optional("dimensions")
            .optional("timeout")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("var.set_bulk")
            .required("i")
            .required("values")
            .required("types")
            .optional("ranges")
            .optional("dimensions")
            .optional("timeout")
            .optional("retries"),
    );
    let rpc = initial
        .init_rpc(eapi::Handlers::new(info, tx.clone()))
        .await?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("Unable to set RPC"))?;
    let client = rpc.client().clone();
    initial.drop_privileges()?;
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    set_poc(config.panic_in);
    if config.opcua.trust_server_x509 {
        opcua::client::set_trust_any_server_cert(true);
    }
    comm::init_session(config.opcua, &initial, timeout, config.ping.map(|p| p.node)).await?;
    DEFAULT_RETRIES.store(config.retries.unwrap_or(0), atomic::Ordering::SeqCst);
    let mut action_oids = Vec::new();
    for oid in config.action_map.keys() {
        action_oids.push(oid);
    }
    ACTT.set(actt::Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
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
        if let Some(interval) = config.pull_interval {
            let nodes: Vec<common::PullNode> = config.pull.into_iter().collect();
            let rpc_c = rpc.clone();
            let tx_c = tx.clone();
            let startup_timeout = initial.startup_timeout();
            let pull_cache_sec = config.pull_cache_sec;
            tokio::spawn(async move {
                let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
                let tx_cc = tx_c.clone();
                tokio::spawn(async move {
                    pull::launch(nodes, interval, pull_cache_sec, tx_cc).await;
                });
            });
        }
    }
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
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
