use eva_common::events::RawStateEvent;
use eva_common::events::RAW_STATE_TOPIC;
use eva_common::prelude::*;
use eva_sdk::controller::{actt, Action};
use eva_sdk::prelude::*;
use eva_sdk::service::set_poc;
use itertools::Itertools;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;

err_logger!();

mod actions;
mod client;
mod common;
mod eapi;
mod modbus;
mod pull;
mod types;

lazy_static! {
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref VERIFY_DELAY: OnceCell<Option<Duration>> = <_>::default();
    static ref ACTION_QUEUES: OnceCell<HashMap<OID, async_channel::Sender<Action>>> =
        <_>::default();
    static ref ACTT: OnceCell<actt::Actt> = <_>::default();
    static ref MODBUS_CLIENT: OnceCell<client::Client> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Modbus master controller";

const SLEEP_ON_ERROR_INTERVAL: Duration = Duration::from_secs(5);

static ACTIONS_VERIFY: atomic::AtomicBool = atomic::AtomicBool::new(false);
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
    let mut config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    if initial.is_mode_rtf() {
        println!("marking all mapped items as failed");
        let payload_failed = pack(&RawStateEvent::new0(eva_common::ITEM_STATUS_ERROR))?;
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
    set_poc(config.panic_in);
    ACTIONS_VERIFY.store(config.actions_verify, atomic::Ordering::SeqCst);
    DEFAULT_RETRIES.store(config.retries, atomic::Ordering::SeqCst);
    VERIFY_DELAY
        .set(config.verify_delay)
        .map_err(|_| Error::core("Unable to set VERIFY_DELAY"))?;
    let action_oids = config.action_map.keys().collect::<Vec<&OID>>();
    ACTT.set(actt::Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("reg.get")
            .required("unit")
            .required("reg")
            .optional("count")
            .optional("type")
            .optional("bit")
            .optional("timeout")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("reg.set")
            .required("unit")
            .required("reg")
            .required("value")
            .optional("type")
            .optional("bit")
            .optional("verify")
            .optional("timeout")
            .optional("retries"),
    );
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(config.queue_size);
    let rpc = initial
        .init_rpc(eapi::Handlers::new(info, tx.clone()))
        .await?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("unable to set RPC"))?;
    for pull in &mut config.pull {
        pull.init(config.modbus.unit)?;
    }
    for (oid, action_map) in &mut config.action_map {
        action_map.init(config.modbus.unit, oid)?;
    }
    let pool_size = initial.workers();
    let modbus_client = client::Client::create(
        &config.modbus.path,
        timeout,
        pool_size as usize,
        config.modbus.protocol,
        config.modbus.frame_delay,
        config.modbus.fc16_supported,
        config.modbus.keep_alive,
    )
    .await?;
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
    MODBUS_CLIENT
        .set(modbus_client)
        .map_err(|_| Error::core("Unable to set MODBUS_CLIENT"))?;
    svc_start_signal_handlers();
    if !config.pull.is_empty() {
        if let Some(interval) = config.pull_interval {
            let tags = config.pull;
            let rpc_c = rpc.clone();
            let tx_c = tx.clone();
            let startup_timeout = initial.startup_timeout();
            let pull_cache_sec = config.pull_cache_sec;
            let workers = initial.workers();
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            #[allow(clippy::cast_precision_loss)]
            let tasks_per_worker = (tags.len() as f64 / f64::from(workers)).ceil() as usize;
            tokio::spawn(async move {
                let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
                for (id, ch) in (&tags.into_iter().chunks(tasks_per_worker))
                    .into_iter()
                    .enumerate()
                {
                    let tx_cc = tx_c.clone();
                    let tasks = ch.collect();
                    tokio::spawn(async move {
                        pull::launch(tasks, interval, pull_cache_sec, tx_cc, id + 1, timeout).await;
                    });
                }
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
