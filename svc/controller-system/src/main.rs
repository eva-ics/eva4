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

mod cpu;
mod disks;
mod memory;
mod metric;
mod network;
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
    report_oid_prefix: String,
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

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    macro_rules! launch_worker {
        ($mod: ident) => {
            tokio::spawn($mod::report_worker());
        };
    }
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    metric::set_oid_prefix(
        config
            .report_oid_prefix
            .replace("${system_name}", initial.system_name())
            .parse()?,
    )?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    eapi_bus::init(&initial, Handlers { info }).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    eapi_bus::mark_ready().await?;
    tokio::spawn(async move {
        let _ = eapi_bus::wait_core(true).await;
        launch_worker!(cpu);
        launch_worker!(memory);
        launch_worker!(disks);
        launch_worker!(network);
        report_common().await;
    });
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
