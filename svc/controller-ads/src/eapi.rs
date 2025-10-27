use eva_common::common_payloads::{ParamsOID, ParamsUuid};
use eva_common::prelude::*;
use eva_sdk::controller::{format_action_topic, Action};
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{atomic, Arc};
use std::time::Duration;

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

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "var.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsSymbolGet {
                        i: Arc<String>,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    let p: ParamsSymbolGet = unpack(payload)?;
                    Ok(Some(pack(
                        &crate::adsbr::read_by_name(
                            p.i,
                            p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                            p.retries.unwrap_or_else(|| {
                                crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                            }),
                        )
                        .await?,
                    )?))
                }
            }
            "var.set" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsSymbolSet {
                        i: Arc<String>,
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
                    let p: ParamsSymbolSet = unpack(payload)?;
                    crate::adsbr::write_by_name(
                        p.i,
                        p.value,
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
            "var.set_bulk" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsSymbolSetBulk {
                        i: Vec<Arc<String>>,
                        values: Vec<Value>,
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
                    #[derive(Serialize)]
                    struct RespSymbolSetBulk {
                        #[serde(skip_serializing_if = "Vec::is_empty")]
                        failed: Vec<Arc<String>>,
                    }
                    let p: ParamsSymbolSetBulk = unpack(payload)?;
                    let failed = crate::adsbr::write_by_names_multi(
                        p.i,
                        p.values,
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                        p.verify,
                        p.retries.unwrap_or_else(|| {
                            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                        }),
                    )
                    .await?;
                    Ok(Some(pack(&RespSymbolSetBulk { failed })?))
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
