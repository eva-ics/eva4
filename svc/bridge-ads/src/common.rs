use eva_common::OID;
use serde::Deserialize;

#[inline]
fn default_ads_port() -> u16 {
    ::ads::PORT
}

#[inline]
fn default_local_port() -> u16 {
    58913
}

#[inline]
fn default_ams_ping_port() -> u16 {
    200
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub host: String,
    #[serde(default = "default_ads_port")]
    pub port: u16,
    pub local_ams_netid: Option<String>,
    #[serde(default = "default_local_port")]
    pub local_ams_port: u16,
    pub ping_ams_netid: Option<String>,
    #[serde(default = "default_ams_ping_port")]
    pub ping_ams_port: u16,
    pub store_ads_state: Option<OID>,
}
