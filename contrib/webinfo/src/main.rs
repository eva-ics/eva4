use axum::routing::get;
use axum::Router;
use eva_sdk::prelude::*;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

err_logger!();

mod client_info;
mod http_errors;
mod methods;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "RESTful web info";

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();
static HMI_SVC: OnceCell<String> = OnceCell::new();
static REAL_IP_HEADER: OnceCell<String> = OnceCell::new();

static HELP: &str = r#"RESTful web info
All requests must contain a header X-Auth-Key with a valid API key or token

Methods:

/api/test - get node info
/api/item.state/OID - get item/group state (OID as path, e.g. sensor/tests/t1)
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: String,
    hmi_svc: String,
    real_ip_header: Option<String>,
}

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    // Handle RPC call
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        svc_handle_default_rpc(method, &self.info)
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[allow(clippy::unused_async)]
async fn root() -> &'static str {
    HELP
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let addr: SocketAddr = config.listen.parse().map_err(Error::invalid_params)?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let rpc = initial.init_rpc(Handlers { info }).await?;
    if let Some(h) = config.real_ip_header {
        REAL_IP_HEADER
            .set(h)
            .map_err(|_| Error::core("Unable to set REAL_IP_HEADER"))?;
    }
    HMI_SVC
        .set(config.hmi_svc)
        .map_err(|_| Error::core("Unable to set HMI_SVC"))?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("Unable to set RPC"))?;
    TIMEOUT
        .set(initial.timeout())
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    let app = Router::new()
        .route("/", get(root))
        .route("/api/test", get(methods::test::handle))
        .route("/api/item.state/*path", get(methods::state::handle));
    tokio::spawn(async move {
        axum::Server::bind(&addr)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .log_ef();
    });
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
