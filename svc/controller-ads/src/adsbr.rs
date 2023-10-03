use crate::get_device;
use eva_common::op::Op;
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use eva_sdk::service::safe_rpc_call;
use log::error;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;
use zerocopy::{AsBytes, FromBytes};

const ADS_SYM_INFO_BUF_SIZE: usize = 256;
const ADS_VAR_MAX_STR_LEN: u32 = 1024;

static BULK_ALLOW: atomic::AtomicBool = atomic::AtomicBool::new(true);

static ERR_INVALID_BUFFER_LENGTH: &str = "invalid buffer length";

#[inline]
pub fn set_bulk_allow(mode: bool) {
    BULK_ALLOW.store(mode, atomic::Ordering::SeqCst);
}

#[inline]
fn bulk_allow() -> bool {
    BULK_ALLOW.load(atomic::Ordering::SeqCst)
}

pub trait ParseAmsNetId {
    fn ams_net_id(&self) -> EResult<[u8; 6]>;
}

impl ParseAmsNetId for String {
    fn ams_net_id(&self) -> EResult<[u8; 6]> {
        let chunks: Vec<&str> = self.split('.').collect();
        if chunks.len() == 6 || chunks.get(6).map_or(false, |v| v.is_empty()) {
            let mut res = Vec::with_capacity(6);
            for c in chunks.iter().take(6) {
                res.push(
                    c.parse()
                        .map_err(|e| Error::invalid_data(format!("invalid AMSNetId: {}", e)))?,
                );
            }
            res.try_into()
                .map_err(|e| Error::invalid_data(format!("invalid AMSNetId: {:?}", e)))
        } else {
            Err(Error::invalid_data("invalid AMSNetId: too many chunks"))
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    #[serde(alias = "g")]
    index_group: u32,
    #[serde(alias = "o")]
    index_offset: u32,
    #[serde(alias = "s")]
    size: usize,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteReadRequest<'a> {
    #[serde(alias = "g")]
    index_group: u32,
    #[serde(alias = "o")]
    index_offset: u32,
    #[serde(alias = "d")]
    data: &'a [u8],
    #[serde(alias = "s")]
    size: usize,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRequest<'a> {
    #[serde(alias = "g")]
    index_group: u32,
    #[serde(alias = "o")]
    index_offset: u32,
    #[serde(alias = "d")]
    data: &'a [u8],
}

#[derive(Serialize, Deserialize)]
pub struct SumUpResult {
    #[serde(rename = "c")]
    code: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "d")]
    data: Option<Vec<u8>>,
}

#[derive(Serialize)]
struct ParamsPing {
    n: [u8; 6],
    p: u16,
}

#[derive(Serialize)]
struct ParamsWriteRead<'a> {
    n: [u8; 6],
    p: u16,
    g: u32,
    o: u32,
    d: &'a [u8],
    s: usize,
}

#[derive(Serialize)]
struct ParamsRead {
    n: [u8; 6],
    p: u16,
    g: u32,
    o: u32,
    s: usize,
}

#[derive(Serialize)]
struct ParamsWrite<'a> {
    n: [u8; 6],
    p: u16,
    g: u32,
    o: u32,
    d: &'a [u8],
}

#[derive(Serialize)]
struct ParamsSuRead {
    n: [u8; 6],
    p: u16,
    r: Vec<ReadRequest>,
}

#[derive(Serialize)]
struct ParamsSuWrite<'a> {
    n: [u8; 6],
    p: u16,
    r: Vec<WriteRequest<'a>>,
}

#[derive(Serialize)]
struct ParamsSuWriteRead<'a> {
    n: [u8; 6],
    p: u16,
    r: Vec<WriteReadRequest<'a>>,
}

#[derive(Debug, Clone)]
pub struct Var {
    name: String,
    kind: Kind,
    array_len: Option<u32>,
    // TODO remove Arc when async client will be available
    handle: Arc<atomic::AtomicU32>,
    size: u32,
}

impl Var {
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn check(&self) -> EResult<()> {
        if self.handle.load(atomic::Ordering::SeqCst) == 0 {
            Err(Error::io(format!("{} handle not created", self.name)))
        } else {
            match self.kind {
                Kind::Other => Err(Error::io(format!("{} type not supported", self.name))),
                Kind::NotFound => Err(Error::io(format!("{} not found", self.name))),
                Kind::UnsupportedImpl => Err(Error::io(format!(
                    "{} type unsupported implementation",
                    self.name
                ))),
                _ => Ok(()),
            }
        }
    }
    fn buf_size(&self) -> usize {
        match self.kind {
            Kind::Str | Kind::WStr => (ADS_VAR_MAX_STR_LEN * self.array_len.unwrap_or(1))
                .try_into()
                .unwrap(),
            _ => self.size.try_into().unwrap(),
        }
    }
    fn prepare_value_buf(&self, value: &Value) -> EResult<Vec<u8>> {
        if let Value::Seq(s) = value {
            let mut result = Vec::with_capacity(s.len());
            for v in s {
                result.extend(self.prepare_value_buf(v)?);
            }
            Ok(result)
        } else {
            Ok(match self.kind {
                Kind::Bool => {
                    let val: bool = value.clone().try_into()?;
                    vec![u8::from(val)]
                }
                Kind::Int => TryInto::<i16>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Dint => TryInto::<i32>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Real => TryInto::<f32>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Lreal => TryInto::<f64>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Sint => TryInto::<i8>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Usint => TryInto::<u8>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Uint => TryInto::<u16>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Udint => TryInto::<u32>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Lint => TryInto::<i64>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Ulint => TryInto::<u64>::try_into(value.clone())?.as_bytes().to_vec(),
                Kind::Str => {
                    let mut val = if let Value::String(s) = value {
                        s.as_bytes().to_vec()
                    } else {
                        value.to_string().as_bytes().to_vec()
                    };
                    val.push(0);
                    val
                }
                Kind::WStr => {
                    let mut val = if let Value::String(s) = value {
                        utf16string::WString::<utf16string::LittleEndian>::from(s)
                            .as_bytes()
                            .to_vec()
                    } else {
                        utf16string::WString::<utf16string::LittleEndian>::from(&value.to_string())
                            .as_bytes()
                            .to_vec()
                    };
                    val.extend([0, 0]);
                    val
                }
                _ => {
                    return Err(Error::io("unable to write the value type"));
                }
            })
        }
    }
    #[inline]
    fn normalize_value(&self, value: Value) -> EResult<Value> {
        Ok(match self.kind {
            Kind::Int => Value::I16(value.try_into()?),
            Kind::Dint => Value::I32(value.try_into()?),
            Kind::Real => Value::F32(value.try_into()?),
            Kind::Lreal => Value::F64(value.try_into()?),
            Kind::Sint => Value::I8(value.try_into()?),
            Kind::Usint => Value::U8(value.try_into()?),
            Kind::Bool => {
                if let Value::Bool(v) = value {
                    Value::U8(u8::from(v))
                } else {
                    Value::U8(value.try_into()?)
                }
            }
            Kind::Uint => Value::U16(value.try_into()?),
            Kind::Udint => Value::U32(value.try_into()?),
            Kind::Lint => Value::I64(value.try_into()?),
            Kind::Ulint => Value::U64(value.try_into()?),
            Kind::Str | Kind::WStr => value,
            _ => {
                return Err(Error::io("unable to write the value type"));
            }
        })
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum Kind {
    Int = 2,
    Dint = 3,
    Real = 4,
    Lreal = 5,
    Sint = 16,
    Usint = 17,
    Uint = 18,
    Udint = 19,
    Lint = 20,
    Ulint = 21,
    Str = 30,
    WStr = 31,
    Bool = 33,
    Other = 0xffff_ffff,
    NotFound = 0xffff_fffe,
    UnsupportedImpl = 0xffff_fffd,
}

impl Kind {
    fn size(self) -> u32 {
        match self {
            Kind::Sint | Kind::Usint | Kind::Bool | Kind::Str | Kind::WStr => 1,
            Kind::Int | Kind::Uint => 2,
            Kind::Dint | Kind::Udint | Kind::Real => 4,
            Kind::Lint | Kind::Ulint | Kind::Lreal => 8,
            _ => 0,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct SymbolInfo {
    kind: Kind,
    size: u32,
    array_len: Option<u32>,
}

impl From<u32> for Kind {
    fn from(code: u32) -> Self {
        match code {
            v if v == Kind::Int as u32 => Kind::Int,
            v if v == Kind::Dint as u32 => Kind::Dint,
            v if v == Kind::Real as u32 => Kind::Real,
            v if v == Kind::Lreal as u32 => Kind::Lreal,
            v if v == Kind::Sint as u32 => Kind::Sint,
            v if v == Kind::Usint as u32 => Kind::Usint,
            v if v == Kind::Uint as u32 => Kind::Uint,
            v if v == Kind::Udint as u32 => Kind::Udint,
            v if v == Kind::Lint as u32 => Kind::Lint,
            v if v == Kind::Ulint as u32 => Kind::Ulint,
            v if v == Kind::Str as u32 => Kind::Str,
            v if v == Kind::WStr as u32 => Kind::WStr,
            v if v == Kind::Bool as u32 => Kind::Bool,
            _ => Kind::Other,
        }
    }
}

#[allow(clippy::cast_sign_loss)]
async fn bridge_safe_read_multi(requests: Vec<ReadRequest>) -> EResult<Vec<SumUpResult>> {
    let (n, p) = get_device();
    su_call("su_read", &ParamsSuRead { n, p, r: requests }).await
}

#[allow(clippy::cast_sign_loss)]
async fn bridge_safe_write_read_multi(
    requests: Vec<WriteReadRequest<'_>>,
) -> EResult<Vec<SumUpResult>> {
    let (n, p) = get_device();
    if bulk_allow() {
        su_call("su_write_read", &ParamsSuWriteRead { n, p, r: requests }).await
    } else {
        let mut result = Vec::with_capacity(requests.len());
        for req in requests {
            match bridge_write_read(req.index_group, req.index_offset, req.data, req.size).await {
                Ok(data) => result.push(SumUpResult {
                    code: 0,
                    data: Some(data),
                }),
                Err(e) => result.push(SumUpResult {
                    code: -e.code() as u32,
                    data: None,
                }),
            }
        }
        Ok(result)
    }
}

#[allow(clippy::cast_sign_loss)]
async fn bridge_safe_write_multi(requests: Vec<WriteRequest<'_>>) -> EResult<Vec<SumUpResult>> {
    let (n, p) = get_device();
    if bulk_allow() {
        su_call("su_write", &ParamsSuWrite { n, p, r: requests }).await
    } else {
        let mut result = Vec::new();
        for req in requests {
            if let Err(e) = bridge_write(req.index_group, req.index_offset, req.data).await {
                result.push(SumUpResult {
                    code: -e.code() as u32,
                    data: None,
                });
            } else {
                result.push(SumUpResult {
                    code: 0,
                    data: None,
                });
            }
        }
        Ok(result)
    }
}

async fn bridge_read(g: u32, o: u32, size: usize) -> EResult<Vec<u8>> {
    let (n, p) = get_device();
    call(
        "read",
        &ParamsRead {
            n,
            p,
            g,
            o,
            s: size,
        },
    )
    .await
}

async fn bridge_write(g: u32, o: u32, data: &[u8]) -> EResult<()> {
    let (n, p) = get_device();
    call0(
        "write",
        &ParamsWrite {
            n,
            p,
            g,
            o,
            d: data,
        },
    )
    .await
}

async fn bridge_write_read(g: u32, o: u32, data: &[u8], size: usize) -> EResult<Vec<u8>> {
    let (n, p) = get_device();
    call(
        "write_read",
        &ParamsWriteRead {
            n,
            p,
            g,
            o,
            d: data,
            s: size,
        },
    )
    .await
}

async fn create_handles(names: &[&str]) -> EResult<Vec<u32>> {
    let mut result: Vec<u32> = Vec::with_capacity(names.len());
    if !names.is_empty() {
        let mut reqs = Vec::with_capacity(names.len());
        for name in names {
            reqs.push(WriteReadRequest {
                index_group: ::ads::index::GET_SYMHANDLE_BYNAME,
                index_offset: 0,
                data: name.as_bytes(),
                size: 4,
            });
        }
        let sres = bridge_safe_write_read_multi(reqs)
            .await
            .map_err(|e| Error::io(format!("write_read_multi GET_SYMHANDLE_BYNAME: {}", e)))?;
        for r in sres {
            let mut processed = false;
            if r.code == 0 {
                if let Some(data) = r.data {
                    result.push(u32::read_from(data.as_slice()).unwrap_or_default());
                    processed = true;
                }
            }
            if !processed {
                result.push(0);
            }
        }
    }
    Ok(result)
}

pub async fn release_handles(handles: &[u32]) -> EResult<()> {
    if !handles.is_empty() {
        let mut reqs = Vec::with_capacity(handles.len());
        for handle in handles {
            reqs.push(WriteRequest {
                index_group: ::ads::index::RELEASE_SYMHANDLE,
                index_offset: 0,
                data: handle.as_bytes(),
            });
        }
        bridge_safe_write_multi(reqs)
            .await
            .map_err(|e| Error::io(format!("write_multi RELEASE_SYMHANDLE: {}", e)))?;
    }
    Ok(())
}

pub async fn destroy_vars(vars: &[&Var]) -> EResult<()> {
    release_handles(
        &vars
            .iter()
            .map(|v| v.handle.load(atomic::Ordering::SeqCst))
            .collect::<Vec<u32>>(),
    )
    .await?;
    for var in vars {
        var.handle.store(0, atomic::Ordering::SeqCst);
    }
    Ok(())
}

pub async fn create_vars(names: &[&str]) -> EResult<Vec<Var>> {
    let infos = query_type_infos(names).await?;
    let handles = create_handles(names).await?;
    Ok(names
        .iter()
        .zip(infos.into_iter().zip(handles.into_iter()))
        .map(|(name, (info, handle))| Var {
            name: (*name).to_owned(),
            kind: info.kind,
            array_len: info.array_len,
            size: info.size,
            handle: Arc::new(atomic::AtomicU32::new(handle)),
        })
        .collect())
}

async fn query_type_infos(names: &[&str]) -> EResult<Vec<SymbolInfo>> {
    let mut result: Vec<SymbolInfo> = Vec::with_capacity(names.len());
    if !names.is_empty() {
        let mut reqs = Vec::with_capacity(names.len());
        for name in names {
            reqs.push(WriteReadRequest {
                index_group: ::ads::index::GET_SYMINFO_BYNAME_EX,
                index_offset: 0,
                data: name.as_bytes(),
                size: ADS_SYM_INFO_BUF_SIZE,
            });
        }
        let sres = bridge_safe_write_read_multi(reqs)
            .await
            .map_err(|e| Error::io(format!("write_read_multi GET_SYMINFO_BYNAME_EX: {}", e)))?;
        for r in sres {
            let mut processed = false;
            if r.code == 0 {
                if let Some(buf) = r.data {
                    if buf.len() > 24 {
                        let len: u32 = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                        if len == 0 {
                            result.push(SymbolInfo {
                                kind: Kind::NotFound,
                                array_len: None,
                                size: 0,
                            });
                        } else {
                            let size: u32 = u32::from_le_bytes(buf[12..16].try_into().unwrap());
                            let mut kind: Kind =
                                u32::from_le_bytes(buf[16..20].try_into().unwrap()).into();
                            let flags: u32 = u32::from_le_bytes(buf[20..24].try_into().unwrap());
                            let kind_size = kind.size();
                            if kind_size == 0 {
                                kind = Kind::Other;
                            }
                            if flags & 0x1000 == 0 {
                                let array_len = if kind_size > 0 { size / kind_size } else { 0 };
                                result.push(SymbolInfo {
                                    kind,
                                    array_len: if array_len > 1 { Some(array_len) } else { None },
                                    size,
                                });
                            } else {
                                match kind {
                                    Kind::Other
                                    | Kind::NotFound
                                    | Kind::UnsupportedImpl
                                    | Kind::Str
                                    | Kind::WStr => {}
                                    _ => {
                                        if size != kind_size {
                                            kind = Kind::UnsupportedImpl;
                                        }
                                    }
                                }
                                result.push(SymbolInfo {
                                    kind,
                                    array_len: None,
                                    size,
                                });
                            }
                        }
                        processed = true;
                    }
                }
            }
            if !processed {
                result.push(SymbolInfo {
                    kind: Kind::NotFound,
                    array_len: None,
                    size: 0,
                });
            }
        }
    }
    Ok(result)
}

async fn read_vals_multi(vars: &[&Var]) -> EResult<Vec<EResult<Value>>> {
    let mut result: Vec<EResult<Value>> = Vec::with_capacity(vars.len());
    if !vars.is_empty() {
        let mut reqs = Vec::with_capacity(vars.len());
        for var in vars {
            reqs.push(ReadRequest {
                index_group: ::ads::index::RW_SYMVAL_BYHANDLE,
                index_offset: var.handle.load(atomic::Ordering::SeqCst),
                size: var.buf_size(),
            });
        }
        let sresp = bridge_safe_read_multi(reqs)
            .await
            .map_err(|e| Error::io(format!("read_multi RW_SYMVAL_BYHANDLE: {}", e)))?;
        for (r, var) in sresp.into_iter().zip(vars.iter()) {
            if r.code == 0 {
                if let Some(data) = r.data {
                    if data.len() == var.buf_size() {
                        result.push(parse_buf(&data, var));
                    } else {
                        result.push(Err(Error::io("truncated data received")));
                    }
                } else {
                    result.push(Err(Error::io("no data received")));
                }
            } else {
                result.push(Err(Error::io(format!("read error: {}", r.code))));
            }
        }
    }
    Ok(result)
}

async fn read_val(var: &Var) -> EResult<Value> {
    let res = bridge_read(
        ::ads::index::RW_SYMVAL_BYHANDLE,
        var.handle.load(atomic::Ordering::SeqCst),
        var.buf_size(),
    )
    .await
    .map_err(|e| Error::io(format!("read_exact RW_SYMVAL_BYHANDLE: {e}")))?;
    parse_buf(&res, var)
}
async fn write_vals_multi(vars: &[&Var], values: &[&Value]) -> EResult<Vec<EResult<()>>> {
    let mut result: Vec<EResult<()>> = Vec::with_capacity(vars.len());
    let mut val_idx = 0;
    if !vars.is_empty() {
        let mut reqs = Vec::with_capacity(vars.len());
        let mut jobs = Vec::with_capacity(vars.len());
        let mut pre_results = Vec::with_capacity(vars.len());
        for var in vars {
            let value = values[val_idx];
            val_idx += 1;
            match var.prepare_value_buf(value) {
                Ok(v) => {
                    pre_results.push(None);
                    jobs.push((v, var.handle.load(atomic::Ordering::SeqCst)));
                }
                Err(e) => pre_results.push(Some(e)),
            }
        }
        for job in &mut jobs {
            #[allow(clippy::unnecessary_mut_passed)]
            reqs.push(WriteRequest {
                index_group: ::ads::index::RW_SYMVAL_BYHANDLE,
                index_offset: job.1,
                data: &job.0,
            });
        }
        let sresp = bridge_safe_write_multi(reqs)
            .await
            .map_err(|e| Error::io(format!("write_multi RW_SYMVAL_BYHANDLE {e}")))?;
        let mut pr_cnt = 0;
        for res in pre_results {
            if let Some(err) = res {
                result.push(Err(err));
            } else {
                let write_res = &sresp[pr_cnt];
                pr_cnt += 1;
                if write_res.code == 0 {
                    result.push(Ok(()));
                } else {
                    result.push(Err(Error::io(format!("write error: {}", write_res.code))));
                }
            }
        }
    }
    Ok(result)
}
async fn write_val(var: &Var, value: &Value) -> EResult<()> {
    bridge_write(
        ::ads::index::RW_SYMVAL_BYHANDLE,
        var.handle.load(atomic::Ordering::SeqCst),
        &var.prepare_value_buf(value)?,
    )
    .await
    .map_err(|e| Error::io(format!("write RW_SYMVAL_BYHANDLE {e}")))
}

fn parse_buf(buf: &[u8], var: &Var) -> EResult<Value> {
    if var.array_len.is_some() {
        if var.kind == Kind::Str || var.kind == Kind::WStr {
            parse_buf_value(buf, var)
        } else {
            let kind_size = var.kind.size().try_into()?;
            let vals = buf
                .chunks(kind_size)
                .map(|v| parse_buf_value(v, var))
                .collect::<Result<Vec<Value>, _>>()?;
            Ok(Value::Seq(vals))
        }
    } else {
        parse_buf_value(buf, var)
    }
}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::too_many_lines)]
fn parse_buf_value(buf: &[u8], var: &Var) -> EResult<Value> {
    Ok(match var.kind {
        Kind::Int => {
            if buf.len() < 2 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::I16(
                i16::read_from(&buf[0..2])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Dint => {
            if buf.len() < 4 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::I32(
                i32::read_from(&buf[0..4])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Real => {
            if buf.len() < 4 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::F32(
                f32::read_from(&buf[0..4])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Lreal => {
            if buf.len() < 8 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::F64(
                f64::read_from(&buf[0..8])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Sint => {
            if buf.is_empty() {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::I8(buf[0] as i8)
        }
        Kind::Usint | Kind::Bool => {
            if buf.is_empty() {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::U8(buf[0])
        }
        Kind::Uint => {
            if buf.len() < 2 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::U16(
                u16::read_from(&buf[0..2])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Udint => {
            if buf.len() < 4 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::U32(
                u32::read_from(&buf[0..4])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Lint => {
            if buf.len() < 8 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::I64(
                i64::read_from(&buf[0..8])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Ulint => {
            if buf.len() < 8 {
                return Err(Error::invalid_data(ERR_INVALID_BUFFER_LENGTH));
            }
            Value::U64(
                u64::read_from(&buf[0..8])
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_BUFFER_LENGTH))?,
            )
        }
        Kind::Str => {
            if let Ok(s) = std::str::from_utf8(buf.split(|v| *v == 0).next().unwrap()) {
                Value::String(s.to_owned())
            } else {
                Value::Unit
            }
        }
        Kind::WStr => {
            let slice = if let Some(pos) = buf.chunks(2).position(|c| c == [0, 0]) {
                &buf[..pos * 2]
            } else {
                buf
            };
            if let Ok(s) = utf16string::WStr::from_utf16le(slice) {
                Value::String(s.to_string())
            } else {
                Value::Unit
            }
        }
        _ => Value::Unit,
    })
}

pub async fn read_multi(
    vars: &[&Var],
    timeout: Duration,
    retries: u8,
) -> EResult<Vec<EResult<Value>>> {
    let mut result = Err(Error::timeout());
    let op = Op::new(timeout);
    for _ in 0..=retries {
        result = read_vals_multi(vars).await;
        if result.is_ok() {
            return result;
        }
        op.timeout()?;
    }
    result
}

/// returns vec of failed-to-write vars
async fn write_var_jobs_multi(
    jobs: &[WriteVarJob],
    timeout: Duration,
    verify: bool,
) -> EResult<Vec<Arc<Var>>> {
    let op = Op::new(timeout);
    let mut failed = Vec::new();
    let mut vars = Vec::with_capacity(jobs.len());
    let mut values = Vec::with_capacity(jobs.len());
    for job in jobs {
        vars.push(job.var.as_ref());
        values.push(job.value.as_ref());
    }
    let res = write_vals_multi(&vars, &values).await?;
    let mut to_verify: Option<Vec<&WriteVarJob>> = if verify { Some(Vec::new()) } else { None };
    for (job, result) in jobs.iter().zip(res.into_iter()) {
        if result.is_err() {
            failed.push(job.var.clone());
        } else if verify {
            to_verify.as_mut().unwrap().push(job);
        }
    }
    if verify {
        if let Some(verify_delay) = crate::VERIFY_DELAY.get().unwrap() {
            op.enough(*verify_delay)?;
            std::thread::sleep(*verify_delay);
        }
        op.timeout()?;
        let vars_to_verify = to_verify
            .as_ref()
            .unwrap()
            .iter()
            .map(|job| job.var.as_ref())
            .collect::<Vec<&Var>>();
        let res = read_vals_multi(&vars_to_verify).await?;
        for (job, result) in to_verify.unwrap().into_iter().zip(res.iter()) {
            match result {
                Ok(value) => {
                    let val_to_check = job.var.normalize_value(value.clone())?;
                    if val_to_check != *value {
                        failed.push(job.var.clone());
                    }
                }
                Err(_) => failed.push(job.var.clone()),
            }
        }
    }
    Ok(failed)
}

async fn get_var_by_name(name: &str) -> EResult<Arc<Var>> {
    if let Some(var) = crate::ADS_VARS.lock().unwrap().get(name) {
        return Ok(var.clone());
    }
    let vars = create_vars(&[name]).await?;
    let var = vars
        .into_iter()
        .next()
        .ok_or_else(|| Error::failed("unable to create ADS handle"))?;
    var.check()?;
    let var = Arc::new(var);
    crate::ADS_VARS
        .lock()
        .unwrap()
        .insert(name.to_owned(), var.clone());
    Ok(var)
}

async fn prepare_write_var_jobs(jobs: &[WriteJob]) -> EResult<(Vec<WriteVarJob>, Vec<Var>)> {
    let mut res = Vec::new();
    let mut to_get = Vec::new();
    let mut failed = Vec::new();
    let mut jobs_to_get = Vec::new();
    {
        let ads_vars = crate::ADS_VARS.lock().unwrap();
        for job in jobs {
            if let Some(var) = ads_vars.get(&job.name) {
                res.push(WriteVarJob {
                    var: var.clone(),
                    value: job.value.clone(),
                });
            } else {
                to_get.push(job.name.as_str());
                jobs_to_get.push(job);
            }
        }
    }
    if !jobs_to_get.is_empty() {
        let new_vars = create_vars(&to_get).await?;
        let mut ads_vars = crate::ADS_VARS.lock().unwrap();
        for (job, var) in jobs_to_get.iter().zip(new_vars.into_iter()) {
            if var.check().is_ok() {
                let var = Arc::new(var);
                ads_vars.insert(var.name.clone(), var.clone());
                res.push(WriteVarJob {
                    var,
                    value: job.value.clone(),
                });
            } else {
                failed.push(var);
            }
        }
    }
    Ok((res, failed))
}

pub async fn read_var_by_name(name: &str, timeout: Duration) -> EResult<Value> {
    let op = Op::new(timeout);
    let var = get_var_by_name(name).await?;
    op.timeout()?;
    read_val(&var).await
}

async fn write_and_verify(var: &Var, value: Value, timeout: Duration, verify: bool) -> EResult<()> {
    let op = Op::new(timeout);
    let val_to_check = if verify {
        Some(var.normalize_value(value.clone())?)
    } else {
        None
    };
    write_val(var, &value).await?;
    if verify {
        if let Some(verify_delay) = crate::VERIFY_DELAY.get().unwrap() {
            op.enough(*verify_delay)?;
            std::thread::sleep(*verify_delay);
        }
        op.timeout()?;
        let real_value = read_val(var).await?;
        if real_value != val_to_check.unwrap() {
            return Err(Error::failed("value mismatch"));
        }
    }
    Ok(())
}

async fn write_var_by_name(
    name: Arc<String>,
    value: Value,
    timeout: Duration,
    verify: bool,
) -> EResult<()> {
    let op = Op::new(timeout);
    let var = get_var_by_name(&name).await?;
    write_and_verify(&var, value, op.timeout()?, verify).await
}

pub async fn read_by_name(name: String, timeout: Duration, retries: u8) -> EResult<Value> {
    let mut result = Err(Error::timeout());
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let t = op.timeout()?;
        result = read_var_by_name(&name, t).await;
        if result.is_ok() {
            return result;
        }
    }
    result
}

pub async fn write_by_name(
    name: String,
    value: Value,
    timeout: Duration,
    verify: bool,
    retries: u8,
) -> EResult<()> {
    let mut result = Err(Error::timeout());
    let name = Arc::new(name);
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let name_c = name.clone();
        let t = op.timeout()?;
        let val = value.clone();
        result = write_var_by_name(name_c, val, t, verify).await;
        if result.is_ok() {
            return result;
        }
        op.timeout()?;
    }
    result
}

struct WriteJob {
    name: String,
    value: Arc<Value>,
}

struct WriteVarJob {
    var: Arc<Var>,
    value: Arc<Value>,
}

/// returns vec of failed-to-write symbol
async fn write_jobs_multi(
    write_jobs: &[WriteJob],
    timeout: Duration,
    verify: bool,
) -> EResult<Vec<String>> {
    let op = Op::new(timeout);
    let (jobs, failed_vars) = prepare_write_var_jobs(write_jobs).await?;
    let mut failed: HashSet<String> = failed_vars.into_iter().map(|var| var.name).collect();
    for var in write_var_jobs_multi(&jobs, op.timeout()?, verify).await? {
        failed.insert(var.name.clone());
    }
    Ok(failed.into_iter().collect())
}

/// returns vec of failed-to-write symbols
pub async fn write_by_names_multi(
    names: Vec<String>,
    values: Vec<Value>,
    timeout: Duration,
    verify: bool,
    retries: u8,
) -> EResult<Vec<String>> {
    if names.len() != values.len() {
        return Err(Error::invalid_data("names/values seq len mismatch"));
    }
    let mut jobs: Vec<WriteJob> = names
        .into_iter()
        .zip(values.into_iter())
        .map(|(name, value)| WriteJob {
            name,
            value: Arc::new(value),
        })
        .collect();
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let Ok(t) = op.timeout() else {
            return Ok(jobs.iter().map(|job| job.name.clone()).collect());
        };
        if let Ok(failed_symbols) = write_jobs_multi(&jobs, t, verify).await {
            let fset: HashSet<String> = failed_symbols.into_iter().collect();
            jobs.retain(|job| fset.contains(&job.name));
        }
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
    }
    Ok(jobs.iter().map(|job| job.name.clone()).collect())
}

pub async fn write(
    var: Arc<Var>,
    value: Value,
    timeout: Duration,
    verify: bool,
    retries: u8,
) -> EResult<()> {
    let mut result = Err(Error::timeout());
    let op = Op::new(timeout);
    for _ in 0..=retries {
        let value_c = value.clone();
        let var_c = var.clone();
        let t = op.timeout()?;
        result = write_and_verify(&var_c, value_c, t, verify).await;
        if result.is_ok() {
            return result;
        }
        op.timeout()?;
    }
    result
}

pub async fn ping_worker(timeout: Duration) {
    let mut int = tokio::time::interval(timeout / 2);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (n, p) = get_device();
    let params = ParamsPing { n, p };
    while !eva_sdk::service::svc_is_terminating() {
        if let Err(e) = call0("ping", &params).await {
            error!("ADS ping error: {}", e);
            eva_sdk::service::poc();
        }
        int.tick().await;
    }
}

async fn call0<S: Serialize>(method: &str, params: &S) -> EResult<()> {
    let rpc = crate::RPC.get().unwrap();
    let svc_id = crate::BRIDGE_ID.get().unwrap();
    let timeout = *crate::TIMEOUT.get().unwrap();
    safe_rpc_call(
        rpc,
        svc_id,
        method,
        pack(params)?.into(),
        busrt::QoS::Processed,
        timeout,
    )
    .await?;
    Ok(())
}

async fn call<S: Serialize>(method: &str, params: &S) -> EResult<Vec<u8>> {
    let rpc = crate::RPC.get().unwrap();
    let svc_id = crate::BRIDGE_ID.get().unwrap();
    let timeout = *crate::TIMEOUT.get().unwrap();
    let result = safe_rpc_call(
        rpc,
        svc_id,
        method,
        pack(params)?.into(),
        busrt::QoS::Processed,
        timeout,
    )
    .await?;
    unpack(result.payload())
}

async fn su_call<S: Serialize>(method: &str, params: &S) -> EResult<Vec<SumUpResult>> {
    let rpc = crate::RPC.get().unwrap();
    let svc_id = crate::BRIDGE_ID.get().unwrap();
    let timeout = *crate::TIMEOUT.get().unwrap();
    let result = safe_rpc_call(
        rpc,
        svc_id,
        method,
        pack(params)?.into(),
        busrt::QoS::Processed,
        timeout,
    )
    .await?;
    unpack(result.payload())
}
