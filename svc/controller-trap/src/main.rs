use eva_common::events::{AAA_ACL_TOPIC, AAA_KEY_TOPIC};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use ipnetwork::IpNetwork;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::net::UdpSocket;

err_logger!();

mod aaa;
mod common;
mod native;
mod snmp;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "SNMP/UDP trap controller";

const MAX_TRAP_SIZE: usize = 65000;

static TIMEOUT: OnceLock<Duration> = OnceLock::new();
static KEY_SVC: OnceLock<String> = OnceLock::new();

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_handle_default_rpc(event.parse_method()?, &self.info)
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
        svc_need_ready!();
        if frame.kind() == busrt::FrameKind::Publish
            && let Some(topic) = frame.topic()
        {
            if let Some(key_id) = topic.strip_prefix(AAA_KEY_TOPIC) {
                aaa::KEYS.lock().unwrap().remove(key_id);
                aaa::ENC_OPTS.lock().unwrap().remove(key_id);
            } else if let Some(acl_id) = topic.strip_prefix(AAA_ACL_TOPIC) {
                aaa::ACLS
                    .lock()
                    .unwrap()
                    .retain(|_, v| !v.contains_acl(acl_id));
            }
        }
    }
}

#[derive(Deserialize, Copy, Clone, Debug, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
enum Proto {
    SnmpV1,
    #[default]
    SnmpV2c,
    EvaV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    listen: String,
    #[serde(default)]
    protocol: Proto,
    communities: Option<HashSet<String>>,
    hosts_allow: Option<HashSet<IpNetwork>>,
    process: Option<OID>,
    key_svc: Option<String>,
    #[serde(default)]
    require_auth: bool,
    #[serde(default)]
    verbose: bool,
    #[serde(default)]
    mibs: MibsConfig,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MibsConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    files: HashSet<String>,
    #[serde(default)]
    dirs: HashSet<String>,
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("unable to set TIMEOUT"))?;
    let rpc = initial
        .init_rpc(Handlers {
            info: ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION),
        })
        .await?;
    let socket = UdpSocket::bind(&config.listen).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    if config.protocol == Proto::EvaV1 {
        if let Some(ref key_svc) = config.key_svc {
            KEY_SVC
                .set(key_svc.clone())
                .map_err(|_| Error::core("unable to set KEY_SVC"))?;
        }
        client
            .lock()
            .await
            .subscribe_bulk(&[&format!("{AAA_KEY_TOPIC}#")], QoS::No)
            .await?;
    }
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    if config.mibs.enabled {
        match config.protocol {
            Proto::SnmpV1 | Proto::SnmpV2c => unsafe {
                snmptools::init(
                    &snmptools::Config::new()
                        .mibs(
                            config
                                .mibs
                                .files
                                .iter()
                                .map(String::as_str)
                                .collect::<Vec<&str>>()
                                .as_slice(),
                        )
                        .mib_dirs(
                            config
                                .mibs
                                .dirs
                                .iter()
                                .map(String::as_str)
                                .collect::<Vec<&str>>()
                                .as_slice(),
                        ),
                )
                .map_err(Error::failed)?;
            },
            Proto::EvaV1 => {}
        }
    }
    let rpc_c = rpc.clone();
    #[allow(clippy::large_futures)]
    tokio::spawn(async move {
        loop {
            match config.protocol {
                Proto::SnmpV1 | Proto::SnmpV2c => {
                    snmp::launch_handler(&socket, &config, &rpc_c)
                        .await
                        .log_ef();
                }
                Proto::EvaV1 => {
                    native::launch_handler(&socket, &config, &rpc_c)
                        .await
                        .log_ef();
                }
            }
        }
    });
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
