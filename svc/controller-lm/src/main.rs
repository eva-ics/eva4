use busrt::tools::pubsub;
use eva_common::acl::OIDMaskList;
use eva_common::common_payloads::{ParamsId, ParamsOID, ParamsUuid};
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::prelude::*;
use eva_sdk::controller::{actt::Actt, format_action_topic, Action};
use eva_sdk::prelude::*;
use eva_sdk::service::svc_block2;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use submap::SubMap;
use tokio::sync::Mutex;

err_logger!();

mod common;
mod cycle;
mod job;
mod opener;
mod rule;

use common::StateX;
use cycle::Cycle;
use job::Job;
use opener::Opener;
use rule::Rule;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Logic Manager programmable controller";

const QUEUE_SIZE: usize = 8192;

lazy_static! {
    static ref RULES: OnceCell<HashMap<String, Arc<Rule>>> = <_>::default();
    static ref RULE_MATRIX: OnceCell<SubMap<Arc<Rule>>> = <_>::default();
    static ref CYCLES: OnceCell<HashMap<String, Arc<Cycle>>> = <_>::default();
    static ref JOBS: OnceCell<HashMap<String, Arc<Job>>> = <_>::default();
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
    static ref PREV_STATES: Mutex<HashMap<OID, StateX>> = <_>::default();
    static ref ACTION_QUEUES: OnceCell<HashMap<OID, async_channel::Sender<Action>>> =
        <_>::default();
    static ref ACTT: OnceCell<Actt> = <_>::default();
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
    topic_broker: pubsub::TopicBroker,
    tx: async_channel::Sender<(String, Vec<u8>)>,
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "rule.list" => {
                if payload.is_empty() {
                    let mut rule_infos: Vec<rule::Info> =
                        RULES.get().unwrap().iter().map(|(_, v)| v.info()).collect();
                    rule_infos.sort();
                    Ok(Some(pack(&rule_infos)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "rule.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(rule) = RULES.get().unwrap().get(p.i) {
                        Ok(Some(pack(&rule.info())?))
                    } else {
                        Err(Error::not_found("rule not found").into())
                    }
                }
            }
            "job.list" => {
                if payload.is_empty() {
                    let mut job_infos: Vec<job::Info> =
                        JOBS.get().unwrap().iter().map(|(_, v)| v.info()).collect();
                    job_infos.sort();
                    Ok(Some(pack(&job_infos)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "job.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(job) = JOBS.get().unwrap().get(p.i) {
                        Ok(Some(pack(&job.info())?))
                    } else {
                        Err(Error::not_found("job not found").into())
                    }
                }
            }
            "cycle.list" => {
                if payload.is_empty() {
                    let mut cycles: Vec<&Cycle> = CYCLES
                        .get()
                        .unwrap()
                        .iter()
                        .map(|(_, v)| v.as_ref())
                        .collect();
                    cycles.sort();
                    Ok(Some(pack(&cycles)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "cycle.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(cycle) = CYCLES.get().unwrap().get(p.i) {
                        Ok(Some(pack(cycle)?))
                    } else {
                        Err(Error::not_found("cycle not found").into())
                    }
                }
            }
            "cycle.start" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(cycle) = CYCLES.get().unwrap().get(p.i) {
                        cycle::start(cycle.clone())?;
                        Ok(None)
                    } else {
                        Err(Error::not_found("cycle not found").into())
                    }
                }
            }
            "cycle.stop" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(cycle) = CYCLES.get().unwrap().get(p.i) {
                        cycle::stop(cycle);
                        Ok(None)
                    } else {
                        Err(Error::not_found("cycle not found").into())
                    }
                }
            }
            "cycle.reset" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(cycle) = CYCLES.get().unwrap().get(p.i) {
                        cycle.reset();
                        Ok(None)
                    } else {
                        Err(Error::not_found("cycle not found").into())
                    }
                }
            }
            "action" => {
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let action: Action = unpack(payload)?;
                let actt = crate::ACTT.get().unwrap();
                let action_topic = format_action_topic(action.oid());
                let payload_pending = pack(&action.event_pending())?;
                let action_uuid = *action.uuid();
                let action_oid = action.oid().clone();
                if let Some(tx) = crate::ACTION_QUEUES.get().unwrap().get(action.oid()) {
                    actt.append(action.oid(), action_uuid)?;
                    if let Err(e) = tx.send(action).await {
                        actt.remove(&action_oid, &action_uuid)?;
                        Err(Error::core(format!("action queue broken: {}", e)).into())
                    } else if let Err(e) = self
                        .tx
                        .send((action_topic, payload_pending))
                        .await
                        .map_err(Error::io)
                    {
                        actt.remove(&action_oid, &action_uuid)?;
                        Err(e.into())
                    } else {
                        Ok(None)
                    }
                } else {
                    Err(Error::not_found(format!("{} has no action map", action.oid())).into())
                }
            }
            "terminate" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsUuid = unpack(payload)?;
                    if crate::ACTT.get().unwrap().mark_terminated(&p.u).is_err() {
                        opener::terminate(p.u).await?;
                    }
                    Ok(None)
                }
            }
            "kill" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsOID = unpack(payload)?;
                    let _r = crate::ACTT.get().unwrap().mark_killed(&p.i);
                    if let Err(e) = opener::kill(&p.i).await {
                        if e.kind() != ErrorKind::ResourceNotFound {
                            return Err(e.into());
                        }
                    }
                    Ok(None)
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
        if frame.kind() == busrt::FrameKind::Publish {
            self.topic_broker.process(frame).await.log_ef();
        }
    }
}

async fn state_handler(rx: async_channel::Receiver<pubsub::Publication>) {
    while let Ok(frame) = rx.recv().await {
        process_state(frame.topic(), frame.subtopic(), frame.payload())
            .await
            .log_ef();
    }
}

async fn process_state(topic: &str, path: &str, payload: &[u8]) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<StateX>(payload) {
            Ok(state) => {
                let mut rules: Vec<Arc<Rule>> = RULE_MATRIX
                    .get()
                    .unwrap()
                    .get_subscribers(path)
                    .into_iter()
                    .collect();
                rules.sort_by(|a, b| a.priority.cmp(&b.priority));
                let mut prev_states = PREV_STATES.lock().await;
                let prev = prev_states.get(&oid);
                for rule in rules {
                    match rule.process(&oid, &state, prev).await {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(e) => error!("{}", e),
                    }
                }
                prev_states.insert(oid, state);
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    rules: Vec<Rule>,
    #[serde(default)]
    cycles: Vec<Cycle>,
    #[serde(default)]
    jobs: Vec<Job>,
    #[serde(default)]
    openers: Vec<Opener>,
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let timeout = initial.timeout();
    let startup_timeout = initial.startup_timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    let mut rule_matrix: SubMap<Arc<Rule>> =
        SubMap::new().separator('/').match_any("+").wildcard("#");
    let mut rules = HashMap::new();
    let mut rule_oids = HashSet::new();
    for (i, mut rule) in config.rules.into_iter().enumerate() {
        rule.init();
        rule.set_priority(i);
        let rule_c = Arc::new(rule);
        rules.insert(rule_c.id().to_owned(), rule_c.clone());
        rule_matrix.register_client(&rule_c);
        rule_matrix.subscribe(&rule_c.oid().as_path(), &rule_c);
        rule_oids.insert(rule_c.oid().clone());
    }
    let rule_mask_list = OIDMaskList::new(rule_oids);
    RULES
        .set(rules)
        .map_err(|_| Error::core("Unable to set RULES"))?;
    RULE_MATRIX
        .set(rule_matrix)
        .map_err(|_| Error::core("Unable to set RULE_MATRIX"))?;
    let mut cycles = HashMap::new();
    for cycle in config.cycles {
        cycles.insert(cycle.id().to_owned(), Arc::new(cycle));
    }
    CYCLES
        .set(cycles)
        .map_err(|_| Error::core("Unable to set CYCLES"))?;
    let mut jobs = HashMap::new();
    for job in config.jobs {
        jobs.insert(job.id().to_owned(), Arc::new(job));
    }
    JOBS.set(jobs)
        .map_err(|_| Error::core("Unable to set JOBS"))?;
    let action_oids = config.openers.iter().map(|v| &v.oid).collect::<Vec<&OID>>();
    ACTT.set(Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("rule.list"));
    info.add_method(ServiceMethod::new("rule.get").required("i"));
    info.add_method(ServiceMethod::new("job.list"));
    info.add_method(ServiceMethod::new("job.get").required("i"));
    info.add_method(ServiceMethod::new("cycle.list"));
    info.add_method(ServiceMethod::new("cycle.get").required("i"));
    info.add_method(ServiceMethod::new("cycle.start").required("i"));
    info.add_method(ServiceMethod::new("cycle.stop").required("i"));
    info.add_method(ServiceMethod::new("cycle.reset").required("i"));
    let mut topic_broker: pubsub::TopicBroker = <_>::default();
    let queue_size = initial.bus_queue_size();
    let (tx, rx) = topic_broker.register_prefix(LOCAL_STATE_TOPIC, queue_size)?;
    topic_broker.register_prefix_tx(REMOTE_STATE_TOPIC, tx)?;
    tokio::spawn(async move {
        state_handler(rx).await;
    });
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(QUEUE_SIZE);
    let handlers = Handlers {
        info,
        topic_broker,
        tx: tx.clone(),
    };
    let (rpc, rpc_secondary) = initial.init_rpc_blocking_with_secondary(handlers).await?;
    eva_sdk::service::subscribe_oids(
        rpc.as_ref(),
        &rule_mask_list,
        eva_sdk::service::EventKind::Actual,
    )
    .await?;
    RPC.set(rpc_secondary.clone())
        .map_err(|_| Error::core("Unable to set RPC"))?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
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
    let mut opener_action_queues: HashMap<OID, async_channel::Sender<Action>> = HashMap::new();
    for cfg in config.openers {
        let (atx, arx) = async_channel::bounded::<Action>(QUEUE_SIZE);
        debug!("starting action queue for {}", cfg.oid);
        opener_action_queues.insert(cfg.oid.clone(), atx);
        opener::start_action_handler(cfg, arx, tx.clone());
    }
    ACTION_QUEUES
        .set(opener_action_queues)
        .map_err(|_| Error::core("Unable to set ACTION_QUEUES"))?;
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    let rpc_c = rpc.clone();
    tokio::spawn(async move {
        let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
        for c in CYCLES.get().unwrap().values() {
            if c.auto_start() {
                cycle::start(c.clone()).log_ef();
            }
        }
        for j in JOBS.get().unwrap().values() {
            tokio::spawn(async move {
                j.scheduler().await.log_ef();
                j.clear_next_launch();
            });
        }
    });
    svc_block2(&rpc, &rpc_secondary).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
