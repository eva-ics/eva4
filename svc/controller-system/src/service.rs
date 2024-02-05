use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;

err_logger!();

const DESCRIPTION: &str = "System service";

const REPLACE_UNSUPPORTED_SYMBOLS: &str = "___";

mod common;
mod metric;
mod providers;
mod tools;

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
    report: common::ReportConfig,
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
    let info = ServiceInfo::new(common::AUTHOR, common::VERSION, DESCRIPTION);
    eapi_bus::init(&initial, Handlers { info }).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    eapi_bus::mark_ready().await?;
    tokio::spawn(async move {
        let _ = eapi_bus::wait_core(true).await;
        common::spawn_workers();
    });
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
