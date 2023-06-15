use crate::enip;
use crate::types::{self, EipType};
use eva_common::prelude::*;
use eva_sdk::controller::transform::{self, Transform};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[inline]
fn default_enip_port() -> u16 {
    44818
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
pub struct PlcConfig {
    protocol: types::ProtocolKind,
    host: String,
    #[serde(default = "default_enip_port")]
    port: u16,
    path: String,
    cpu: types::ProtocolCpu,
}

impl PlcConfig {
    #[inline]
    pub fn generate_path(&self) -> String {
        format!(
            "protocol={}&gateway={}:{}&path={}&cpu={}",
            self.protocol, self.host, self.port, self.path, self.cpu
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionMap {
    tag: String,
    #[serde(skip)]
    tag_path: String,
    #[serde(rename = "type")]
    tp: EipType,
    #[serde(default)]
    bit: Option<u32>,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

impl ActionMap {
    #[inline]
    pub fn tag(&self) -> &str {
        &self.tag
    }
    #[inline]
    pub fn tp(&self) -> EipType {
        self.tp
    }
    #[inline]
    pub fn bit(&self) -> Option<u32> {
        self.bit
    }
    #[inline]
    pub fn need_transform(&self) -> bool {
        !self.transform.is_empty()
    }
    #[inline]
    pub fn transform_value<T: Transform>(&self, value: T, oid: &OID) -> EResult<f64> {
        transform::transform(&self.transform, oid, value)
    }
    pub fn create_tags<'a>(
        &'a mut self,
        oid: &OID,
        plc_path: &str,
        plc_timeout: i32,
        tags_exist: &[&str],
        tags_pending: &mut HashSet<types::PendingTag<'a>>,
    ) -> EResult<()> {
        if self.tp == EipType::Bit {
            if self.bit.is_none() {
                return Err(Error::invalid_data(format!(
                    "action bit not specified for {}",
                    oid
                )));
            }
        } else if self.bit.is_some() {
            return Err(Error::invalid_data(format!(
                "action bit specified for {} but the type is not bit",
                oid
            )));
        }
        self.tag_path = enip::format_tag_path(plc_path, &self.tag, None, None);
        if !tags_exist.contains(&self.tag_path.as_str()) {
            let id = enip::create_client_tag(&self.tag_path, plc_timeout)?;
            tags_pending.insert(types::PendingTag::new(id, &self.tag_path));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub plc: PlcConfig,
    #[serde(default)]
    pub pull: Vec<PullTag>,
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
pub struct PullTag {
    tag: String,
    size: Option<u32>,
    count: Option<u32>,
    map: Vec<PullTask>,
    #[serde(skip)]
    id: i32,
    #[serde(skip)]
    path: String,
}

impl PullTag {
    #[inline]
    pub fn path(&self) -> &str {
        &self.path
    }
    #[inline]
    pub fn id(&self) -> i32 {
        self.id
    }
    #[inline]
    pub fn map(&self) -> &[PullTask] {
        &self.map
    }
    #[inline]
    pub fn init(&mut self, plc_path: &str, plc_timeout: i32) -> EResult<()> {
        for task in &mut self.map {
            if task.tp == EipType::Bit {
                if task.offset.has_bit() {
                    task.offset.make_bit_offset();
                } else {
                    return Err(Error::invalid_data(format!(
                        "pull bit not specified for {}",
                        task.oid
                    )));
                }
            } else if task.offset.has_bit() {
                return Err(Error::invalid_data(format!(
                    "pull bit specified for {} but the type is not bit",
                    task.oid
                )));
            }
        }
        self.path = enip::format_tag_path(plc_path, &self.tag, self.size, self.count);
        self.id = enip::create_client_tag(&self.path, plc_timeout)?;
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullTask {
    offset: types::Offset,
    oid: OID,
    #[serde(default)]
    value_delta: Option<f64>,
    #[serde(rename = "type", default)]
    tp: EipType,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

impl PullTask {
    #[inline]
    pub fn oid(&self) -> &OID {
        &self.oid
    }
    #[inline]
    pub fn tp(&self) -> EipType {
        self.tp
    }
    #[inline]
    pub fn offset(&self) -> u32 {
        self.offset.offset()
    }
    #[inline]
    pub fn bit(&self) -> u32 {
        self.offset.bit()
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
}
