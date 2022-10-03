use crate::ads::{self, ParseAmsNetId};
use ::ads::AmsAddr;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;

pub struct Handlers {
    info: ServiceInfo,
}

impl Handlers {
    #[inline]
    pub fn new(info: ServiceInfo) -> Self {
        Self { info }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NetId {
    Bytes([u8; 6]),
    Str(String),
}

impl NetId {
    fn into_ams_addr(self, port: u16) -> EResult<AmsAddr> {
        Ok(match self {
            NetId::Bytes(b) => AmsAddr::new(b.into(), port),
            NetId::Str(s) => AmsAddr::new(s.ams_net_id()?.into(), port),
        })
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
            "ping" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                ads::ping(device_addr).await?;
                Ok(None)
            }
            "read" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                    #[serde(alias = "g")]
                    index_group: u32,
                    #[serde(alias = "o")]
                    index_offset: u32,
                    #[serde(alias = "s")]
                    size: usize,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                let result = ads::read(device_addr, p.index_group, p.index_offset, p.size).await?;
                Ok(Some(pack(&result)?))
            }
            "su_read" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                    #[serde(default, alias = "r")]
                    requests: Vec<crate::ads::ReadRequest>,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                let result = ads::read_multi(device_addr, p.requests).await?;
                Ok(Some(pack(&result)?))
            }
            "write" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                    #[serde(alias = "g")]
                    index_group: u32,
                    #[serde(alias = "o")]
                    index_offset: u32,
                    #[serde(alias = "d")]
                    data: Vec<u8>,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                ads::write(device_addr, p.index_group, p.index_offset, p.data).await?;
                Ok(None)
            }
            "su_write" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                    #[serde(default, alias = "r")]
                    requests: Vec<crate::ads::WriteRequest>,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                let result = ads::write_multi(device_addr, p.requests).await?;
                Ok(Some(pack(&result)?))
            }
            "write_read" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                    #[serde(alias = "g")]
                    index_group: u32,
                    #[serde(alias = "o")]
                    index_offset: u32,
                    #[serde(alias = "d")]
                    data: Vec<u8>,
                    #[serde(alias = "s")]
                    size: usize,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                let result =
                    ads::write_read(device_addr, p.index_group, p.index_offset, p.data, p.size)
                        .await?;
                Ok(Some(pack(&result)?))
            }
            "su_write_read" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    #[serde(alias = "n")]
                    net_id: NetId,
                    #[serde(alias = "p")]
                    port: u16,
                    #[serde(default, alias = "r")]
                    requests: Vec<crate::ads::WriteReadRequest>,
                }
                let p: Params = unpack(payload)?;
                let device_addr: AmsAddr = p.net_id.into_ams_addr(p.port)?;
                let result = ads::write_read_multi(device_addr, p.requests).await?;
                Ok(Some(pack(&result)?))
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}
