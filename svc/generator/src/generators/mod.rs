use crate::target::Target;
use eva_common::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub const DEFAULT_SAMPLING: u32 = 1;

#[derive(Serialize)]
pub struct GenData {
    pub t: f64,
    pub value: Value,
}

#[async_trait::async_trait]
pub trait GeneratorSource: Send + Sync {
    async fn start(
        &self,
        _name: &str,
        _params: Value,
        _sampling: u32,
        _targets: Arc<Vec<Target>>,
    ) -> EResult<JoinHandle<()>> {
        Err(Error::unsupported(
            "the generator does not support starting",
        ))
    }
    fn plan(&self, _params: Value, _sampling: u32, _duration: Duration) -> EResult<Vec<GenData>> {
        Err(Error::unsupported(
            "the generator does not support planning",
        ))
    }
    async fn apply(
        &self,
        _params: Value,
        _sampling: u32,
        _t_start: f64,
        _t_end: f64,
        _targets: Vec<OID>,
    ) -> EResult<Uuid> {
        Err(Error::unsupported(
            "the generator does not support applying",
        ))
    }
}

pub mod counter;
pub mod random;
pub mod random_float;
pub mod time;
pub mod udp_float;
pub mod wave;
