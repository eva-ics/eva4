use eva_common::events::RawStateEvent;
use eva_common::events::RAW_STATE_TOPIC;
use eva_common::prelude::*;
use eva_sdk::controller::{actt, Action};
use eva_sdk::prelude::*;
use eva_sdk::service::set_poc;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

err_logger!();

mod actions;
mod adsbr;
mod common;
mod eapi;
mod pull;

use crate::adsbr::ParseAmsNetId;

static CHECK_READY: atomic::AtomicBool = atomic::AtomicBool::new(true);

pub fn need_check_ready() -> bool {
    CHECK_READY.load(atomic::Ordering::Relaxed)
}

lazy_static! {
    static ref ADS_VARS: Mutex<HashMap<Arc<String>, Arc<adsbr::Var>>> = <_>::default();
    static ref BRIDGE_ID: OnceCell<String> = <_>::default();
    static ref DEVICE_ADDR: OnceCell<::ads::AmsAddr> = <_>::default();
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref VERIFY_DELAY: OnceCell<Option<Duration>> = <_>::default();
    static ref ACTION_QUEUES: OnceCell<HashMap<OID, async_channel::Sender<Action>>> =
        <_>::default();
    static ref ACTT: OnceCell<actt::Actt> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
}

fn get_device() -> ([u8; 6], u16) {
    let a = DEVICE_ADDR.get().unwrap();
    (a.netid().0, a.port())
}

async fn shutdown_vars() {
    let vars = {
        let ads_vars = ADS_VARS.lock().unwrap();
        ads_vars.values().cloned().collect::<Vec<Arc<adsbr::Var>>>()
    };
    let _r = adsbr::destroy_vars(
        vars.iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&adsbr::Var>>()
            .as_slice(),
    )
    .await;
}

async fn poc() {
    shutdown_vars().await;
    if RESTART_BRIDGE_ON_PANIC.load(atomic::Ordering::SeqCst) {
        safe_rpc_call(
            RPC.get().unwrap(),
            BRIDGE_ID.get().unwrap(),
            "stop",
            busrt::empty_payload!(),
            QoS::No,
            *TIMEOUT.get().unwrap(),
        )
        .await
        .log_ef();
    }
    eva_sdk::service::poc();
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "TwinCAT ADS controller";

static ACTIONS_VERIFY: atomic::AtomicBool = atomic::AtomicBool::new(false);
static DEFAULT_RETRIES: atomic::AtomicU8 = atomic::AtomicU8::new(0);
static RESTART_BRIDGE_ON_PANIC: atomic::AtomicBool = atomic::AtomicBool::new(false);

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
    let mut config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    CHECK_READY.store(config.ads.check_ready, atomic::Ordering::Relaxed);
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
    let device_addr = ::ads::AmsAddr::new(
        config.ads.ams_netid.ams_net_id()?.into(),
        config.ads.ams_port,
    );
    DEVICE_ADDR
        .set(device_addr)
        .map_err(|_| Error::core("Unable to set DEVICE_ADDR"))?;
    BRIDGE_ID
        .set(config.ads.bridge_svc)
        .map_err(|_| Error::core("Unable to set BRIDGE_ID"))?;
    crate::adsbr::set_bulk_allow(config.ads.bulk_allow);
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(config.queue_size);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("var.get")
            .required("i")
            .optional("timeout")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("var.set")
            .required("i")
            .required("value")
            .optional("timeout")
            .optional("verify")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("var.set_bulk")
            .required("i")
            .required("values")
            .optional("timeout")
            .optional("verify")
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
    info!("scanning variables");
    let mut oids_failed = HashSet::new();
    {
        let mut symbols: HashSet<Arc<String>> = HashSet::new();
        for pull in &config.pull {
            symbols.insert(pull.symbol());
        }
        for action in config.action_map.values() {
            symbols.insert(action.symbol());
        }
        set_poc(config.panic_in);
        let vars =
            match adsbr::create_vars(symbols.into_iter().collect::<Vec<Arc<String>>>().as_slice())
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    poc().await;
                    return Err(e);
                }
            };
        {
            let mut ads_vars = ADS_VARS.lock().unwrap();
            for var in vars {
                match var.check() {
                    Ok(()) => {
                        ads_vars.insert(var.name(), Arc::new(var));
                    }
                    Err(e) => {
                        error!("{}", e);
                    }
                }
            }
            for pull in &mut config.pull {
                if let Some(var) = ads_vars.get(&pull.symbol()) {
                    pull.set_var(var.clone());
                } else {
                    for m in pull.map() {
                        oids_failed.insert(m.oid());
                    }
                }
            }
            for action in config.action_map.values_mut() {
                if let Some(var) = ads_vars.get(&action.symbol()) {
                    action.set_var(var.clone());
                }
            }
        }
    }
    ACTIONS_VERIFY.store(config.actions_verify, atomic::Ordering::SeqCst);
    DEFAULT_RETRIES.store(config.retries.unwrap_or(0), atomic::Ordering::SeqCst);
    VERIFY_DELAY
        .set(config.verify_delay)
        .map_err(|_| Error::core("Unable to set VERIFY_DELAY"))?;
    RESTART_BRIDGE_ON_PANIC.store(config.restart_bridge_on_panic, atomic::Ordering::SeqCst);
    let mut action_oids = Vec::new();
    for oid in config.action_map.keys() {
        action_oids.push(oid);
    }
    ACTT.set(actt::Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
    {
        let mut c = client.lock().await;
        for oid in oids_failed {
            c.publish(
                &format!("{}{}", RAW_STATE_TOPIC, oid.as_path()),
                payload_failed.as_slice().into(),
                busrt::QoS::No,
            )
            .await?;
        }
    }
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
    adsbr::ping().await?;
    if !config.pull.is_empty() {
        if let Some(interval) = config.pull_interval {
            let symbols: Vec<common::PullSymbol> = config
                .pull
                .into_iter()
                .filter(|pull| pull.var().is_some())
                .collect();
            let rpc_c = rpc.clone();
            let tx_c = tx.clone();
            let startup_timeout = initial.startup_timeout();
            let pull_cache_sec = config.pull_cache_sec;
            tokio::spawn(async move {
                let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
                let tx_cc = tx_c.clone();
                tokio::spawn(async move {
                    pull::launch(symbols, interval, pull_cache_sec, tx_cc).await;
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
    tokio::spawn(async move {
        adsbr::ping_worker(config.ping_interval.unwrap_or_else(|| timeout / 2)).await;
    });
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    shutdown_vars().await;
    println!("ADS disconnected");
    Ok(())
}
