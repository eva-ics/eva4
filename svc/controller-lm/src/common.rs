use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Serialize, Debug, Copy, Clone)]
struct LParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    args: &'a Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kwargs: &'a Option<HashMap<String, Value>>,
}

#[derive(Serialize, Debug, Copy, Clone)]
pub struct ParamsRun<'a> {
    i: &'a OID,
    params: LParams<'a>,
    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
    wait: Duration,
}

impl<'a> ParamsRun<'a> {
    #[inline]
    pub fn new(
        oid: &'a OID,
        args: &'a Option<Vec<Value>>,
        kwargs: &'a Option<HashMap<String, Value>>,
        wait: Duration,
    ) -> Self {
        Self {
            i: oid,
            params: LParams { args, kwargs },
            wait,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct MacroResult {
    err: Value,
    exitcode: Option<i16>,
}

type MacroErr = (String, Option<Value>);

pub enum ParamsCow<'a> {
    Params(ParamsRun<'a>),
    Packed(Vec<u8>),
}

pub async fn safe_run_macro<'a>(
    rpc: &RpcClient,
    params: ParamsCow<'a>,
    timeout: Duration,
) -> Result<(), MacroErr> {
    let payload = match params {
        ParamsCow::Params(p) => pack(&p).map_err(|e| (e.to_string(), None))?,
        ParamsCow::Packed(v) => v,
    };
    match tokio::time::timeout(
        timeout,
        rpc.call("eva.core", "run", payload.into(), QoS::RealtimeProcessed),
    )
    .await
    {
        Ok(Ok(res)) => {
            let result: MacroResult = unpack(res.payload()).map_err(|e| (e.to_string(), None))?;
            if let Some(code) = result.exitcode {
                if code == 0 {
                    Ok(())
                } else {
                    Err(("lmacro".to_owned(), Some(result.err)))
                }
            } else {
                Err(("lmacro".to_owned(), Some("timeout".into())))
            }
        }
        Err(_) => Err(("lmacro".to_owned(), Some("timeout".into()))),
        Ok(Err(e)) => Err(("exec".to_owned(), Some(e.code().into()))),
    }
}

pub async fn run_err_macro(
    rpc: &RpcClient,
    oid: &OID,
    err: MacroErr,
    kwargs: &Option<HashMap<String, Value>>,
    timeout: Duration,
) -> EResult<()> {
    let args = Some(vec![Value::String(err.0), err.1.unwrap_or_default()]);
    let params = ParamsRun::new(oid, &args, kwargs, timeout);
    let result: MacroResult = unpack(
        safe_rpc_call(
            rpc,
            "eva.core",
            "run",
            pack(&params)?.into(),
            QoS::RealtimeProcessed,
            timeout,
        )
        .await?
        .payload(),
    )?;
    if let Some(code) = result.exitcode {
        if code == 0 {
            Ok(())
        } else {
            Err(Error::failed(format!("{}: {}", code, result.err)))
        }
    } else {
        Err(Error::timeout())
    }
}

#[derive(Deserialize, Debug)]
pub struct StateX {
    pub status: ItemStatus,
    #[serde(default)]
    pub value: Value,
    pub act: Option<u32>,
}

#[derive(Serialize, Debug)]
pub struct Source<'a> {
    oid: &'a OID,
    status: ItemStatus,
    value: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    act: Option<u32>,
}

impl<'a> Source<'a> {
    #[inline]
    pub fn new(oid: &'a OID, state: &'a StateX) -> Self {
        Self {
            oid,
            status: state.status,
            value: &state.value,
            act: state.act,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct SourceOwned {
    oid: OID,
    status: ItemStatus,
    value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    act: Option<u32>,
}

impl SourceOwned {
    #[inline]
    pub fn new(oid: &OID, state: &StateX) -> Self {
        Self {
            oid: oid.clone(),
            status: state.status,
            value: state.value.clone(),
            act: state.act,
        }
    }
}
