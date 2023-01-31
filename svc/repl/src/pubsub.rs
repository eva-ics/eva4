use eva_common::common_payloads::ParamsIdOwned;
use eva_common::events::{
    FullItemStateAndInfoOwned, ReplicationStateEvent, REPLICATION_STATE_TOPIC,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::pubsub::PS_ITEM_BULK_STATE_TOPIC;
use eva_sdk::types::FullItemState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

use crate::aaa;
use crate::nodes;

err_logger!();

pub struct PubSubHandlers {
    api_enabled: bool,
    topic_broker: psrpc::tools::TopicBroker,
    replicate_remote: bool,
}

impl PubSubHandlers {
    pub fn new(
        api_enabled: bool,
        topic_broker: psrpc::tools::TopicBroker,
        replicate_remote: bool,
    ) -> Self {
        Self {
            api_enabled,
            topic_broker,
            replicate_remote,
        }
    }
    pub fn start(&mut self, queue_size: usize, timeout: Duration) -> EResult<()> {
        let (_, mut rx) = self
            .topic_broker
            .register_prefix(crate::PS_NODE_STATE_TOPIC, queue_size)?;
        tokio::spawn(async move {
            svc_wait_core(crate::RPC.get().unwrap(), timeout, true)
                .await
                .log_ef();
            while !svc_is_terminating() {
                discovery_handler(&mut rx).await.log_ef();
            }
        });
        Ok(())
    }
}

#[async_trait::async_trait]
impl psrpc::RpcHandlers for PubSubHandlers {
    async fn handle_message(&self, message: psrpc::PsMessage) {
        self.topic_broker.process(message).await.log_ef();
    }
    #[allow(clippy::too_many_lines)]
    async fn handle_call(
        &self,
        key_id: Option<&str>,
        method: &str,
        params: Option<Value>,
    ) -> EResult<Value> {
        if !self.api_enabled {
            return Err(Error::access("Pub/Sub API is disabled"));
        }
        trace!("pub/sub {} call, key id: {:?}", method, key_id);
        match method {
            "ping" => Ok(Value::Unit),
            "pull" => {
                if let Some(key_id) = key_id {
                    if params.is_some() {
                        Err(Error::invalid_params("the method has no params"))
                    } else {
                        #[derive(Serialize)]
                        struct ListPayload<'a> {
                            i: &'a str,
                            #[serde(skip_serializing_if = "Option::is_none")]
                            node: Option<&'a str>,
                            include: Vec<String>,
                            exclude: Vec<String>,
                        }
                        let rpc = crate::RPC.get().unwrap();
                        let acl = aaa::get_acl(rpc, key_id).await?;
                        let mut data = crate::PULL_DATA.get().unwrap().clone();
                        let (allow, deny) = acl.get_items_allow_deny_reading();
                        let payload = ListPayload {
                            i: "#",
                            node: if self.replicate_remote {
                                None
                            } else {
                                Some(".local")
                            },
                            include: allow,
                            exclude: deny,
                        };
                        let items: Vec<FullItemStateAndInfoOwned> = unpack(
                            safe_rpc_call(
                                rpc,
                                "eva.core",
                                "item.list",
                                pack(&payload)?.into(),
                                QoS::Processed,
                                *crate::TIMEOUT.get().unwrap(),
                            )
                            .await?
                            .payload(),
                        )?;
                        data.items
                            .replace(items.into_iter().map(Into::into).collect());
                        Ok(to_value(data)?)
                    }
                } else {
                    Err(Error::access("no API key provided"))
                }
            }
            "action" | "action.toggle" | "run" | "lvar.set" | "lvar.reset" | "lvar.clear"
            | "lvar.toggle" | "lvar.incr" | "lvar.decr" => {
                if let Some(key_id) = key_id {
                    if let Some(p) = params {
                        let params: BTreeMap<String, Value> = BTreeMap::deserialize(p)?;
                        let rpc = crate::RPC.get().unwrap();
                        let acl = aaa::get_acl(rpc, key_id).await?;
                        let i: Value = params
                            .get("i")
                            .ok_or_else(|| Error::invalid_params("oid not provided"))?
                            .clone();
                        if let Value::String(s) = i {
                            acl.require_item_write(&s.parse::<OID>()?)?;
                        } else {
                            return Err(Error::invalid_params("oid is not a string"));
                        }
                        let res = safe_rpc_call(
                            rpc,
                            "eva.core",
                            method,
                            pack(&params)?.into(),
                            QoS::Processed,
                            *crate::TIMEOUT.get().unwrap(),
                        )
                        .await?;
                        let res_p = res.payload();
                        Ok(if res_p.is_empty() {
                            Value::Unit
                        } else {
                            unpack(res_p)?
                        })
                    } else {
                        Err(Error::invalid_params("no params provided"))
                    }
                } else {
                    Err(Error::access("no API key provided"))
                }
            }
            "rpvt" => {
                if let Some(key_id) = key_id {
                    if let Some(p) = params {
                        let params = ParamsIdOwned::deserialize(p)?;
                        let rpc = crate::RPC.get().unwrap();
                        let acl = aaa::get_acl(rpc, key_id).await?;
                        acl.require_rpvt_read(&params.i)?;
                        let mut sp = params.i.splitn(2, '/');
                        let node = sp.next().unwrap();
                        if node == ".local" {
                            let url = sp
                                .next()
                                .ok_or_else(|| Error::invalid_params("url not specified"))?;
                            let client = crate::HTTP_CLIENT.get().unwrap();
                            let res = client.get_response(url).await?;
                            Ok(to_value(res)?)
                        } else {
                            Err(Error::access("pub/sub RPVT request to non-local resource"))
                        }
                    } else {
                        Err(Error::invalid_params("no params provided"))
                    }
                } else {
                    Err(Error::access("no API key provided"))
                }
            }
            "action.result" => {
                #[derive(Serialize, Deserialize)]
                struct ParamsResult {
                    u: Uuid,
                }
                if let Some(key_id) = key_id {
                    if let Some(p) = params {
                        let rpc = crate::RPC.get().unwrap();
                        let acl = aaa::get_acl(rpc, key_id).await?;
                        let p: ParamsResult = ParamsResult::deserialize(p)?;
                        let result: BTreeMap<String, Value> = unpack(
                            safe_rpc_call(
                                crate::RPC.get().unwrap(),
                                "eva.core",
                                "action.result",
                                pack(&p)?.into(),
                                QoS::Processed,
                                *crate::TIMEOUT.get().unwrap(),
                            )
                            .await?
                            .payload(),
                        )?;
                        let oid_c = result
                            .get("oid")
                            .ok_or_else(|| Error::invalid_data("no OID in the result"))?;
                        if let Value::String(ref o) = oid_c {
                            let oid: OID = o.parse()?;
                            acl.require_item_read(&oid)?;
                        } else {
                            return Err(Error::invalid_data("invalid OID in the result"));
                        }
                        Ok(to_value(result)?)
                    } else {
                        Err(Error::invalid_params("no params provided"))
                    }
                } else {
                    Err(Error::access("no API key provided"))
                }
            }
            _ => {
                if let Some(bus_call) = method.strip_prefix("bus::") {
                    if let Some(key_id) = key_id {
                        let rpc = crate::RPC.get().unwrap();
                        let acl = aaa::get_acl(rpc, key_id).await?;
                        acl.require_admin()?;
                        let mut sp = bus_call.splitn(2, "::");
                        let target = sp
                            .next()
                            .ok_or_else(|| Error::invalid_params("bus target not specified"))?;
                        let bus_method = sp
                            .next()
                            .ok_or_else(|| Error::invalid_params("bus method not specified"))?;
                        let payload: busrt::borrow::Cow = if let Some(params) = params {
                            pack(&params)?.into()
                        } else {
                            busrt::empty_payload!()
                        };
                        let res = safe_rpc_call(
                            rpc,
                            target,
                            bus_method,
                            payload,
                            QoS::Processed,
                            *crate::TIMEOUT.get().unwrap(),
                        )
                        .await?;
                        let res_p = res.payload();
                        Ok(if res_p.is_empty() {
                            Value::Unit
                        } else {
                            unpack(res_p)?
                        })
                    } else {
                        Err(Error::access("no API key provided"))
                    }
                } else {
                    Err(Error::not_implemented("Pub/Sub RPC method not implemented"))
                }
            }
        }
    }
    async fn get_key(&self, key_id: &str) -> EResult<String> {
        aaa::get_key(crate::RPC.get().unwrap(), key_id).await
    }
}

async fn discovery_handler(
    rx: &mut async_channel::Receiver<psrpc::tools::Publication>,
) -> EResult<()> {
    while let Ok(message) = rx.recv().await {
        let p: eva_sdk::pubsub::PsNodeStatus = serde_json::from_slice(message.data())?;
        let name = message.subtopic();
        match p.status() {
            eva_sdk::pubsub::NodeStatus::Running => nodes::append_discovered_node(name).await?,
            eva_sdk::pubsub::NodeStatus::Terminating => {
                nodes::mark_node(name, false, None, false, None).await?;
            }
        }
    }
    Ok(())
}

pub async fn ps_bulk_state_handler(rx: psrpc::tools::PublicationReceiver) -> EResult<()> {
    while let Ok(msg) = rx.recv().await {
        ps_process_bulk_state(msg).await.log_ef();
    }
    Ok(())
}

pub async fn ps_state_handler(rx: psrpc::tools::PublicationReceiver) -> EResult<()> {
    while let Ok(msg) = rx.recv().await {
        ps_process_state(msg).await.log_ef();
    }
    Ok(())
}

async fn ps_process_state(msg: psrpc::tools::Publication) -> EResult<()> {
    let state: ReplicationStateEvent = serde_json::from_slice(msg.data())?;
    if &state.node == crate::SYSTEM_NAME.get().unwrap() {
        return Ok(());
    }
    trace!(
        "pub/sub state push from {}, topic {}",
        state.node,
        msg.topic()
    );
    let oid: OID = OID::from_path(msg.subtopic())?;
    crate::RPC
        .get()
        .unwrap()
        .client()
        .lock()
        .await
        .publish(
            &format!("{}{}", REPLICATION_STATE_TOPIC, oid.as_path()),
            pack(&state)?.into(),
            QoS::No,
        )
        .await?;
    Ok(())
}

async fn ps_process_bulk_state(msg: psrpc::tools::Publication) -> EResult<()> {
    if let Some(bulk_topic) = msg.topic().strip_prefix(PS_ITEM_BULK_STATE_TOPIC) {
        let frame = msg.data();
        if frame.len() < 4 {
            return Err(Error::invalid_data("invalid bulk frame len"));
        }
        if frame[0] != 0x00 {
            return Err(Error::invalid_data("invalid bulk frame header"));
        }
        if frame[1] != psrpc::PROTO_VERSION {
            return Err(Error::invalid_data("unsupported bulk proto"));
        }
        let is_secure = if let Some(secure_topics) = crate::BULK_SECURE_TOPICS.get() {
            secure_topics.contains(bulk_topic)
        } else {
            false
        };
        let (encryption, compression) = psrpc::options::parse_flags(frame[2])?;
        if is_secure && encryption == psrpc::options::Encryption::No {
            warn!("unencrypted frame to secure topic {}, ignoring", bulk_topic);
            return Err(Error::access("unencrypted message ignored"));
        }
        let mut sp = frame[5..].splitn(3, |v| *v == 0x00);
        let system_name_buf = sp.next().unwrap();
        let sname = std::str::from_utf8(system_name_buf)?;
        trace!(
            "pub/sub bulk state push from {}, topic {}",
            sname,
            msg.topic()
        );
        if sname == crate::SYSTEM_NAME.get().unwrap() {
            return Ok(());
        }
        let system_name = sname.to_owned();
        let key_id_buf = sp
            .next()
            .ok_or_else(|| Error::invalid_data("bulk frame is missing key id"))?;
        let _payload = sp
            .next()
            .ok_or_else(|| Error::invalid_data("bulk frame is missing payload"))?;
        let rpc = crate::RPC.get().unwrap();
        let (opts, auth) = if encryption == psrpc::options::Encryption::No {
            (
                psrpc::options::Options::new().compression(compression),
                None,
            )
        } else {
            let key_id = std::str::from_utf8(key_id_buf)?;
            (
                aaa::get_enc_opts(rpc, key_id)
                    .await?
                    .compression(compression),
                Some((aaa::get_acl(rpc, key_id).await?, key_id.to_owned())),
            )
        };
        let len = key_id_buf.len() + system_name_buf.len() + 7;
        let p = opts.unpack_payload(msg.message, len).await?;
        let data: Vec<FullItemState> = unpack(&p)?;
        for d in data {
            if let Some((ref acl, ref key_id)) = auth {
                if !acl.check_item_write(&d.oid) {
                    warn!("key {} is not allowed to replicate {}", key_id, d.oid);
                    continue;
                }
            }
            let topic = format!("{}{}", REPLICATION_STATE_TOPIC, d.oid.as_path());
            let rse = d.into_replication_state_event(&system_name);
            crate::RPC
                .get()
                .unwrap()
                .client()
                .lock()
                .await
                .publish(&topic, pack(&rse)?.into(), QoS::No)
                .await?;
        }
    }
    Ok(())
}
