use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Dummy template service";

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
    // init rpc
    let rpc = initial
        .init_rpc(Handlers {
            info: ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION),
        })
        .await?;
    // services are usually started under root and should drop privileges after the bus socket is
    // connected
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    // init logs
    svc_init_logs(&initial, client.clone())?;
    // start sigterm handler
    svc_start_signal_handlers();
    // mark ready and active
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
