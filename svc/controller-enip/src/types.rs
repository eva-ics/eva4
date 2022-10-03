use bmart::tools::EnumStr;
use eva_common::prelude::*;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize, EnumStr)]
pub enum ProtocolKind {
    #[serde(rename = "ab_eip")]
    AbEip,
}

#[derive(Deserialize, EnumStr)]
#[serde(rename_all = "UPPERCASE")]
#[enumstr(rename_all = "UPPERCASE")]
pub enum ProtocolCpu {
    Lgx,
    Mlgx,
    Plc,
    Mlgx800,
}

#[derive(Deserialize, Eq, PartialEq, Copy, Clone)]
#[serde(rename_all = "lowercase")]
pub enum EipType {
    Bit,
    #[serde(alias = "byte", alias = "BOOL", alias = "USINT")]
    Uint8,
    #[serde(alias = "sint8", alias = "SINT")]
    Int8,
    #[serde(alias = "word", alias = "UINT")]
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

impl Default for EipType {
    fn default() -> Self {
        EipType::Bit
    }
}

impl EipType {
    #[inline]
    pub fn size_bytes(self) -> u8 {
        match self {
            EipType::Bit => 0,
            EipType::Uint8 | EipType::Int8 => 1,
            EipType::Uint16 | EipType::Int16 => 2,
            EipType::Uint32 | EipType::Int32 | EipType::Real32 => 4,
            EipType::Uint64 | EipType::Int64 | EipType::Real64 => 8,
        }
    }
    #[inline]
    pub fn normalize_value(self, value: Value) -> EResult<Value> {
        Ok(match self {
            EipType::Bit | EipType::Uint8 => Value::U8(value.try_into()?),
            EipType::Int8 => Value::I8(value.try_into()?),
            EipType::Uint16 => Value::U16(value.try_into()?),
            EipType::Int16 => Value::I16(value.try_into()?),
            EipType::Uint32 => Value::U32(value.try_into()?),
            EipType::Int32 => Value::I32(value.try_into()?),
            EipType::Uint64 => Value::U64(value.try_into()?),
            EipType::Int64 => Value::I64(value.try_into()?),
            EipType::Real32 => Value::F32(value.try_into()?),
            EipType::Real64 => Value::F64(value.try_into()?),
        })
    }
}

#[derive(Copy, Clone)]
pub struct Offset {
    offset: u32,
    bit: Option<u32>,
}

impl Offset {
    #[inline]
    pub fn offset(&self) -> u32 {
        self.offset
    }
    #[inline]
    pub fn bit(&self) -> u32 {
        self.bit.unwrap()
    }
    #[inline]
    pub fn has_bit(&self) -> bool {
        self.bit.is_some()
    }
    #[inline]
    pub fn make_bit_offset(&mut self) {
        self.offset *= 8;
    }
}

impl<'de> Deserialize<'de> for Offset {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Offset, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v: Value = Deserialize::deserialize(deserializer)?;
        if let Value::String(val) = v {
            let mut offset = 0;
            let mut sp_bit = val.split('/');
            let s = sp_bit.next().unwrap();
            let bit: Option<u32> = if let Some(b) = sp_bit.next() {
                Some(b.parse().map_err(serde::de::Error::custom)?)
            } else {
                None
            };
            let sp = s.split('+');
            for v in sp {
                let rel_offset: u32 = v.parse().map_err(serde::de::Error::custom)?;
                offset += rel_offset;
            }
            Ok(Offset { offset, bit })
        } else {
            let offset: u32 = v.try_into().map_err(serde::de::Error::custom)?;
            Ok(Offset { offset, bit: None })
        }
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone)]
pub struct PendingTag<'a> {
    id: i32,
    path: &'a str,
}

impl<'a> PendingTag<'a> {
    #[inline]
    pub fn new(id: i32, path: &'a str) -> Self {
        Self { id, path }
    }
    #[inline]
    pub fn id(&self) -> i32 {
        self.id
    }
    #[inline]
    pub fn path(&self) -> &str {
        self.path
    }
}
