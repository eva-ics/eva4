//! JSON-path parsers: mirror `controller-pubsub` input mapping semantics for LM item state.

use crate::common::StateX;
use eva_common::acl::OIDMask;
use eva_common::common_payloads::ParamsId;
use eva_common::events::{RAW_STATE_TOPIC, RawStateEventOwned};
use eva_common::prelude::*;
use eva_sdk::controller::transform;
use eva_sdk::prelude::*;
use eva_sdk::types::State;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

err_logger!();

static ENGINE: OnceLock<Arc<ParserEngine>> = OnceLock::new();

#[inline]
fn default_jp() -> String {
    "$.".to_owned()
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ParserGroup {
    #[serde(default)]
    config: ParserGroupConfig,
    items: Vec<ParserItem>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct ParserGroupConfig {
    #[serde(default)]
    ignore_events: bool,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    interval: Option<Duration>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ParserItem {
    oid: OID,
    #[serde(default)]
    map: Vec<ParserMapEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ParserMapEntry {
    #[serde(default = "default_jp")]
    path: String,
    target: OID,
    #[serde(default)]
    value_map: BTreeMap<String, Value>,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

pub struct ParserGroupRun {
    ignore_events: bool,
    interval: Option<Duration>,
    items: Vec<ParserItem>,
}

pub struct ParserEngine {
    groups: Vec<Arc<ParserGroupRun>>,
    index: HashMap<OID, Vec<(usize, usize)>>,
}

impl ParserEngine {
    pub fn new(groups: Vec<ParserGroup>) -> Self {
        let mut runs: Vec<Arc<ParserGroupRun>> = Vec::with_capacity(groups.len());
        let mut index: HashMap<OID, Vec<(usize, usize)>> = HashMap::new();
        for (gi, g) in groups.into_iter().enumerate() {
            let run = Arc::new(ParserGroupRun {
                ignore_events: g.config.ignore_events,
                interval: g.config.interval,
                items: g.items,
            });
            for (ii, item) in run.items.iter().enumerate() {
                index.entry(item.oid.clone()).or_default().push((gi, ii));
            }
            runs.push(run);
        }
        Self {
            groups: runs,
            index,
        }
    }

    /// OID masks for `EventKind::Actual` when at least one parser group wants bus-driven updates.
    pub fn event_subscription_masks(&self) -> HashSet<OIDMask> {
        let mut set = HashSet::new();
        for (oid, slots) in &self.index {
            let mut need = false;
            for &(gi, _) in slots {
                if !self.groups[gi].ignore_events {
                    need = true;
                    break;
                }
            }
            if need {
                set.insert(oid.clone().into());
            }
        }
        set
    }

    pub async fn on_state_event(
        &self,
        oid: &OID,
        state: &StateX,
        tx: &async_channel::Sender<(String, Vec<u8>)>,
    ) {
        let Some(slots) = self.index.get(oid) else {
            return;
        };
        for &(gi, ii) in slots {
            let g = &self.groups[gi];
            if g.ignore_events {
                continue;
            }
            process_item_maps(&g.items[ii], state, tx).await;
        }
    }

    pub async fn tick_group_interval(
        &self,
        group_idx: usize,
        tx: &async_channel::Sender<(String, Vec<u8>)>,
    ) {
        let Some(g) = self.groups.get(group_idx) else {
            return;
        };
        let rpc = crate::RPC.get().unwrap();
        let timeout = *crate::TIMEOUT.get().unwrap();
        for item in &g.items {
            match pull_item_state(rpc, &item.oid, timeout).await {
                Ok(st) => process_item_maps(item, &st, tx).await,
                Err(e) => debug!("parser pull {}: {}", item.oid, e),
            }
        }
    }
}

pub fn init(engine: Arc<ParserEngine>) -> EResult<()> {
    ENGINE
        .set(engine)
        .map_err(|_| Error::core("Unable to set PARSER_ENGINE"))?;
    Ok(())
}

pub async fn on_state(oid: &OID, state: &StateX, tx: &async_channel::Sender<(String, Vec<u8>)>) {
    if let Some(eng) = ENGINE.get() {
        eng.on_state_event(oid, state, tx).await;
    }
}

pub fn spawn_interval_workers(
    engine: Arc<ParserEngine>,
    tx: async_channel::Sender<(String, Vec<u8>)>,
) {
    for (gi, g) in engine.groups.iter().enumerate() {
        if let Some(interval) = g.interval {
            let eng = engine.clone();
            let tx_c = tx.clone();
            tokio::spawn(async move {
                let mut int = tokio::time::interval(interval);
                int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    int.tick().await;
                    eng.tick_group_interval(gi, &tx_c).await;
                }
            });
        }
    }
}

async fn pull_item_state(rpc: &RpcClient, oid: &OID, timeout: Duration) -> EResult<StateX> {
    let res = safe_rpc_call(
        rpc,
        "eva.core",
        "item.state",
        pack(&ParamsId { i: oid.as_str() })?.into(),
        QoS::Processed,
        timeout,
    )
    .await?;
    let states: Vec<State> = unpack(res.payload())?;
    let st = states
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found(format!("no state for {}", oid)))?;
    // RPC may return `value: null`; treat as Unit so JSON-path lookups simply miss.
    Ok(StateX {
        status: st.status,
        value: st.value.unwrap_or_default(),
        act: None,
    })
}

async fn process_item_maps(
    item: &ParserItem,
    state: &StateX,
    tx: &async_channel::Sender<(String, Vec<u8>)>,
) {
    if item.map.is_empty() {
        return;
    }
    for m in &item.map {
        match state.value.jp_lookup(&m.path) {
            Ok(Some(mut v)) => {
                if !m.value_map.is_empty()
                    && let Some(v_mapped) = m.value_map.get(&v.to_string())
                {
                    v = v_mapped;
                }
                let v = if !m.transform.is_empty() {
                    let f = match f64::try_from(v.clone()) {
                        Ok(n) => n,
                        Err(e) => {
                            debug!("parser transform skip {} path {}: {}", item.oid, m.path, e);
                            continue;
                        }
                    };
                    match transform::transform(&m.transform, &m.target, f) {
                        Ok(n) => Value::F64(n),
                        Err(e) => {
                            debug!("parser transform skip {} path {}: {}", item.oid, m.path, e);
                            continue;
                        }
                    }
                } else {
                    v.clone()
                };
                let raw = RawStateEventOwned::new(state.status, v);
                let topic = format!("{}{}", RAW_STATE_TOPIC, m.target.as_path());
                match pack(&raw) {
                    Ok(payload) => {
                        if let Err(e) = tx.send((topic, payload)).await {
                            error!("parser publish: {}", e);
                        }
                    }
                    Err(e) => error!("parser pack {}: {}", m.target, e),
                }
            }
            Ok(None) => trace!("parser jp miss {} {}", item.oid, m.path),
            Err(e) => debug!("parser jp_lookup {} {}: {}", item.oid, m.path, e),
        }
    }
}

#[cfg(test)]
fn parser_item_for_test(oid: &str, map: Vec<ParserMapEntry>) -> ParserItem {
    ParserItem {
        oid: oid.parse().expect("test oid"),
        map,
    }
}

#[cfg(test)]
fn map_entry(path: &str, target: &str) -> ParserMapEntry {
    ParserMapEntry {
        path: path.to_owned(),
        target: target.parse().expect("test target oid"),
        value_map: BTreeMap::new(),
        transform: Vec::new(),
    }
}

#[cfg(test)]
fn group_cfg_test(ignore_events: bool, interval: Option<Duration>) -> ParserGroupConfig {
    ParserGroupConfig {
        ignore_events,
        interval,
    }
}

#[cfg(test)]
fn parser_group_test(config: ParserGroupConfig, items: Vec<ParserItem>) -> ParserGroup {
    ParserGroup { config, items }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eva_common::payload::unpack;

    #[tokio::test]
    async fn parser_emits_raw_state_with_source_status() {
        let (tx, rx) = async_channel::bounded(4);
        let item = parser_item_for_test(
            "sensor:tests/src",
            vec![map_entry("$.v", "sensor:tests/dst")],
        );
        let state = StateX {
            status: eva_common::ITEM_STATUS_ERROR,
            value: serde_json::json!({ "v": 99 }).try_into().expect("value"),
            act: None,
        };
        process_item_maps(&item, &state, &tx).await;
        let (topic, payload) = rx.recv().await.expect("one message");
        let dst: OID = "sensor:tests/dst".parse().expect("dst oid");
        assert_eq!(topic, format!("{}{}", RAW_STATE_TOPIC, dst.as_path()));
        let raw: RawStateEventOwned = unpack(&payload).expect("unpack raw");
        assert_eq!(raw.status, state.status);
        let expected: Value = serde_json::json!(99).try_into().expect("v");
        assert_eq!(raw.value, ValueOptionOwned::Value(expected));
    }

    #[tokio::test]
    async fn parser_empty_map_silent() {
        let (tx, rx) = async_channel::bounded(4);
        let item = parser_item_for_test("sensor:tests/a", vec![]);
        let state = StateX {
            status: 1,
            value: Value::U64(1),
            act: None,
        };
        process_item_maps(&item, &state, &tx).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn parser_nested_and_array_paths_emit_three_targets() {
        let (tx, rx) = async_channel::bounded(8);
        let item = parser_item_for_test(
            "sensor:tests/src",
            vec![
                map_entry("$.nested.inner", "sensor:tests/n"),
                map_entry("$.readings[0].v", "sensor:tests/r0"),
                map_entry("$.readings[1].v", "sensor:tests/r1"),
            ],
        );
        let state = StateX {
            status: 1,
            value: serde_json::json!({
                "nested": {"inner": 11, "other": "x"},
                "readings": [{"t": 1, "v": 22}, {"t": 2, "v": 33}]
            })
            .try_into()
            .expect("value"),
            act: None,
        };
        process_item_maps(&item, &state, &tx).await;
        let mut got: Vec<(String, i64)> = Vec::new();
        for _ in 0..3 {
            let (topic, payload) = rx.recv().await.expect("msg");
            let raw: RawStateEventOwned = unpack(&payload).expect("raw");
            let v = match &raw.value {
                ValueOptionOwned::Value(val) => i64::try_from(val.clone()).expect("i64"),
                _ => panic!("expected value"),
            };
            got.push((topic, v));
        }
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got[0].1, 11);
        assert_eq!(got[1].1, 22);
        assert_eq!(got[2].1, 33);
    }

    #[test]
    fn event_masks_skip_ignore_only_groups() {
        let eng = ParserEngine::new(vec![
            parser_group_test(
                group_cfg_test(true, Some(Duration::from_secs(1))),
                vec![parser_item_for_test(
                    "sensor:only/pull",
                    vec![map_entry("$.x", "sensor:t/a")],
                )],
            ),
            parser_group_test(
                group_cfg_test(false, None),
                vec![parser_item_for_test(
                    "sensor:bus/and/pull",
                    vec![map_entry("$.x", "sensor:t/b")],
                )],
            ),
        ]);
        let masks = eng.event_subscription_masks();
        assert!(masks.contains(&"sensor:bus/and/pull".parse::<OID>().unwrap().into()));
        assert!(!masks.contains(&"sensor:only/pull".parse::<OID>().unwrap().into()));
    }

    #[test]
    fn parser_group_json_omits_config_subscribes_sources() {
        let v = serde_json::json!({
            "items": [{"oid": "sensor:defs/a", "map": []}]
        });
        let g: ParserGroup = serde_json::from_value(v).expect("deserialize");
        let eng = ParserEngine::new(vec![g]);
        let masks = eng.event_subscription_masks();
        assert!(masks.contains(&"sensor:defs/a".parse::<OID>().unwrap().into()));
    }
}
