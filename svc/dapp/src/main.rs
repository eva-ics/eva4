use eva_common::prelude::*;
use eva_sdk::prelude::*;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::path::{Path, PathBuf};

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Docker App Runner";

static PATH: OnceCell<PathBuf> = OnceCell::new();

mod dc;

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[inline]
fn default_compose_version() -> u16 {
    2
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    path: String,
    #[serde(default = "default_compose_version")]
    compose_version: u16,
}

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        let payload = event.payload();
        #[allow(clippy::match_single_binding)]
        match method {
            "app.get_config" => {
                if payload.is_empty() {
                    let mut path = PATH.get().unwrap().clone();
                    path.push("docker-compose.yml");
                    let s = tokio::fs::read_to_string(path).await?;
                    let config: Value = serde_yaml::from_str(&s).map_err(Error::invalid_data)?;
                    Ok(Some(pack(&config)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let compose_version = config.compose_version.try_into()?;
    let path = if config.path.starts_with('/') {
        config.path
    } else {
        let eva_dir = Path::new(initial.eva_dir());
        let mut d_app_dir = PathBuf::from(&eva_dir);
        d_app_dir.push("runtime");
        d_app_dir.push("dapp");
        d_app_dir.push(config.path);
        d_app_dir.to_string_lossy().to_string()
    };
    PATH.set(Path::new(&path).to_owned())
        .map_err(|_| Error::core("unable to set PATH"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("app.get_config"));
    let rpc = initial.init_rpc(Handlers { info }).await?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    info!("starting Docker compose with app in {}", path);
    svc_start_signal_handlers();
    let mut app = dc::App::new(&path, compose_version, initial.timeout());
    let (tx_out, tx_err) = app.start().await?;
    let client_c = client.clone();
    svc_mark_ready(&client).await?;
    tokio::spawn(async move {
        while let Ok(s) = tx_out.recv().await {
            info!("APP: {}", s);
        }
        if !svc_is_terminating() {
            error!("app terminated");
            let _ = svc_mark_terminating(&client_c).await;
        }
    });
    tokio::spawn(async move {
        while let Ok(s) = tx_err.recv().await {
            info!("APP: {}", s);
        }
    });
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    app.stop().await?;
    Ok(())
}
