use eva_common::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;

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
    pub fn iter(&self) -> OIDSingleOrMultiIter {
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
