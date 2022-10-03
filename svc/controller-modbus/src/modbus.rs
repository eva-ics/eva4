use crate::types::ModbusType;
use crate::types::{Register, RegisterKind, RegisterValue};
use eva_common::op::Op;
use eva_common::{value::Value, EResult, Error};
use eva_sdk::bitman::BitMan;
use ieee754::{Bits, Ieee754};
use std::time::Duration;

const ERR_INVALID_VALUE: &str = "invalid modbus value";

#[allow(clippy::too_many_arguments)]
pub async fn set(
    unit: u8,
    reg: Register,
    value: Value,
    tp: Option<ModbusType>,
    bit: Option<u8>,
    timeout: Duration,
    verify: bool,
    retries: u8,
) -> EResult<()> {
    match value {
        Value::Bool(_) => {}
        Value::String(_) => {
            return Err(Error::invalid_params(ERR_INVALID_VALUE));
        }
        ref v => {
            if !v.is_numeric() {
                return Err(Error::invalid_params(ERR_INVALID_VALUE));
            }
        }
    }
    let mut result = None;
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let r = bus_set(unit, reg, value.clone(), tp, bit, verify, &op).await;
        if r.is_ok() {
            result = Some(r);
            break;
        }
        result = Some(r);
    }
    result.unwrap_or_else(|| Err(Error::timeout()))
}

pub async fn get(
    unit: u8,
    reg: Register,
    count: Option<u16>,
    tp: Option<ModbusType>,
    bit: Option<u8>,
    timeout: Duration,
    retries: u8,
) -> EResult<Vec<RegisterValue>> {
    let tp = tp.unwrap_or_else(|| {
        if bit.is_some() || reg.kind().is_bit_reg() {
            ModbusType::Bit
        } else {
            ModbusType::Uint16
        }
    });
    let mut result = None;
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let r = bus_get(unit, reg, count, tp, bit, op.timeout()?).await;
        if r.is_ok() {
            result = Some(r);
            break;
        }
        result = Some(r);
    }
    result.unwrap_or_else(|| Err(Error::timeout()))
}

async fn bus_set(
    unit: u8,
    reg: Register,
    value: Value,
    tp: Option<ModbusType>,
    bit: Option<u8>,
    verify: bool,
    op: &Op,
) -> EResult<()> {
    let tp = tp.unwrap_or_else(|| {
        if bit.is_some() || reg.kind().is_bit_reg() {
            ModbusType::Bit
        } else {
            ModbusType::Uint16
        }
    });
    let verify_value = if verify {
        Some(tp.normalize_value(value.clone())?)
    } else {
        None
    };
    set_reg(unit, reg, value, tp, bit, op).await?;
    if verify {
        if let Some(delay) = crate::VERIFY_DELAY.get().unwrap() {
            tokio::time::sleep(*delay).await;
        }
        let val = bus_get(unit, reg, None, tp, bit, op.timeout()?)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::invalid_data("invalid data received"))?;
        if Into::<Value>::into(val) != verify_value.unwrap() {
            return Err(Error::failed("modbus set op verification failed"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::cast_sign_loss)]
async fn set_reg(
    unit: u8,
    reg: Register,
    value: Value,
    tp: ModbusType,
    bit: Option<u8>,
    op: &Op,
) -> EResult<()> {
    let modbus_client = crate::MODBUS_CLIENT.get().unwrap();
    match reg.kind() {
        RegisterKind::Input | RegisterKind::Discrete => Err(Error::access(
            "attempt to write to read/only modbus register",
        )),
        RegisterKind::Coil => {
            if tp != ModbusType::Bit {
                return Err(Error::invalid_params("Only bits can be written into coils"));
            }
            let val = if let Value::Bool(v) = value {
                v
            } else {
                TryInto::<u64>::try_into(value)? > 0
            };
            tokio::time::timeout(op.timeout()?, modbus_client.set_bool(unit, reg, val))
                .await?
                .map_err(Into::into)
        }
        RegisterKind::Holding => {
            let vals: Vec<u16> = match tp {
                ModbusType::Bit => {
                    let bit_no =
                        bit.ok_or_else(|| Error::invalid_params("bit number not specified"))?;
                    let val = if let Value::Bool(v) = value {
                        v
                    } else {
                        TryInto::<u64>::try_into(value)? > 0
                    };
                    let data = bus_get(unit, reg, None, tp, None, op.timeout()?).await?;
                    let oldv = data
                        .into_iter()
                        .next()
                        .ok_or_else(|| Error::invalid_data("invalid data received"))?;
                    let n: u16 = Into::<Value>::into(oldv).try_into()?;
                    vec![n.with_bit(u32::from(bit_no), val)]
                }
                ModbusType::Uint16 => {
                    vec![value.try_into()?]
                }
                ModbusType::Int16 => {
                    vec![TryInto::<i16>::try_into(value)? as u16]
                }
                ModbusType::Uint32 => {
                    let val: u32 = value.try_into()?;
                    let b = val.to_be_bytes();
                    vec![
                        (u16::from(b[0]) << 8) + u16::from(b[1]),
                        (u16::from(b[2]) << 8) + u16::from(b[3]),
                    ]
                }
                ModbusType::Int32 => {
                    let val: i32 = value.try_into()?;
                    let b = val.to_be_bytes();
                    vec![
                        (u16::from(b[0]) << 8) + u16::from(b[1]),
                        (u16::from(b[2]) << 8) + u16::from(b[3]),
                    ]
                }
                ModbusType::Uint64 => {
                    let val: u64 = value.try_into()?;
                    let b = val.to_be_bytes();
                    vec![
                        (u16::from(b[0]) << 8) + u16::from(b[1]),
                        (u16::from(b[2]) << 8) + u16::from(b[3]),
                        (u16::from(b[4]) << 8) + u16::from(b[5]),
                        (u16::from(b[6]) << 8) + u16::from(b[7]),
                    ]
                }
                ModbusType::Int64 => {
                    let val: i64 = value.try_into()?;
                    let b = val.to_be_bytes();
                    vec![
                        (u16::from(b[0]) << 8) + u16::from(b[1]),
                        (u16::from(b[2]) << 8) + u16::from(b[3]),
                        (u16::from(b[4]) << 8) + u16::from(b[5]),
                        (u16::from(b[6]) << 8) + u16::from(b[7]),
                    ]
                }
                ModbusType::Real32 => {
                    let val: f32 = value.try_into()?;
                    #[allow(clippy::cast_possible_truncation)]
                    let n: u32 = val.bits().as_u64() as u32;
                    let b = n.to_be_bytes();
                    vec![
                        (u16::from(b[2]) << 8) + u16::from(b[3]),
                        (u16::from(b[0]) << 8) + u16::from(b[1]),
                    ]
                }
                ModbusType::Real64 => {
                    let val: f64 = value.try_into()?;
                    let n: u64 = val.bits().as_u64();
                    let b = n.to_be_bytes();
                    vec![
                        (u16::from(b[6]) << 8) + u16::from(b[7]),
                        (u16::from(b[4]) << 8) + u16::from(b[5]),
                        (u16::from(b[2]) << 8) + u16::from(b[3]),
                        (u16::from(b[0]) << 8) + u16::from(b[1]),
                    ]
                }
            };
            tokio::time::timeout(op.timeout()?, modbus_client.set_u16_bulk(unit, reg, &vals))
                .await?
                .map_err(Into::into)
        }
    }
}

#[allow(clippy::cast_sign_loss)]
#[allow(clippy::cast_possible_truncation)]
async fn bus_get(
    unit: u8,
    reg: Register,
    count: Option<u16>,
    tp: ModbusType,
    bit: Option<u8>,
    timeout: Duration,
) -> EResult<Vec<RegisterValue>> {
    let modbus_client = crate::MODBUS_CLIENT.get().unwrap();
    let reg_kind = reg.kind();
    let reg_no = reg.number();
    match reg_kind {
        RegisterKind::Coil | RegisterKind::Discrete => {
            if tp == ModbusType::Bit {
                let mut result = Vec::new();
                for (i, val) in tokio::time::timeout(
                    timeout,
                    modbus_client.get_bool(unit, reg, count.unwrap_or(1)),
                )
                .await??
                .into_iter()
                .enumerate()
                {
                    let rn = i + reg_no as usize;
                    if rn > u16::MAX as usize {
                        break;
                    };
                    result.push(RegisterValue::new0(
                        reg_kind,
                        rn as u16,
                        Value::U8(if val { 1 } else { 0 }),
                    ));
                }
                Ok(result)
            } else {
                Err(Error::invalid_params(
                    "only type \"bit\" can be specified for coils/discretes",
                ))
            }
        }
        RegisterKind::Holding | RegisterKind::Input => {
            let count = count.unwrap_or(1);
            let get_count = if tp == ModbusType::Bit {
                (f64::from(count) / 8.0).ceil() as u16
            } else {
                count
            };
            let tp_size = tp.size();
            if get_count as usize * tp_size as usize > u16::MAX as usize {
                return Err(Error::invalid_params("invalid range"));
            }
            let data = tokio::time::timeout(
                timeout,
                modbus_client.get_u16(unit, reg, get_count * tp_size),
            )
            .await??;
            let mut result = Vec::new();
            if tp == ModbusType::Bit {
                let mut bit_no = bit.unwrap_or_default();
                if bit_no > 15 {
                    return Err(Error::invalid_params("invalid bit range"));
                }
                let mut rn = reg_no;
                for _ in 0..count {
                    let val = parse_block_value(&data, rn - reg_no, tp, Some(bit_no))?;
                    result.push(RegisterValue::new(reg_kind, rn, bit_no, val));
                    if bit_no > 15 {
                        bit_no = 0;
                        rn += 1;
                    } else {
                        bit_no += 1;
                    }
                }
            } else {
                for i in 0..count {
                    let rn = i * tp_size;
                    let val = parse_block_value(&data, rn, tp, None)?;
                    result.push(RegisterValue::new0(reg_kind, reg_no + rn, val));
                }
            }
            Ok(result)
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::too_many_lines)]
pub fn parse_block_value(
    block: &[u16],
    offset: u16,
    tp: ModbusType,
    bit: Option<u8>,
) -> EResult<Value> {
    macro_rules! get {
        ($o: expr) => {
            *block.get($o as usize).ok_or_else(|| {
                Error::invalid_data("the block does not contain the requested data")
            })?
        };
    }
    Ok(match tp {
        ModbusType::Bit => {
            let reg_value = get!(offset);
            Value::U8(
                if reg_value.get_bit(u32::from(
                    bit.ok_or_else(|| Error::invalid_params("bit not specified"))?,
                )) {
                    1
                } else {
                    0
                },
            )
        }
        ModbusType::Uint16 => Value::U16(get!(offset)),
        ModbusType::Int16 => Value::I16(get!(offset) as i16),
        ModbusType::Uint32 => {
            let v1 = get!(offset);
            let v2 = get!(offset + 1);
            Value::U32(u32::from_be_bytes([
                (v1 >> 8) as u8,
                v1 as u8,
                (v2 >> 8) as u8,
                v2 as u8,
            ]))
        }
        ModbusType::Int32 => {
            let v1 = get!(offset);
            let v2 = get!(offset + 1);
            Value::I32(i32::from_be_bytes([
                (v1 >> 8) as u8,
                v1 as u8,
                (v2 >> 8) as u8,
                v2 as u8,
            ]))
        }
        ModbusType::Uint64 => {
            let v1 = get!(offset);
            let v2 = get!(offset + 1);
            let v3 = get!(offset + 2);
            let v4 = get!(offset + 3);
            Value::U64(u64::from_be_bytes([
                (v1 >> 8) as u8,
                v1 as u8,
                (v2 >> 8) as u8,
                v2 as u8,
                (v3 >> 8) as u8,
                v3 as u8,
                (v4 >> 8) as u8,
                v4 as u8,
            ]))
        }
        ModbusType::Int64 => {
            let v1 = get!(offset);
            let v2 = get!(offset + 1);
            let v3 = get!(offset + 2);
            let v4 = get!(offset + 3);
            Value::I64(i64::from_be_bytes([
                (v1 >> 8) as u8,
                v1 as u8,
                (v2 >> 8) as u8,
                v2 as u8,
                (v3 >> 8) as u8,
                v3 as u8,
                (v4 >> 8) as u8,
                v4 as u8,
            ]))
        }
        ModbusType::Real32 => {
            let v1 = get!(offset);
            let v2 = get!(offset + 1);
            let val = u32::from_be_bytes([(v2 >> 8) as u8, v2 as u8, (v1 >> 8) as u8, v1 as u8]);
            let val_f: f32 = Ieee754::from_bits(val);
            if !val_f.is_finite() {
                return Err(Error::invalid_data(ERR_INVALID_VALUE));
            }
            Value::F32(val_f)
        }
        ModbusType::Real64 => {
            let v1 = get!(offset);
            let v2 = get!(offset + 1);
            let v3 = get!(offset + 2);
            let v4 = get!(offset + 3);
            let val = u64::from_be_bytes([
                (v4 >> 8) as u8,
                v4 as u8,
                (v3 >> 8) as u8,
                v3 as u8,
                (v2 >> 8) as u8,
                v2 as u8,
                (v1 >> 8) as u8,
                v1 as u8,
            ]);
            let val_f: f64 = Ieee754::from_bits(val);
            if !val_f.is_finite() {
                return Err(Error::invalid_data(ERR_INVALID_VALUE));
            }
            Value::F64(val_f)
        }
    })
}
