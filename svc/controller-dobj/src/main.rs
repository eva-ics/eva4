use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use eva_common::err_logger;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;
use serde::Serialize;
use tokio::net::UdpSocket;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Data Object Controller";

static VERBOSE: AtomicBool = AtomicBool::new(false);

#[derive(Default, Deserialize, Copy, Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Endianess {
    Big,
    #[default]
    Little,
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        svc_handle_default_rpc(method, &self.info)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    input: Vec<InputMap>,
    #[serde(default)]
    verbose: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputMap {
    bind: String,
    data_object: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    endianess: Option<Endianess>,
    buffer: Option<usize>,
}

#[derive(Serialize)]
struct PushPayload<'a> {
    #[serde(rename = "i")]
    name: &'a str,
    #[serde(rename = "d")]
    data: &'a [u8],
    #[serde(rename = "e", skip_serializing_if = "Option::is_none")]
    endianess: Option<Endianess>,
}

async fn spawn_input_listener(
    bind: &str,
    data_object: String,
    endianess: Option<Endianess>,
    buffer_size: Option<usize>,
) -> EResult<()> {
    info!("data object {} input on {}", data_object, bind);
    let socket = UdpSocket::bind(bind).await?;
    tokio::spawn(async move {
        let mut buf = vec![0; buffer_size.unwrap_or(usize::from(u16::MAX))];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let data = &buf[..len];
                    if VERBOSE.load(Ordering::Relaxed) {
                        info!(
                            "data object {} input {} bytes from {}",
                            data_object, len, addr
                        );
                    }
                    let Ok(payload) = pack(&PushPayload {
                        name: &data_object,
                        data,
                        endianess,
                    }) else {
                        error!("data object {} pack error", data_object);
                        continue;
                    };
                    eapi_bus::call("eva.core", "dobj.push", payload.into())
                        .await
                        .log_ef_with("Unable to call dobj.push");
                }
                Err(e) => {
                    error!("data object {} input error: {}", data_object, e);
                    continue;
                }
            }
        }
    });
    Ok(())
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    VERBOSE.store(config.verbose, Ordering::Relaxed);
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let handlers = Handlers { info };
    eapi_bus::init(&initial, handlers).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    for input in config.input {
        spawn_input_listener(
            &input.bind,
            input.data_object,
            input.endianess,
            input.buffer,
        )
        .await?;
    }
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
