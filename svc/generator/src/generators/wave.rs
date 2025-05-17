use super::{GenData, GeneratorSource};
use crate::target::{notify, notify_archive, Target};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

err_logger!();

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    formula: String,
    #[serde(default)]
    shift: i32,
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
        let expr: meval::Expr = params.formula.parse().map_err(Error::invalid_params)?;
        let _ = expr.clone().bind("x").map_err(Error::invalid_params)?;
        let fut = tokio::spawn(async move {
            let mut int = tokio::time::interval(Duration::from_secs_f64(1.0 / sampling));
            let mut t = params.shift;
            while !svc_is_terminating() {
                int.tick().await;
                let x = f64::from(t) / sampling * std::f64::consts::FRAC_PI_2;
                let val = {
                    let f = expr.clone().bind("x").unwrap();
                    Value::F64(f(x))
                };
                notify(&name, &targets, val).await;
                t += 1;
            }
        });
        Ok(fut)
    }
    fn plan(&self, params: Value, sampling: f64, duration: Duration) -> EResult<Vec<GenData>> {
        let params = Params::deserialize(params)?;
        let interval = Duration::from_secs_f64(1.0 / sampling);
        let mut now = Duration::from_secs(0);
        let mut result = Vec::new();
        let expr: meval::Expr = params.formula.parse().map_err(Error::invalid_params)?;
        let _ = expr.clone().bind("x").map_err(Error::invalid_params)?;
        let mut t = params.shift;
        while now <= duration {
            let x = f64::from(t) / sampling * std::f64::consts::FRAC_PI_2;
            let value = {
                let f = expr.clone().bind("x").unwrap();
                Value::F64(f(x))
            };
            result.push(GenData {
                t: now.as_secs_f64(),
                value,
            });
            now += interval;
            t += 1;
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
        let mut now = t_start;
        let expr: meval::Expr = params.formula.parse().map_err(Error::invalid_params)?;
        let _ = expr.clone().bind("x").map_err(Error::invalid_params)?;
        let mut t = params.shift;
        let job_id = Uuid::new_v4();
        tokio::spawn(async move {
            info!("apply job started: {}", job_id);
            while now <= t_end {
                let x = f64::from(t) / sampling * std::f64::consts::FRAC_PI_2;
                let value = {
                    let f = expr.clone().bind("x").unwrap();
                    Value::F64(f(x))
                };
                notify_archive(&targets, now, value).await.log_ef();
                now += interval;
                t += 1;
            }
            info!("apply job completed: {}", job_id);
        });
        Ok(job_id)
    }
}
