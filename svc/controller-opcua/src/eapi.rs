use crate::common::{deserialize_node_id_from_str, deserialize_vec_node_id_from_str};
use crate::conv::ValueConv;
use eva_common::common_payloads::{ParamsOID, ParamsUuid};
use eva_common::prelude::*;
use eva_sdk::controller::{Action, format_action_topic};
use eva_sdk::prelude::*;
use opcua::types::{NodeId, Variant, VariantTypeId};
use serde::{Deserialize, Serialize};
use std::sync::atomic;
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
                    struct ParamsNodeGet {
                        #[serde(deserialize_with = "deserialize_node_id_from_str")]
                        i: NodeId,
                        #[serde(
                            default,
                            deserialize_with = "crate::common::deserialize_opt_range"
                        )]
                        range: Option<String>,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    let p: ParamsNodeGet = unpack(payload)?;
                    let node_id = p.i.clone();
                    let result = crate::comm::read_multi(
                        vec![p.i],
                        vec![p.range.as_deref()],
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                        p.retries.unwrap_or_else(|| {
                            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                        }),
                    )
                    .await?;
                    let d = result
                        .into_iter()
                        .next()
                        .ok_or_else(|| Error::io("no data received from OPC-UA"))?;
                    Ok(Some(pack(
                        &d.value
                            .ok_or_else(|| Error::io(format!("node {node_id} read error")))?
                            .into_eva_value()?,
                    )?))
                }
            }
            "var.set" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsNodeSet {
                        #[serde(deserialize_with = "deserialize_node_id_from_str")]
                        i: NodeId,
                        value: Value,
                        #[serde(
                            default,
                            deserialize_with = "crate::common::deserialize_opt_range"
                        )]
                        range: Option<String>,
                        dimensions: Option<Vec<usize>>,
                        #[serde(rename = "type")]
                        tp: crate::common::OpcType,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    let p: ParamsNodeSet = unpack(payload)?;
                    crate::comm::write(
                        p.i,
                        p.range.as_deref(),
                        Variant::from_eva_value(p.value, p.tp.into(), p.dimensions.as_deref())?,
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                        p.retries.unwrap_or_else(|| {
                            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                        }),
                        None,
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
                    struct ParamsNodeSetBulk {
                        #[serde(deserialize_with = "deserialize_vec_node_id_from_str")]
                        i: Vec<NodeId>,
                        values: Vec<Value>,
                        #[serde(default)]
                        ranges: Vec<Option<String>>,
                        #[serde(default)]
                        dimensions: Vec<Option<Vec<usize>>>,
                        #[serde(rename = "types")]
                        tp: Vec<crate::common::OpcType>,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(default)]
                        retries: Option<u8>,
                    }
                    #[derive(Serialize)]
                    struct RespNodeSetBulk {
                        #[serde(skip_serializing_if = "Vec::is_empty")]
                        failed: Vec<String>,
                    }
                    let mut p: ParamsNodeSetBulk = unpack(payload)?;
                    let tp: Vec<VariantTypeId> = p.tp.into_iter().map(Into::into).collect();
                    p.dimensions.resize(tp.len(), None);
                    let vals: Vec<Variant> = p
                        .values
                        .into_iter()
                        .zip(tp)
                        .zip(p.dimensions)
                        .map(|((v, t), d)| Variant::from_eva_value(v, t, d.as_deref()))
                        .collect::<Result<Vec<Variant>, Error>>()?;
                    let mut ranges: Vec<Option<&str>> =
                        p.ranges.iter().map(Option::as_deref).collect();
                    ranges.resize(p.i.len(), None);
                    let failed = crate::comm::write_multi(
                        p.i,
                        ranges,
                        vals,
                        p.timeout.unwrap_or_else(|| *crate::TIMEOUT.get().unwrap()),
                        p.retries.unwrap_or_else(|| {
                            crate::DEFAULT_RETRIES.load(atomic::Ordering::SeqCst)
                        }),
                        None,
                    )
                    .await?;
                    Ok(Some(pack(&RespNodeSetBulk {
                        failed: failed.into_iter().map(|v| v.to_string()).collect(),
                    })?))
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
