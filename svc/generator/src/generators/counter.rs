use super::{GenData, GeneratorSource};
use crate::target::{Target, notify, notify_archive};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

err_logger!();

fn default_min() -> f64 {
    0.0
}

fn default_max() -> f64 {
    f64::from(i32::MAX)
}

fn default_step() -> f64 {
    1.0
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default = "default_min")]
    min: f64,
    #[serde(default = "default_max")]
    max: f64,
    #[serde(default = "default_step")]
    step: f64,
}

pub struct GenSource {}

#[async_trait::async_trait]
impl GeneratorSource for GenSource {
    async fn start(
        &self,
        name: &str,
        params: Value,
        sampling: f64,
        targets: Arc<Vec<Target>>,
    ) -> EResult<JoinHandle<()>> {
        let params = Params::deserialize(params)?;
        let name = name.to_owned();
        let fut = tokio::spawn(async move {
            let mut int = tokio::time::interval(Duration::from_secs_f64(1.0 / sampling));
            let mut c = params.min;
            while !svc_is_terminating() {
                int.tick().await;
                let val = Value::F64(c);
                notify(&name, &targets, val).await;
                c += params.step / sampling;
                if c > params.max {
                    c = params.min;
                }
            }
        });
        Ok(fut)
    }
    fn plan(&self, params: Value, sampling: f64, duration: Duration) -> EResult<Vec<GenData>> {
        let params = Params::deserialize(params)?;
        let interval = Duration::from_secs_f64(1.0 / sampling);
        let mut now = Duration::from_secs(0);
        let mut result = Vec::new();
        let mut c = params.min;
        while now <= duration {
            let value = Value::F64(c);
            result.push(GenData {
                t: now.as_secs_f64(),
                value,
            });
            c += params.step / sampling;
            if c > params.max {
                c = params.min;
            }
            now += interval;
        }
        Ok(result)
    }
    async fn apply(
        &self,
        params: Value,
        sampling: f64,
        t_start: f64,
        t_end: f64,
        targets: Vec<OID>,
    ) -> EResult<Uuid> {
        let params = Params::deserialize(params)?;
        let interval = 1.0 / sampling;
        let mut c = params.min;
        let mut now = t_start;
        let job_id = Uuid::new_v4();
        tokio::spawn(async move {
            info!("apply job started: {}", job_id);
            while now <= t_end {
                let value = Value::F64(c);
                notify_archive(&targets, now, value).await.log_ef();
                c += params.step / sampling;
                if c > params.max {
                    c = params.min;
                }
                now += interval;
            }
            info!("apply job completed: {}", job_id);
        });
        Ok(job_id)
    }
}
