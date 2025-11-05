use eva_common::acl::OIDMaskList;
use serde::Deserialize;
use std::time::Duration;

#[inline]
fn default_queue_size() -> usize {
    8192
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct Config {
    pub db: String,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub buf_ttl_sec: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub interval: Option<Duration>,
    #[serde(default)]
    pub skip_disconnected: bool,
    pub pool_size: Option<u32>,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default)]
    pub ignore_events: bool,
    pub keep: Option<f64>,
    #[serde(default)]
    pub cleanup_oids: bool,
    #[serde(default)]
    pub oids: OIDMaskList,
    #[serde(default)]
    pub oids_exclude: OIDMaskList,
    #[serde(default)]
    pub oids_exclude_null: OIDMaskList,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub panic_in: Option<Duration>,
    #[serde(default)]
    pub eva_pg: bool,
}
