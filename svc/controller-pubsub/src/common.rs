use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Debug)]
pub struct LParams {
    pub(crate) kwargs: PubSubData,
}

#[derive(Serialize, Debug)]
pub struct ParamsRun<'a> {
    pub(crate) i: &'a OID,
    pub(crate) params: LParams,
    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
    pub(crate) wait: Duration,
}

#[derive(Serialize, Debug)]
pub struct PubSubData {
    pub(crate) pubsub_topic: String,
    pub(crate) pubsub_payload: Value,
}

#[derive(Deserialize)]
struct MacroResult {
    exitcode: Option<i16>,
}

pub async fn safe_run_macro(params: ParamsRun<'_>) -> EResult<()> {
    let res = tokio::time::timeout(
        eapi_bus::timeout(),
        eapi_bus::rpc_secondary().call(
            "eva.core",
            "run",
            pack(&params)?.into(),
            QoS::RealtimeProcessed,
        ),
    )
    .await??;
    let result: MacroResult = unpack(res.payload())?;
    if let Some(code) = result.exitcode {
        if code == 0 {
            Ok(())
        } else {
            Err(Error::failed(format!("exit code {}", code)))
        }
    } else {
        Err(Error::failed("timeout"))
    }
}
