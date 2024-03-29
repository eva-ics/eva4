use eva_common::prelude::*;
use eva_common::value::Index;
use eva_sdk::controller::transform::{self, Transform};
use log::{error, info};
use opcua::client::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic;
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
pub struct OpcUaConfig {
    pub url: String,
    pub pki_dir: Option<String>,
    #[serde(default)]
    pub trust_server_certs: bool,
    #[serde(default)]
    pub trust_server_x509: bool,
    #[serde(default)]
    pub security_policy: SecurityPolicy,
    #[serde(default)]
    pub message_security_mode: MessageSecurityMode,
    #[serde(default)]
    pub create_keys: bool,
    pub auth: Option<OpcAuth>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum OpcAuth {
    Credentials(OpcAuthCredentials),
    X509(OpcAuthX509),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpcAuthCredentials {
    pub user: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpcAuthX509 {
    pub cert_file: String,
    pub key_file: String,
}

#[derive(Deserialize, Eq, PartialEq, Copy, Clone)]
#[serde(rename_all = "lowercase")]
pub enum OpcType {
    #[serde(alias = "BOOL")]
    Bool,
    #[serde(alias = "byte", alias = "BYTE", alias = "BOOL", alias = "USINT")]
    Uint8,
    #[serde(alias = "sint8", alias = "SINT")]
    Int8,
    #[serde(alias = "word", alias = "WORD", alias = "UINT")]
    Uint16,
    #[serde(alias = "sint16", alias = "INT")]
    Int16,
    #[serde(alias = "dword", alias = "DWORD", alias = "UDINT")]
    Uint32,
    #[serde(alias = "sint32", alias = "DINT")]
    Int32,
    #[serde(alias = "qword", alias = "QWORD", alias = "ULINT")]
    Uint64,
    #[serde(alias = "sint64", alias = "LINT")]
    Int64,
    #[serde(alias = "real", alias = "REAL")]
    Real32,
    #[serde(alias = "LREAL")]
    Real64,
}

impl From<OpcType> for VariantTypeId {
    fn from(t: OpcType) -> Self {
        match t {
            OpcType::Bool => VariantTypeId::Boolean,
            OpcType::Uint8 => VariantTypeId::Byte,
            OpcType::Int8 => VariantTypeId::SByte,
            OpcType::Uint16 => VariantTypeId::UInt16,
            OpcType::Int16 => VariantTypeId::Int16,
            OpcType::Uint32 => VariantTypeId::UInt32,
            OpcType::Int32 => VariantTypeId::Int32,
            OpcType::Uint64 => VariantTypeId::UInt64,
            OpcType::Int64 => VariantTypeId::Int64,
            OpcType::Real32 => VariantTypeId::Float,
            OpcType::Real64 => VariantTypeId::Double,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionMap {
    #[serde(deserialize_with = "deserialize_node_id_from_str")]
    node: NodeId,
    #[serde(default, deserialize_with = "deserialize_opt_range")]
    range: Option<String>,
    #[serde(default)]
    transform: Vec<transform::Task>,
    #[serde(rename = "type", deserialize_with = "deserialize_opc_tp")]
    tp: VariantTypeId,
    dimensions: Option<Vec<usize>>,
}

#[inline]
pub fn deserialize_node_id_from_str<'de, D>(deserializer: D) -> Result<NodeId, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let buf = String::deserialize(deserializer)?;
    buf.parse().map_err(serde::de::Error::custom)
}

#[inline]
pub fn deserialize_vec_node_id_from_str<'de, D>(deserializer: D) -> Result<Vec<NodeId>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let buf: Vec<String> = Vec::deserialize(deserializer)?;
    let ids: Vec<NodeId> = buf
        .into_iter()
        .map(|v| {
            v.parse()
                .map_err(|e| Error::invalid_params(format!("invalid node id {v}: {e}")))
        })
        .collect::<Result<Vec<NodeId>, Error>>()
        .map_err(serde::de::Error::custom)?;
    Ok(ids)
}

#[inline]
pub fn deserialize_opc_tp<'de, D>(deserializer: D) -> Result<VariantTypeId, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let buf = OpcType::deserialize(deserializer)?;
    Ok(buf.into())
}

impl ActionMap {
    #[inline]
    pub fn node(&self) -> &NodeId {
        &self.node
    }
    #[inline]
    pub fn range(&self) -> Option<&str> {
        self.range.as_deref()
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
    pub fn tp(&self) -> VariantTypeId {
        self.tp
    }
    #[inline]
    pub fn dimensions(&self) -> Option<&[usize]> {
        self.dimensions.as_deref()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub opcua: OpcUaConfig,
    #[serde(default)]
    pub pull: Vec<PullNode>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub pull_interval: Option<Duration>,
    #[serde(default)]
    pub action_map: HashMap<OID, ActionMap>,
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
    pub ping: Option<Ping>,
}

fn default_online() -> atomic::AtomicBool {
    atomic::AtomicBool::new(true)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullNode {
    #[serde(deserialize_with = "deserialize_node_id_from_str")]
    node: NodeId,
    #[serde(default, deserialize_with = "deserialize_opt_range")]
    range: Option<String>,
    map: Vec<PullTask>,
    #[serde(skip, default = "default_online")]
    online: atomic::AtomicBool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ping {
    #[serde(deserialize_with = "deserialize_node_id_from_str")]
    pub node: NodeId,
}

#[inline]
pub fn deserialize_opt_range<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let buf: Option<Value> = Option::deserialize(deserializer)?;
    Ok(buf.map(|v| v.to_string()))
}

impl PullNode {
    #[inline]
    pub fn node(&self) -> &NodeId {
        &self.node
    }
    #[inline]
    pub fn range(&self) -> Option<&str> {
        self.range.as_deref()
    }
    #[inline]
    pub fn map(&self) -> &[PullTask] {
        &self.map
    }
    pub fn set_online(&self) {
        if !self.online.swap(true, atomic::Ordering::Relaxed) {
            info!("{} back online", self.node);
        }
    }
    pub fn report_error(&self, action: &str, error: Option<Error>) -> Error {
        let mut msg = format!("{}: {}", self.node, action);
        if let Some(e) = error {
            msg.push(' ');
            msg.push_str(&e.to_string());
        }
        if self.online.swap(false, atomic::Ordering::Relaxed) {
            error!("{}", msg);
        }
        Error::io(msg)
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
    idx: Option<Index>,
    #[serde(skip, default = "default_online")]
    online: atomic::AtomicBool,
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
    pub fn idx(&self) -> Option<&Index> {
        self.idx.as_ref()
    }
    pub fn set_online(&self) {
        if !self.online.swap(true, atomic::Ordering::Relaxed) {
            info!("{} back online", self.oid);
        }
    }
    pub fn report_error(&self, action: &str, error: Option<Error>) -> Error {
        let mut msg = format!("{}: {}", self.oid, action);
        if let Some(e) = error {
            msg.push(' ');
            msg.push_str(&e.to_string());
        }
        if self.online.swap(false, atomic::Ordering::Relaxed) {
            error!("{}", msg);
        }
        Error::io(msg)
    }
}
