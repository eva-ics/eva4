use crate::types::EipType;
use eva_common::common_payloads::{ParamsOID, ParamsUuid};
use eva_common::prelude::*;
use eva_sdk::controller::{Action, format_action_topic};
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::sync::atomic;
use std::time::Duration;

use crate::enip;

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
                    struct ParamsTagGet {
                        i: String,
                        #[serde(rename = "type")]
                        tp: EipType,
                        size: Option<u32>,
                        count: Option<u32>,
                        bit: Option<u32>,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    let p: ParamsTagGet = unpack(payload)?;
                    Ok(Some(pack(
                        &enip::read_tag(
                            p.i,
                            p.tp,
                            p.size,
                            p.count,
                            p.bit,
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
                    struct ParamsTagSet {
                        i: String,
                        value: Value,
                        #[serde(rename = "type")]
                        tp: EipType,
                        bit: Option<u32>,
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
                    let p: ParamsTagSet = unpack(payload)?;
                    enip::write_tag(
                        p.i,
                        p.value,
                        p.tp,
                        p.bit,
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
