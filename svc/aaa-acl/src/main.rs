use bmart::tools::Sorting;
use busrt::QoS;
use eva_common::common_payloads::{IdOrList, ParamsId, ParamsIdOrList};
use eva_common::events::AAA_ACL_TOPIC;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

err_logger!();

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "ACL service";
const ACL_NONE: &str = "none";

lazy_static::lazy_static! {
    static ref ACLS: Mutex<HashMap<String, Acl>> = <_>::default();
    static ref REG: OnceCell<Registry> = <_>::default();
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(val: &bool) -> bool {
    !val
}

pub const ID_ALLOWED_SYMBOLS: &str = "_.()[]-\\";

/// full Acl object, stored in the registry
#[derive(Default, Debug, Serialize, Deserialize, Sorting)]
#[serde(deny_unknown_fields)]
struct Acl {
    id: String,
    #[serde(default, skip_serializing_if = "is_false")]
    admin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    read: Option<HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write: Option<HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deny_read: Option<HashMap<String, HashSet<Value>>>,
    #[serde(alias = "deny", skip_serializing_if = "Option::is_none")]
    deny_write: Option<HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ops: Option<HashSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<HashMap<String, HashSet<Value>>>,
}

/// Acl info object, provided on list
#[derive(Serialize, Sorting)]
struct AclInfo<'a> {
    id: &'a str,
    #[serde(default, skip_serializing_if = "is_false")]
    admin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    read: Option<&'a HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write: Option<&'a HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deny_read: Option<&'a HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deny_write: Option<&'a HashMap<String, HashSet<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ops: Option<&'a HashSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<&'a HashMap<String, HashSet<Value>>>,
}

/// Acl data object, provided by "format", contains either a single Acl or combined
#[derive(Serialize, Default)]
struct AclData<'a> {
    id: String,
    from: HashSet<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    admin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    read: Option<HashMap<&'a str, HashSet<&'a Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write: Option<HashMap<&'a str, HashSet<&'a Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deny_read: Option<HashMap<&'a str, HashSet<&'a Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deny_write: Option<HashMap<&'a str, HashSet<&'a Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ops: Option<HashSet<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<HashMap<&'a str, HashSet<&'a Value>>>,
}

impl<'a> From<&'a Acl> for AclInfo<'a> {
    fn from(acl: &'a Acl) -> AclInfo<'a> {
        AclInfo {
            id: &acl.id,
            admin: acl.admin,
            read: acl.read.as_ref(),
            write: acl.write.as_ref(),
            deny_read: acl.deny_read.as_ref(),
            deny_write: acl.deny_write.as_ref(),
            ops: acl.ops.as_ref(),
            meta: acl.meta.as_ref(),
        }
    }
}

impl<'a> From<&'a Acl> for AclData<'a> {
    fn from(acl: &'a Acl) -> AclData<'a> {
        AclData {
            id: acl.id.clone(),
            from: {
                let mut h = HashSet::new();
                h.insert(acl.id.clone());
                h
            },
            admin: acl.admin,
            read: acl.read.as_ref().map(|r| {
                r.iter()
                    .map(|(k, v)| (k.as_str(), v.iter().collect()))
                    .collect()
            }),
            write: acl.write.as_ref().map(|r| {
                r.iter()
                    .map(|(k, v)| (k.as_str(), v.iter().collect()))
                    .collect()
            }),
            deny_read: acl.deny_read.as_ref().map(|r| {
                r.iter()
                    .map(|(k, v)| (k.as_str(), v.iter().collect()))
                    .collect()
            }),
            deny_write: acl.deny_write.as_ref().map(|r| {
                r.iter()
                    .map(|(k, v)| (k.as_str(), v.iter().collect()))
                    .collect()
            }),
            ops: acl
                .ops
                .as_ref()
                .map(|r| r.iter().map(String::as_str).collect()),
            meta: acl.meta.as_ref().map(|r| {
                r.iter()
                    .map(|(k, v)| (k.as_str(), v.iter().collect()))
                    .collect()
            }),
        }
    }
}

impl<'a> From<Vec<&'a Acl>> for AclData<'a> {
    fn from(acls: Vec<&'a Acl>) -> AclData<'a> {
        let mut acl_data = AclData::default();
        let mut ids = Vec::new();
        for acl in acls {
            ids.push(acl.id.clone());
            if acl.admin {
                acl_data.admin = true;
            }
            macro_rules! form_field {
                ($src: expr, $dst: expr) => {
                    $src.as_ref().map(|r| {
                        if let Some(ref mut f) = $dst {
                            // $dst is already allocated
                            for (k, v) in r {
                                if let Some(set) = f.get_mut(k.as_str()) {
                                    // $dst has the key
                                    for val in v {
                                        set.insert(val);
                                    }
                                } else {
                                    // $dst has no key - collect $src key values
                                    let set = v.iter().collect();
                                    f.insert(k, set);
                                }
                            }
                        } else {
                            // $dst is not allocated yet - collect $src hashmap
                            let h = r
                                .iter()
                                .map(|(k, v)| (k.as_str(), v.iter().collect()))
                                .collect();
                            $dst.replace(h);
                        }
                    });
                };
            }
            form_field!(acl.read, acl_data.read);
            form_field!(acl.write, acl_data.write);
            form_field!(acl.deny_read, acl_data.deny_read);
            form_field!(acl.deny_write, acl_data.deny_write);
            if let Some(ops) = acl.ops.as_ref() {
                if let Some(ref mut f) = acl_data.ops {
                    for o in ops {
                        f.insert(o);
                    }
                } else {
                    let v = ops.iter().map(String::as_str).collect();
                    acl_data.ops.replace(v);
                }
            };
            form_field!(acl.meta, acl_data.meta);
        }
        ids.sort();
        acl_data.id = ids.join("+");
        for i in ids {
            acl_data.from.insert(i);
        }
        acl_data
    }
}

struct Handlers {
    info: ServiceInfo,
    tx: async_channel::Sender<(String, Option<Value>)>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AclOrId {
    Acl(Box<Acl>),
    Id(String),
}
impl AclOrId {
    fn as_str(&self) -> &str {
        match self {
            Self::Acl(acl) => acl.id.as_str(),
            Self::Id(id) => id.as_str(),
        }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        macro_rules! reg {
            () => {{
                let reg = REG.get();
                if reg.is_none() {
                    return Err(Error::not_ready("not loaded yet").into());
                }
                reg
            }};
        }
        match method {
            "acl.list" => {
                if payload.is_empty() {
                    let acls = ACLS.lock().unwrap();
                    let mut acl_info: Vec<AclInfo> = acls.values().map(Into::into).collect();
                    acl_info.sort();
                    Ok(Some(pack(&acl_info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "acl.deploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Acls {
                        acls: Vec<Acl>,
                    }
                    let new_acls: Acls = unpack(payload)?;
                    let reg = reg!();
                    let mut reg_keys = Vec::new();
                    {
                        let mut acls = ACLS.lock().unwrap();
                        for acl in &new_acls.acls {
                            for c in acl.id.chars() {
                                if !c.is_alphanumeric()
                                    && !ID_ALLOWED_SYMBOLS.contains(c)
                                    && c != '/'
                                {
                                    return Err(Error::invalid_data(format!(
                                        "invalid symbol in ACL id {}: {}",
                                        acl.id, c
                                    ))
                                    .into());
                                }
                            }
                        }
                        for acl in new_acls.acls {
                            let acl_id = acl.id.clone();
                            reg_keys.push((acl_id.clone(), to_value(&acl)?));
                            acls.insert(acl_id, acl);
                        }
                    }
                    for (key, val) in reg_keys {
                        warn!("ACL created: {}", key);
                        reg.as_ref()
                            .unwrap()
                            .key_set(&format!("acl/{key}"), val.clone())
                            .await?;
                        self.tx.send((key, Some(val))).await.log_ef();
                    }
                    Ok(None)
                }
            }
            "acl.undeploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Acls {
                        acls: Vec<AclOrId>,
                    }
                    let acl_ids: Acls = unpack(payload)?;
                    let reg = reg!();
                    let mut ids = Vec::new();
                    {
                        let mut acls = ACLS.lock().unwrap();
                        for acl_id in acl_ids.acls {
                            if let Some(acl) = acls.remove(acl_id.as_str()) {
                                warn!("ACL destroyed: {}", acl.id);
                                ids.push(acl.id);
                            }
                        }
                    }
                    for id in ids {
                        reg.as_ref()
                            .unwrap()
                            .key_delete(&format!("acl/{id}"))
                            .await?;
                        self.tx.send((id, None)).await.log_ef();
                    }
                    Ok(None)
                }
            }
            "acl.get_config" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    if let Some(acl) = ACLS.lock().unwrap().get(p.i) {
                        Ok(Some(pack(acl)?))
                    } else {
                        Err(Error::not_found(format!("ACL {} not found", p.i)).into())
                    }
                }
            }
            "acl.export" => {
                #[derive(Serialize)]
                #[serde(deny_unknown_fields)]
                struct Acls<'a> {
                    acls: Vec<&'a Acl>,
                }
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let mut configs = Vec::new();
                    let acls = ACLS.lock().unwrap();
                    if p.i == "#" || p.i == "*" {
                        for acl in acls.values() {
                            configs.push(acl);
                        }
                    } else if let Some(acl) = acls.get(p.i) {
                        configs.push(acl);
                    } else {
                        return Err(Error::not_found(format!("ACL {} not found", p.i)).into());
                    }
                    configs.sort();
                    Ok(Some(pack(&Acls { acls: configs })?))
                }
            }
            "acl.destroy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let reg = reg!();
                    if ACLS.lock().unwrap().remove(p.i).is_some() {
                        reg.as_ref()
                            .unwrap()
                            .key_delete(&format!("acl/{}", p.i))
                            .await?;
                        warn!("ACL destroyed: {}", p.i);
                        self.tx.send((p.i.to_owned(), None)).await.log_ef();
                        Ok(None)
                    } else {
                        Err(Error::not_found(format!("ACL {} not found", p.i)).into())
                    }
                }
            }
            "acl.format" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdOrList = unpack(payload)?;
                    let acls = ACLS.lock().unwrap();
                    let mut acl_data: AclData = match p.i {
                        IdOrList::Single(i) => {
                            if let Some(acl) = acls.get(i) {
                                acl.into()
                            } else {
                                AclData::default()
                            }
                        }
                        IdOrList::Multiple(ids) => {
                            let mut srcs: Vec<&Acl> = Vec::with_capacity(ids.len());
                            for i in ids {
                                if let Some(acl) = acls.get(i) {
                                    srcs.push(acl);
                                }
                            }
                            srcs.into()
                        }
                    };
                    if acl_data.from.is_empty() {
                        warn!("ACLs not found, using empty ACL");
                        acl_data.id = ACL_NONE.to_owned();
                    }
                    Ok(Some(pack(&acl_data)?))
                }
            }
            m => svc_handle_default_rpc(m, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let _config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let (tx, rx) = async_channel::bounded(1024);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("acl.list").description("list ACLs"));
    info.add_method(
        ServiceMethod::new("acl.deploy")
            .description("deploy ACLs")
            .required("acls"),
    );
    info.add_method(
        ServiceMethod::new("acl.undeploy")
            .description("undeploy ACLs")
            .required("acls"),
    );
    info.add_method(
        ServiceMethod::new("acl.get_config")
            .description("get ACL config")
            .required("i"),
    );
    info.add_method(ServiceMethod::new("acl.export").required("i"));
    info.add_method(
        ServiceMethod::new("acl.destroy")
            .description("destroy ACL")
            .required("i"),
    );
    info.add_method(
        ServiceMethod::new("acl.format")
            .description("format ACL")
            .required("i"),
    );
    let handlers = Handlers { info, tx };
    let rpc = initial.init_rpc(handlers).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    let cl = client.clone();
    tokio::spawn(async move {
        while let Ok((key, val)) = rx.recv().await {
            let payload: busrt::borrow::Cow = if let Some(v) = val {
                if let Ok(p) = pack(&v).log_err() {
                    p.into()
                } else {
                    continue;
                }
            } else {
                busrt::empty_payload!()
            };
            cl.lock()
                .await
                .publish(&format!("{}{}", AAA_ACL_TOPIC, key), payload, QoS::No)
                .await
                .log_ef();
        }
    });
    svc_init_logs(&initial, client.clone())?;
    let registry = initial.init_registry(&rpc);
    let reg_acls = registry.key_get_recursive("acl").await?;
    {
        let mut acls = ACLS.lock().unwrap();
        for (k, v) in reg_acls {
            debug!("ACL loaded: {}", k);
            acls.insert(k, Acl::deserialize(v)?);
        }
    }
    REG.set(registry)
        .map_err(|_| Error::core("unable to set registry object"))?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
