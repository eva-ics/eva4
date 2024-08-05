use eva_common::prelude::*;
use eva_sdk::controller::transform::{self, Transform};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[inline]
fn default_ams_port() -> u16 {
    851
}

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
pub struct AdsConfig {
    pub bridge_svc: String,
    pub ams_netid: String,
    #[serde(default = "default_ams_port")]
    pub ams_port: u16,
    #[serde(default = "eva_common::tools::default_true")]
    pub bulk_allow: bool,
    #[serde(default = "eva_common::tools::default_true")]
    pub check_ready: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionMap {
    symbol: String,
    #[serde(default)]
    transform: Vec<transform::Task>,
    #[serde(skip)]
    var: Option<Arc<crate::adsbr::Var>>,
}

impl ActionMap {
    #[inline]
    pub fn symbol(&self) -> &str {
        &self.symbol
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
    pub fn var(&self) -> Option<Arc<crate::adsbr::Var>> {
        self.var.as_ref().map(Clone::clone)
    }
    #[inline]
    pub fn set_var(&mut self, var: Arc<crate::adsbr::Var>) {
        self.var.replace(var);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub ads: AdsConfig,
    #[serde(default)]
    pub pull: Vec<PullSymbol>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub pull_interval: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub ping_interval: Option<Duration>,
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
    pub panic_in: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub pull_cache_sec: Option<Duration>,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default = "default_action_queue_size")]
    pub action_queue_size: usize,
    #[serde(default)]
    pub restart_bridge_on_panic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullSymbol {
    symbol: String,
    map: Vec<PullTask>,
    #[serde(skip)]
    var: Option<Arc<crate::adsbr::Var>>,
}

impl PullSymbol {
    #[inline]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
    #[inline]
    pub fn map(&self) -> &[PullTask] {
        &self.map
    }
    #[inline]
    pub fn var(&self) -> Option<Arc<crate::adsbr::Var>> {
        self.var.as_ref().map(Clone::clone)
    }
    #[inline]
    pub fn set_var(&mut self, var: Arc<crate::adsbr::Var>) {
        self.var.replace(var);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullTask {
    oid: OID,
    #[serde(default)]
    value_delta: Option<f64>,
    #[serde(default)]
    transform: Vec<transform::Task>,
    idx: Option<usize>,
}

impl PullTask {
    #[inline]
    pub fn oid(&self) -> &OID {
        &self.oid
    }
    #[inline]
    pub fn need_transform(&self) -> bool {
        !self.transform.is_empty()
    }
    #[inline]
    pub fn transform_value<T: Transform>(&self, value: T) -> EResult<f64> {
        transform::transform(&self.transform, &self.oid, value)
    }
    #[inline]
    pub fn value_delta(&self) -> Option<f64> {
        self.value_delta
    }
    #[inline]
    pub fn idx(&self) -> Option<usize> {
        self.idx
    }
}
