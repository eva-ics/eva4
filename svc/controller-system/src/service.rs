use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_system_common::{
    common::{self, spawn_workers, ReportConfig},
    metric, HEADER_API_AUTH_KEY, HEADER_API_SYSTEM_NAME, VAR_HOST, VAR_SYSTEM_NAME,
};
use serde::Deserialize;
use std::time::Duration;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "System service";
const SLEEP_STEP_ERR: Duration = Duration::from_secs(1);

mod api;

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
            .replace(VAR_SYSTEM_NAME, initial.system_name()),
    )?;
    config.report.set()?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    eapi_bus::init(&initial, Handlers { info }).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    let timeout = initial.timeout();
    if let Some(api_config) = config.api {
        api::set_oid_prefix(
            api_config
                .client_oid_prefix
                .replace(VAR_SYSTEM_NAME, initial.system_name()),
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
        spawn_workers();
    });
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
