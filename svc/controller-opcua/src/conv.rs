use eva_common::value::Value;
use eva_common::{EResult, Error};
use opcua::types::{Array, Variant, VariantTypeId};

#[allow(clippy::module_name_repetitions)]
pub trait ValueConv {
    fn into_eva_value(self) -> EResult<Value>;
    fn from_eva_value(val: Value, vt: VariantTypeId, dimensions: Option<&[usize]>) -> EResult<Self>
    where
        Self: Sized;
}

impl ValueConv for Variant {
    fn into_eva_value(self) -> EResult<Value> {
        match self {
            Variant::Empty => Ok(Value::Unit),
            Variant::Boolean(v) => Ok(Value::Bool(v)),
            Variant::SByte(v) => Ok(Value::I8(v)),
            Variant::Byte(v) => Ok(Value::U8(v)),
            Variant::Int16(v) => Ok(Value::I16(v)),
            Variant::UInt16(v) => Ok(Value::U16(v)),
            Variant::Int32(v) => Ok(Value::I32(v)),
            Variant::UInt32(v) => Ok(Value::U32(v)),
            Variant::Int64(v) => Ok(Value::I64(v)),
            Variant::UInt64(v) => Ok(Value::U64(v)),
            Variant::Float(v) => Ok(Value::F32(v)),
            Variant::Double(v) => Ok(Value::F64(v)),
            Variant::String(v) => Ok(Value::String(v.into())),
            Variant::DateTime(v) => Ok(Value::I64(v.ticks() * 100)),
            Variant::ByteString(v) => Ok(Value::Bytes(v.value.unwrap_or_default())),
            Variant::Array(v) => {
                let mut data = Vec::with_capacity(v.values.len());
                for val in v.values {
                    data.push(val.into_eva_value()?);
                }
                let value = Value::Seq(data);
                if let Some(dim) = v.dimensions {
                    let dimensions: Vec<usize> = dim
                        .into_iter()
                        .map(usize::try_from)
                        .collect::<Result<_, _>>()?;
                    Ok(value.into_seq_reshaped(&dimensions))
                } else {
                    Ok(value)
                }
            }
            _ => Err(Error::unsupported("unsupported OPC to data type")),
        }
    }
    fn from_eva_value(
        val: Value,
        vt: VariantTypeId,
        dimensions: Option<&[usize]>,
    ) -> EResult<Self> {
        match val {
            Value::Unit => Ok(Variant::Empty),
            Value::Bool(v) => Ok(Variant::Boolean(v)),
            Value::I8(v) => Ok(Variant::SByte(v).cast(vt)),
            Value::U8(v) => Ok(Variant::Byte(v).cast(vt)),
            Value::I16(v) => Ok(Variant::Int16(v).cast(vt)),
            Value::U16(v) => Ok(Variant::UInt16(v).cast(vt)),
            Value::I32(v) => Ok(Variant::Int32(v).cast(vt)),
            Value::U32(v) => Ok(Variant::UInt32(v).cast(vt)),
            Value::I64(v) => Ok(Variant::Int64(v).cast(vt)),
            Value::U64(v) => Ok(Variant::UInt64(v).cast(vt)),
            Value::F32(v) => Ok(Variant::Float(v).cast(vt)),
            Value::F64(v) => Ok(Variant::Double(v).cast(vt)),
            Value::String(v) => Ok(Variant::String(v.into())),
            Value::Bytes(v) => Ok(Variant::ByteString(v.into())),
            Value::Seq(_) => {
                let Value::Seq(s) = val.into_seq_flatten() else { panic!() };
                let mut data = Vec::with_capacity(s.len());
                for value in s {
                    data.push(Variant::from_eva_value(value, vt, None)?);
                }
                let mut arr = Array::new(vt, data).map_err(Error::invalid_data)?;
                if let Some(dim) = dimensions {
                    arr.dimensions = Some(
                        dim.iter()
                            .map(|v| u32::try_from(*v))
                            .collect::<Result<_, _>>()?,
                    );
                    if !arr.is_valid() {
                        return Err(Error::invalid_data(
                            "invalid array length for the chosen dimensions",
                        ));
                    }
                }
                Ok(Variant::Array(Box::new(arr)))
            }
            _ => Err(Error::unsupported("unsupported data type to OPC")),
        }
    }
}
