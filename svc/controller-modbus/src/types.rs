use bmart::tools::EnumStr;
use eva_common::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Deserialize, EnumStr)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolKind {
    Tcp,
    Udp,
    Rtu,
    Ascii,
    Native,
}

impl From<ProtocolKind> for rmodbus::ModbusProto {
    fn from(kind: ProtocolKind) -> Self {
        match kind {
            ProtocolKind::Tcp | ProtocolKind::Udp => rmodbus::ModbusProto::TcpUdp,
            ProtocolKind::Rtu | ProtocolKind::Native => rmodbus::ModbusProto::Rtu,
            ProtocolKind::Ascii => rmodbus::ModbusProto::Ascii,
        }
    }
}

#[derive(Deserialize, Eq, PartialEq, Copy, Clone, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModbusType {
    #[default]
    Bit,
    Uint16,
    #[serde(alias = "sint16", alias = "INT")]
    Int16,
    #[serde(alias = "dword", alias = "UDINT")]
    Uint32,
    #[serde(alias = "sint32", alias = "DINT")]
    Int32,
    #[serde(alias = "qword", alias = "ULINT")]
    Uint64,
    #[serde(alias = "sint64", alias = "LINT")]
    Int64,
    #[serde(alias = "real", alias = "REAL")]
    Real32,
    #[serde(alias = "LREAL")]
    Real64,
}

impl ModbusType {
    pub fn size(self) -> u16 {
        match self {
            ModbusType::Bit | ModbusType::Uint16 | ModbusType::Int16 => 1,
            ModbusType::Uint32 | ModbusType::Int32 | ModbusType::Real32 => 2,
            ModbusType::Uint64 | ModbusType::Int64 | ModbusType::Real64 => 4,
        }
    }
    #[inline]
    pub fn normalize_value(self, value: Value) -> EResult<Value> {
        Ok(match self {
            ModbusType::Bit => Value::U8(value.try_into()?),
            ModbusType::Uint16 => Value::U16(value.try_into()?),
            ModbusType::Int16 => Value::I16(value.try_into()?),
            ModbusType::Uint32 => Value::U32(value.try_into()?),
            ModbusType::Int32 => Value::I32(value.try_into()?),
            ModbusType::Uint64 => Value::U64(value.try_into()?),
            ModbusType::Int64 => Value::I64(value.try_into()?),
            ModbusType::Real32 => Value::F32(value.try_into()?),
            ModbusType::Real64 => Value::F64(value.try_into()?),
        })
    }
}

#[derive(Serialize)]
pub struct RegisterValue {
    register: Register,
    #[serde(skip_serializing_if = "Option::is_none")]
    bit: Option<u8>,
    value: Value,
}

impl RegisterValue {
    #[inline]
    pub fn new0(kind: RegisterKind, number: u16, value: Value) -> Self {
        Self {
            register: Register { kind, number },
            bit: None,
            value,
        }
    }
    #[inline]
    pub fn new(kind: RegisterKind, number: u16, bit: u8, value: Value) -> Self {
        Self {
            register: Register { kind, number },
            bit: Some(bit),
            value,
        }
    }
    #[inline]
    pub fn bit(&self) -> Option<u8> {
        self.bit
    }
    #[inline]
    pub fn value(&self) -> &Value {
        &self.value
    }
}

impl From<RegisterValue> for Value {
    #[inline]
    fn from(rv: RegisterValue) -> Self {
        rv.value
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RegisterKind {
    Coil,
    Discrete,
    Input,
    Holding,
}

impl RegisterKind {
    fn from_char(ch: char) -> EResult<Self> {
        match ch {
            'c' => Ok(RegisterKind::Coil),
            'd' => Ok(RegisterKind::Discrete),
            'i' => Ok(RegisterKind::Input),
            'h' => Ok(RegisterKind::Holding),
            _ => Err(Error::invalid_params(format!(
                "invalid register type: {}",
                ch
            ))),
        }
    }
    fn as_char(self) -> char {
        match self {
            RegisterKind::Coil => 'c',
            RegisterKind::Discrete => 'd',
            RegisterKind::Holding => 'h',
            RegisterKind::Input => 'i',
        }
    }
    pub fn is_bit_reg(self) -> bool {
        self == RegisterKind::Coil || self == RegisterKind::Discrete
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Register {
    kind: RegisterKind,
    number: u16,
}

impl Register {
    #[inline]
    pub fn kind(self) -> RegisterKind {
        self.kind
    }
    #[inline]
    pub fn number(self) -> u16 {
        self.number
    }
}

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.kind.as_char(), self.number)
    }
}

impl Serialize for Register {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}{}", self.kind.as_char(), self.number))
    }
}

impl<'de> Deserialize<'de> for Register {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Register, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v: Value = Deserialize::deserialize(deserializer)?;
        if let Value::String(valstr) = v {
            if valstr.is_empty() {
                return Err(serde::de::Error::custom(Error::invalid_params(
                    "empty register",
                )));
            }
            let kind = RegisterKind::from_char(valstr.chars().next().ok_or_else(|| {
                serde::de::Error::custom(Error::invalid_params("empty register"))
            })?)
            .map_err(serde::de::Error::custom)?;
            let number: u16 = valstr[1..].parse().map_err(serde::de::Error::custom)?;
            Ok(Register { kind, number })
        } else {
            Err(serde::de::Error::custom(Error::invalid_params(format!(
                "invalid register {}",
                v
            ))))
        }
    }
}

#[derive(Clone)]
pub struct Offset {
    offset: u16,
    bit: Option<u8>,
    absolute: bool,
}

impl Offset {
    #[inline]
    pub fn offset(&self) -> u16 {
        self.offset
    }
    #[inline]
    pub fn bit(&self) -> Option<u8> {
        self.bit
    }
    #[inline]
    pub fn has_bit(&self) -> bool {
        self.bit.is_some()
    }
    #[inline]
    pub fn is_absolute(&self) -> bool {
        self.absolute
    }
}

impl<'de> Deserialize<'de> for Offset {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Offset, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v: Value = Deserialize::deserialize(deserializer)?;
        if let Value::String(valstr) = v {
            let (val, absolute) = if let Some(val) = valstr.strip_prefix('=') {
                (val, true)
            } else {
                (valstr.as_str(), false)
            };
            let mut offset = 0;
            let mut sp_bit = val.split('/');
            let s = sp_bit.next().unwrap();
            let bit: Option<u8> = if let Some(b) = sp_bit.next() {
                Some(b.parse().map_err(serde::de::Error::custom)?)
            } else {
                None
            };
            let sp = s.split('+');
            for v in sp {
                let rel_offset: u16 = v.parse().map_err(serde::de::Error::custom)?;
                offset += rel_offset;
            }
            Ok(Offset {
                offset,
                bit,
                absolute,
            })
        } else {
            let offset: u16 = v.try_into().map_err(serde::de::Error::custom)?;
            Ok(Offset {
                offset,
                bit: None,
                absolute: false,
            })
        }
    }
}
