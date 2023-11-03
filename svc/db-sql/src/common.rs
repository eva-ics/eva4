use eva_common::acl::OIDMaskList;
use serde::Deserialize;
use std::time::Duration;

#[inline]
fn default_queue_size() -> usize {
    8192
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TsExtension {
    Timescale,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub db: String,
    pub ts_extension: Option<TsExtension>,
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
    #[serde(default)]
    pub simple_cleaning: bool,
    pub keep: Option<u64>,
    pub oids: OIDMaskList,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub panic_in: Option<Duration>,
}
