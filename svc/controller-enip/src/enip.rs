use crate::types::EipType;
use eva_common::op::Op;
use eva_common::prelude::*;
use log::{error, trace};
use std::ffi;
use std::time::Duration;

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub fn safe_get_bit(tag: i32, bit_offset: i32) -> EResult<u8> {
    let rc = unsafe { plctag::plc_tag_get_bit(tag, bit_offset) };
    if rc < 0 {
        Err(Error::failed(format!("libplctag error {}", rc)))
    } else {
        Ok(rc as u8)
    }
}

#[allow(clippy::cast_possible_truncation)]
fn plc_read_tag(path: &str, tag_id: i32, timeout: Duration) -> EResult<()> {
    let op = Op::new(timeout);
    loop {
        let rc = unsafe { plctag::plc_tag_read(tag_id, timeout.as_millis() as i32) };
        match rc {
            plctag::PLCTAG_STATUS_OK => break,
            plctag::PLCTAG_ERR_BUSY => std::thread::sleep(crate::SLEEP_STEP),
            plctag::PLCTAG_ERR_TIMEOUT => {
                error!("PLC tag read timeout {}", path);
                return Err(Error::timeout());
            }
            _ => {
                let msg = format!("PLC tag read error {}: {}", path, rc);
                error!("{}", msg);
                return Err(Error::failed(msg));
            }
        }
        if op.is_timed_out() {
            error!("PLC tag read timeout (busy) {}", path);
            return Err(Error::timeout());
        }
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
fn plc_write_tag(path: &str, tag_id: i32, timeout: Duration) -> EResult<()> {
    let op = Op::new(timeout);
    loop {
        let rc = unsafe { plctag::plc_tag_write(tag_id, timeout.as_millis() as i32) };
        match rc {
            plctag::PLCTAG_STATUS_OK => break,
            plctag::PLCTAG_ERR_BUSY => std::thread::sleep(crate::SLEEP_STEP),
            plctag::PLCTAG_ERR_TIMEOUT => {
                error!("PLC tag write timeout {}", path);
                return Err(Error::timeout());
            }
            _ => {
                let msg = format!("PLC tag write error {}: {}", path, rc);
                error!("{}", msg);
                return Err(Error::failed(msg));
            }
        }
        if op.is_timed_out() {
            error!("PLC tag write timeout (busy) {}", path);
            return Err(Error::timeout());
        }
    }
    Ok(())
}

pub fn pull_tag(path: &str, tag_id: i32, timeout: Duration, retries: u8) -> EResult<()> {
    let mut result = None;
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let res = plc_read_tag(path, tag_id, op.timeout()?);
        if res.is_ok() {
            result = Some(res);
            break;
        }
        result = Some(res);
    }
    result.unwrap_or_else(|| Err(Error::timeout()))
}

pub fn create_client_tag(path: &str, plc_timeout: i32) -> EResult<i32> {
    trace!("creating client tag: {}", path);
    let c_path = ffi::CString::new(path.to_owned()).map_err(Error::failed)?;
    Ok(unsafe { plctag::plc_tag_create(c_path.as_ptr(), plc_timeout) })
}

// TODO wait if pending by another thread
#[allow(clippy::cast_possible_truncation)]
fn get_tag_id(path: &str, timeout: Duration) -> EResult<i32> {
    let op = Op::new(timeout);
    if let Some(id) = crate::PREPARED_TAGS.lock().unwrap().get(path) {
        return Ok(*id);
    }
    let id = create_client_tag(path, timeout.as_millis() as i32)?;
    loop {
        let rc = unsafe { plctag::plc_tag_status(id) };
        if rc == plctag::PLCTAG_STATUS_OK {
            break;
        } else if rc != plctag::PLCTAG_STATUS_PENDING {
            return Err(Error::failed(format!(
                "Unable to create client PLC tag {}: {}",
                path, rc
            )));
        }
        if op.is_timed_out() {
            error!("PLC timeout while creating client tag {}", path);
            return Err(Error::timeout());
        }
        std::thread::sleep(crate::SLEEP_STEP);
    }
    crate::PREPARED_TAGS
        .lock()
        .unwrap()
        .insert(path.to_owned(), id);
    Ok(id)
}

#[inline]
pub fn format_tag_path(plc_path: &str, tag: &str, size: Option<u32>, count: Option<u32>) -> String {
    format!(
        "{}{}{}&name={}",
        plc_path,
        if let Some(s) = size {
            format!("&elem_size={}", s)
        } else {
            String::new()
        },
        if let Some(c) = count {
            format!("&elem_count={}", c)
        } else {
            String::new()
        },
        tag
    )
}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::cast_possible_truncation)]
fn read_tag_sync(
    tag: &str,
    tp: EipType,
    size: Option<u32>,
    count: Option<u32>,
    bit: Option<u32>,
    timeout: Duration,
) -> EResult<Value> {
    if tp == EipType::Bit && bit.is_none() {
        return Err(Error::invalid_params("no bit index specified"));
    }
    let op = Op::new(timeout);
    let path = format_tag_path(
        crate::PLC_PATH.get().unwrap(),
        tag,
        size,
        if bit.is_none() { count } else { None },
    );
    let id = get_tag_id(&path, op.timeout()?)?;
    plc_read_tag(&path, id, op.timeout()?)?;
    macro_rules! process_tag {
        ($fn:path, $vt: expr) => {{
            let cnt = count.unwrap_or(1);
            if cnt == 1 {
                $vt($fn(id, 0))
            } else {
                let mut values: Vec<Value> = Vec::new();
                let sb = i32::from(tp.size_bytes());
                for c in 0..cnt {
                    let val = $vt($fn(id, sb * c as i32));
                    values.push(val);
                }
                Value::Seq(values)
            }
        }};
    }
    let value = match tp {
        EipType::Bit => {
            let offset = bit.unwrap();
            let cnt = count.unwrap_or(1);
            if cnt == 1 {
                if offset > i32::MAX as u32 {
                    return Err(Error::invalid_params("bit index overflow"));
                }
                Value::U8(safe_get_bit(id, offset as i32)?)
            } else {
                let mut values: Vec<Value> = Vec::new();
                for c in 0..cnt {
                    let bit_no = c + offset;
                    if bit_no > i32::MAX as u32 {
                        return Err(Error::invalid_params("bit index overflow"));
                    }
                    let val = safe_get_bit(id, bit_no as i32)?;
                    values.push(Value::U8(val));
                }
                Value::Seq(values)
            }
        }
        EipType::Uint8 => unsafe { process_tag!(plctag::plc_tag_get_uint8, Value::U8) },
        EipType::Int8 => unsafe { process_tag!(plctag::plc_tag_get_int8, Value::I8) },
        EipType::Uint16 => unsafe { process_tag!(plctag::plc_tag_get_uint16, Value::U16) },
        EipType::Int16 => unsafe { process_tag!(plctag::plc_tag_get_int16, Value::I16) },
        EipType::Uint32 => unsafe { process_tag!(plctag::plc_tag_get_uint32, Value::U32) },
        EipType::Int32 => unsafe { process_tag!(plctag::plc_tag_get_int32, Value::I32) },
        EipType::Uint64 => unsafe { process_tag!(plctag::plc_tag_get_uint64, Value::U64) },
        EipType::Int64 => unsafe { process_tag!(plctag::plc_tag_get_int64, Value::I64) },
        EipType::Real32 => unsafe { process_tag!(plctag::plc_tag_get_float32, Value::F32) },
        EipType::Real64 => unsafe { process_tag!(plctag::plc_tag_get_float64, Value::F64) },
    };
    Ok(value)
}

pub async fn read_tag(
    tag: String,
    tp: EipType,
    size: Option<u32>,
    count: Option<u32>,
    bit: Option<u32>,
    timeout: Duration,
    retries: u8,
) -> EResult<Value> {
    tokio::task::spawn_blocking(move || {
        let mut result = None;
        let op = Op::new(timeout);
        for _ in 0..=retries {
            let res = read_tag_sync(&tag, tp, size, count, bit, op.timeout()?);
            if res.is_ok() {
                result = Some(res);
                break;
            }
            result = Some(res);
        }
        result.unwrap_or_else(|| Err(Error::timeout()))
    })
    .await
    .map_err(Error::failed)?
}

pub async fn write_tag(
    tag: String,
    value: Value,
    tp: EipType,
    bit: Option<u32>,
    timeout: Duration,
    verify: bool,
    retries: u8,
) -> EResult<()> {
    tokio::task::spawn_blocking(move || {
        let mut result = None;
        let op = Op::new(timeout);
        for _ in 0..=retries {
            let res = write_tag_sync(
                &tag,
                tp.normalize_value(value.clone())?,
                tp,
                bit,
                op.timeout()?,
                verify,
            );
            if res.is_ok() {
                result = Some(res);
                break;
            }
            result = Some(res);
        }
        result.unwrap_or_else(|| Err(Error::timeout()))
    })
    .await
    .map_err(Error::failed)?
}

#[allow(clippy::cast_possible_wrap)]
fn write_tag_sync(
    tag: &str,
    value: Value,
    tp: EipType,
    bit: Option<u32>,
    timeout: Duration,
    verify: bool,
) -> EResult<()> {
    let op = Op::new(timeout);
    if tp == EipType::Bit && bit.is_none() {
        return Err(Error::invalid_params("no bit index specified"));
    }
    let path = format_tag_path(crate::PLC_PATH.get().unwrap(), tag, None, None);
    let id = get_tag_id(&path, timeout)?;
    let val_check = if verify { Some(value.clone()) } else { None };
    macro_rules! process_tag {
        ($fn:path) => {{
            let rc = $fn(id, 0, value.try_into()?);
            if rc != plctag::PLCTAG_STATUS_OK {
                return Err(Error::failed(format!("PLC tag {} set error: {}", path, rc)));
            }
        }};
    }
    match tp {
        EipType::Bit => {
            let bit_no = bit.unwrap();
            if bit_no > i32::MAX as u32 {
                return Err(Error::invalid_params(format!(
                    "bit index overflow for {}",
                    path
                )));
            }
            let rc = unsafe {
                plctag::plc_tag_set_bit(
                    id,
                    bit_no as i32,
                    i32::from(TryInto::<u8>::try_into(value)?),
                )
            };
            if rc != plctag::PLCTAG_STATUS_OK {
                return Err(Error::failed(format!("PLC tag {} set error: {}", path, rc)));
            }
        }
        EipType::Uint8 => unsafe { process_tag!(plctag::plc_tag_set_uint8) },
        EipType::Int8 => unsafe { process_tag!(plctag::plc_tag_set_int8) },
        EipType::Uint16 => unsafe { process_tag!(plctag::plc_tag_set_uint16) },
        EipType::Int16 => unsafe { process_tag!(plctag::plc_tag_set_int16) },
        EipType::Uint32 => unsafe { process_tag!(plctag::plc_tag_set_uint32) },
        EipType::Int32 => unsafe { process_tag!(plctag::plc_tag_set_int32) },
        EipType::Uint64 => unsafe { process_tag!(plctag::plc_tag_set_uint64) },
        EipType::Int64 => unsafe { process_tag!(plctag::plc_tag_set_int64) },
        EipType::Real32 => unsafe { process_tag!(plctag::plc_tag_set_float32) },
        EipType::Real64 => unsafe { process_tag!(plctag::plc_tag_set_float64) },
    }
    plc_write_tag(&path, id, op.timeout()?)?;
    if verify {
        if let Some(d) = crate::VERIFY_DELAY.get().unwrap() {
            let verify_delay = *d;
            op.enough(verify_delay)?;
            std::thread::sleep(verify_delay);
        }
        let val = read_tag_sync(
            tag,
            tp,
            Some(u32::from(tp.size_bytes())),
            None,
            bit,
            op.timeout()?,
        )?;
        if val != val_check.unwrap() {
            return Err(Error::failed(format!(
                "PLC tag {} set/write error: verification failed",
                path
            )));
        }
    }
    Ok(())
}
