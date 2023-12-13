use eva_common::events::RawStateEvent;
use eva_common::events::RAW_STATE_TOPIC;
use eva_common::op::Op;
use eva_common::prelude::*;
use eva_sdk::controller::{actt, Action};
use eva_sdk::prelude::*;
use eva_sdk::service::set_poc;
use itertools::Itertools;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::Mutex;
use std::time::Duration;

err_logger!();

mod actions;
mod common;
mod eapi;
mod enip;
mod pull;
mod types;

lazy_static! {
    static ref PREPARED_TAGS: Mutex<HashMap<String, i32>> = <_>::default();
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref PLC_PATH: OnceCell<String> = <_>::default();
    static ref VERIFY_DELAY: OnceCell<Option<Duration>> = <_>::default();
    static ref ACTION_QUEUES: OnceCell<HashMap<OID, async_channel::Sender<Action>>> =
        <_>::default();
    static ref ACTT: OnceCell<actt::Actt> = <_>::default();
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "En/IP controller gateway";

const SLEEP_ON_ERROR_INTERVAL: Duration = Duration::from_secs(5);
const SLEEP_STEP: Duration = Duration::from_millis(1);

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
    #[allow(clippy::cast_possible_truncation)]
    let plc_timeout = initial.timeout().as_millis() as i32;
    let mut config: common::Config = common::Config::deserialize(
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
    set_poc(config.panic_in);
    ACTIONS_VERIFY.store(config.actions_verify, atomic::Ordering::SeqCst);
    DEFAULT_RETRIES.store(config.retries.unwrap_or(0), atomic::Ordering::SeqCst);
    VERIFY_DELAY
        .set(config.verify_delay)
        .map_err(|_| Error::core("Unable to set VERIFY_DELAY"))?;
    let plc_path = config.plc.generate_path();
    let mut tags_created = HashSet::new();
    let mut tags_pending = HashSet::new();
    let mut tag_oid_map: HashMap<&str, Vec<&OID>> = HashMap::new();
    for tag in &mut config.pull {
        tag.init(&plc_path, plc_timeout)?;
        tags_created.insert(tag.path());
        tags_pending.insert(types::PendingTag::new(tag.id(), tag.path()));
        tag_oid_map.insert(
            tag.path(),
            tag.map().iter().map(common::PullTask::oid).collect(),
        );
    }
    let mut action_oids = Vec::new();
    for (oid, map) in &mut config.action_map {
        action_oids.push(oid);
        map.create_tags(
            oid,
            &plc_path,
            plc_timeout,
            &tags_created.iter().copied().collect::<Vec<&str>>(),
            &mut tags_pending,
        )?;
    }
    ACTT.set(actt::Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
    PLC_PATH
        .set(plc_path)
        .map_err(|_| Error::core("Unable to set PLC_PATH"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("var.get")
            .required("i")
            .required("type")
            .optional("size")
            .optional("count")
            .optional("bit")
            .optional("timeout")
            .optional("retries"),
    );
    info.add_method(
        ServiceMethod::new("var.set")
            .required("i")
            .required("value")
            .required("type")
            .optional("bit")
            .optional("timeout")
            .optional("verify")
            .optional("retries"),
    );
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(config.queue_size);
    let rpc = initial
        .init_rpc(eapi::Handlers::new(info, tx.clone()))
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
    svc_start_signal_handlers();
    debug!("creating client tags");
    let mut oids_failed = HashSet::new();
    let op = Op::new(timeout);
    {
        while !tags_pending.is_empty() {
            let mut created = Vec::new();
            let mut failed = Vec::new();
            for t in &tags_pending {
                let rc = unsafe { plctag::plc_tag_status(t.id()) };
                if rc == plctag::PLCTAG_STATUS_OK {
                    created.push(*t);
                } else if rc != plctag::PLCTAG_STATUS_PENDING {
                    error!("Unable to create PLC tag {}, status: {}", t.path(), rc);
                    failed.push(*t);
                    if let Some(oids) = tag_oid_map.get(t.path()) {
                        for oid in oids {
                            oids_failed.insert(oid);
                        }
                    }
                }
            }
            {
                let mut prepared_tags = PREPARED_TAGS.lock().unwrap();
                for c in created {
                    tags_pending.remove(&c);
                    prepared_tags.insert(c.path().to_owned(), c.id());
                }
                for c in failed {
                    tags_pending.remove(&c);
                }
            }
            if op.is_timed_out() {
                return Err(Error::timeout());
            }
            tokio::time::sleep(SLEEP_STEP).await;
        }
    }
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
    debug!("client tags created successfully");
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
                        pull::launch(tasks, interval, pull_cache_sec, tx_cc, id + 1).await;
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
