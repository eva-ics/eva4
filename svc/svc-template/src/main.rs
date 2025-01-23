use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Dummy template service";

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
    async fn handle_frame(&self, _frame: Frame) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

#[svc_main]
// the proc attribute renames the function to eva_service_main and generates the following main
// instead:
//
// fn main() -> eva_common::EResult<()> { svc_launch(eva_service_main) }
async fn main(mut initial: Initial) -> EResult<()> {
    // get config from initial and deserialize it
    let _config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    // init EAPI bus
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    eapi_bus::init(&initial, Handlers { info }).await?;
    // services are usually started under root and should drop privileges after the bus socket is
    // connected
    initial.drop_privileges()?;
    // init logs
    eapi_bus::init_logs(&initial)?;
    // start sigterm handler
    svc_start_signal_handlers();
    // mark ready and active
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
