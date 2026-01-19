use crate::common::{LParams, ParamsRun, TrapData, safe_run_macro};
use crate::{Config, MAX_TRAP_SIZE, Proto};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use snmp_parser::ObjectSyntax;
use std::collections::HashMap;
use tokio::net::UdpSocket;

err_logger!();

#[allow(clippy::too_many_lines)]
pub async fn launch_handler(socket: &UdpSocket, config: &Config, rpc: &RpcClient) -> EResult<()> {
    loop {
        #[allow(clippy::large_stack_arrays)]
        let mut buf = [0; MAX_TRAP_SIZE];
        let (_len, addr) = socket.recv_from(&mut buf).await?;
        let ip = addr.ip();
        if let Some(ref ha) = config.hosts_allow {
            let mut can_process = false;
            for h in ha {
                if h.contains(ip) {
                    can_process = true;
                }
            }
            if !can_process {
                warn!("{} is not in hosts_allow, ignoring SNMP trap", ip);
                continue;
            }
        }
        let res_parse = match config.protocol {
            Proto::SnmpV1 => snmp_parser::parse_snmp_v1(&buf),
            Proto::SnmpV2c => snmp_parser::parse_snmp_v2c(&buf),
            Proto::EvaV1 => continue,
        };
        match res_parse {
            Ok(trap) => {
                let msg = trap.1;
                if config.verbose {
                    info!("SNMP trap addr: {}, community: {}", ip, msg.community);
                }
                if let Some(ref comm) = config.communities
                    && !comm.contains(&msg.community)
                {
                    warn!(
                        "community {} is not in communities list, ignoring SNMP trap from {}",
                        msg.community, ip
                    );
                }
                let mut vars = HashMap::new();
                for var in msg.vars_iter() {
                    macro_rules! add {
                        ($v: expr) => {{
                            let k = if config.mibs.enabled {
                                unsafe {
                                    snmptools::get_name(&var.oid)
                                        .log_err()
                                        .unwrap_or_else(|_| var.oid.to_id_string())
                                }
                            } else {
                                var.oid.to_id_string()
                            };
                            vars.insert(k, $v);
                        }};
                    }
                    match var.val {
                        ObjectSyntax::Number(ref bo) => {
                            if let Ok(n) = bo.as_u64() {
                                add!(Value::U64(n));
                            } else if let Ok(n) = bo.as_i64() {
                                add!(Value::I64(n));
                            } else if let Ok(n) = bo.as_bool() {
                                add!(Value::Bool(n));
                            } else if let Ok(s) = bo.as_str() {
                                add!(Value::String(s.to_owned()));
                            }
                        }
                        ObjectSyntax::Counter32(n)
                        | ObjectSyntax::Gauge32(n)
                        | ObjectSyntax::UInteger32(n) => {
                            add!(Value::U32(n));
                        }
                        ObjectSyntax::Counter64(n) => {
                            add!(Value::U64(n));
                        }
                        ObjectSyntax::String(v) => {
                            if let Ok(s) = std::str::from_utf8(v) {
                                add!(Value::String(s.to_owned()));
                            }
                        }
                        _ => {}
                    }
                }
                if !vars.is_empty() {
                    if config.verbose {
                        for (k, v) in &vars {
                            info!("{}={:?}", k, v);
                        }
                    }
                    let data = TrapData {
                        trap_source: ip.to_string(),
                        trap_community: msg.community,
                        trap_vars: vars,
                    };
                    if let Some(ref process) = config.process {
                        let timeout = *crate::TIMEOUT.get().unwrap();
                        let params = ParamsRun {
                            i: process,
                            params: LParams { kwargs: data },
                            wait: timeout,
                        };
                        if let Err(e) = safe_run_macro(rpc, params, timeout).await {
                            error!("process lmacro {} error: {}", process, e);
                        }
                    } else {
                        warn!("trap process lmacro is not set");
                    }
                }
            }
            Err(e) => {
                error!("invalid SNMP {:?} trap from {}: {}", config.protocol, ip, e);
            }
        }
    }
}
