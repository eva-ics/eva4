use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[inline]
fn default_queue_size() -> usize {
    2048
}

#[inline]
fn default_action_queue_size() -> usize {
    32
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub update: Vec<crate::updates::Update>,
    #[serde(default)]
    pub update_pipe: Vec<crate::updates::UpdatePipe>,
    #[serde(default)]
    pub action_map: HashMap<OID, crate::actions::ActionMap>,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default = "default_action_queue_size")]
    pub action_queue_size: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum OIDSingleOrMulti {
    Single(OID),
    Multi(Vec<OID>),
}

impl OIDSingleOrMulti {
    pub fn iter(&self) -> OIDSingleOrMultiIter<'_> {
        OIDSingleOrMultiIter { osm: self, curr: 0 }
    }
}

pub struct OIDSingleOrMultiIter<'a> {
    osm: &'a OIDSingleOrMulti,
    curr: usize,
}

impl<'a> Iterator for OIDSingleOrMultiIter<'a> {
    type Item = &'a OID;
    fn next(&mut self) -> Option<Self::Item> {
        match self.osm {
            OIDSingleOrMulti::Single(oid) => {
                if self.curr > 0 {
                    None
                } else {
                    self.curr += 1;
                    Some(oid)
                }
            }
            OIDSingleOrMulti::Multi(h) => {
                let res = h.get(self.curr);
                self.curr += 1;
                res
            }
        }
    }
}

pub fn init_cmd_options_basic<'a>() -> bmart::process::Options<'a> {
    bmart::process::Options::new()
        .env("EVA_DIR", crate::EVA_DIR.get().unwrap())
        .env("EVA_BUS_PATH", crate::BUS_PATH.get().unwrap())
}

pub fn init_cmd_options<'a>(oid: &'a OID, kind_str: &'a str) -> bmart::process::Options<'a> {
    let mut cmd_options = init_cmd_options_basic();
    cmd_options = cmd_options.env("EVA_ITEM_OID", oid.as_str());
    cmd_options = cmd_options.env("EVA_ITEM_ID", oid.id());
    cmd_options = cmd_options.env("EVA_ITEM_TYPE", kind_str);
    cmd_options = cmd_options.env("EVA_ITEM_FULL_ID", oid.full_id());
    if let Some(group) = oid.group() {
        cmd_options = cmd_options.env("EVA_ITEM_GROUP", group);
        let mut sp = group.rsplitn(2, '/');
        let parent = sp.next().unwrap();
        cmd_options = cmd_options.env("EVA_ITEM_PARENT_GROUP", parent);
    } else {
        cmd_options = cmd_options
            .env("EVA_ITEM_GROUP", "")
            .env("EVA_ITEM_PARENT_GROUP", "");
    }
    cmd_options
}

#[derive(Serialize, Debug)]
pub struct LParams {
    pub(crate) kwargs: LineData,
}

#[derive(Serialize, Debug)]
pub struct ParamsRun<'a> {
    pub(crate) i: &'a OID,
    pub(crate) params: LParams,
    #[serde(serialize_with = "eva_common::tools::serialize_duration_as_f64")]
    pub(crate) wait: Duration,
}

#[derive(Serialize, Debug)]
pub struct LineData {
    pub(crate) line: Option<String>,
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
