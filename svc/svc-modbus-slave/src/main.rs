use async_trait::async_trait;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::service::{poc, set_poc};
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Modbus slave context service";

const BUS_UNIT: u8 = 1;
const BUS_METHOD: &[u8] = &[0x4D, 0x42];

mod server {
    use async_trait::async_trait;
    use eva_common::{EResult, Error};
    pub mod serial;
    pub mod tcp;
    pub mod udp;
    #[async_trait]
    pub trait Server {
        async fn launch(&mut self) -> EResult<()>;
    }
    pub fn parse_host_port(path: &str) -> EResult<(&str, u16)> {
        let mut sp = path.splitn(2, ':');
        let host = sp.next().unwrap();
        let port = if let Some(v) = sp.next() {
            v.parse()?
        } else {
            502
        };
        Ok((host, port))
    }
    pub async fn handle_frame(
        frame: &mut rmodbus::server::ModbusFrame<'_, Vec<u8>>,
    ) -> EResult<()> {
        frame.parse().map_err(Error::io)?;
        if frame.processing_required {
            if frame.readonly {
                frame
                    .process_read(&*crate::CONTEXT.read().await)
                    .map_err(Error::io)?;
            } else {
                frame
                    .process_write(&mut *crate::CONTEXT.write().await)
                    .map_err(Error::io)?;
            };
        }
        Ok(())
    }
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

lazy_static! {
    pub static ref CONTEXT: RwLock<rmodbus::server::context::ModbusContext> =
        RwLock::new(rmodbus::server::context::ModbusContext::new());
    static ref DATA_FILE: OnceCell<String> = <_>::default();
}

struct Handlers {
    info: ServiceInfo,
}

#[async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        if event.method() == BUS_METHOD {
            let mut req = event.payload().to_vec();
            if req.len() > 256 {
                return Err(Error::io("invalid frame len").into());
            }
            req.resize(256, 0);
            let mut response = Vec::new();
            let mut frame = rmodbus::server::ModbusFrame::new(
                BUS_UNIT,
                req.as_slice().try_into().map_err(Error::io)?,
                rmodbus::ModbusProto::Rtu,
                &mut response,
            );
            server::handle_frame(&mut frame).await?;
            if frame.response_required {
                frame.finalize_response().map_err(Error::io)?;
                Ok(Some(response))
            } else {
                Ok(None)
            }
        } else {
            let method = event.parse_method()?;
            match method {
                "save" => {
                    save_context().await?;
                    Ok(None)
                }
                _ => svc_handle_default_rpc(method, &self.info),
            }
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: Vec<Listener>,
    #[serde(default)]
    persistent: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listener {
    path: String,
    unit: u8,
    protocol: ProtocolKind,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    timeout: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    keep_alive_timeout: Option<Duration>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProtocolKind {
    Tcp,
    Udp,
    Rtu,
    Ascii,
}

async fn load_context_data() -> Result<Option<Vec<u8>>, std::io::Error> {
    if let Some(data_file) = DATA_FILE.get() {
        let mut data = Vec::new();
        let mut f = tokio::fs::OpenOptions::new()
            .read(true)
            .open(data_file)
            .await?;
        f.read_to_end(&mut data).await?;
        Ok(Some(data))
    } else {
        Ok(None)
    }
}

async fn save_context() -> Result<(), std::io::Error> {
    let ctx = CONTEXT.write().await;
    if let Some(data_file) = DATA_FILE.get() {
        let mut data = Vec::new();
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(data_file)
            .await?;
        for i in ctx.iter() {
            data.push(i);
        }
        f.write_all(&data).await?;
        f.sync_all().await?;
        info!("context saved");
    }
    Ok(())
}

async fn save_and_poc() {
    poc();
    save_context().await.unwrap();
    std::process::exit(1);
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let timeout = initial.timeout();
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("save"));
    let rpc = initial.init_rpc(Handlers { info }).await?;
    set_poc(Some(Duration::from_secs(3)));
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    let mut servers: Vec<Box<dyn server::Server + Send + Sync + 'static>> = Vec::new();
    for c in config.listen {
        match c.protocol {
            ProtocolKind::Tcp => {
                let server = server::tcp::TcpServer::create(
                    &c.path,
                    c.unit,
                    c.timeout.unwrap_or(timeout),
                    c.keep_alive_timeout.unwrap_or(timeout),
                )
                .await?;
                servers.push(Box::new(server));
            }
            ProtocolKind::Udp => {
                let server =
                    server::udp::UdpServer::create(&c.path, c.unit, c.timeout.unwrap_or(timeout))
                        .await?;
                servers.push(Box::new(server));
            }
            ProtocolKind::Rtu => {
                let server = server::serial::SerialServer::create(
                    &c.path,
                    c.unit,
                    c.timeout.unwrap_or(timeout),
                    rmodbus::ModbusProto::Rtu,
                )
                .await?;
                servers.push(Box::new(server));
            }
            ProtocolKind::Ascii => {
                let server = server::serial::SerialServer::create(
                    &c.path,
                    c.unit,
                    c.timeout.unwrap_or(timeout),
                    rmodbus::ModbusProto::Ascii,
                )
                .await?;
                servers.push(Box::new(server));
            }
        }
    }
    initial.drop_privileges()?;
    if config.persistent {
        if let Some(data_path) = initial.data_path() {
            let data_file = format!("{}/{}", data_path, "context");
            DATA_FILE
                .set(data_file)
                .map_err(|_| Error::core("Unable to set DATA_FILE"))?;
            match load_context_data().await {
                Ok(Some(data)) => {
                    info!("context loaded");
                    CONTEXT
                        .write()
                        .await
                        .create_writer()
                        .write_bulk(&data)
                        .map_err(Error::io)?;
                }
                Ok(None) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    info!("context not loaded (file not found), empty context created");
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
    }
    {
        let mut ctx = CONTEXT.write().await;
        let mut sp = initial.eva_version().split('.');
        let major: u16 = sp.next().map_or(0, |v| v.parse().unwrap_or(0));
        let minor: u16 = sp.next().map_or(0, |v| v.parse().unwrap_or(0));
        let micro: u16 = sp.next().map_or(0, |v| v.parse().unwrap_or(0));
        ctx.set_inputs_bulk(9000, &[major, minor, micro])
            .map_err(Error::core)?;
        ctx.set_inputs_from_u64(9003, initial.eva_build())
            .map_err(Error::core)?;
    }
    for mut srv in servers {
        tokio::spawn(async move {
            srv.launch().await.log_ef();
            save_and_poc().await;
        });
    }
    svc_start_signal_handlers();
    info!(
        "bus listener {}::\\x{:x}\\x{:x}, unit {}",
        initial.id(),
        BUS_METHOD[0],
        BUS_METHOD[1],
        BUS_UNIT
    );
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    save_context().await?;
    Ok(())
}
