/// Contains the action manager and its types
use crate::{EResult, Error};
use busrt::rpc::{Rpc, RpcClient};
use busrt::QoS;
use eva_common::acl::OIDMask;
use eva_common::actions::{ActionEvent, Params, ParamsView, Status, ACTION_COMPLETED};
use eva_common::err_logger;
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use eva_common::time;
use log::error;
use log::trace;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

err_logger!();

const INTERVAL_CLEAN_ACTIONS: Duration = std::time::Duration::from_secs(60);

pub const DEFAULT_KEEP: Duration = Duration::from_secs(600);

pub const ERR_NO_UUID: &str = "invalid local action: no uuid";

pub const DEFAULT_FILTER_LIMIT: usize = 50;

/// Action status type for filtered list queries
///
/// Some statuses are combined
#[derive(Deserialize, Debug, Copy, Clone)]
#[serde(rename_all = "lowercase")]
pub enum StatusQuery {
    Waiting,   // not running yet (created, accepted, pending)
    Running,   // currently running (running)
    Completed, // completed
    Failed,    // failed, canceled, terminated
    Finished,  // all finished (completed and failed)
}

impl StatusQuery {
    fn matches(self, s: Status) -> bool {
        match self {
            StatusQuery::Waiting => s < Status::Running,
            StatusQuery::Running => s == Status::Running,
            StatusQuery::Completed => s == Status::Completed,
            StatusQuery::Failed => s >= Status::Failed,
            StatusQuery::Finished => s >= Status::Completed,
        }
    }
}

struct RemoteActionData {
    node: String,
    svc: String,
    oid: OID,
    started: Instant,
}

#[derive(Serialize)]
struct ActionResultParams<'a> {
    u: &'a Uuid,
    node: &'a str,
    // DEPRECATED: remove at v3 EOL (v3 compat helper param)
    i: &'a OID,
}

/// action filter for list queries
#[derive(Debug, Deserialize)]
pub struct Filter {
    i: Option<OIDMask>,
    sq: Option<StatusQuery>,
    svc: Option<String>,
    time: Option<u32>,
    limit: Option<usize>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            i: None,
            sq: None,
            svc: None,
            time: None,
            limit: Some(DEFAULT_FILTER_LIMIT),
        }
    }
}

impl Filter {
    #[inline]
    fn matches(&self, action: &Action, ts_now: f64) -> bool {
        if let Some(ref mask) = self.i {
            if !mask.matches(&action.i) {
                return false;
            }
        }
        if let Some(sq) = self.sq {
            if !sq.matches(action.status) {
                return false;
            }
        }
        if let Some(ref svc) = self.svc {
            if svc != &action.target {
                return false;
            }
        }
        if let Some(time) = self.time {
            if let Some(created) = action.time.get(&Status::Created) {
                if created < &(ts_now - f64::from(time)) {
                    return false;
                }
            }
        }
        true
    }
}

/// The primary action structure
///
/// Stored in full for action history, partially serialized for service action requests
#[derive(Serialize)]
pub struct Action {
    #[serde(skip_serializing_if = "Option::is_none")]
    // can be None for remote actions only
    uuid: Option<Uuid>,
    i: OID,
    #[serde(skip)]
    status: Status,
    #[serde(skip)]
    started: Instant,
    #[serde(skip)]
    time: HashMap<Status, f64>,
    #[serde(
        serialize_with = "eva_common::tools::serialize_opt_duration_as_micros",
        skip_serializing_if = "Option::is_none"
    )]
    timeout: Option<Duration>,
    priority: u8,
    params: Params,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    #[serde(skip)]
    out: Option<Value>,
    #[serde(skip)]
    err: Option<Value>,
    #[serde(skip)]
    exitcode: Option<i16>,
    #[serde(skip)]
    completed: triggered::Trigger,
    #[serde(skip)]
    core_completed: triggered::Trigger,
    #[serde(skip)]
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    // sent to replication services only
    wait: Option<f64>,
}

/// Action info structure
///
/// Returned for clients when an action is started or the result is requested
#[derive(Serialize, bmart::tools::Sorting)]
#[sorting(id = "started")]
pub struct ActionInfo<'a> {
    uuid: Option<&'a Uuid>,
    oid: &'a OID,
    status: Status,
    time: &'a HashMap<Status, f64>,
    priority: u8,
    params: ParamsView<'a>,
    out: Option<&'a Value>,
    err: Option<&'a Value>,
    exitcode: Option<i16>,
    node: &'a str,
    svc: &'a str,
    finished: bool,
    #[serde(skip)]
    started: Instant,
}

impl<'a> ActionInfo<'a> {
    fn from_action(action: &'a Action, system_name: &'a str) -> Self {
        Self {
            uuid: action.uuid.as_ref(),
            oid: &action.i,
            status: action.status,
            time: &action.time,
            priority: action.priority,
            params: action.params.as_view(),
            out: action.out.as_ref(),
            err: action.err.as_ref(),
            exitcode: action.exitcode,
            node: action.node.as_ref().map_or(system_name, String::as_str),
            svc: &action.target,
            started: action.started,
            finished: action.status >= Status::Completed,
        }
    }
}

fn log_incompleted(
    uuid: &Uuid,
    oid: &OID,
    status: Status,
    err: Option<&Value>,
    exitcode: Option<i16>,
) {
    let mut msg = format!(
        "action {} for {} is not finished successfully ({:?})",
        uuid, oid, status
    );
    if let Some(c) = exitcode {
        let _r = write!(msg, ". Exit code: {}", c);
    }
    if let Some(e) = err {
        let _r = write!(msg, ". Err: {}", e);
    }
    error!("{}", msg);
}

// returned from replication svcs
#[derive(Serialize, Deserialize)]
pub struct ActionInfoOwned {
    pub uuid: Uuid,
    oid: OID,
    status: Status,
    time: HashMap<Status, f64>,
    priority: u8,
    params: Params,
    out: Option<Value>,
    err: Option<Value>,
    exitcode: Option<i16>,
    node: String,
    #[serde(default)]
    pub finished: bool,
    #[serde(skip_deserializing)]
    // replaced to local replication svc
    svc: String,
}

impl ActionInfoOwned {
    pub fn check_completed(&self) -> bool {
        if self.status == Status::Completed {
            true
        } else {
            log_incompleted(
                &self.uuid,
                &self.oid,
                self.status,
                self.err.as_ref(),
                self.exitcode,
            );
            false
        }
    }
}

pub struct ActionArgs<'a> {
    pub uuid: Option<Uuid>,
    pub oid: &'a OID,
    pub params: Params,
    pub timeout: Option<Duration>,
    pub priority: u8,
    pub config: Option<Value>,
    pub node: Option<String>,
    pub target: String,
    pub wait: Option<Duration>,
}

impl Action {
    pub fn create(args: ActionArgs) -> (Self, triggered::Listener, triggered::Listener) {
        let (trigger, listener) = triggered::trigger();
        let (core_trigger, core_listener) = triggered::trigger();
        let mut times = HashMap::new();
        let remote = args.node.is_some();
        times.insert(Status::Created, time::Time::now().timestamp());
        (
            Self {
                uuid: if args.uuid.is_some() {
                    args.uuid
                } else if remote {
                    None
                } else {
                    Some(Uuid::new_v4())
                },
                i: args.oid.clone(),
                status: Status::Created,
                started: Instant::now(),
                time: times,
                timeout: args.timeout,
                priority: args.priority,
                params: args.params,
                config: args.config,
                out: None,
                err: None,
                exitcode: None,
                completed: trigger,
                core_completed: core_trigger,
                node: args.node,
                target: args.target,
                wait: if remote {
                    args.wait.map(|v| v.as_secs_f64())
                } else {
                    None
                },
            },
            listener,
            core_listener,
        )
    }
    #[inline]
    pub fn uuid(&self) -> Option<&Uuid> {
        self.uuid.as_ref()
    }
    #[inline]
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
    #[inline]
    pub fn node(&self) -> Option<&str> {
        self.node.as_deref()
    }
    fn mark_failed(&mut self, e: Error) {
        if self.status < Status::Completed {
            error!(
                "action {} for {} failed at launch: {}",
                self.uuid.unwrap_or_default(),
                self.i,
                e
            );
            self.status = Status::Failed;
            self.err = Some(e.to_string().into());
            self.time
                .insert(Status::Failed, time::Time::now().timestamp());
            self.completed.trigger();
            self.core_completed.trigger();
        }
    }
    #[inline]
    fn mark_accepted(&mut self) {
        if self.status < Status::Accepted {
            self.status = Status::Accepted;
        }
        self.time
            .insert(Status::Accepted, time::Time::now().timestamp());
    }
}

/// The action manager
pub struct Manager {
    actions: Arc<Mutex<HashMap<Uuid, Action>>>,
    remote_actions: Arc<Mutex<HashMap<Uuid, Arc<RemoteActionData>>>>,
    keep_for: Mutex<Duration>,
    rpc: OnceCell<Arc<RpcClient>>,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            actions: <_>::default(),
            remote_actions: <_>::default(),
            keep_for: Mutex::new(DEFAULT_KEEP),
            rpc: <_>::default(),
        }
    }
}

impl Manager {
    #[inline]
    pub fn set_rpc(&self, rpc: Arc<RpcClient>) -> EResult<()> {
        self.rpc
            .set(rpc)
            .map_err(|_| Error::core("unable to set RPC"))
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn set_keep_for(&self, keep_for: Duration) {
        *self.keep_for.lock().unwrap() = keep_for;
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned
    pub fn process_event(&self, event: ActionEvent) -> EResult<()> {
        let mut actions = self.actions.lock().unwrap();
        if let Some(action) = actions.get_mut(&event.uuid) {
            let status: Status = event.status.try_into()?;
            if (action.status as u8) < event.status {
                let need_trigger =
                    event.status >= ACTION_COMPLETED && (action.status as u8) < ACTION_COMPLETED;
                action.status = status;
                action.out = event.out;
                action.err = event.err;
                action.exitcode = event.exitcode;
                if need_trigger {
                    action.completed.trigger();
                    action.core_completed.trigger();
                }
            }
            action.time.insert(status, time::Time::now().timestamp());
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned or rpc is not set
    pub async fn terminate_action(&self, uuid: &Uuid, timeout: Duration) -> EResult<()> {
        #[derive(Serialize)]
        struct ParamsUuid<'a> {
            u: &'a Uuid,
        }
        let (target, payload) = {
            let actions = self.actions.lock().unwrap();
            let action = actions
                .get(uuid)
                .ok_or_else(|| Error::not_found("action not found"))?;
            if action.status >= Status::Completed {
                return Err(Error::failed("action is already finished"));
            }
            (action.target.clone(), pack(&ParamsUuid { u: uuid })?)
        };
        let rpc = self.rpc.get().unwrap();
        perform(&target, "terminate", &payload, timeout, rpc).await?;
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if rpc is not set
    pub async fn kill_actions(&self, oid: &OID, target: &str, timeout: Duration) -> EResult<()> {
        #[derive(Serialize)]
        struct ParamsId<'a> {
            i: &'a OID,
        }
        let rpc = self.rpc.get().unwrap();
        perform(target, "kill", &pack(&ParamsId { i: oid })?, timeout, rpc).await?;
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned
    ///
    /// For remote actions returns repl svc reply as-is
    pub async fn launch_action(
        &self,
        action: Action,
        accepted: Option<triggered::Trigger>,
        timeout: Duration,
    ) -> EResult<Option<ActionInfoOwned>> {
        let payload = pack(&action)?;
        let target = action.target.clone();
        let remote = action.node.is_some();
        let method = match action.params {
            Params::Unit(_) => "action",
            Params::Lmacro(_) => "run",
        };
        let node = action.node.clone();
        let r_oid = if remote { Some(action.i.clone()) } else { None };
        let uuid = if remote {
            None
        } else {
            let uuid = action.uuid.ok_or_else(|| Error::core(ERR_NO_UUID))?;
            self.actions.lock().unwrap().insert(uuid, action);
            Some(uuid)
        };
        // trigger that local action is accepted
        if let Some(a) = accepted {
            a.trigger();
        }
        // drop the lock until the launcher is done
        let rpc = self.rpc.get().unwrap();
        if remote {
            let started = Instant::now();
            // for remote actions - return the result from the repl svc
            let result = perform_remote(&target, method, &payload, timeout, rpc).await?;
            let uuid = result.uuid;
            self.remote_actions.lock().unwrap().insert(
                uuid,
                Arc::new(RemoteActionData {
                    node: node
                        .ok_or_else(|| Error::core("node not set while processing result"))?,
                    svc: target,
                    oid: r_oid.ok_or_else(|| Error::core("oid not set while processing result"))?,

                    started,
                }),
            );
            return Ok(Some(result));
        }
        let result = perform(&target, method, &payload, timeout, rpc).await;
        let mut actions = self.actions.lock().unwrap();
        let action = actions
            .get_mut(&uuid.unwrap())
            .ok_or_else(|| Error::core("action not stored"))?;
        if let Err(e) = result {
            action.mark_failed(e);
        } else {
            action.mark_accepted();
        }
        Ok(None)
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub async fn start(&self) -> EResult<()> {
        let keep_for = *self.keep_for.lock().unwrap();
        let actions = self.actions.clone();
        let remote_actions = self.remote_actions.clone();
        eva_common::cleaner!(
            "action_history",
            cleanup_action_history,
            INTERVAL_CLEAN_ACTIONS,
            keep_for,
            &actions,
            &remote_actions
        );
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned
    pub async fn must_be_completed(&self, uuid: &Uuid, timeout: Duration) -> bool {
        if let Some(action) = self.actions.lock().unwrap().get(uuid) {
            if action.status == Status::Completed {
                return true;
            }
            log_incompleted(
                uuid,
                &action.i,
                action.status,
                action.err.as_ref(),
                action.exitcode,
            );
            if action.status >= Status::Completed {
                return false;
            }
        } else {
            error!("action {} not found", uuid);
            return false;
        };
        self.terminate_action(uuid, timeout).await.log_ef();
        false
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned
    #[inline]
    pub async fn get_action_serialized(
        &self,
        uuid: &Uuid,
        system_name: &str,
        timeout: Duration,
    ) -> EResult<Option<Vec<u8>>> {
        if let Some(action) = self.actions.lock().unwrap().get(uuid) {
            return Ok(Some(pack(&ActionInfo::from_action(action, system_name))?));
        }
        let ra_data = if let Some(a) = self.remote_actions.lock().unwrap().get(uuid) {
            a.clone()
        } else {
            return Ok(None);
        };
        let rpc = self.rpc.get().unwrap();
        let info = perform_remote(
            &ra_data.svc,
            "action.result",
            &pack(&ActionResultParams {
                u: uuid,
                i: &ra_data.oid,
                node: &ra_data.node,
            })?,
            timeout,
            rpc,
        )
        .await?;
        Ok(Some(pack(&info)?))
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned
    ///
    #[inline]
    pub fn get_actions_filtered_serialized(
        &self,
        filter: &Filter,
        system_name: &str,
    ) -> EResult<Vec<u8>> {
        // Actions aren't wrapped to Arc, so they're locked until serialized, no actions can be
        // executed during this period. Consider making filters as light as possible
        let actions = self.actions.lock().unwrap();
        let ts_now = eva_common::time::now_ns_float();
        let act = actions
            .values()
            .filter(|a| filter.matches(a, ts_now))
            .collect::<Vec<&Action>>();
        let mut result: Vec<ActionInfo> = act
            .iter()
            .map(|a| ActionInfo::from_action(a, system_name))
            .collect();
        result.sort();
        let limit = filter.limit.unwrap_or(DEFAULT_FILTER_LIMIT);
        if result.len() > limit {
            result.drain(..result.len() - limit);
        }
        pack(&result)
    }
    /// # Panics
    ///
    /// Will panic if the actions mutex is poisoned
    #[inline]
    pub fn mark_action_timed_out(&self, uuid: &Uuid) {
        if let Some(action) = self.actions.lock().unwrap().get_mut(uuid) {
            action.mark_failed(Error::timeout());
        }
    }
}

#[inline]
async fn perform(
    target: &str,
    method: &str,
    payload: &[u8],
    timeout: Duration,
    rpc: &RpcClient,
) -> EResult<()> {
    tokio::time::timeout(
        timeout,
        rpc.call(target, method, payload.into(), QoS::RealtimeProcessed),
    )
    .await??;
    Ok(())
}

#[inline]
async fn perform_remote(
    target: &str,
    method: &str,
    payload: &[u8],
    timeout: Duration,
    rpc: &RpcClient,
) -> EResult<ActionInfoOwned> {
    let result = tokio::time::timeout(
        timeout,
        rpc.call(target, method, payload.into(), QoS::RealtimeProcessed),
    )
    .await??;
    let mut info: ActionInfoOwned = unpack(result.payload()).map_err(|e| {
        error!("invalid action info received from {}: {}", target, e);
        e
    })?;
    target.clone_into(&mut info.svc);
    Ok(info)
}

#[allow(clippy::unused_async)]
async fn cleanup_action_history(
    keep_for: Duration,
    actions: &Mutex<HashMap<Uuid, Action>>,
    remote_actions: &Mutex<HashMap<Uuid, Arc<RemoteActionData>>>,
) {
    trace!("cleaning actions");
    let t = Instant::now();
    actions
        .lock()
        .unwrap()
        .retain(|_, v| v.started + keep_for > t || v.status < Status::Completed);
    remote_actions
        .lock()
        .unwrap()
        .retain(|_, v| v.started + keep_for > t);
}
