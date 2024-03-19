use eva_common::prelude::*;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::Cursor,
    ops::Deref,
};

use binrw::prelude::*;

use serde::{Deserialize, Deserializer, Serialize};

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[derive(Default, Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Endianess {
    Big,
    #[default]
    Little,
}

impl From<Endianess> for binrw::Endian {
    #[inline]
    fn from(e: Endianess) -> binrw::Endian {
        match e {
            Endianess::Big => binrw::Endian::Big,
            Endianess::Little => binrw::Endian::Little,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ObjectMap {
    pub(crate) objects: BTreeMap<Name, DataObject>,
}

fn parse_value_by_kind(
    buf: &mut Cursor<&[u8]>,
    kind: &Kind,
    map: &BTreeMap<Name, DataObject>,
    endianess: Endianess,
) -> EResult<Value> {
    let value = match kind {
        Kind::Bool => {
            let v = u8::read(buf).map_err(Error::invalid_data)?;
            Value::Bool(v > 0)
        }
        Kind::U8 => {
            let v = u8::read(buf).map_err(Error::invalid_data)?;
            Value::U8(v)
        }
        Kind::I8 => {
            let v = i8::read(buf).map_err(Error::invalid_data)?;
            Value::I8(v)
        }
        Kind::U16 => {
            let v = u16::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::U16(v)
        }
        Kind::I16 => {
            let v = i16::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::I16(v)
        }
        Kind::U32 => {
            let v = u32::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::U32(v)
        }
        Kind::I32 => {
            let v = i32::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::I32(v)
        }
        Kind::U64 => {
            let v = u64::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::U64(v)
        }
        Kind::I64 => {
            let v = i64::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::I64(v)
        }
        Kind::F32 => {
            let v = f32::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::F32(v)
        }
        Kind::F64 => {
            let v = f64::read_options(buf, endianess.into(), ()).map_err(Error::invalid_data)?;
            Value::F64(v)
        }
        Kind::Array(k, n) => {
            let mut values = Vec::with_capacity(*k);
            for _ in 0..*k {
                let value = parse_value_by_kind(buf, n, map, endianess)?;
                values.push(value);
            }
            Value::Seq(values)
        }
        Kind::DataObject(ref s) => {
            if let Some(object) = map.get(s) {
                let mut values = BTreeMap::new();
                for field in &object.fields {
                    let kind = &field.kind;
                    let value = parse_value_by_kind(buf, kind, map, endianess)?;
                    values.insert(Value::String(field.name.to_string()), value);
                }
                Value::Map(values)
            } else {
                return Err(Error::not_found(s));
            }
        }
    };
    Ok(value)
}

impl ObjectMap {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn merge(&mut self, other: Self) {
        for (k, v) in other.objects {
            self.objects.insert(k, v);
        }
    }
    pub fn extend(&mut self, objects: Vec<DataObject>) {
        for object in objects {
            self.objects.insert(object.name.clone(), object);
        }
    }
    pub fn remove_bulk<B: Borrow<str> + Ord>(&mut self, names: &[B]) {
        for name in names {
            self.objects.remove(name.borrow());
        }
    }
    pub fn validate(&self) -> EResult<()> {
        let mut invalid_objects: BTreeSet<&Name> = <_>::default();
        for v in self.objects.values() {
            for field in &v.fields {
                self.validate_kind(&field.kind, &mut invalid_objects);
            }
        }
        if !invalid_objects.is_empty() {
            let mut err = String::new();
            for (s, i) in invalid_objects.into_iter().enumerate() {
                if s > 0 {
                    err.push_str(", ");
                }
                err.push_str(i);
            }
            return Err(Error::invalid_data(format!("objects not found: {}", err)));
        }
        Ok(())
    }
    pub fn parse_values(
        &self,
        object: &Name,
        buf: &[u8],
        endianess: Endianess,
    ) -> EResult<BTreeMap<OID, Value>> {
        let mut cursor = Cursor::new(buf);
        let mut values = BTreeMap::new();
        self.parse_values_recursive(&mut cursor, object, &self.objects, &mut values, endianess)?;
        Ok(values)
    }
    fn parse_values_recursive(
        &self,
        buf: &mut Cursor<&[u8]>,
        object: &Name,
        map: &BTreeMap<Name, DataObject>,
        values: &mut BTreeMap<OID, Value>,
        endianess: Endianess,
    ) -> EResult<()> {
        let Some(object) = map.get(object) else {
            return Err(Error::not_found(object));
        };
        for field in &object.fields {
            let kind = &field.kind;
            if let Some(ref oid) = field.oid {
                let pos = buf.position();
                let value = parse_value_by_kind(buf, kind, map, endianess)?;
                values.insert(oid.clone(), value);
                buf.set_position(pos);
            }
            match kind {
                Kind::Array(n, ref k) => {
                    if let Kind::DataObject(ref s) = k.as_ref() {
                        for _ in 0..*n {
                            self.parse_values_recursive(buf, s, map, values, endianess)?;
                        }
                    } else {
                        buf.set_position(buf.position() + u64::try_from(self.kind_size(kind)?)?);
                    }
                    continue;
                }
                Kind::DataObject(ref s) => {
                    self.parse_values_recursive(buf, s, map, values, endianess)?;
                    continue;
                }
                _ => {
                    buf.set_position(buf.position() + u64::try_from(self.kind_size(kind)?)?);
                    continue;
                }
            }
        }
        Ok(())
    }
    pub fn size_of(&self, name: &Name) -> EResult<usize> {
        let mut size = 0;
        for field in &self
            .objects
            .get(name)
            .ok_or_else(|| Error::invalid_data(format!("object not found: {}", name)))?
            .fields
        {
            size += self.kind_size(&field.kind)?;
        }
        Ok(size)
    }
    fn kind_size(&self, kind: &Kind) -> EResult<usize> {
        match kind {
            Kind::Bool | Kind::I8 | Kind::U8 => Ok(1),
            Kind::I16 | Kind::U16 => Ok(2),
            Kind::I32 | Kind::U32 | Kind::F32 => Ok(4),
            Kind::I64 | Kind::U64 | Kind::F64 => Ok(8),
            Kind::Array(n, k) => {
                let k_size = self.kind_size(k)?;
                Ok(n * k_size)
            }
            Kind::DataObject(ref s) => self.size_of(s),
        }
    }
    fn validate_kind<'a>(&self, kind: &'a Kind, invalid_objects: &mut BTreeSet<&'a Name>) {
        match kind {
            Kind::Array(_, ref k) => self.validate_kind(k, invalid_objects),
            Kind::DataObject(ref s) => {
                if !self.objects.contains_key(s) {
                    invalid_objects.insert(s);
                }
            }
            _ => {}
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DataObject {
    pub(crate) name: Name,
    #[serde(default)]
    fields: Vec<Field>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Field {
    name: Name,
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    oid: Option<OID>,
}

#[derive(Debug, Ord, PartialEq, Eq, PartialOrd, Clone)]
pub struct Name(String);

impl Deref for Name {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn is_valid_name(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_alphanumeric() || c.is_numeric() || c == '_')
}

impl TryFrom<String> for Name {
    type Error = Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if is_valid_name(&s) {
            Ok(Name(s))
        } else {
            Err(Error::invalid_data(format!("invalid name: {}", s)))
        }
    }
}

impl TryFrom<&str> for Name {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        if is_valid_name(s) {
            Ok(Name(s.to_owned()))
        } else {
            Err(Error::invalid_data(format!("invalid name: {}", s)))
        }
    }
}

impl From<Name> for String {
    fn from(n: Name) -> String {
        n.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Name::try_from(s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Name {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Kind {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Array(usize, Box<Kind>),
    DataObject(Name),
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let mut sp = s.split(',');
        let kind = sp.next().unwrap();
        let maybe_array: Option<usize> = if let Some(n) = sp.next() {
            let n = n.parse().map_err(serde::de::Error::custom)?;
            Some(n)
        } else {
            None
        };
        if sp.next().is_some() {
            return Err(serde::de::Error::custom("too many commas in type"));
        }
        let data_kind = match kind {
            "bool" | "BOOL" | "BOOLEAN" => {
                maybe_array.map_or_else(|| Kind::Bool, |n| Kind::Array(n, Box::new(Kind::Bool)))
            }
            "i8" | "SINT" => {
                maybe_array.map_or_else(|| Kind::I8, |n| Kind::Array(n, Box::new(Kind::I8)))
            }
            "i16" | "INT" => {
                maybe_array.map_or_else(|| Kind::I16, |n| Kind::Array(n, Box::new(Kind::I16)))
            }
            "i32" | "DINT" => {
                maybe_array.map_or_else(|| Kind::I32, |n| Kind::Array(n, Box::new(Kind::I32)))
            }
            "i64" | "LINT" => {
                maybe_array.map_or_else(|| Kind::I64, |n| Kind::Array(n, Box::new(Kind::I64)))
            }
            "u8" | "USINT" => {
                maybe_array.map_or_else(|| Kind::U8, |n| Kind::Array(n, Box::new(Kind::U8)))
            }
            "u16" | "UINT" => {
                maybe_array.map_or_else(|| Kind::U16, |n| Kind::Array(n, Box::new(Kind::U16)))
            }
            "u32" | "UDINT" => {
                maybe_array.map_or_else(|| Kind::U32, |n| Kind::Array(n, Box::new(Kind::U32)))
            }
            "u64" | "ULINT" => {
                maybe_array.map_or_else(|| Kind::U64, |n| Kind::Array(n, Box::new(Kind::U64)))
            }
            "f32" | "REAL" => {
                maybe_array.map_or_else(|| Kind::F32, |n| Kind::Array(n, Box::new(Kind::F32)))
            }
            "f64" | "LREAL" => {
                maybe_array.map_or_else(|| Kind::F64, |n| Kind::Array(n, Box::new(Kind::F64)))
            }
            v => {
                let name: Name = Name::try_from(v).map_err(serde::de::Error::custom)?;
                if let Some(n) = maybe_array {
                    Kind::Array(n, Box::new(Kind::DataObject(name)))
                } else {
                    Kind::DataObject(name)
                }
            }
        };
        Ok(data_kind)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Bool => write!(f, "bool"),
            Kind::I8 => write!(f, "i8"),
            Kind::I16 => write!(f, "i16"),
            Kind::I32 => write!(f, "i32"),
            Kind::I64 => write!(f, "i64"),
            Kind::U8 => write!(f, "u8"),
            Kind::U16 => write!(f, "u16"),
            Kind::U32 => write!(f, "u32"),
            Kind::U64 => write!(f, "u64"),
            Kind::F32 => write!(f, "f32"),
            Kind::F64 => write!(f, "f64"),
            Kind::Array(n, k) => write!(f, "{},{}", k, n),
            Kind::DataObject(s) => write!(f, "{}", s),
        }
    }
}

impl Serialize for Kind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
