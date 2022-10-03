use eva_common::prelude::*;
use eva_sdk::controller::transform::{self, Transform};
use eva_sdk::types::StateProp;
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropActionMap {
    symbol: String,
    #[serde(default)]
    transform: Vec<transform::Task>,
    #[serde(skip)]
    var: Option<Arc<crate::adsbr::Var>>,
}

impl PropActionMap {
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
pub struct ActionMap {
    #[serde(default)]
    status: Option<PropActionMap>,
    #[serde(default)]
    value: Option<PropActionMap>,
}

impl ActionMap {
    #[inline]
    pub fn status(&self) -> Option<&PropActionMap> {
        self.status.as_ref()
    }
    #[inline]
    pub fn value(&self) -> Option<&PropActionMap> {
        self.value.as_ref()
    }
    #[inline]
    pub fn status_mut(&mut self) -> Option<&mut PropActionMap> {
        self.status.as_mut()
    }
    #[inline]
    pub fn value_mut(&mut self) -> Option<&mut PropActionMap> {
        self.value.as_mut()
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

#[inline]
fn default_task_prop() -> StateProp {
    StateProp::Value
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullTask {
    oid: OID,
    #[serde(default = "default_task_prop")]
    prop: StateProp,
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
    pub fn prop(&self) -> StateProp {
        self.prop
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
