use eva_common::acl::OIDMaskList;
use eva_common::events::{
    FullItemStateAndInfoOwned, LocalStateEvent, RemoteStateEvent, AAA_KEY_TOPIC, LOCAL_STATE_TOPIC,
    REMOTE_STATE_TOPIC,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::FullItemState;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

use crate::{aaa, ReplicationData};
use crate::{get_mtu, pubsub_publish};

err_logger!();

pub struct Handlers {
    tx: async_channel::Sender<FullItemState>,
    info: ServiceInfo,
    replicate_remote: bool,
}

impl Handlers {
    pub fn new(
        tx: async_channel::Sender<FullItemState>,
        info: ServiceInfo,
        replicate_remote: bool,
    ) -> Self {
        Self {
            tx,
            info,
            replicate_remote,
        }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        svc_handle_default_rpc(method, &self.info)
    }
    async fn handle_frame(&self, frame: Frame) {
        svc_need_ready!();
        if frame.kind() == busrt::FrameKind::Publish {
            if let Some(topic) = frame.topic() {
                if let Some(o) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                    process_local_state(topic, o, frame.payload(), &self.tx)
                        .await
                        .log_ef();
                } else if let Some(o) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
                    if self.replicate_remote {
                        process_remote_state(topic, o, frame.payload(), &self.tx)
                            .await
                            .log_ef();
                    }
                } else if let Some(key_id) = topic.strip_prefix(AAA_KEY_TOPIC) {
                    aaa::KEYS.lock().unwrap().remove(key_id);
                    aaa::ENC_OPTS.lock().unwrap().remove(key_id);
                }
            }
        }
    }
}

async fn send_frame(
    mut data: Vec<u8>,
    count: usize,
    cfg: &crate::BulkSendConfig,
    opts: &psrpc::options::Options,
) -> EResult<()> {
    let len = data.len();
    if len < 6 || count == 0 {
        return Ok(());
    }
    data[1..5].copy_from_slice(&(u32::try_from(count)?).to_be_bytes());
    let mut frame = vec![0x00, psrpc::PROTO_VERSION, opts.flags(), 0x00, 0x00];
    frame.extend(crate::SYSTEM_NAME.get().unwrap().as_bytes());
    frame.push(0x00);
    if let Some(ref key_id) = cfg.encryption_key {
        frame.extend(key_id.as_bytes().to_vec());
    }
    frame.push(0x00);
    frame.extend(opts.pack_payload(data).await?);
    pubsub_publish(crate::BULK_STATE_TOPIC.get().unwrap(), &frame)
        .await
        .map_err(Error::io)?;
    Ok(())
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn estimate_compressed_size(data: &[u8]) -> usize {
    (f64::from(entropy::shannon_entropy(data)) * data.len() as f64 / 8.0 * 0.8).ceil() as usize
}

async fn notify(data: &[ReplicationData]) -> EResult<()> {
    let mtu = get_mtu();
    let mut packet = Vec::with_capacity(mtu);
    let mut count = 0;
    packet.extend([0xdd, 0x00, 0x00, 0x00, 0x00]);
    let max_size = mtu - 5;
    let cfg = crate::BULK_SEND_CONFIG.get().unwrap();
    let mut opts = if let Some(ref key_id) = cfg.encryption_key {
        aaa::get_enc_opts(crate::RPC.get().unwrap(), key_id).await?
    } else {
        psrpc::options::Options::new()
    };
    if cfg.compress {
        opts = opts.compression(psrpc::options::Compression::Bzip2);
    }
    for d in data {
        let packed_data = pack(d)?;
        let pos = packet.len();
        packet.extend(&packed_data);
        count += 1;
        let est_size = if cfg.compress {
            estimate_compressed_size(&packet)
        } else {
            packet.len()
        };
        if est_size > max_size {
            packet.truncate(pos);
            send_frame(
                std::mem::replace(&mut packet, Vec::with_capacity(mtu)),
                count - 1,
                cfg,
                &opts,
            )
            .await?;
            packet.clear();
            packet.extend([0xdd, 0x00, 0x00, 0x00, 0x00]);
            packet.extend(packed_data);
            count = 1;
        }
    }
    send_frame(packet, count, cfg, &opts).await?;
    Ok(())
}

pub async fn sender(rx: async_channel::Receiver<FullItemState>, buf_ttl: Option<Duration>) {
    if let Some(bttl) = buf_ttl {
        let mut buf_interval = tokio::time::interval(bttl);
        let mut data_buf: Vec<ReplicationData> = Vec::new();
        loop {
            tokio::select! {
                f = rx.recv() => {
                    if let Ok(state) = f {
                        data_buf.push(state.into());
                    } else {
                        break;
                    }
                }

                _ = buf_interval.tick() => {
                    if !data_buf.is_empty() {
                        notify(&data_buf).await.log_ef();
                        data_buf.clear();
                    }
                }
            }
        }
    } else {
        while let Ok(state) = rx.recv().await {
            notify(&[state.into()]).await.log_ef();
        }
    }
}

async fn process_local_state(
    topic: &str,
    path: &str,
    payload: &[u8],
    tx: &async_channel::Sender<FullItemState>,
) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<LocalStateEvent>(payload) {
            Ok(v) => {
                tx.send(FullItemState::from_local_state_event(v, oid))
                    .await
                    .map_err(Error::core)?;
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

async fn process_remote_state(
    topic: &str,
    path: &str,
    payload: &[u8],
    tx: &async_channel::Sender<FullItemState>,
) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<RemoteStateEvent>(payload) {
            Ok(v) => {
                tx.send(FullItemState::from_remote_state_event(v, oid))
                    .await
                    .map_err(Error::core)?;
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

pub async fn submit_periodic(
    interval: Duration,
    oids: &OIDMaskList,
    oids_exclude: &OIDMaskList,
    replicate_remote: bool,
) -> EResult<()> {
    #[derive(Serialize)]
    struct Request<'a> {
        i: &'a OIDMaskList,
        exclude: &'a OIDMaskList,
        #[serde(skip_serializing_if = "Option::is_none")]
        node: Option<&'a str>,
    }
    let payload = busrt::borrow::Cow::Referenced(Arc::new(pack(&Request {
        i: oids,
        exclude: oids_exclude,
        node: if replicate_remote {
            None
        } else {
            Some(".local")
        },
    })?));
    let mut int = tokio::time::interval(interval);
    loop {
        int.tick().await;
        let res: Vec<FullItemStateAndInfoOwned> = unpack(
            safe_rpc_call(
                crate::RPC.get().unwrap(),
                "eva.core",
                "item.list",
                payload.clone(),
                QoS::Processed,
                *crate::TIMEOUT.get().unwrap(),
            )
            .await?
            .payload(),
        )?;
        notify(
            &res.into_iter()
                .map(|v| ReplicationData::Inventory(v.into()))
                .collect::<Vec<ReplicationData>>(),
        )
        .await?;
    }
}
