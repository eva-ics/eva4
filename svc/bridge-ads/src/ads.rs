use crate::get_client;
use ::ads::AmsAddr;
use busrt::rpc::Rpc;
use busrt::QoS;
use eva_common::events::{RawStateEventOwned, RAW_STATE_TOPIC};
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_sdk::service::poc;
use log::error;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const ADS_SUM_LIMIT: usize = 500;

const SLEEP_STEP: Duration = Duration::from_millis(1);

pub trait ParseAmsNetId {
    fn ams_net_id(&self) -> EResult<[u8; 6]>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    #[serde(alias = "g")]
    index_group: u32,
    #[serde(alias = "o")]
    index_offset: u32,
    #[serde(alias = "s")]
    size: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteReadRequest {
    #[serde(alias = "g")]
    index_group: u32,
    #[serde(alias = "o")]
    index_offset: u32,
    #[serde(alias = "d")]
    data: Vec<u8>,
    #[serde(alias = "s")]
    size: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRequest {
    #[serde(alias = "g")]
    index_group: u32,
    #[serde(alias = "o")]
    index_offset: u32,
    #[serde(alias = "d")]
    data: Vec<u8>,
}

#[derive(Serialize)]
pub struct SumUpResult {
    #[serde(rename = "c")]
    code: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "d")]
    data: Option<Vec<u8>>,
}

impl SumUpResult {
    #[inline]
    pub fn success(data: Option<&[u8]>) -> Self {
        Self {
            code: 0,
            data: data.map(Vec::from),
        }
    }
    #[inline]
    pub fn error(err: ::ads::Error) -> Self {
        let code = match err {
            ::ads::Error::Io(_, _) => 0xffff_ffff,
            ::ads::Error::Overflow(_) => 0xffff_fffe,
            ::ads::Error::Ads(_, _, code) | ::ads::Error::Reply(_, _, code) => code,
        };
        Self { code, data: None }
    }
}

impl ParseAmsNetId for String {
    fn ams_net_id(&self) -> EResult<[u8; 6]> {
        let chunks: Vec<&str> = self.split('.').into_iter().collect();
        if chunks.len() == 6 || chunks.get(6).map_or(false, |v| v.is_empty()) {
            let mut res = Vec::with_capacity(6);
            for c in chunks.iter().take(6) {
                res.push(
                    c.parse()
                        .map_err(|e| Error::invalid_data(format!("invalid AMSNetId: {}", e)))?,
                );
            }
            res.try_into()
                .map_err(|e| Error::invalid_data(format!("invalid AMSNetId: {:?}", e)))
        } else {
            Err(Error::invalid_data("invalid AMSNetId: too many chunks"))
        }
    }
}

fn ping_sync(addr: AmsAddr) -> EResult<u16> {
    let client = crate::ADS_CLIENT.get().unwrap().lock().unwrap();
    let device = client.device(addr);
    device
        .get_state()
        .map_err(|e| Error::io(format!("device state query error: {}", e)))
        .map(|v| v.0 as u16)
}

pub async fn ping(addr: AmsAddr) -> EResult<u16> {
    tokio::task::spawn_blocking(move || ping_sync(addr)).await?
}

pub async fn ping_worker(
    addr: AmsAddr,
    timeout: Duration,
    ping_ads_state: Option<OID>,
) -> EResult<()> {
    let mut int = tokio::time::interval(timeout / 2);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let ping_oid_topic = ping_ads_state.map(|v| format!("{}{}", RAW_STATE_TOPIC, v.as_path()));
    let rpc = crate::RPC.get().unwrap();
    while !eva_sdk::service::svc_is_terminating() {
        match ping(addr).await {
            Ok(state) => {
                if let Some(ref topic) = ping_oid_topic {
                    let event = RawStateEventOwned::new(1, Value::U16(state));
                    rpc.client()
                        .lock()
                        .await
                        .publish(topic, pack(&event)?.into(), QoS::No)
                        .await?;
                }
            }
            Err(e) => {
                error!("ADS ping error: {}", e);
                if let Some(ref topic) = ping_oid_topic {
                    let event = RawStateEventOwned::new0(-1);
                    rpc.client()
                        .lock()
                        .await
                        .publish(topic, pack(&event)?.into(), QoS::No)
                        .await?;
                }
                poc();
            }
        }
        int.tick().await;
    }
    Ok(())
}

fn read_sync(addr: AmsAddr, group: u32, offset: u32, size: usize) -> EResult<Vec<u8>> {
    let mut result = vec![0; size];
    let client = get_client!();
    let device = client.device(addr);
    let l = device
        .read(group, offset, &mut result)
        .map_err(|e| Error::io(format!("ADS read: {}", e)))?;
    result.truncate(l);
    Ok(result)
}

fn read_multi_sync(addr: AmsAddr, reqs: Vec<ReadRequest>) -> EResult<Vec<SumUpResult>> {
    let mut result: Vec<SumUpResult> = Vec::with_capacity(reqs.len());
    for (i, curr_reqs) in reqs.chunks(ADS_SUM_LIMIT).enumerate() {
        if !curr_reqs.is_empty() {
            if i > 0 {
                std::thread::sleep(SLEEP_STEP);
            }
            let client = get_client!();
            let device = client.device(addr);
            let mut buffers: Vec<Vec<u8>> = Vec::with_capacity(curr_reqs.len());
            for req in curr_reqs {
                buffers.push(vec![0; req.size]);
            }
            let mut buffers_s = buffers.as_mut_slice().chunks_mut(1);
            let mut ads_requests = Vec::with_capacity(curr_reqs.len());
            for req in curr_reqs {
                ads_requests.push(::ads::client::ReadRequest::new(
                    req.index_group,
                    req.index_offset,
                    buffers_s.next().unwrap()[0].as_mut_slice(),
                ));
            }
            device
                .read_multi(&mut ads_requests)
                .map_err(|e| Error::io(format!("read_multi: {}", e)))?;
            for r in ads_requests {
                match r.data() {
                    Ok(v) => result.push(SumUpResult::success(Some(v))),
                    Err(e) => result.push(SumUpResult::error(e)),
                }
            }
        }
    }
    Ok(result)
}

fn write_multi_sync(addr: AmsAddr, reqs: Vec<WriteRequest>) -> EResult<Vec<SumUpResult>> {
    let mut result: Vec<SumUpResult> = Vec::with_capacity(reqs.len());
    for (i, curr_reqs) in reqs.chunks(ADS_SUM_LIMIT).enumerate() {
        if !curr_reqs.is_empty() {
            if i > 0 {
                std::thread::sleep(SLEEP_STEP);
            }
            let client = get_client!();
            let device = client.device(addr);
            let mut ads_requests = Vec::with_capacity(curr_reqs.len());
            for req in curr_reqs {
                ads_requests.push(::ads::client::WriteRequest::new(
                    req.index_group,
                    req.index_offset,
                    &req.data,
                ));
            }
            device
                .write_multi(&mut ads_requests)
                .map_err(|e| Error::io(format!("read_multi: {}", e)))?;
            for r in ads_requests {
                match r.ensure() {
                    Ok(()) => result.push(SumUpResult::success(None)),
                    Err(e) => result.push(SumUpResult::error(e)),
                }
            }
        }
    }
    Ok(result)
}

fn write_read_multi_sync(addr: AmsAddr, reqs: Vec<WriteReadRequest>) -> EResult<Vec<SumUpResult>> {
    let mut result: Vec<SumUpResult> = Vec::with_capacity(reqs.len());
    for (i, curr_reqs) in reqs.chunks(ADS_SUM_LIMIT).enumerate() {
        if !curr_reqs.is_empty() {
            if i > 0 {
                std::thread::sleep(SLEEP_STEP);
            }
            let client = get_client!();
            let device = client.device(addr);
            let mut buffers: Vec<Vec<u8>> = Vec::with_capacity(curr_reqs.len());
            for req in curr_reqs {
                buffers.push(vec![0; req.size]);
            }
            let mut buffers_s = buffers.as_mut_slice().chunks_mut(1);
            let mut ads_requests = Vec::with_capacity(curr_reqs.len());
            for req in curr_reqs {
                ads_requests.push(::ads::client::WriteReadRequest::new(
                    req.index_group,
                    req.index_offset,
                    &req.data,
                    buffers_s.next().unwrap()[0].as_mut_slice(),
                ));
            }
            device
                .write_read_multi(&mut ads_requests)
                .map_err(|e| Error::io(format!("write_read_multi: {}", e)))?;
            for r in ads_requests {
                match r.data() {
                    Ok(v) => result.push(SumUpResult::success(Some(v))),
                    Err(e) => result.push(SumUpResult::error(e)),
                }
            }
        }
    }
    Ok(result)
}

fn write_read_sync(
    addr: AmsAddr,
    group: u32,
    offset: u32,
    data: Vec<u8>,
    size: usize,
) -> EResult<Vec<u8>> {
    let mut result = vec![0; size];
    let client = get_client!();
    let device = client.device(addr);
    let l = device
        .write_read(group, offset, &data, &mut result)
        .map_err(|e| Error::io(format!("ADS write_read: {}", e)))?;
    result.truncate(l);
    Ok(result)
}

fn write_sync(addr: AmsAddr, group: u32, offset: u32, data: Vec<u8>) -> EResult<()> {
    let client = get_client!();
    let device = client.device(addr);
    device
        .write(group, offset, &data)
        .map_err(|e| Error::io(format!("ADS write: {}", e)))?;
    Ok(())
}

pub async fn read(addr: AmsAddr, group: u32, offset: u32, size: usize) -> EResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || read_sync(addr, group, offset, size)).await?
}

pub async fn write(addr: AmsAddr, group: u32, offset: u32, data: Vec<u8>) -> EResult<()> {
    tokio::task::spawn_blocking(move || write_sync(addr, group, offset, data)).await?
}

pub async fn write_read(
    addr: AmsAddr,
    group: u32,
    offset: u32,
    data: Vec<u8>,
    size: usize,
) -> EResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || write_read_sync(addr, group, offset, data, size)).await?
}

pub async fn read_multi(
    addr: AmsAddr,
    read_requests: Vec<ReadRequest>,
) -> EResult<Vec<SumUpResult>> {
    tokio::task::spawn_blocking(move || read_multi_sync(addr, read_requests)).await?
}

pub async fn write_multi(
    addr: AmsAddr,
    write_requests: Vec<WriteRequest>,
) -> EResult<Vec<SumUpResult>> {
    tokio::task::spawn_blocking(move || write_multi_sync(addr, write_requests)).await?
}

pub async fn write_read_multi(
    addr: AmsAddr,
    write_read_requests: Vec<WriteReadRequest>,
) -> EResult<Vec<SumUpResult>> {
    tokio::task::spawn_blocking(move || write_read_multi_sync(addr, write_read_requests)).await?
}
