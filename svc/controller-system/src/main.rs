use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::time::Duration;
use sysinfo::System;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "System service";

const REPLACE_UNSUPPORTED_SYMBOLS: &str = "___";

const DISK_REFRESH: Duration = Duration::from_secs(10);
const CPU_REFRESH: Duration = Duration::from_secs(1);
const MEMORY_REFRESH: Duration = Duration::from_secs(1);
const NETWORK_REFRESH: Duration = Duration::from_secs(1);

mod metric;
mod providers;
mod tools;

use metric::Metric;

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_handle_default_rpc(event.parse_method()?, &self.info)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    report: ReportConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportConfig {
    oid_prefix: String,
    #[serde(default)]
    cpu: providers::cpu::Config,
    #[serde(default)]
    load_avg: providers::load_avg::Config,
    #[serde(default)]
    memory: providers::memory::Config,
    #[serde(default)]
    disks: providers::disks::Config,
    #[serde(default)]
    network: providers::network::Config,
}

impl ReportConfig {
    fn set(self) -> EResult<()> {
        providers::cpu::set_config(self.cpu)?;
        providers::load_avg::set_config(self.load_avg)?;
        providers::memory::set_config(self.memory)?;
        providers::disks::set_config(self.disks)?;
        providers::network::set_config(self.network)?;
        Ok(())
    }
}

async fn report_common() {
    Metric::new0("os", "name")
        .report(System::long_os_version())
        .await;
    Metric::new0("os", "version")
        .report(System::os_version())
        .await;
    Metric::new0("os", "kernel")
        .report(System::kernel_version())
        .await;
    Metric::new0("os", "distribution_id")
        .report(System::distribution_id())
        .await;
    Metric::new0("cpu", "arch").report(System::cpu_arch()).await;
}

async fn spawn_workers() {
    macro_rules! launch_provider_worker {
        ($mod: ident) => {
            tokio::spawn(providers::$mod::report_worker());
        };
    }
    launch_provider_worker!(cpu);
    launch_provider_worker!(load_avg);
    launch_provider_worker!(memory);
    launch_provider_worker!(disks);
    launch_provider_worker!(network);
    report_common().await;
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    metric::set_oid_prefix(
        config
            .report
            .oid_prefix
            .replace("${system_name}", initial.system_name())
            .parse()?,
    )?;
    config.report.set()?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    eapi_bus::init(&initial, Handlers { info }).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    eapi_bus::mark_ready().await?;
    tokio::spawn(async move {
        let _ = eapi_bus::wait_core(true).await;
        spawn_workers().await;
    });
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
