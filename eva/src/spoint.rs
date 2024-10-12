use crate::core::{get_hostname, Core};
use crate::eapi;
use crate::{EResult, Error};
use busrt::rpc::{self, Rpc, RpcClient};
use busrt::QoS;
use eva_common::common_payloads::ParamsId;
use eva_common::err_logger;
use eva_common::payload::{pack, unpack};
use eva_common::registry;
use eva_common::services::RealtimeConfig;
use log::{info, trace, LevelFilter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

pub const SPOINT_WORKERS: u32 = 2;

pub const SPOINT_LOAD_CLIENT_PFX: &str = "eva.spload.";
pub const SPOINT_CLIENT_PFX: &str = "eva.spoint.";

err_logger!();

fn default_buf_ttl() -> Duration {
    busrt::DEFAULT_BUF_TTL
}

fn default_buf_size() -> usize {
    busrt::DEFAULT_BUF_SIZE
}

fn default_queue_size() -> usize {
    busrt::DEFAULT_QUEUE_SIZE
}

#[derive(Deserialize)]
struct BusConfig {
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration_us",
        default = "default_buf_ttl"
    )]
    buf_ttl: Duration,
    #[serde(default = "default_buf_size")]
    buf_size: usize,
    #[serde(default = "default_queue_size")]
    queue_size: usize,
}

fn default_timeout() -> Duration {
    eva_common::DEFAULT_TIMEOUT
}

#[derive(Deserialize)]
struct CoreTimeouts {
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration",
        default = "default_timeout"
    )]
    timeout: Duration,
    #[serde(
        deserialize_with = "eva_common::tools::de_float_as_duration",
        default = "default_timeout"
    )]
    suicide_timeout: Duration,
}

#[derive(Deserialize)]
struct LogConfigs {
    level: String,
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    dir_eva: &str,
    system_name: Option<&str>,
    pid_file: Option<&str>,
    connection_path: &str,
    realtime: RealtimeConfig,
) -> EResult<()> {
    let system_name = if let Some(name) = system_name {
        name.to_owned()
    } else {
        get_hostname()?
    };
    let (bus_config_data, core_timeouts, level_filter) = {
        let mut max_filter = LevelFilter::Error;
        let load_name = format!("{}{}", SPOINT_LOAD_CLIENT_PFX, system_name);
        let client =
            busrt::ipc::Client::connect(&busrt::ipc::Config::new(connection_path, &load_name))
                .await
                .map_err(|_| Error::not_ready("bus not ready"))?;
        let rpc = RpcClient::new0(client);
        let bus_config_data =
            BusConfig::deserialize(registry::key_get(registry::R_CONFIG, "bus", &rpc).await?)?;
        let core_timeouts =
            CoreTimeouts::deserialize(registry::key_get(registry::R_CONFIG, "core", &rpc).await?)?;
        let log_configs: Vec<LogConfigs> =
            Vec::deserialize(registry::key_get(registry::R_CONFIG, "logs", &rpc).await?)?;
        for l in log_configs {
            let lf = LevelFilter::from_str(&l.level).map_err(Error::failed)?;
            if lf > max_filter {
                max_filter = lf;
            };
        }
        (bus_config_data, core_timeouts, max_filter)
    };
    let name = format!("{}{}", SPOINT_CLIENT_PFX, system_name);
    let bus_config = busrt::ipc::Config::new(connection_path, &name)
        .buf_size(bus_config_data.buf_size)
        .buf_ttl(bus_config_data.buf_ttl)
        .queue_size(bus_config_data.queue_size)
        .timeout(core_timeouts.timeout);
    let core_client = busrt::ipc::Client::connect(&bus_config).await?;
    let core = Arc::new(Core::new_spoint(
        dir_eva,
        &system_name,
        pid_file,
        core_timeouts.suicide_timeout,
        core_timeouts.timeout,
    ));
    let bus_api = eapi::BusApi::new(core.clone(), 0);
    let core_sec_client = core_client.register_secondary().await?;
    let core_rpc = RpcClient::create(core_client, bus_api, rpc::Options::new().blocking_frames());
    eva_common::logger::init_bus(
        core_rpc.client(),
        bus_config_data.queue_size,
        level_filter,
        false,
    )?;
    info!("starting EVA ICS node {}", core.system_name());
    info!("mode: secondary point");
    info!("dir: {}", dir_eva);
    let rpc = Arc::new(RpcClient::new0(core_sec_client));
    core.set_rpc(rpc.clone())?;
    core.set_components()?;
    trace!("writing the pid file");
    core.write_pid_file().await?;
    trace!("registering the signal handler");
    core.register_signals();
    let launcher_name = format!("{}{}", crate::launcher::LAUNCHER_CLIENT_PFX, system_name);
    let launcher_bus_config = busrt::ipc::Config::new(connection_path, &launcher_name)
        .buf_size(bus_config_data.buf_size)
        .buf_ttl(bus_config_data.buf_ttl)
        .queue_size(bus_config_data.queue_size)
        .timeout(core_timeouts.timeout);
    let launcher_client = busrt::ipc::Client::connect(&launcher_bus_config).await?;
    let launcher_client_secondary = launcher_client.register_secondary().await?;
    trace!("initializing the launcher");
    crate::launcher::init(
        launcher_client,
        launcher_client_secondary,
        bus_config_data.queue_size,
        realtime,
    )
    .await?;
    core.mark_loaded().await;
    let core_c = core.clone();
    let rpc_c = rpc.clone();
    tokio::spawn(async move {
        while core_rpc.is_connected() && rpc_c.is_connected() {
            tokio::time::sleep(eva_common::SLEEP_STEP).await;
        }
        let _r = core_c.set_reload_flag().await;
        core_c.shutdown();
    });
    rpc.call(
        "eva.core",
        "svc.start_by_launcher",
        pack(&ParamsId { i: &launcher_name })?.into(),
        QoS::Processed,
    )
    .await?;
    core.block(false).await;
    rpc.call0(
        &launcher_name,
        "shutdown",
        busrt::empty_payload!(),
        QoS::Processed,
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
struct BrokerClientInfo {
    clients: Vec<Info>,
}

#[derive(Deserialize, Serialize, bmart::tools::Sorting)]
#[sorting(id = "name")]
pub struct Info {
    name: String,
    port: Option<String>,
    source: Option<String>,
    build: Option<u64>,
    version: Option<String>,
}

#[derive(Deserialize)]
struct SpointCoreInfo {
    build: u64,
    version: String,
}

type CollectMap = Arc<Mutex<Option<HashMap<String, Info>>>>;

async fn collect_spoint_info(
    rpc: &RpcClient,
    name: &str,
    map: CollectMap,
    timeout: Duration,
) -> EResult<()> {
    let res = tokio::time::timeout(
        timeout,
        rpc.call(name, "test", busrt::empty_payload!(), QoS::Processed),
    )
    .await??;
    let data: SpointCoreInfo = unpack(res.payload())?;
    if let Some(entry) = map.lock().unwrap().as_mut().unwrap().get_mut(name) {
        entry.build = Some(data.build);
        entry.version = Some(data.version);
    }
    Ok(())
}

/// # Panics
///
/// should not panic
pub async fn list_remote(rpc: Arc<RpcClient>, timeout: Duration) -> EResult<Vec<Info>> {
    let spoints: CollectMap = Arc::new(Mutex::new(Some(
        unpack::<BrokerClientInfo>(
            rpc.call(
                ".broker",
                "client.list",
                busrt::empty_payload!(),
                QoS::Processed,
            )
            .await?
            .payload(),
        )?
        .clients
        .into_iter()
        .filter(|v| v.name.starts_with(SPOINT_CLIENT_PFX))
        .map(|v| (v.name.clone(), v))
        .collect(),
    )));
    let spoint_names: Vec<String> = spoints
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut futs = Vec::new();
    for name in spoint_names {
        let map = spoints.clone();
        let rpc_c = rpc.clone();
        let fut = tokio::spawn(async move {
            collect_spoint_info(&rpc_c, &name, map, timeout)
                .await
                .log_ef();
        });
        futs.push(fut);
    }
    for fut in futs {
        fut.await.log_ef();
    }
    let sp = spoints.lock().unwrap().take().unwrap();
    let mut data: Vec<Info> = sp.into_values().collect();
    data.sort();
    Ok(data)
}
