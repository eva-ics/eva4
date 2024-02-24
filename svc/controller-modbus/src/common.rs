use crate::types::{self, ModbusType, Register, RegisterKind};
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
pub struct ModBusConfig {
    pub protocol: types::ProtocolKind,
    pub path: String,
    pub unit: Option<u8>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub frame_delay: Option<Duration>,
    #[serde(default = "eva_common::tools::default_true")]
    pub fc16_supported: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionMap {
    reg: Register,
    #[serde(rename = "type", default)]
    tp: ModbusType,
    #[serde(default)]
    bit: Option<u8>,
    #[serde(default)]
    unit: u8,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

impl ActionMap {
    #[inline]
    pub fn reg(&self) -> Register {
        self.reg
    }
    #[inline]
    pub fn tp(&self) -> ModbusType {
        self.tp
    }
    #[inline]
    pub fn bit(&self) -> Option<u8> {
        self.bit
    }
    #[inline]
    pub fn unit(&self) -> u8 {
        self.unit
    }
    #[inline]
    pub fn init(&mut self, default_unit: Option<u8>, oid: &OID) -> EResult<()> {
        if self.unit == 0 {
            self.unit = default_unit.ok_or_else(|| {
                Error::invalid_params("no unit for action task and no default unit is set")
            })?;
        }
        match self.reg.kind() {
            RegisterKind::Coil | RegisterKind::Discrete => {
                if self.tp != ModbusType::Bit {
                    return Err(Error::invalid_data(format!(
                        "invalid action map for {}: coils can not have types/params",
                        oid
                    )));
                }
            }
            RegisterKind::Holding | RegisterKind::Input => {
                if self.tp == ModbusType::Bit {
                    if self.bit.is_none() {
                        return Err(Error::invalid_data(format!(
                            "action map bit not specified for {}",
                            oid
                        )));
                    }
                } else if self.bit.is_some() {
                    return Err(Error::invalid_data(format!(
                        "action map bit specified for {} but the type is not bit",
                        oid
                    )));
                }
            }
        }
        Ok(())
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
    pub modbus: ModBusConfig,
    #[serde(default)]
    pub pull: Vec<PullReg>,
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
    pub retries: u8,
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
pub struct PullReg {
    pub reg: Register,
    pub count: u16,
    #[serde(default)]
    pub unit: u8,
    map: Vec<PullTask>,
}

impl PullReg {
    #[inline]
    pub fn map(&self) -> &[PullTask] {
        &self.map
    }
    #[inline]
    pub fn init(&mut self, default_unit: Option<u8>) -> EResult<()> {
        if self.unit == 0 {
            self.unit = default_unit.ok_or_else(|| {
                Error::invalid_params("no unit for pull task and no default unit is set")
            })?;
        }
        for task in &mut self.map {
            let o = task.offset.offset();
            let reg_no = self.reg.number();
            if task.offset.is_absolute() {
                if reg_no > o {
                    return Err(Error::invalid_data(format!(
                        "absolute offset specified for {} but the register block does not contain it",
                        task.oid
                    )));
                }
                task.block_offset = o - reg_no;
            } else {
                task.block_offset = o;
            }
            match self.reg.kind() {
                RegisterKind::Coil | RegisterKind::Discrete => {
                    if task.tp != ModbusType::Bit || task.value_delta.is_some() {
                        return Err(Error::invalid_data(format!(
                            "invalid pull map for {}: coils can not have types/params",
                            task.oid
                        )));
                    }
                }
                RegisterKind::Holding | RegisterKind::Input => {
                    if task.tp == ModbusType::Bit {
                        if !task.offset.has_bit() {
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
            }
        }
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
    tp: ModbusType,
    #[serde(default)]
    transform: Vec<transform::Task>,
    #[serde(default, skip)]
    block_offset: u16,
}

impl PullTask {
    #[inline]
    pub fn oid(&self) -> &OID {
        &self.oid
    }
    #[inline]
    pub fn tp(&self) -> ModbusType {
        self.tp
    }
    #[inline]
    pub fn block_offset(&self) -> u16 {
        self.block_offset
    }
    #[inline]
    pub fn bit(&self) -> Option<u8> {
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
