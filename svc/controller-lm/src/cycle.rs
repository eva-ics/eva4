use crate::common::{run_err_macro, safe_run_macro, ParamsCow, ParamsRun};
use eva_common::prelude::*;
use eva_common::tools::serialize_atomic_u64;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::sync::atomic;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(bmart::tools::EnumStr, Eq, PartialEq, Copy, Clone)]
#[repr(u8)]
enum CycleStatus {
    Stopped = 0,
    Starting = 1,
    Running = 0xff,
    Stopping = 2,
}

impl CycleStatus {
    fn from_u8(s: u8) -> Self {
        match s {
            1 => CycleStatus::Starting,
            0xff => CycleStatus::Running,
            2 => CycleStatus::Stopping,
            _ => CycleStatus::Stopped,
        }
    }
}

pub fn serialize_atomic_as_cycle_status<S>(
    status: &atomic::AtomicU8,
    s: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&CycleStatus::from_u8(status.load(atomic::Ordering::SeqCst)).to_string())
}

#[derive(Deserialize, Serialize, bmart::tools::Sorting)]
#[serde(deny_unknown_fields)]
pub struct Cycle {
    id: String,
    #[serde(skip_serializing, default)]
    auto_start: bool,
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration",
        serialize_with = "eva_common::tools::serialize_duration_as_f64"
    )]
    interval: Duration,
    run: OID,
    #[serde(default, skip_serializing)]
    args: Option<Vec<Value>>,
    #[serde(default, skip_serializing)]
    kwargs: Option<HashMap<String, Value>>,
    #[serde(default, skip_serializing)]
    on_error: Option<OID>,
    #[serde(skip_deserializing, serialize_with = "serialize_atomic_u64")]
    iters_ok: atomic::AtomicU64,
    #[serde(skip_deserializing, serialize_with = "serialize_atomic_u64")]
    timed_out: atomic::AtomicU64,
    #[serde(skip_deserializing, serialize_with = "serialize_atomic_u64")]
    iters_err: atomic::AtomicU64,
    #[serde(
        skip_deserializing,
        serialize_with = "serialize_atomic_as_cycle_status"
    )]
    status: atomic::AtomicU8,
}

impl Cycle {
    #[inline]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[inline]
    pub fn auto_start(&self) -> bool {
        self.auto_start
    }
    async fn run(&self) {
        info!("running cycle #{}, interval: {:?}", self.id, self.interval);
        let timeout = *crate::TIMEOUT.get().unwrap();
        let rpc = crate::RPC.get().unwrap();
        macro_rules! run_err {
            ($e: expr) => {
                if let Some(ref on_error) = self.on_error {
                    if let Err(e) = run_err_macro(rpc, on_error, $e, &None, timeout).await {
                        error!("Cycle {} error handler macro failed: {}", self.id, e);
                    }
                }
            };
        }
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_ticked: Option<Instant> = None;
        self.status
            .store(CycleStatus::Running as u8, atomic::Ordering::SeqCst);
        while self.status.load(atomic::Ordering::SeqCst) == CycleStatus::Running as u8 {
            let t = ticker.tick().await.into_std();
            if let Some(prev) = last_ticked {
                if t - prev > self.interval {
                    warn!("cycle {} timeout", self.id);
                    self.timed_out.fetch_add(1, atomic::Ordering::SeqCst);
                    run_err!((
                        "cycle".to_owned(),
                        Some(Value::String("timeout".to_owned()))
                    ));
                }
            }
            let params = ParamsRun::new(&self.run, &self.args, &self.kwargs, self.interval);
            trace!("cycle {} run", self.id);
            if let Err(e) = safe_run_macro(rpc, ParamsCow::Params(params), self.interval).await {
                self.iters_err.fetch_add(1, atomic::Ordering::SeqCst);
                warn!("cycle {} error: {}/{:?}", self.id, e.0, e.1);
                run_err!(e);
            } else {
                trace!("cycle {} ok", self.id);
                self.iters_ok.fetch_add(1, atomic::Ordering::SeqCst);
            }
            last_ticked.replace(t);
        }
        self.status
            .store(CycleStatus::Stopped as u8, atomic::Ordering::SeqCst);
    }

    pub fn reset(&self) {
        self.iters_ok.store(0, atomic::Ordering::SeqCst);
        self.timed_out.store(0, atomic::Ordering::SeqCst);
        self.iters_err.store(0, atomic::Ordering::SeqCst);
    }
}

pub fn start(cycle: Arc<Cycle>) -> EResult<()> {
    info!("starting cycle {}", cycle.id);
    if cycle.status.load(atomic::Ordering::SeqCst) == CycleStatus::Stopped as u8 {
        cycle
            .status
            .store(CycleStatus::Starting as u8, atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            cycle.run().await;
        });
        Ok(())
    } else {
        Err(Error::busy("cycle is not in the stopped state"))
    }
}
pub fn stop(cycle: &Cycle) {
    info!("stopping cycle {}", cycle.id);
    if cycle.status.load(atomic::Ordering::SeqCst) != CycleStatus::Stopped as u8 {
        cycle
            .status
            .store(CycleStatus::Stopping as u8, atomic::Ordering::SeqCst);
    }
}
