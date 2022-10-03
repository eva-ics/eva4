use crate::{aaa, Config, Proto, MAX_TRAP_SIZE};
use eva_common::actions::Params as ActionParams;
use eva_common::actions::UnitParams as UnitActionParams;
use eva_common::events::RAW_STATE_TOPIC;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use psrpc::pubsub::Message;
use serde::Serialize;
use tokio::net::UdpSocket;

struct Frame {
    data: Vec<u8>,
}

impl Message for Frame {
    fn topic(&self) -> &str {
        ""
    }
    fn data(&self) -> &[u8] {
        &self.data
    }
}

err_logger!();

#[derive(Serialize)]
struct Update {
    #[serde(skip_serializing)]
    oid: OID,
    status: ItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
}

#[derive(Serialize)]
struct UnitAction {
    #[serde(rename = "i")]
    oid: OID,
    params: ActionParams,
}

#[derive(Serialize)]
struct UnitActionToggle {
    #[serde(rename = "i")]
    oid: OID,
}

enum Trap {
    Update(Update),
    UnitAction(UnitAction),
    UnitActionToggle(UnitActionToggle),
}

#[allow(clippy::too_many_lines)]
async fn parse_native_trap(
    frame: Frame,
    rpc: &RpcClient,
    require_auth: bool,
) -> EResult<Vec<Trap>> {
    let (data, acl) = if frame.data[0] == 0x00 {
        if frame.data.len() < 4 {
            return Err(Error::invalid_data("invalid frame"));
        }
        if frame.data[1] != psrpc::PROTO_VERSION {
            return Err(Error::invalid_data("unsupported proto"));
        }
        let (encryption, compression) = psrpc::options::parse_flags(frame.data[2])?;
        if encryption == psrpc::options::Encryption::No {
            return Err(Error::access("encryption is required"));
        }
        let mut sp = frame.data[5..].splitn(3, |v| *v == 0x00);
        let sender_buf = sp.next().unwrap();
        let _sender_name = std::str::from_utf8(sender_buf)?;
        let key_id_buf = sp
            .next()
            .ok_or_else(|| Error::invalid_data("frame is missing key id"))?;
        let _payload = sp
            .next()
            .ok_or_else(|| Error::invalid_data("frame is missing payload"))?;
        let (opts, acl) = if encryption == psrpc::options::Encryption::No {
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
                Some(aaa::get_acl(rpc, key_id).await?),
            )
        };
        let len = key_id_buf.len() + sender_buf.len() + 7;
        let p = opts.unpack_payload(Box::new(frame), len).await?;
        (p, acl)
    } else if require_auth {
        return Err(Error::access("encryption is required"));
    } else {
        (frame.data, None)
    };
    let mut result = Vec::new();
    for line in data.split(|v| *v == b'\n') {
        let line_str = std::str::from_utf8(line)?;
        if !line_str.is_empty() {
            let mut sp = line_str.split(' ');
            let command = sp.next().unwrap().to_lowercase();
            match command.as_str() {
                "u" | "update" => {
                    let oid_str = sp
                        .next()
                        .ok_or_else(|| Error::invalid_data("trap OID missing"))?;
                    let oid: OID = oid_str.parse()?;
                    let status_str = sp
                        .next()
                        .ok_or_else(|| Error::invalid_data("update trap status missing"))?;
                    if let Some(ref acl) = acl {
                        acl.require_item_write(&oid)?;
                    }
                    let status: ItemStatus = status_str.parse()?;
                    let value: Option<Value> = sp.next().map(|v| v.parse().unwrap());
                    result.push(Trap::Update(Update { oid, status, value }));
                }
                "a" | "action" => {
                    let oid_str = sp
                        .next()
                        .ok_or_else(|| Error::invalid_data("trap OID missing"))?;
                    let oid: OID = oid_str.parse()?;
                    let status_str = sp
                        .next()
                        .ok_or_else(|| Error::invalid_data("update trap status missing"))?;
                    if let Some(ref acl) = acl {
                        acl.require_item_write(&oid)?;
                    }
                    match status_str {
                        "t" | "toggle" => {
                            result.push(Trap::UnitActionToggle(UnitActionToggle { oid }));
                        }
                        _ => {
                            let status: ItemStatus = status_str.parse()?;
                            let value: Option<Value> = sp.next().map(|v| v.parse().unwrap());
                            result.push(Trap::UnitAction(UnitAction {
                                oid,
                                params: ActionParams::Unit(UnitActionParams {
                                    status,
                                    value: value.into(),
                                }),
                            }));
                        }
                    }
                }
                v => {
                    warn!("unsupported trap command: {}", v);
                }
            }
        }
    }
    Ok(result)
}

async fn process_update(data: Update, rpc: &RpcClient) -> EResult<()> {
    let topic = format!("{}{}", RAW_STATE_TOPIC, data.oid.as_path());
    rpc.client()
        .lock()
        .await
        .publish(&topic, pack(&data)?.into(), QoS::No)
        .await?;
    Ok(())
}

async fn process_unit_action(data: UnitAction, rpc: &RpcClient) -> EResult<()> {
    rpc.call0("eva.core", "action", pack(&data)?.into(), QoS::No)
        .await?;
    Ok(())
}

async fn process_unit_action_toggle(data: UnitActionToggle, rpc: &RpcClient) -> EResult<()> {
    rpc.call0("eva.core", "action.toggle", pack(&data)?.into(), QoS::No)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn launch_handler(socket: &UdpSocket, config: &Config, rpc: &RpcClient) -> EResult<()> {
    loop {
        let mut buf = vec![0; MAX_TRAP_SIZE];
        let (len, addr) = socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        let ip = addr.ip();
        if let Some(ref ha) = config.hosts_allow {
            let mut can_process = false;
            for h in ha {
                if h.contains(ip) {
                    can_process = true;
                }
            }
            if !can_process {
                warn!("{} is not in hosts_allow, ignoring native trap", ip);
                continue;
            }
        }
        let res_parse = match config.protocol {
            Proto::SnmpV1 | Proto::SnmpV2c => continue,
            Proto::EvaV1 => parse_native_trap(Frame { data: buf }, rpc, config.require_auth).await,
        };
        match res_parse {
            Ok(trap_batch) => {
                if config.verbose {
                    info!("native trap addr: {}", ip);
                    for trap in trap_batch {
                        match trap {
                            Trap::Update(data) => {
                                if config.verbose {
                                    info!("update {}", data.oid);
                                }
                                process_update(data, rpc).await.log_ef();
                            }
                            Trap::UnitAction(data) => {
                                info!("action {}", data.oid);
                                process_unit_action(data, rpc).await.log_ef();
                            }
                            Trap::UnitActionToggle(data) => {
                                info!("action.toggle {}", data.oid);
                                process_unit_action_toggle(data, rpc).await.log_ef();
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!(
                    "invalid native {:?} trap from {}: {}",
                    config.protocol, ip, e
                );
            }
        }
    }
}
