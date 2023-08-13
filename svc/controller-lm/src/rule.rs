use crate::common::{safe_run_macro, ParamsCow, ParamsRun};
use crate::common::{Source, SourceOwned, StateX};
use eva_common::acl::OIDMask;
use eva_common::logic::Range;
use eva_common::prelude::*;
use eva_sdk::bitman::BitMan;
use eva_sdk::prelude::*;
use eva_sdk::types::StatePropExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DURATION_ZERO: Duration = Duration::from_secs(0);

err_logger!();

#[inline]
fn prop_value() -> StatePropExt {
    StatePropExt::Value
}

#[derive(Deserialize, Eq, PartialEq, Copy, Clone, Default)]
enum InitialKind {
    #[default]
    Process,
    Skip,
    Only,
}

#[derive(Default)]
struct Chillout {
    active: Option<Instant>,
    event: Option<SourceOwned>,
}

#[derive(Serialize, bmart::tools::Sorting)]
#[sorting(id = "priority")]
pub struct Info<'a> {
    id: &'a str,
    run: &'a OID,
    #[serde(serialize_with = "eva_common::tools::serialize_opt_duration_as_f64")]
    chillout_time: Option<Duration>,
    #[serde(serialize_with = "eva_common::tools::serialize_opt_duration_as_f64")]
    chillout_remaining: Option<Duration>,
    chillout_event_pending: bool,
    #[serde(skip)]
    priority: usize,
}

#[derive(Deserialize, bmart::tools::Sorting)]
#[serde(deny_unknown_fields)]
#[sorting(id = "priority")]
pub struct Rule {
    id: Arc<String>,
    #[serde(skip)]
    digest: submap::digest::Sha256Digest,
    oid: OIDMask,
    #[serde(default = "prop_value")]
    prop: StatePropExt,
    #[serde(default, deserialize_with = "eva_common::logic::de_range")]
    condition: Range,
    #[serde(default)]
    initial: InitialKind,
    #[serde(default)]
    block: bool,
    #[serde(default)]
    bit: Option<u32>,
    #[serde(default, rename = "break")]
    brk: bool,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    chillout_time: Option<Duration>,
    #[serde(skip)]
    chillout: Arc<Mutex<Chillout>>,
    run: Arc<OID>,
    args: Arc<Option<Vec<Value>>>,
    #[serde(default, skip_serializing)]
    kwargs: Arc<Option<HashMap<String, Value>>>,
    #[serde(default)]
    pub priority: usize,
}

impl Hash for Rule {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

async fn run(params: Vec<u8>, id: Arc<String>, timeout: Duration) {
    if let Err(e) = safe_run_macro(
        crate::RPC.get().unwrap(),
        ParamsCow::Packed(params),
        timeout,
    )
    .await
    {
        warn!("rule {} error: {}/{:?}", id, e.0, e.1);
    }
}

async fn process_chillout(
    id: Arc<String>,
    chillout: Arc<Mutex<Chillout>>,
    macro_oid: Arc<OID>,
    macro_args: Arc<Option<Vec<Value>>>,
    macro_kwargs: Arc<Option<HashMap<String, Value>>>,
    chillout_time: Duration,
    timeout: Duration,
) -> EResult<()> {
    tokio::time::sleep(chillout_time).await;
    let params = {
        let mut ch = chillout.lock().unwrap();
        ch.active.take();
        if let Some(event) = ch.event.take() {
            trace!("event happened during chillout");
            let mut kwargs = macro_kwargs
                .as_ref()
                .as_ref()
                .map_or_else(Default::default, Clone::clone);
            kwargs.insert("source".to_owned(), to_value(event)?);
            let params = pack(&ParamsRun::new(
                &macro_oid,
                &macro_args,
                &Some(kwargs),
                timeout,
            ))?;
            Some(params)
        } else {
            None
        }
    };
    if let Some(p) = params {
        run(p, id, timeout).await;
    }
    Ok(())
}

impl Rule {
    #[inline]
    pub fn init(&mut self) {
        self.digest = submap::digest::sha256(&*self.id);
    }
    #[inline]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[inline]
    pub fn oid(&self) -> &OIDMask {
        &self.oid
    }
    #[inline]
    pub fn set_priority(&mut self, p: usize) {
        self.priority = p;
    }
    pub fn info(&self) -> Info {
        let (chillout_remaining, chillout_event_pending) =
            if let Some(chillout_time) = self.chillout_time {
                let ch = self.chillout.lock().unwrap();
                if let Some(a) = ch.active {
                    let elapsed = a.elapsed();
                    let remaining = if chillout_time > elapsed {
                        chillout_time - elapsed
                    } else {
                        DURATION_ZERO
                    };
                    (Some(remaining), ch.event.is_some())
                } else {
                    (None, false)
                }
            } else {
                (None, false)
            };
        Info {
            id: &self.id,
            run: &self.run,
            chillout_time: self.chillout_time,
            chillout_remaining,
            chillout_event_pending,
            priority: self.priority,
        }
    }
    pub async fn process(&self, oid: &OID, state: &StateX, prev: Option<&StateX>) -> EResult<bool> {
        if self.need_process(state, prev) && self.matches(state) {
            trace!("rule {} matched for {}", self.id, oid);
            if self.chillout_time.is_some() {
                let mut chillout = self.chillout.lock().unwrap();
                if chillout.active.is_some() {
                    chillout.event.replace(SourceOwned::new(oid, state));
                    return Ok(!self.brk);
                }
                chillout.active.replace(Instant::now());
            }
            let timeout = *crate::TIMEOUT.get().unwrap();
            let mut kwargs = self
                .kwargs
                .as_ref()
                .as_ref()
                .map_or_else(Default::default, Clone::clone);
            kwargs.insert("source".to_owned(), to_value(Source::new(oid, state))?);
            let params = pack(&ParamsRun::new(
                &self.run,
                &self.args,
                &Some(kwargs),
                timeout,
            ))?;
            if self.block {
                run(params, self.id.clone(), timeout).await;
            } else {
                let id = self.id.clone();
                // TODO move to task pool
                tokio::spawn(async move {
                    run(params, id, timeout).await;
                });
            }
            if let Some(chillout_time) = self.chillout_time {
                let id = self.id.clone();
                let chillout = self.chillout.clone();
                let macro_oid = self.run.clone();
                let macro_args = self.args.clone();
                let macro_kwargs = self.kwargs.clone();
                // TODO move to task pool
                tokio::spawn(async move {
                    process_chillout(
                        id,
                        chillout,
                        macro_oid,
                        macro_args,
                        macro_kwargs,
                        chillout_time,
                        timeout,
                    )
                    .await
                    .log_ef();
                });
            }
            Ok(!self.brk)
        } else {
            Ok(true)
        }
    }
    fn matches(&self, state: &StateX) -> bool {
        self.condition.matches_any()
            || match self.prop {
                StatePropExt::Status => self.condition.matches(f64::from(state.status)),
                StatePropExt::Value => {
                    if let Some(bit) = self.bit {
                        if let Ok(v) = TryInto::<u64>::try_into(state.value.clone()) {
                            self.condition
                                .matches(if v.get_bit(bit) { 1.0 } else { 0.0 })
                        } else {
                            false
                        }
                    } else {
                        self.condition.matches_value(&state.value)
                    }
                }
                StatePropExt::Act => {
                    if let Some(act) = state.act {
                        self.condition.matches(f64::from(act))
                    } else {
                        false
                    }
                }
            }
    }
    fn need_process(&self, state: &StateX, prev: Option<&StateX>) -> bool {
        if let Some(st_prev) = prev {
            if self.initial == InitialKind::Only {
                false
            } else {
                self.condition.matches_any()
                    || match self.prop {
                        StatePropExt::Status => {
                            st_prev.status != state.status
                                && !self.condition.matches(f64::from(st_prev.status))
                        }
                        StatePropExt::Value => {
                            st_prev.value != state.value
                                && if let Some(bit) = self.bit {
                                    if let Ok(v) = TryInto::<u64>::try_into(st_prev.value.clone()) {
                                        !self.condition.matches(if v.get_bit(bit) {
                                            1.0
                                        } else {
                                            0.0
                                        })
                                    } else {
                                        true
                                    }
                                } else {
                                    !self.condition.matches_value(&st_prev.value)
                                }
                        }
                        StatePropExt::Act => {
                            st_prev.act != state.act
                                && if let Some(act) = st_prev.act {
                                    !self.condition.matches(f64::from(act))
                                } else {
                                    true
                                }
                        }
                    }
            }
        } else {
            self.initial != InitialKind::Skip
        }
    }
}
