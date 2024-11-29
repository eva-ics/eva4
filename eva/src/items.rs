use crate::{EResult, Error};
use eva_common::acl::{OIDMask, OIDMaskList};
use eva_common::events::{
    DbState, Force, FullItemStateAndInfo, ItemStateAndInfo, LocalStateEvent, RawStateEventOwned,
    ReplicationInventoryItem, ReplicationState,
};
use eva_common::logic::Range;
use eva_common::prelude::*;
use eva_common::time::monotonic_ns;
use eva_common::time::Time;
use eva_common::tools::default_true;
use eva_common::ITEM_STATUS_ERROR;
use log::warn;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::str::Split;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;
use submap::mkmf::MapKeysMatchFormula as _;

#[cfg(test)]
mod tests {
    use eva_common::payload::{pack, unpack};
    use eva_common::prelude::*;

    #[test]
    #[allow(clippy::items_after_statements, clippy::field_reassign_with_default)]
    fn test_ser() {
        //let loc = super::Location::Geo(1.0, 2.0, 3.0);
        let mut logic = super::Logic::default();
        let mut range = super::Range::default();
        range.max = Some(33.22);
        logic.range = Some(range);
        let mut opts = super::ActionConfig {
            svc: "test".to_owned(),
            timeout: Some(5.0),
            config: None,
        };
        opts.timeout = Some(33.3);
        let item = std::sync::Arc::new(super::ItemData {
            oid: "unit:tests/t1".parse::<OID>().unwrap().into(),
            state: None,
            meta: None,
            //location: Some(loc),
            source: None,
            enabled: std::sync::atomic::AtomicBool::new(true),
            logic: Some(logic),
            action: None,
        });
        let payload = serde_json::to_string(&item).unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let _item: super::ItemData = serde_json::from_value(v).unwrap();
        let payload = pack(&item).unwrap();
        let _item: super::ItemData = unpack(&payload).unwrap();
    }
    #[test]
    fn test_logic() {
        let oid: OID = "sensor:tests/s1".parse().unwrap();
        let mut state = super::ItemState::new0(IEID::new(1, 1), ItemKind::Sensor);
        state.value = Value::from(33.99);
        assert!(!state.apply_logic(None, &oid, 1));
        assert_eq!(state.status, 1);
        let mut logic = super::Logic { range: None };
        assert!(!state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, 1);
        logic.range = Some(super::Range {
            min: Some(0.0),
            max: None,
            min_eq: false,
            max_eq: false,
        });
        // test invalid value type
        state.value = Value::Unit;
        assert!(state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, -1);
        // test min
        state.status = 1;
        state.value = Value::from(1);
        assert!(!state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, 1);
        // test value below min
        state.value = Value::from(0);
        assert!(state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, -1);
        // change the range to min eq
        state.status = 1;
        logic.range = Some(super::Range {
            min: Some(0.0),
            max: None,
            min_eq: true,
            max_eq: false,
        });
        // test value min eq
        state.value = Value::from(0);
        assert!(!state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, 1);
        state.value = Value::from(-1);
        assert!(state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, -1);
        state.status = 1;
        // test max
        logic.range = Some(super::Range {
            min: Some(0.0),
            max: Some(10.0),
            min_eq: false,
            max_eq: false,
        });
        // test value above max
        state.value = Value::from(1);
        assert!(!state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, 1);
        state.value = Value::from(10.0);
        assert!(state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, -1);
        // change the range to max eq
        state.status = 1;
        logic.range = Some(super::Range {
            min: Some(0.0),
            max: Some(10.0),
            min_eq: true,
            max_eq: true,
        });
        // test value max eq
        state.value = Value::from(10);
        assert!(!state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, 1);
        // test value above max
        state.value = Value::from(11);
        assert!(state.apply_logic(Some(&logic), &oid, 1));
        assert_eq!(state.status, -1);
    }
}

#[derive(Serialize)]
pub struct InventoryStats {
    sources: HashMap<String, usize>,
    items: usize,
}

#[derive(Debug)]
pub struct ItemSource {
    node: String,
    data: SourceData,
}

trait IeidX {
    fn generate(boot_id: u64) -> Self;
}

impl IeidX for IEID {
    #[inline]
    fn generate(boot_id: u64) -> Self {
        Self::new(boot_id, monotonic_ns())
    }
}

#[inline]
fn create_source(source_id: &str, svc: &str) -> Source {
    Arc::new(ItemSource::new(source_id, svc))
}

#[inline]
fn validate_logic(value: Option<&Value>, logic: Option<&Logic>) -> bool {
    if let Some(range) = logic.and_then(|l| l.range.as_ref()) {
        value.map_or(false, |v| {
            if let Ok(val) = TryInto::<f64>::try_into(v) {
                range.matches(val)
            } else {
                false
            }
        })
    } else {
        true
    }
}

impl ItemSource {
    pub fn new(node: &str, svc: &str) -> Self {
        Self {
            node: node.to_owned(),
            data: SourceData::new(svc),
        }
    }
    #[inline]
    pub fn mark_online(&self, online: bool) {
        self.data.online.store(online, atomic::Ordering::SeqCst);
    }
    #[inline]
    pub fn mark_destroyed(&self) {
        self.data.destroyed.store(true, atomic::Ordering::SeqCst);
    }
    #[inline]
    pub fn online(&self) -> bool {
        self.data.online.load(atomic::Ordering::SeqCst)
    }
    #[inline]
    pub fn node(&self) -> &str {
        &self.node
    }
    #[inline]
    pub fn svc(&self) -> &str {
        &self.data.svc
    }
    #[inline]
    pub fn is_destroyed(&self) -> bool {
        self.data.destroyed.load(atomic::Ordering::SeqCst)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ItemState {
    status: ItemStatus,
    value: Value,
    #[serde(skip_deserializing)]
    act: Option<usize>, // None for all except units
    ieid: IEID,
    t: f64,
}

macro_rules! mark_out_of_range {
    ($self: expr, $oid: expr, $value: expr) => {
        warn!("{} value is invalid/out of range: {:?}", $oid, $value);
        $self.status = ITEM_STATUS_ERROR;
    };
}

impl ItemState {
    #[inline]
    pub fn new0(ieid: IEID, tp: ItemKind) -> Self {
        let (status, act) = if tp == ItemKind::Unit {
            (1, Some(0))
        } else {
            (1, None)
        };
        Self {
            status,
            value: Value::Unit,
            act,
            ieid,
            t: Time::now().timestamp(),
        }
    }
    #[inline]
    pub fn new(status: ItemStatus, value: Value, act: Option<usize>, ieid: IEID, t: f64) -> Self {
        Self {
            status,
            value,
            act,
            ieid,
            t,
        }
    }
    pub fn mark_failed(&mut self, ieid: IEID) -> bool {
        if self.status == ITEM_STATUS_ERROR {
            false
        } else {
            self.status = ITEM_STATUS_ERROR;
            self.ieid = ieid;
            self.t = Time::now().timestamp();
            true
        }
    }
    // used by lvar functions and core source sensors only
    pub fn force_set_state(
        &mut self,
        status: Option<ItemStatus>,
        mut value: Option<Value>,
        ieid: IEID,
    ) {
        if let Some(status) = status {
            self.status = status;
        }
        if let Some(value) = value.take() {
            self.value = value;
        }
        self.ieid = ieid;
        self.t = Time::now().timestamp();
    }
    // used by lvar functions only
    pub fn force_set_value(&mut self, value: Value, ieid: IEID) {
        self.value = value;
        self.ieid = ieid;
        self.t = Time::now().timestamp();
    }
    /// # Panics
    ///
    /// Will panic if attempted to reduce number of actions for non-busy unit. The core MUST
    /// decrease act counter only ONCE per action
    // used by unit actions
    pub fn act_decr(&mut self, ieid: IEID) {
        let a = self.act.unwrap_or_default();
        if a == 0 {
            warn!("attempt to decr zero act for the unit!");
        } else {
            self.act = Some(a - 1);
        }
        self.ieid = ieid;
        self.t = Time::now().timestamp();
    }
    pub fn act_incr(&mut self, ieid: IEID) {
        self.act = Some(self.act.map_or(1, |a| a + 1));
        self.ieid = ieid;
        self.t = Time::now().timestamp();
    }
    #[inline]
    pub fn set_from_rs(&mut self, rpl: ReplicationState) {
        let status = rpl.status;
        self.status = status;
        if let Some(act) = rpl.act {
            self.act = Some(act);
        }
        self.value = rpl.value;
        self.ieid = rpl.ieid;
        self.t = rpl.t;
    }
    /// Returns true if the item state is changed. Note: if the state has been forcibly updated,
    /// the method still returns the real modification flag only, despite the forced update
    ///
    /// # Panics
    ///
    /// Will not panic
    #[inline]
    pub fn set_from_raw(
        &mut self,
        raw: RawStateEventOwned,
        logic: Option<&Logic>,
        oid: &OID,
        boot_id: u64,
        time: f64,
    ) -> bool {
        let mut modified = false;
        let mut status = raw.status;
        let mut value = raw.value;
        let mut compared_match = raw.status_compare.map_or(true, |v| v == self.status);
        if let ValueOptionOwned::Value(v) = raw.value_compare {
            if v != self.value {
                compared_match = false;
            }
        }
        if !compared_match {
            status = raw.status_else.unwrap_or(ITEM_STATUS_ERROR);
            value = raw.value_else;
        }
        if status == ITEM_STATUS_ERROR || validate_logic(value.as_ref(), logic) {
            if self.status != status {
                self.status = status;
                modified = true;
            }
            if let Some(val) = Into::<Option<Value>>::into(value) {
                if self.value != val {
                    self.value = val;
                    modified = true;
                }
            }
        } else {
            mark_out_of_range!(self, oid, value);
            modified = true;
        }
        #[allow(clippy::float_cmp)]
        if let Some(t) = raw.t {
            if t != 0.0 && self.t != t {
                self.t = t;
                modified = true;
            }
        }
        if modified || raw.force != Force::None {
            self.ieid = IEID::generate(boot_id);
            // if the raw event time is not set, set it to the current time
            if raw.t.is_none() {
                self.t = time;
            }
            modified
        } else {
            false
        }
    }
    pub fn serialize_db_into(&self, result: &mut BTreeMap<Value, Value>) {
        result.insert("status".into(), Value::from(self.status));
        result.insert("value".into(), self.value.clone());
        result.insert("ieid".into(), self.ieid.to_value());
        result.insert("t".into(), Value::from(self.t));
    }
    #[inline]
    pub fn serialize_into(&self, result: &mut BTreeMap<Value, Value>) {
        self.serialize_db_into(result);
        self.act
            .as_ref()
            .map(|v| result.insert("act".into(), Value::U64(*v as u64)));
    }
    /// returns true if the item status is changed after the application
    #[inline]
    pub fn apply_logic(&mut self, logic: Option<&Logic>, oid: &OID, boot_id: u64) -> bool {
        if self.status == ITEM_STATUS_ERROR || validate_logic(Some(&self.value), logic) {
            false
        } else {
            mark_out_of_range!(self, oid, self.value);
            self.ieid = IEID::generate(boot_id);
            self.t = Time::now().timestamp();
            true
        }
    }
    pub fn ieid(&self) -> &IEID {
        &self.ieid
    }
    pub fn status(&self) -> ItemStatus {
        self.status
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn t(&self) -> f64 {
        self.t
    }
}

impl From<&ItemState> for LocalStateEvent {
    fn from(state: &ItemState) -> Self {
        LocalStateEvent {
            status: state.status,
            value: state.value.clone(),
            act: state.act,
            ieid: state.ieid,
            t: state.t,
        }
    }
}

impl From<&ItemState> for DbState {
    fn from(state: &ItemState) -> Self {
        DbState {
            status: state.status,
            value: state.value.clone(),
            ieid: state.ieid,
            t: state.t,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logic {
    #[serde(default, deserialize_with = "eva_common::logic::de_opt_range")]
    range: Option<Range>, // Optional value checker (local only)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionConfig {
    pub(crate) svc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<f64>, // zero for the default timeout
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config: Option<Value>, // optional config
}

impl ActionConfig {
    #[inline]
    pub fn svc(&self) -> &str {
        &self.svc
    }
    #[inline]
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout.map(Duration::from_secs_f64)
    }
    #[inline]
    pub fn config(&self) -> Option<&Value> {
        self.config.as_ref()
    }
}

#[derive(Debug)]
pub struct StateData {
    status: Option<ItemStatus>,
    value: ValueOptionOwned,
    t: Option<f64>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ItemConfigData {
    pub oid: Arc<OID>,
    #[serde(default)]
    meta: Option<Value>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    logic: Option<Logic>,
    #[serde(default)]
    action: Option<ActionConfig>,
    #[serde(default)]
    status: Option<ItemStatus>,
    #[serde(default)]
    value: ValueOptionOwned,
    t: Option<f64>,
}

impl ItemConfigData {
    pub fn blank(oid: &OID, sender: &str) -> Self {
        let action = if oid.kind() == ItemKind::Unit {
            Some(ActionConfig {
                svc: sender.to_owned(),
                timeout: None,
                config: None,
            })
        } else {
            None
        };
        Self {
            oid: oid.clone().into(),
            meta: None,
            enabled: true,
            logic: None,
            action,
            status: None,
            value: ValueOptionOwned::No,
            t: None,
        }
    }
    pub fn from_raw_event(oid: &OID, ev: RawStateEventOwned, sender: &str) -> Self {
        let action = if oid.kind() == ItemKind::Unit {
            Some(ActionConfig {
                svc: sender.to_owned(),
                timeout: None,
                config: None,
            })
        } else {
            None
        };
        Self {
            oid: oid.clone().into(),
            meta: None,
            enabled: true,
            logic: None,
            action,
            status: Some(ev.status),
            value: ev.value,
            t: ev.t,
        }
    }
    pub fn split(self) -> (ItemData, StateData) {
        let item_data: ItemData = ItemData {
            oid: self.oid,
            meta: self.meta,
            enabled: atomic::AtomicBool::new(self.enabled),
            logic: self.logic,
            action: self.action,
            source: None,
            state: None,
        };
        let state_data: StateData = StateData {
            status: self.status,
            value: self.value,
            t: self.t,
        };
        (item_data, state_data)
    }
}

#[derive(Serialize, Deserialize, Debug, bmart::tools::Sorting)]
#[serde(deny_unknown_fields)]
#[sorting(id = "oid")]
pub struct ItemData {
    //#[serde(deserialize_with = "eva_common::deserialize_oid")]
    oid: Arc<OID>,
    #[serde(skip_serializing)]
    state: Option<Mutex<ItemState>>, // None for macros
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<Value>, // Optional location
    // props
    #[serde(skip)]
    source: Option<Source>, // None for local items
    #[serde(
        default = "eva_common::tools::atomic_true",
        serialize_with = "eva_common::tools::serialize_atomic_bool",
        deserialize_with = "eva_common::tools::deserialize_atomic_bool"
    )]
    enabled: atomic::AtomicBool,
    // local config only
    #[serde(skip_serializing_if = "Option::is_none")]
    logic: Option<Logic>, // Optional logic (for all, except macros), local only
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<ActionConfig>, // for units and lmacros
}

#[inline]
fn check_item_data(data: &ItemData) -> EResult<()> {
    macro_rules! check_none {
        ($field: expr, $n: expr) => {
            if $field.is_some() {
                return Err(Error::invalid_data(format!("unsupported field: {}", $n)));
            }
        };
    }
    match data.oid.kind() {
        ItemKind::Unit => {}
        ItemKind::Sensor | ItemKind::Lvar => {
            check_none!(data.action, "action");
        }
        ItemKind::Lmacro => {
            check_none!(data.logic, "logic");
        }
    }
    Ok(())
}

impl Hash for ItemData {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.oid.hash(state);
    }
}

impl ItemData {
    fn from_repl_item(i: ReplicationInventoryItem, source: Source) -> EResult<Self> {
        let state = if i.oid.kind() == ItemKind::Lmacro {
            None
        } else {
            Some(Mutex::new(ItemState::new(
                i.status.unwrap_or_default(),
                Into::<Option<Value>>::into(i.value).unwrap_or_default(),
                i.act,
                i.ieid
                    .ok_or_else(|| Error::invalid_data(format!("IEID missing ({})", i.oid)))?,
                i.t.ok_or_else(|| Error::invalid_data(format!("Set time missing ({})", i.oid)))?,
            )))
        };
        Ok(Self {
            oid: i.oid.into(),
            state,
            meta: i.meta,
            source: Some(source),
            enabled: atomic::AtomicBool::new(i.enabled),
            logic: None,
            action: None,
        })
    }
    #[inline]
    pub fn oid(&self) -> &OID {
        &self.oid
    }
    #[inline]
    pub fn state(&self) -> Option<&Mutex<ItemState>> {
        self.state.as_ref()
    }
    #[inline]
    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }
    #[inline]
    pub fn meta(&self) -> Option<&Value> {
        self.meta.as_ref()
    }
    #[inline]
    pub fn enabled(&self) -> bool {
        self.enabled.load(atomic::Ordering::SeqCst)
    }
    #[inline]
    pub fn set_enabled(&self, val: bool) {
        self.enabled.store(val, atomic::Ordering::SeqCst);
    }
    #[inline]
    pub fn logic(&self) -> Option<&Logic> {
        self.logic.as_ref()
    }
    #[inline]
    pub fn action(&self) -> Option<&ActionConfig> {
        self.action.as_ref()
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    pub fn local_state_event(&self) -> Option<LocalStateEvent> {
        if let Some(ref st) = self.state {
            let state = st.lock();
            Some((&*state).into())
        } else {
            None
        }
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    pub fn db_state(&self) -> Option<DbState> {
        if let Some(ref st) = self.state {
            let state = st.lock();
            Some((&*state).into())
        } else {
            None
        }
    }
    pub fn config(&self) -> EResult<Value> {
        if self.source.is_some() {
            Err(Error::failed("unable to get config for the remote item"))
        } else {
            to_value(self).map_err(Into::into)
        }
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    pub fn state_and_info<'a>(&'a self, system_name: &'a str) -> ItemStateAndInfo<'a> {
        let (status, value, act, ieid, t) = if let Some(ref st) = self.state {
            let state = st.lock();
            (
                Some(state.status),
                ValueOptionOwned::Value(state.value.clone()),
                state.act,
                Some(state.ieid),
                Some(state.t),
            )
        } else {
            (None, ValueOptionOwned::No, None, None, None)
        };
        let (node, connected) = if let Some(ref src) = self.source {
            (src.node(), src.online())
        } else {
            (system_name, true)
        };
        ItemStateAndInfo {
            oid: &self.oid,
            status,
            value,
            act,
            ieid: if connected {
                ieid
            } else if ieid.is_some() {
                Some(IEID::new(0, 0))
            } else {
                None
            },
            t,
            node,
            connected,
        }
    }
    /// # Panics
    ///
    /// Will panic if the state mutex is poisoned
    pub fn full_state_and_info<'a>(&'a self, system_name: &'a str) -> FullItemStateAndInfo<'a> {
        FullItemStateAndInfo {
            si: self.state_and_info(system_name),
            meta: self.meta.as_ref(),
            enabled: self.enabled(),
        }
    }
}

#[derive(Debug)]
pub struct SourceData {
    online: atomic::AtomicBool,
    destroyed: atomic::AtomicBool,
    svc: String,
}

impl SourceData {
    fn new(svc: &str) -> Self {
        Self {
            online: atomic::AtomicBool::new(true),
            destroyed: <_>::default(),
            svc: svc.to_owned(),
        }
    }
}

pub type Item = Arc<ItemData>;
pub type Source = Arc<ItemSource>;

#[derive(Default)]
pub struct Inventory {
    items: ItemMap,
    // all elements should be ARC as hashmaps are CLONED for replication purposes
    items_by_source: HashMap<Option<String>, HashMap<Arc<OID>, Item>>,
    sources: HashMap<String, Source>,
}

// do not create heavy remove functions as they lock the inventory for a long time
impl Inventory {
    pub fn new() -> Self {
        <_>::default()
    }
    pub fn stats(&self) -> InventoryStats {
        let mut st = HashMap::new();
        let mut item_cnt = 0;
        for (src, items) in &self.items_by_source {
            item_cnt += items.len();
            st.insert(
                src.as_ref()
                    .map_or_else(|| crate::LOCAL_NODE_ALIAS.to_owned(), ToOwned::to_owned),
                items.len(),
            );
        }
        InventoryStats {
            sources: st,
            items: item_cnt,
        }
    }
    #[inline]
    pub fn get_or_create_source(&self, source_id: &str, svc: &str) -> Source {
        self.sources
            .get(source_id)
            .map_or_else(|| create_source(source_id, svc), Clone::clone)
    }
    #[inline]
    pub fn get_items_by_source(&self, source_id: &str) -> Option<HashMap<Arc<OID>, Item>> {
        self.items_by_source
            .get(&Some(source_id.to_owned()))
            .map(Clone::clone)
    }
    #[inline]
    pub fn list_local_items(&self) -> Vec<Item> {
        let mut result = Vec::new();
        if let Some(items) = self.items_by_source.get(&None) {
            for item in items.values() {
                result.push(item.clone());
            }
        }
        result
    }
    #[inline]
    pub fn mark_source_online(&self, source_id: &str, online: bool) {
        if let Some(source) = self.sources.get(source_id) {
            source.mark_online(online);
        }
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    ///
    /// requires boot id - generates new IEID and state if missing
    pub fn append_item_from_value(
        &mut self,
        oid: &OID,
        config_value: serde_json::Value, // use serde json value as registry returns them
        state: Option<ItemState>,
        boot_id: u64,
    ) -> EResult<Item> {
        let mut data: ItemData = serde_json::from_value(config_value)?;
        if data.oid.as_ref() != oid {
            return Err(Error::invalid_data("oid does not match"));
        }
        check_item_data(&data)?;
        let tp = data.oid.kind();
        if let Some(mut st) = state {
            if tp == ItemKind::Unit {
                st.act = Some(0);
            }
            data.state = Some(Mutex::new(st));
        } else if data.state.is_none() && tp != ItemKind::Lmacro {
            data.state = Some(Mutex::new(ItemState::new0(IEID::generate(boot_id), tp)));
        }
        self.append_item(Arc::new(data), true)
    }
    #[inline]
    fn get_or_generate_state(&self, oid: &OID, boot_id: u64) -> ItemState {
        if let Some(item) = self.get_item(oid) {
            if let Some(ref old_stc) = item.state {
                return old_stc.lock().clone();
            }
        }
        ItemState::new0(IEID::generate(boot_id), oid.kind())
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    ///
    /// requires boot id - generates new IEID and state if missing
    pub fn append_item_from_config_data(
        &mut self,
        config_data: ItemConfigData, // use serde json value as registry returns them
        boot_id: u64,
    ) -> EResult<Item> {
        let (mut data, sd) = config_data.split();
        check_item_data(&data)?;
        let tp = data.oid.kind();
        if data.oid.kind() != ItemKind::Lmacro {
            let mut state = self.get_or_generate_state(&data.oid, boot_id);
            if tp == ItemKind::Unit {
                state.act = Some(0);
            }
            if let Some(status) = sd.status {
                state.status = status;
            }
            if let ValueOptionOwned::Value(value) = sd.value {
                state.value = value;
            }
            if let Some(t) = sd.t {
                state.t = t;
            }
            data.state = Some(Mutex::new(state));
        }
        self.append_item(Arc::new(data), true)
    }
    // creates empty item
    pub fn create_item(
        &mut self,
        oid: OID,
        ieid: Option<IEID>,
        from: Option<(&str, &str)>, // source_id , svc
    ) -> EResult<Item> {
        let tp = oid.kind();
        let source = from.as_ref().map(|i| {
            if let Some(src) = self.sources.get(i.0) {
                src.clone()
            } else {
                create_source(i.0, i.1)
            }
        });
        let item = Arc::new(ItemData {
            oid: oid.into(),
            state: if tp == ItemKind::Lmacro {
                None
            } else {
                Some(Mutex::new(ItemState::new0(
                    ieid.ok_or_else(|| Error::invalid_data("IEID not specified"))?,
                    tp,
                )))
            },
            source,
            enabled: atomic::AtomicBool::new(true),
            meta: None,
            logic: None,
            action: None,
        });
        self.append_item(item, false)
    }
    pub fn append_remote_item(
        &mut self,
        remote_item: ReplicationInventoryItem,
        source: Source,
    ) -> EResult<Item> {
        let item_data: ItemData = ItemData::from_repl_item(remote_item, source)?;
        self.append_item(Arc::new(item_data), true)
    }
    fn append_item(&mut self, item: Item, replace: bool) -> EResult<Item> {
        let source = item.source.clone();
        self.items.append(item.clone(), replace)?;
        let mut node = source.as_ref().map(|i| i.node.clone());
        if let Some(items) = self.items_by_source.get_mut(&node) {
            items.insert(item.oid.clone(), item.clone());
        } else {
            let mut items = HashMap::new();
            items.insert(item.oid.clone(), item.clone());
            if let Some(s) = source {
                self.items_by_source.insert(node.clone(), items);
                self.sources.insert(node.take().unwrap(), s);
            } else {
                self.items_by_source.insert(node, items);
            }
        }
        Ok(item)
    }
    #[inline]
    pub fn get_items_by_mask(
        &self,
        mask: &OIDMask,
        filter: &Filter,
        include_stateless: bool,
    ) -> Vec<Item> {
        self.items.get_by_mask(mask, filter, include_stateless)
    }
    #[inline]
    pub fn get_item(&self, oid: &OID) -> Option<Item> {
        self.items.get(oid)
    }
    #[inline]
    pub fn remove_item(&mut self, oid: &OID) -> Option<Item> {
        if let Some(item) = self.items.remove(oid) {
            let node = item.source.as_ref().map(|x| x.node.clone());
            if let Some(items) = self.items_by_source.get_mut(&node) {
                items.remove(&item.oid);
                if items.is_empty() {
                    self.items_by_source.remove(&node);
                    node.map(|ref i| self.sources.remove(i));
                }
            }
            Some(item)
        } else {
            None
        }
    }
    #[inline]
    pub fn list_items_with_states(
        &self,
        mask_list: &OIDMaskList,
        source_id: Option<NodeFilter<'_>>,
    ) -> Vec<Item> {
        let mut filter = Filter::default();
        if let Some(v) = source_id {
            filter.set_node(v);
        }
        #[allow(clippy::mutable_key_type)]
        let mut h: HashSet<Item> = HashSet::default();
        for mask in mask_list.oid_masks() {
            let items = self.get_items_by_mask(mask, &filter, false);
            for item in items {
                h.insert(item);
            }
        }
        h.into_iter().collect()
    }
}

#[derive(Default, Debug)]
struct ItemTree {
    childs: HashMap<String, ItemTree>,
    members: HashMap<Arc<OID>, Item>,
    members_wildcard: HashMap<Arc<OID>, Item>,
}

impl ItemTree {
    fn is_empty(&self) -> bool {
        self.childs.is_empty() && self.members.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct ItemMap {
    unit: ItemTree,
    sensor: ItemTree,
    lvar: ItemTree,
    lmacro: ItemTree,
}

pub enum NodeFilter<'a> {
    Local,
    Remote(&'a str),
    RemoteAny,
}

#[derive(Default)]
pub struct Filter<'a> {
    include: Option<&'a OIDMaskList>,
    exclude: Option<&'a OIDMaskList>,
    node: Option<NodeFilter<'a>>,
}

impl<'a> Filter<'a> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn include(mut self, mask_list: &'a OIDMaskList) -> Self {
        self.include = Some(mask_list);
        self
    }
    pub fn exclude(mut self, mask_list: &'a OIDMaskList) -> Self {
        self.exclude = Some(mask_list);
        self
    }
    pub fn node(mut self, sid: NodeFilter<'a>) -> Self {
        self.node = Some(sid);
        self
    }
    #[inline]
    pub fn set_include(&mut self, mask_list: &'a OIDMaskList) {
        self.include = Some(mask_list);
    }
    #[inline]
    pub fn set_exclude(&mut self, mask_list: &'a OIDMaskList) {
        self.exclude = Some(mask_list);
    }
    #[inline]
    pub fn set_node(&mut self, sid: NodeFilter<'a>) {
        self.node = Some(sid);
    }
    #[inline]
    pub fn matches(&self, item: &Item) -> bool {
        if let Some(ref node) = self.node {
            match node {
                NodeFilter::Local => {
                    if item.source.is_some() {
                        return false;
                    }
                }
                NodeFilter::RemoteAny => {
                    if item.source.is_none() {
                        return false;
                    }
                }
                NodeFilter::Remote(id) => {
                    if let Some(ref source) = item.source {
                        if *id != "#" && *id != "*" && source.node != *id {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
        }
        if let Some(f) = self.include {
            if !f.matches(&item.oid) {
                return false;
            }
        }
        if let Some(f) = self.exclude {
            !f.matches(item.oid.as_ref())
        } else {
            true
        }
    }
}

impl ItemMap {
    #[inline]
    fn get_tree(&self, tp: ItemKind) -> &ItemTree {
        match tp {
            ItemKind::Unit => &self.unit,
            ItemKind::Sensor => &self.sensor,
            ItemKind::Lvar => &self.lvar,
            ItemKind::Lmacro => &self.lmacro,
            //_ => Err(Error::not_implemented()),
        }
    }
    #[inline]
    fn get_tree_mut(&mut self, tp: ItemKind) -> &mut ItemTree {
        match tp {
            ItemKind::Unit => &mut self.unit,
            ItemKind::Sensor => &mut self.sensor,
            ItemKind::Lvar => &mut self.lvar,
            ItemKind::Lmacro => &mut self.lmacro,
            //_ => Err(Error::not_implemented()),
        }
    }
    #[inline]
    pub fn append(&mut self, item: Item, replace: bool) -> EResult<()> {
        let tree = self.get_tree_mut(item.oid.kind());
        append_item_rec(tree, item.oid.full_id().split('/'), &item, replace)
    }
    #[inline]
    pub fn get(&self, oid: &OID) -> Option<Item> {
        let tree = self.get_tree(oid.kind());
        get_item_rec(tree, oid.full_id().split('/'))
    }
    #[inline]
    pub fn remove(&mut self, oid: &OID) -> Option<Item> {
        let tree = self.get_tree_mut(oid.kind());
        remove_item_rec(tree, oid.full_id().split('/'), oid)
    }
    pub fn get_by_mask(
        &self,
        mask: &OIDMask,
        filter: &Filter,
        include_stateless: bool,
    ) -> Vec<Item> {
        if let Some(tp) = mask.kind() {
            if tp == ItemKind::Lmacro && !include_stateless {
                return Vec::new();
            }
            let tree = self.get_tree(tp);
            if let Some(chunks) = mask.chunks() {
                let mut result = Vec::new();
                get_item_by_mask_rec(tree, chunks.iter(), &mut result, filter);
                result
            } else {
                tree.members_wildcard
                    .values()
                    .filter(|x| filter.matches(x))
                    .cloned()
                    .collect()
            }
        } else {
            let mut result = Vec::new();
            if let Some(chunks) = mask.chunks() {
                get_item_by_mask_rec(&self.unit, chunks.iter(), &mut result, filter);
                get_item_by_mask_rec(&self.sensor, chunks.iter(), &mut result, filter);
                get_item_by_mask_rec(&self.lvar, chunks.iter(), &mut result, filter);
                if include_stateless {
                    get_item_by_mask_rec(&self.lmacro, chunks.iter(), &mut result, filter);
                }
            } else {
                result.extend(
                    self.unit
                        .members_wildcard
                        .values()
                        .filter(|x| filter.matches(x))
                        .cloned()
                        .collect::<Vec<Item>>(),
                );
                result.extend(
                    self.sensor
                        .members_wildcard
                        .values()
                        .filter(|x| filter.matches(x))
                        .cloned()
                        .collect::<Vec<Item>>(),
                );
                result.extend(
                    self.lvar
                        .members_wildcard
                        .values()
                        .filter(|x| filter.matches(x))
                        .cloned()
                        .collect::<Vec<Item>>(),
                );
                if include_stateless {
                    result.extend(
                        self.lmacro
                            .members_wildcard
                            .values()
                            .filter(|x| filter.matches(x))
                            .cloned()
                            .collect::<Vec<Item>>(),
                    );
                }
            }
            result
        }
    }
}

fn get_item_rec(tree: &ItemTree, mut sp: Split<char>) -> Option<Item> {
    if let Some(chunk) = sp.next() {
        if let Some(child) = tree.childs.get(chunk) {
            get_item_rec(child, sp)
        } else {
            None
        }
    } else if tree.members.is_empty() {
        None
    } else {
        Some(tree.members.values().next().unwrap().clone())
    }
}
fn remove_item_rec(tree: &mut ItemTree, mut sp: Split<char>, oid: &OID) -> Option<Item> {
    if let Some(chunk) = sp.next() {
        tree.members_wildcard.remove(oid)?;
        let item = if let Some(c) = tree.childs.get_mut(chunk) {
            let item = remove_item_rec(c, sp.clone(), oid)?;
            if c.is_empty() {
                tree.childs.remove(chunk);
            }
            item
        } else {
            return None;
        };
        Some(item)
    } else {
        tree.members.remove(oid)
    }
}

fn get_item_by_mask_rec(
    tree: &ItemTree,
    mut iter: std::slice::Iter<&str>,
    result: &mut Vec<Item>,
    filter: &Filter,
) {
    if let Some(chunk) = iter.next() {
        if *chunk == "#" {
            result.extend(
                tree.members_wildcard
                    .values()
                    .filter(|x| filter.matches(x))
                    .cloned()
                    .collect::<Vec<Item>>(),
            );
        } else if *chunk == "+" {
            for child in tree.childs.values() {
                get_item_by_mask_rec(child, iter.clone(), result, filter);
            }
        } else if let Some(f) = chunk.strip_prefix('!') {
            for child in tree.childs.values_match_key_formula(f) {
                get_item_by_mask_rec(child, iter.clone(), result, filter);
            }
        } else if let Some(child) = tree.childs.get(*chunk) {
            get_item_by_mask_rec(child, iter, result, filter);
        }
    } else {
        result.extend(
            tree.members
                .values()
                .filter(|x| filter.matches(x))
                .cloned()
                .collect::<Vec<Item>>(),
        );
    }
}

fn append_item_rec(
    tree: &mut ItemTree,
    mut sp: Split<char>,
    item: &Item,
    replace: bool,
) -> EResult<()> {
    if let Some(chunk) = sp.next() {
        if tree.members_wildcard.contains_key(&item.oid) && !replace {
            return Err(Error::duplicate(format!(
                "item {} is already registered",
                item.oid
            )));
        }
        tree.members_wildcard.insert(item.oid.clone(), item.clone());
        if let Some(c) = tree.childs.get_mut(chunk) {
            append_item_rec(c, sp.clone(), item, replace)?;
        } else {
            let mut child = ItemTree::default();
            append_item_rec(&mut child, sp.clone(), item, replace)?;
            tree.childs.insert(chunk.to_owned(), child);
        }
        Ok(())
    } else if tree.members.contains_key(&item.oid) && !replace {
        Err(Error::duplicate(format!(
            "item {} is already registered",
            item.oid
        )))
    } else {
        tree.members.insert(item.oid.clone(), item.clone());
        Ok(())
    }
}
