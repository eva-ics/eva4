use super::{GenData, GeneratorSource};
use crate::target::{notify, notify_archive, Target};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use rand::distributions::{Distribution, Uniform};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

err_logger!();

fn default_min() -> i64 {
    i64::MIN
}

fn default_max() -> i64 {
    i64::MAX
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default = "default_min")]
    min: i64,
    #[serde(default = "default_max")]
    max: i64,
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
            let between = Uniform::from(params.min..=params.max);
            while !svc_is_terminating() {
                int.tick().await;
                let val = {
                    let mut rng = rand::thread_rng();
                    Value::I64(between.sample(&mut rng))
                };
                notify(&name, &targets, val).await;
            }
        });
        Ok(fut)
    }
    fn plan(&self, params: Value, sampling: f64, duration: Duration) -> EResult<Vec<GenData>> {
        let params = Params::deserialize(params)?;
        let interval = Duration::from_secs_f64(1.0 / sampling);
        let mut rng = rand::thread_rng();
        let mut now = Duration::from_secs(0);
        let between = Uniform::from(params.min..=params.max);
        let mut result = Vec::new();
        while now <= duration {
            let value = Value::I64(between.sample(&mut rng));
            result.push(GenData {
                t: now.as_secs_f64(),
                value,
            });
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
        let between = Uniform::from(params.min..=params.max);
        let mut now = t_start;
        let job_id = Uuid::new_v4();
        tokio::spawn(async move {
            info!("apply job started: {}", job_id);
            while now <= t_end {
                let value = {
                    let mut rng = rand::thread_rng();
                    Value::I64(between.sample(&mut rng))
                };
                notify_archive(&targets, now, value).await.log_ef();
                now += interval;
            }
            info!("apply job completed: {}", job_id);
        });
        Ok(job_id)
    }
}
