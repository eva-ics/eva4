use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::time::Duration;

err_logger!();

const DESCRIPTION: &str = "System service";
const SLEEP_STEP_ERR: Duration = Duration::from_secs(1);

const REPLACE_UNSUPPORTED_SYMBOLS: &str = "___";

mod api;
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
    api: Option<api::Config>,
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    metric::svc::set_oid_prefix(
        config
            .report
            .oid_prefix
            .replace("${system_name}", initial.system_name()),
    )?;
    config.report.set()?;
    let info = ServiceInfo::new(common::AUTHOR, common::VERSION, DESCRIPTION);
    eapi_bus::init(&initial, Handlers { info }).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    let timeout = initial.timeout();
    if let Some(api_config) = config.api {
        api::set_oid_prefix(
            api_config
                .client_oid_prefix
                .replace("${system_name}", initial.system_name()),
        )?;
        tokio::spawn(async move {
            loop {
                if let Err(e) = api::launch_server(api_config.clone(), timeout).await {
                    error!("unable to launch API server: {}", e);
                }
                tokio::time::sleep(SLEEP_STEP_ERR).await;
            }
        });
    }
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
