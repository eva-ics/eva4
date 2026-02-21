mod allow;

use std::sync::{Arc, OnceLock};

use allow::Allow;
use eva_common::err_logger;
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "MCP Service";

const MCP_HELP_TEXT: &str = include_str!("../res/mcp_help.txt");

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static ALLOW: OnceLock<Allow> = OnceLock::new();

#[derive(Clone)]
struct McpServer {
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, serde::Serialize, JsonSchema)]
#[allow(clippy::struct_field_names)]
struct BusCallRequest {
    bus_target: String,
    bus_method: String,
    #[serde(default)]
    bus_params: Option<String>,
}

impl McpServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl McpServer {
    #[tool(
        name = "help",
        description = "Returns instructions for EVA ICS: official docs (info.bma.ai) and how to use bus_call (check core_svcs.html and service EAPI methods first)."
    )]
    pub async fn help(&self) -> String {
        MCP_HELP_TEXT.to_string()
    }

    #[tool(
        name = "bus_call",
        description = "Call an EVA ICS bus service (RPC). Args: bus_target (e.g. eva.core), bus_method (e.g. test), optional bus_params (JSON string; omit or null for no params). Before calling: check https://info.bma.ai/en/actual/eva4/core_svcs.html if unsure about params; always check the target service/core EAPI methods (e.g. core.html#eapi-methods, svc pages). Official docs preferred; DeepWiki alttch/bma-info is an alternative."
    )]
    pub async fn bus_call(&self, Parameters(req): Parameters<BusCallRequest>) -> String {
        if !ALLOW
            .get()
            .expect("allow set at startup")
            .allows(&req.bus_target, &req.bus_method)
        {
            return format!(
                "{}",
                Error::access(
                    "MCP method restricted (IMPORTANT: ensure you have read help as well and ensure
                    your target/method exist in the docs"
                )
            );
        }
        info!(
            "bus_call target={} method={} params={:?}",
            req.bus_target, req.bus_method, req.bus_params
        );
        let params: busrt::borrow::Cow<'_> = match req
            .bus_params
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        {
            None | Some(serde_json::Value::Null) => busrt::borrow::Cow::Borrowed(&[]),
            Some(v) => match pack(&v) {
                Ok(vec) => busrt::borrow::Cow::Owned(vec),
                Err(e) => {
                    info!("bus_call pack error: {}", e);
                    return format!("pack error: {}", e);
                }
            },
        };
        let event = match eapi_bus::call(&req.bus_target, &req.bus_method, params).await {
            Ok(ev) => ev,
            Err(e) => {
                info!(
                    "bus_call error: target={} method={} err={}",
                    req.bus_target, req.bus_method, e
                );
                return format!("error: {}", e);
            }
        };
        let payload = event.payload();
        if payload.is_empty() {
            info!(
                "bus_call ok: target={} method={} response_len=0",
                req.bus_target, req.bus_method
            );
            return "null".to_string();
        }
        match unpack::<Value>(payload) {
            Ok(v) => {
                let out =
                    serde_json::to_string(&v).unwrap_or_else(|e| format!("serialize error: {}", e));
                info!(
                    "bus_call ok: target={} method={} response_len={}",
                    req.bus_target,
                    req.bus_method,
                    out.len()
                );
                out
            }
            Err(e) => {
                info!(
                    "bus_call unpack error: target={} method={} err={}",
                    req.bus_target, req.bus_method, e
                );
                format!("unpack error: {}", e)
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {}

struct Handlers {
    info: ServiceInfo,
}

#[async_trait]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: String,
    allow: Allow,
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
    let listen_addr = config.listen.as_str();
    ALLOW.set(config.allow).expect("allow set once at startup");
    let ct = CancellationToken::new();
    let http_service: StreamableHttpService<McpServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(McpServer::new()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: Some(std::time::Duration::from_secs(15)),
                sse_retry: Some(std::time::Duration::from_secs(3)),
                cancellation_token: ct.child_token(),
            },
        );
    let router = axum::Router::new().nest_service("/mcp", http_service);
    let Ok(listener) = tokio::net::TcpListener::bind(listen_addr).await else {
        return Err(Error::invalid_data("mcp bind failed"));
    };
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
