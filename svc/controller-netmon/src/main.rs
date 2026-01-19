use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use atomic_timer::AtomicTimer;
use eva_common::err_logger;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;
use surge_ping::Client;
use tokio::sync::Semaphore;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Network monitor service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static PROBES_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        #[allow(clippy::single_match, clippy::match_single_binding)]
        match method {
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_frame(&self, _frame: Frame) {
        svc_need_ready!();
    }
}

fn default_ping_packet_size() -> usize {
    56
}

fn default_timeout() -> f64 {
    1.0
}

async fn ping_async(
    target: IpAddr,
    timeout: Duration,
    packet_size: usize,
    interface: Option<&str>,
) -> EResult<()> {
    let mut config_builder = surge_ping::Config::builder();
    if let Some(interface) = interface {
        config_builder = config_builder.interface(interface);
    }
    if target.is_ipv6() {
        config_builder = config_builder.kind(surge_ping::ICMP::V6);
    }
    let config = config_builder.build();
    let payload = vec![0; packet_size];
    let client = Client::new(&config).unwrap();
    let mut pinger = client.pinger(target, surge_ping::PingIdentifier(111)).await;
    pinger.timeout(timeout);
    pinger
        .ping(surge_ping::PingSequence(0), &payload)
        .await
        .map_err(Error::io)?;
    Ok(())
}

async fn probe_tcp_async(target: &str, port: u16, timeout: Duration) -> EResult<()> {
    let addr = format!("{}:{}", target, port);
    let conn_future = tokio::net::TcpStream::connect(addr);
    let _stream = tokio::time::timeout(timeout, conn_future).await??;
    Ok(())
}

async fn probe_http_async(url: &str, content: Option<&str>, timeout: Duration) -> EResult<()> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(Error::io)?;
    let resp = client.get(url).send().await.map_err(Error::io)?;
    if !resp.status().is_success() {
        return Err(Error::invalid_data("HTTP status not successful"));
    }
    let body = resp.text().await.map_err(Error::io)?;
    let Some(expected_content) = content else {
        return Ok(());
    };

    if body.contains(expected_content) {
        return Ok(());
    }
    Err(Error::invalid_data("HTTP content mismatch"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind", rename_all = "lowercase", content = "options")]
enum Probe {
    Ping {
        target: IpAddr,
        #[serde(default = "default_timeout")]
        timeout: f64,
        #[serde(default = "default_ping_packet_size")]
        packet_size: usize,
        interface: Option<String>,
    },
    Tcp {
        target: String,
        port: u16,
        #[serde(default = "default_timeout")]
        timeout: f64,
    },
    Http {
        url: String,
        content: Option<String>,
        #[serde(default = "default_timeout")]
        timeout: f64,
    },
}

impl Probe {
    async fn execute(&self) -> EResult<()> {
        let _permit = PROBES_SEMAPHORE
            .get()
            .unwrap()
            .acquire()
            .await
            .map_err(Error::io)?;
        match self {
            Probe::Ping {
                target,
                timeout,
                packet_size,
                interface,
            } => {
                ping_async(
                    *target,
                    Duration::from_secs_f64(*timeout),
                    *packet_size,
                    interface.as_deref(),
                )
                .await
            }
            Probe::Tcp {
                target,
                port,
                timeout,
            } => probe_tcp_async(target, *port, Duration::from_secs_f64(*timeout)).await,
            Probe::Http {
                url,
                content,
                timeout,
            } => probe_http_async(url, content.as_deref(), Duration::from_secs_f64(*timeout)).await,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Host {
    oid: OID,
    probe: Probe,
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
    interval: Duration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    max_parallel_probes: Option<usize>,
    #[serde(default)]
    hosts: Vec<Host>,
}

async fn monitor_remote(host: Host) {
    info!("Started monitoring for host {}", host.oid);
    let mut int = tokio::time::interval(host.interval);
    let last_error_msg = AtomicTimer::new(Duration::from_secs(60));
    last_error_msg.expire_now();
    loop {
        int.tick().await;
        if let Err(e) = host.probe.execute().await {
            if last_error_msg.reset_if_expired() {
                error!("Probe for host {} failed: {}", host.oid, e);
            }
            eapi_bus::publish_item_state(&host.oid, -1, Some(&Value::U8(0)))
                .await
                .ok();
        } else {
            last_error_msg.expire_now();
            eapi_bus::publish_item_state(&host.oid, 1, Some(&Value::U8(1)))
                .await
                .ok();
        }
    }
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let handlers = Handlers { info };
    eapi_bus::init(&initial, handlers).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    let max_parallel_probes = config
        .max_parallel_probes
        .unwrap_or_else(|| usize::try_from(initial.workers()).unwrap());
    PROBES_SEMAPHORE
        .set(Semaphore::new(max_parallel_probes))
        .map_err(|_| Error::core("Failed to set probes semaphore"))?;
    for host in config.hosts {
        tokio::spawn(monitor_remote(host));
    }
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
