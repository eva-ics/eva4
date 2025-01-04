use crate::common::{safe_run_macro, ParamsCow, ParamsRun};
use chrono::{DateTime, Local};
use cron::Schedule;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

#[inline]
pub fn de_schedule_from_str<'de, D>(deserializer: D) -> Result<Schedule, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = String::deserialize(deserializer)?;
    buf.parse().map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    id: String,
    #[serde(deserialize_with = "de_schedule_from_str")]
    schedule: Schedule,
    run: OID,
    #[serde(default)]
    args: Option<Vec<Value>>,
    #[serde(default)]
    kwargs: Option<HashMap<String, Value>>,
    #[serde(skip)]
    next_launch: Mutex<Option<DateTime<Local>>>,
}

impl Job {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn info(&self) -> Info {
        Info {
            id: &self.id,
            run: &self.run,
            next_launch: self.next_launch.lock().unwrap().map(|v| v.to_string()),
        }
    }
    pub async fn scheduler(&self) -> EResult<()> {
        let timeout = *crate::TIMEOUT.get().unwrap();
        let params = ParamsRun::new(
            &self.run,
            self.args.as_deref(),
            self.kwargs.as_ref(),
            timeout,
        );
        let rpc = crate::RPC.get().unwrap();
        for next_launch in self.schedule.upcoming(Local) {
            let now: DateTime<Local> = Local::now();
            if next_launch > now {
                self.next_launch.lock().unwrap().replace(next_launch);
                let run_in = next_launch - now;
                tokio::time::sleep(run_in.to_std().map_err(Error::core)?).await;
                if svc_is_terminating() {
                    return Ok(());
                }
                debug!("executing job {}", self.id);
                if let Err(e) = safe_run_macro(rpc, ParamsCow::Params(params), timeout).await {
                    warn!("job {} error: {}/{:?}", self.id, e.0, e.1);
                }
            } else {
                warn!("job {} skipped", self.id);
            }
        }
        warn!("job {} finished, no upcoming schedules", self.id);
        Ok(())
    }
    pub fn clear_next_launch(&self) {
        self.next_launch.lock().unwrap().take();
    }
}

#[derive(Serialize, bmart::tools::Sorting)]
pub struct Info<'a> {
    id: &'a str,
    run: &'a OID,
    next_launch: Option<String>,
}
