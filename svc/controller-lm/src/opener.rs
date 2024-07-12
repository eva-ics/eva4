use eva_common::common_payloads::{ParamsId, ParamsUuid};
use eva_common::events::{RawStateEventOwned, RAW_STATE_TOPIC};
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, Action};
use eva_sdk::prelude::*;
use lazy_static::lazy_static;
use log::error;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

lazy_static! {
    static ref ACTIVE: Mutex<ActiveOpeners> = <_>::default();
}

#[derive(Default)]
struct ActiveOpeners {
    by_oid: HashMap<OID, Uuid>,
    by_action_uuid: HashMap<Uuid, Uuid>,
}

impl ActiveOpeners {
    fn register(&mut self, oid: OID, action_uuid: Uuid, seq_uuid: Uuid) {
        self.by_oid.insert(oid, seq_uuid);
        self.by_action_uuid.insert(action_uuid, seq_uuid);
    }
    fn unregister(&mut self, oid: &OID, action_uuid: &Uuid) {
        self.by_oid.remove(oid);
        self.by_action_uuid.remove(action_uuid);
    }
}

#[derive(Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Logic {
    Ac,
    Rdc,
}

impl Default for Logic {
    #[inline]
    fn default() -> Self {
        Self::Ac
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    PortOn,
    PortOff,
    DPortOn,
    DPortOff,
    Delay(Duration),
}

#[derive(Debug)]
struct Plan {
    seq: Vec<Command>,
}

impl std::ops::Add for Plan {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        let mut seq = self.seq;
        seq.extend(other.seq);
        Self { seq }
    }
}

impl Plan {
    #[allow(clippy::too_many_lines)]
    fn generate(
        config: &PlanConfig,
        current_pos: i8,
        requested_pos: i8,
        break_on_status_error: bool,
    ) -> EResult<Self> {
        let end_pos: i8 = config.steps.len().try_into().map_err(Error::failed)?;
        if requested_pos < 0 || current_pos < 0 || current_pos > end_pos || requested_pos > end_pos
        {
            return Err(Error::failed("step out of bounds"));
        }
        if current_pos < 0 && break_on_status_error {
            return Err(Error::failed(
                "current step is unknown (the unit has error status)",
            ));
        }
        let mut seq = Vec::new();
        macro_rules! warmup {
            ($w:expr) => {
                if let Some(warmup) = $w {
                    warmup
                } else {
                    Duration::from_secs(0)
                }
            };
        }
        macro_rules! tune {
            ($delay: expr) => {
                if let Some(tuning) = config.tuning {
                    $delay += tuning;
                }
            };
        }
        let steps = config
            .steps
            .iter()
            .map(|d| Duration::from_secs_f64(*d))
            .collect::<Vec<Duration>>();
        match current_pos.cmp(&requested_pos) {
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Less => {
                // going up
                if let Some(te) = config.te {
                    if requested_pos >= te && requested_pos != end_pos {
                        let plan_te =
                            Plan::generate(config, current_pos, end_pos, break_on_status_error)?;
                        let plan_tgt =
                            Plan::generate(config, end_pos, requested_pos, break_on_status_error)?;
                        return Ok(plan_te + plan_tgt);
                    }
                }
                match config.logic {
                    Logic::Ac => {
                        seq.push(Command::DPortOn);
                        seq.push(Command::PortOn);
                    }
                    Logic::Rdc => {
                        seq.push(Command::DPortOff);
                        seq.push(Command::PortOn);
                    }
                }
                let mut delay = warmup!(config.warmup_open);
                delay += steps
                    .iter()
                    .skip(current_pos.try_into().map_err(Error::failed)?)
                    .take(
                        (requested_pos - current_pos)
                            .try_into()
                            .map_err(Error::failed)?,
                    )
                    .sum::<Duration>();
                if requested_pos == end_pos {
                    tune!(delay);
                }
                seq.push(Command::Delay(delay));
                seq.push(Command::PortOff);
            }
            std::cmp::Ordering::Greater => {
                // going down
                if let Some(ts) = config.ts {
                    if requested_pos <= ts && requested_pos != 0 {
                        let plan_ts =
                            Plan::generate(config, current_pos, 0, break_on_status_error)?;
                        let plan_tgt =
                            Plan::generate(config, 0, requested_pos, break_on_status_error)?;
                        return Ok(plan_ts + plan_tgt);
                    }
                }
                match config.logic {
                    Logic::Ac => {
                        seq.push(Command::DPortOff);
                        seq.push(Command::PortOn);
                    }
                    Logic::Rdc => {
                        seq.push(Command::PortOff);
                        seq.push(Command::DPortOn);
                    }
                }
                let mut delay = warmup!(config.warmup_close);
                delay += steps
                    .iter()
                    .rev()
                    .skip((end_pos - current_pos).try_into().map_err(Error::failed)?)
                    .take(
                        (current_pos - requested_pos)
                            .try_into()
                            .map_err(Error::failed)?,
                    )
                    .sum::<Duration>();
                if requested_pos == 0 {
                    tune!(delay);
                }
                seq.push(Command::Delay(delay));
                match config.logic {
                    Logic::Ac => {
                        seq.push(Command::PortOff);
                    }
                    Logic::Rdc => {
                        seq.push(Command::DPortOff);
                    }
                }
            }
        }
        Ok(Self { seq })
    }
}

#[derive(Deserialize)]
pub struct Opener {
    pub(crate) oid: OID,
    #[serde(default)]
    break_on_status_error: bool,
    port: Vec<OID>,
    dport: Vec<OID>,
    #[serde(flatten)]
    plan_config: PlanConfig,
    set_state: bool,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    port_timeout: Option<Duration>,
}

#[derive(Deserialize)]
struct PlanConfig {
    #[serde(default)]
    logic: Logic,
    steps: Vec<f64>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    warmup_open: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    warmup_close: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    tuning: Option<Duration>,
    ts: Option<i8>,
    te: Option<i8>,
}

async fn execute(
    action_uuid: Uuid,
    config: &Opener,
    pos: Value,
    current_pos: i8,
    timeout: Duration,
) -> EResult<()> {
    let port_timeout = config
        .port_timeout
        .unwrap_or_else(|| *crate::TIMEOUT.get().unwrap());
    let plan = Plan::generate(
        &config.plan_config,
        current_pos,
        pos.try_into()?,
        config.break_on_status_error,
    )?;
    let mut seq = eva_robots::Sequence::new(timeout);
    let seq_uuid = seq.uuid();
    macro_rules! actions {
        ($src: expr, $value: expr) => {
            $src.iter()
                .map(|oid| eva_robots::SequenceAction::new_unit(oid, $value, port_timeout))
                .collect::<Vec<eva_robots::SequenceAction>>()
        };
    }
    for s in plan.seq {
        match s {
            Command::PortOn => {
                seq.push_actions_multi(actions!(config.port, Value::I8(1)));
            }
            Command::PortOff => {
                seq.push_actions_multi(actions!(config.port, Value::I8(0)));
            }
            Command::DPortOn => {
                seq.push_actions_multi(actions!(config.dport, Value::I8(1)));
            }
            Command::DPortOff => {
                seq.push_actions_multi(actions!(config.dport, Value::I8(0)));
            }
            Command::Delay(d) => seq.push_delay(d),
        }
    }
    let mut abort_actions = actions!(config.port, Value::U8(0));
    if config.plan_config.logic == Logic::Rdc {
        abort_actions.append(&mut actions!(config.dport, Value::U8(0)));
    }
    seq.set_on_abort_multi(abort_actions);
    let rpc = crate::RPC.get().unwrap();
    ACTIVE
        .lock()
        .unwrap()
        .register(config.oid.clone(), action_uuid, seq_uuid);
    let res = safe_rpc_call(
        rpc,
        "eva.core",
        "seq",
        pack(&seq)?.into(),
        QoS::Processed,
        timeout,
    )
    .await;
    ACTIVE.lock().unwrap().unregister(&config.oid, &action_uuid);
    res?;
    Ok(())
}

async fn get_current_pos(oid: &OID) -> EResult<i8> {
    #[derive(Deserialize)]
    struct St {
        value: Value,
    }
    let rpc = crate::RPC.get().unwrap();
    let res = safe_rpc_call(
        rpc,
        "eva.core",
        "item.state",
        pack(&ParamsId { i: oid.as_str() })?.into(),
        QoS::Processed,
        *crate::TIMEOUT.get().unwrap(),
    )
    .await?;
    let result: Vec<St> = unpack(res.payload())?;
    result
        .into_iter()
        .next()
        .ok_or_else(|| Error::failed("unit not found"))?
        .value
        .try_into()
}

async fn run_action(u: Uuid, cfg: &Opener, pos: Value, timeout: Duration) -> EResult<()> {
    let current_pos = get_current_pos(&cfg.oid).await.unwrap_or(-1);
    execute(u, cfg, pos, current_pos, timeout).await
}

async fn handle_action(
    mut action: Action,
    tx: async_channel::Sender<(String, Vec<u8>)>,
    cfg: &Opener,
    action_topic: &str,
    state_topic: &str,
) -> EResult<()> {
    if !crate::ACTT
        .get()
        .unwrap()
        .remove(action.oid(), action.uuid())?
    {
        let payload_canceled = pack(&action.event_canceled())?;
        tx.send((action_topic.to_owned(), payload_canceled))
            .await
            .map_err(Error::io)?;
        return Ok(());
    }
    let payload_running = pack(&action.event_running())?;
    tx.send((action_topic.to_owned(), payload_running))
        .await
        .map_err(Error::io)?;
    let (payload, state_payload) = if let Ok(params) = action.take_unit_params() {
        match run_action(*action.uuid(), cfg, params.value.clone(), action.timeout()).await {
            Ok(()) => (
                action.event_completed(None),
                if cfg.set_state {
                    Some(RawStateEventOwned {
                        status: 1,
                        value: ValueOptionOwned::Value(params.value),
                        ..RawStateEventOwned::default()
                    })
                } else {
                    None
                },
            ),
            Err(e) if e.kind() == ErrorKind::Aborted => (action.event_terminated(), None),
            Err(e) => (
                action.event_failed(
                    1,
                    None,
                    Some(Value::String(e.message().unwrap_or_default().to_owned())),
                ),
                None,
            ),
        }
    } else {
        (
            action.event_failed(-1, None, Some(to_value("invalid action payload")?)),
            None,
        )
    };
    if let Some(p) = state_payload {
        tx.send((state_topic.to_owned(), pack(&p)?))
            .await
            .map_err(Error::io)?;
    }
    tx.send((action_topic.to_owned(), pack(&payload)?))
        .await
        .map_err(Error::io)?;
    Ok(())
}

pub fn start_action_handler(
    cfg: Opener,
    receiver: async_channel::Receiver<Action>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
) {
    let action_topic = format_action_topic(&cfg.oid);
    let state_topic = format!("{}{}", RAW_STATE_TOPIC, cfg.oid.as_path());
    tokio::spawn(async move {
        while let Ok(action) = receiver.recv().await {
            if let Err(e) =
                handle_action(action, tx.clone(), &cfg, &action_topic, &state_topic).await
            {
                error!("action error: {}", e);
            }
        }
    });
}

async fn terminate_seq(u: Uuid) -> EResult<()> {
    let rpc = crate::RPC.get().unwrap();
    let timeout = *crate::TIMEOUT.get().unwrap();
    safe_rpc_call(
        rpc,
        "eva.core",
        "seq.terminate",
        pack(&ParamsUuid { u })?.into(),
        QoS::Processed,
        timeout,
    )
    .await?;
    Ok(())
}

pub async fn terminate(u: Uuid) -> EResult<()> {
    let seq_uuid = ACTIVE
        .lock()
        .unwrap()
        .by_action_uuid
        .get(&u)
        .map(Clone::clone);
    if let Some(u) = seq_uuid {
        terminate_seq(u).await
    } else {
        Err(Error::not_found("action not active"))
    }
}

pub async fn kill(oid: &OID) -> EResult<()> {
    let seq_uuid = ACTIVE.lock().unwrap().by_oid.get(oid).map(Clone::clone);
    if let Some(u) = seq_uuid {
        terminate_seq(u).await
    } else {
        Err(Error::not_found("action not active"))
    }
}

#[cfg(test)]
mod test {
    use super::{Command, Logic, Plan, PlanConfig};
    use std::time::Duration;

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_up() {
        let warmup = Duration::from_millis(300);
        let tuning = Duration::from_millis(100);
        let delay1 = Duration::from_secs(1);
        let delay2 = Duration::from_secs(2);
        let delay3 = Duration::from_secs(3);
        // 2-position opener
        let mut config = PlanConfig {
            logic: Logic::Ac,
            steps: vec![delay1.as_secs_f64()],
            warmup_open: None,
            warmup_close: None,
            tuning: None,
            ts: None,
            te: None,
        };
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq.as_slice(),
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff
            ]
        );
        assert!(Plan::generate(&config, 0, 2, true).is_err());
        config.logic = Logic::Rdc;
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff
            ]
        );
        // 3-position opener
        let mut config = PlanConfig {
            logic: Logic::Ac,
            steps: vec![delay1.as_secs_f64(), delay2.as_secs_f64()],
            warmup_open: None,
            warmup_close: None,
            tuning: None,
            ts: None,
            te: None,
        };
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 0, 2, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 1, 2, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay2),
                Command::PortOff
            ]
        );
        config.te = Some(1);
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2),
                Command::PortOff,
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay2),
                Command::PortOff,
            ]
        );
        config.logic = Logic::Rdc;
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2),
                Command::PortOff,
                Command::PortOff,
                Command::DPortOn,
                Command::Delay(delay2),
                Command::DPortOff,
            ]
        );
        // 4 - position opener
        let mut config = PlanConfig {
            logic: Logic::Ac,
            steps: vec![
                delay1.as_secs_f64(),
                delay2.as_secs_f64(),
                delay3.as_secs_f64(),
            ],
            warmup_open: Some(warmup),
            warmup_close: None,
            tuning: Some(tuning),
            ts: None,
            te: None,
        };
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + warmup),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 0, 2, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + warmup),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 0, 3, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 1, 3, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay2 + delay3 + warmup + tuning),
                Command::PortOff
            ]
        );
        config.te = Some(1);
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff,
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay2 + delay3),
                Command::PortOff,
            ]
        );
        config.te = Some(2);
        let plan = Plan::generate(&config, 0, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + warmup),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 0, 2, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff,
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay3),
                Command::PortOff,
            ]
        );
        let plan = Plan::generate(&config, 0, 3, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff,
            ]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_down() {
        let warmup = Duration::from_millis(300);
        let tuning = Duration::from_millis(100);
        let delay1 = Duration::from_secs(1);
        let delay2 = Duration::from_secs(2);
        let delay3 = Duration::from_secs(3);
        // 2-position opener
        let mut config = PlanConfig {
            logic: Logic::Ac,
            steps: vec![delay1.as_secs_f64()],
            warmup_open: None,
            warmup_close: None,
            tuning: None,
            ts: None,
            te: None,
        };
        let plan = Plan::generate(&config, 1, 0, true).unwrap();
        assert_eq!(
            &plan.seq.as_slice(),
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff
            ]
        );
        assert!(Plan::generate(&config, 2, 0, true).is_err());
        config.logic = Logic::Rdc;
        let plan = Plan::generate(&config, 1, 0, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::PortOff,
                Command::DPortOn,
                Command::Delay(delay1),
                Command::DPortOff
            ]
        );
        // 3-position opener
        let mut config = PlanConfig {
            logic: Logic::Ac,
            steps: vec![delay1.as_secs_f64(), delay2.as_secs_f64()],
            warmup_open: None,
            warmup_close: None,
            tuning: None,
            ts: None,
            te: None,
        };
        let plan = Plan::generate(&config, 1, 0, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 2, 0, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 2, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay2),
                Command::PortOff
            ]
        );
        config.ts = Some(1);
        let plan = Plan::generate(&config, 2, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2),
                Command::PortOff,
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff,
            ]
        );
        config.logic = Logic::Rdc;
        let plan = Plan::generate(&config, 2, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::PortOff,
                Command::DPortOn,
                Command::Delay(delay1 + delay2),
                Command::DPortOff,
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff,
            ]
        );
        // 4 - position opener
        let mut config = PlanConfig {
            logic: Logic::Ac,
            steps: vec![
                delay1.as_secs_f64(),
                delay2.as_secs_f64(),
                delay3.as_secs_f64(),
            ],
            warmup_open: None,
            warmup_close: Some(warmup),
            tuning: Some(tuning),
            ts: None,
            te: None,
        };
        let plan = Plan::generate(&config, 3, 2, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay3 + warmup),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 3, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay2 + delay3 + warmup),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 3, 0, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff
            ]
        );
        let plan = Plan::generate(&config, 3, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay2 + delay3 + warmup),
                Command::PortOff
            ]
        );
        config.ts = Some(1);
        let plan = Plan::generate(&config, 3, 1, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff,
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1),
                Command::PortOff,
            ]
        );
        config.ts = Some(2);
        let plan = Plan::generate(&config, 3, 2, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff,
                Command::DPortOn,
                Command::PortOn,
                Command::Delay(delay1 + delay2),
                Command::PortOff,
            ]
        );
        let plan = Plan::generate(&config, 3, 0, true).unwrap();
        assert_eq!(
            &plan.seq,
            &[
                Command::DPortOff,
                Command::PortOn,
                Command::Delay(delay1 + delay2 + delay3 + warmup + tuning),
                Command::PortOff,
            ]
        );
    }
}
