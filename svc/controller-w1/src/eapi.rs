use eva_common::common_payloads::{ParamsOID, ParamsUuid};
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, Action};
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;

use crate::w1;

pub struct Handlers {
    info: ServiceInfo,
    tx: async_channel::Sender<(String, Vec<u8>)>,
}

impl Handlers {
    #[inline]
    pub fn new(info: ServiceInfo, tx: async_channel::Sender<(String, Vec<u8>)>) -> Self {
        Self { info, tx }
    }
}
#[derive(Serialize)]
struct W1DeviceInfo<'a> {
    #[serde(rename = "type")]
    tp: Option<&'a str>,
    family: Option<u32>,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    attrs: Option<Vec<&'a str>>,
}
impl PartialEq for W1DeviceInfo<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
impl Eq for W1DeviceInfo<'_> {}

impl Ord for W1DeviceInfo<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.tp.cmp(&other.tp) {
            std::cmp::Ordering::Equal => self.path.cmp(other.path),
            v => v,
        }
    }
}
impl PartialOrd for W1DeviceInfo<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl W1DeviceInfo<'_> {
    #[inline]
    fn prepared(mut self) -> Self {
        if let Some(ref attrs) = self.attrs {
            let mut a = attrs.clone();
            a.sort_unstable();
            self.attrs.replace(a);
        }
        self
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "w1.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsW1Get {
                        path: String,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    let p: ParamsW1Get = unpack(payload)?;
                    let val_str: String = w1::get(
                        Arc::new(p.path),
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                        p.retries.unwrap_or_else(|| {
                            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                        }),
                    )
                    .await?;
                    let val: Value = val_str.parse().unwrap();
                    Ok(Some(pack(&val)?))
                }
            }
            "w1.set" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsW1Set {
                        path: String,
                        value: Value,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        verify: bool,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    let p: ParamsW1Set = unpack(payload)?;
                    w1::set(
                        Arc::new(p.path),
                        Arc::new(p.value.to_string()),
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                        p.verify,
                        p.retries.unwrap_or_else(|| {
                            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                        }),
                    )
                    .await?;
                    Ok(None)
                }
            }
            "w1.scan" => {
                #[derive(Deserialize, Default)]
                #[serde(deny_unknown_fields)]
                struct ParamsW1Scan {
                    types: Option<Value>,
                    attrs_any: Option<Value>,
                    attrs_all: Option<Value>,
                    #[serde(
                        default,
                        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                    )]
                    timeout: Option<Duration>,
                    #[serde(default)]
                    full: bool,
                }
                let p: ParamsW1Scan = if payload.is_empty() {
                    ParamsW1Scan::default()
                } else {
                    unpack(payload)?
                };
                let types_list: Option<Vec<String>> = if let Some(types) = p.types {
                    let types_v_list: Vec<Value> = types.try_into()?;
                    Some(
                        types_v_list
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<String>>(),
                    )
                } else {
                    None
                };
                let attrs_any_list: Option<Vec<String>> = if let Some(attrs_any) = p.attrs_any {
                    let attrs_any_v_list: Vec<Value> = attrs_any.try_into()?;
                    Some(
                        attrs_any_v_list
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<String>>(),
                    )
                } else {
                    None
                };
                let attrs_all_list: Option<Vec<String>> = if let Some(attrs_all) = p.attrs_all {
                    let attrs_all_v_list: Vec<Value> = attrs_all.try_into()?;
                    Some(
                        attrs_all_v_list
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<String>>(),
                    )
                } else {
                    None
                };
                let devices = w1::scan(
                    types_list,
                    attrs_any_list,
                    attrs_all_list,
                    p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                )
                .await?;
                let info_list: Vec<(Result<owfs::DeviceInfo, owfs::Error>, &owfs::Device)> =
                    devices
                        .iter()
                        .map(|dev| unsafe { (dev.info(), dev) })
                        .collect();
                let mut result: Vec<W1DeviceInfo> = info_list
                    .iter()
                    .map(|res| {
                        if let Ok(ref info) = res.0 {
                            W1DeviceInfo {
                                tp: Some(info.w1_type()),
                                family: info.family(),
                                path: res.1.path(),
                                attrs: if p.full { Some(res.1.attrs()) } else { None },
                            }
                        } else {
                            W1DeviceInfo {
                                tp: None,
                                family: None,
                                path: res.1.path(),
                                attrs: if p.full { Some(res.1.attrs()) } else { None },
                            }
                        }
                        .prepared()
                    })
                    .collect();
                result.sort();
                Ok(Some(pack(&result)?))
            }
            "w1.info" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsW1Info {
                        path: String,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                    }
                    let p: ParamsW1Info = unpack(payload)?;
                    let op = eva_common::op::Op::new(
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                    );
                    let dev = Arc::new(w1::load_device(Arc::new(p.path), op.timeout()?).await?);
                    let info = w1::device_info(dev.clone(), op.timeout()?).await?;
                    Ok(Some(pack(
                        &W1DeviceInfo {
                            tp: Some(info.w1_type()),
                            family: info.family(),
                            path: dev.path(),
                            attrs: Some(dev.attrs()),
                        }
                        .prepared(),
                    )?))
                }
            }
            "action" => {
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let action: Action = unpack(payload)?;
                let actt = crate::ACTT.get().unwrap();
                let action_topic = format_action_topic(action.oid());
                let payload_pending = pack(&action.event_pending())?;
                let action_uuid = *action.uuid();
                let action_oid = action.oid().clone();
                if let Some(tx) = crate::ACTION_QUEUES.get().unwrap().get(action.oid()) {
                    actt.append(action.oid(), action_uuid)?;
                    if let Err(e) = tx.send(action).await {
                        actt.remove(&action_oid, &action_uuid)?;
                        Err(Error::core(format!("action queue broken: {}", e)).into())
                    } else if let Err(e) = self
                        .tx
                        .send((action_topic, payload_pending))
                        .await
                        .map_err(Error::io)
                    {
                        actt.remove(&action_oid, &action_uuid)?;
                        Err(e.into())
                    } else {
                        Ok(None)
                    }
                } else {
                    Err(Error::not_found(format!("{} has no action map", action.oid())).into())
                }
            }
            "terminate" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsUuid = unpack(payload)?;
                    crate::ACTT.get().unwrap().mark_terminated(&p.u)?;
                    Ok(None)
                }
            }
            "kill" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsOID = unpack(payload)?;
                    crate::ACTT.get().unwrap().mark_killed(&p.i)?;
                    Ok(None)
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}
