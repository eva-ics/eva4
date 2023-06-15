use eva_common::prelude::*;
use eva_sdk::controller::transform::{self, Transform};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

#[inline]
fn default_queue_size() -> usize {
    32768
}

#[inline]
fn default_action_queue_size() -> usize {
    32
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionMap {
    path: String,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

impl ActionMap {
    #[inline]
    pub fn path(&self) -> &str {
        &self.path
    }
    #[inline]
    pub fn need_transform(&self) -> bool {
        !self.transform.is_empty()
    }
    #[inline]
    pub fn transform_value<T: Transform>(&self, value: T, oid: &OID) -> EResult<f64> {
        transform::transform(&self.transform, oid, value)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub path: String,
    #[serde(default)]
    pub pull: Vec<Pull>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub verify_delay: Option<Duration>,
    #[serde(default)]
    pub action_map: HashMap<OID, ActionMap>,
    #[serde(default)]
    pub actions_verify: bool,
    #[serde(default)]
    pub retries: Option<u8>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub pull_cache_sec: Option<Duration>,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default = "default_action_queue_size")]
    pub action_queue_size: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pull {
    oid: OID,
    #[serde(default, deserialize_with = "eva_common::tools::de_float_as_duration")]
    interval: Duration,
    path: String,
    #[serde(default)]
    value_delta: Option<f64>,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

impl Pull {
    #[inline]
    pub fn oid(&self) -> &OID {
        &self.oid
    }
    #[inline]
    pub fn interval(&self) -> Duration {
        self.interval
    }
    #[inline]
    pub fn path(&self) -> &str {
        &self.path
    }
    #[inline]
    pub fn need_transform(&self) -> bool {
        !self.transform.is_empty()
    }
    #[inline]
    pub fn transform_value<T: Transform>(&self, value: T, oid: &OID) -> EResult<f64> {
        transform::transform(&self.transform, oid, value)
    }
    #[inline]
    pub fn value_delta(&self) -> Option<f64> {
        self.value_delta
    }
}
