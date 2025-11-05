use eva_common::acl::OIDMaskList;
use serde::Deserialize;
use std::time::Duration;

#[inline]
fn default_queue_size() -> usize {
    8192
}

#[inline]
fn default_api_version() -> u16 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_api_version")]
    pub api_version: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    pub url: String,
    pub tls_ca: Option<String>,
    pub db: String,
    #[serde(default)]
    pub org: Option<String>,
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
    pub clients: Option<u32>,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default)]
    pub ignore_events: bool,
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
}
