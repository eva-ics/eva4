use eva_common::hyper_response;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::Server;
use hyper::{http, Body, Method, Request, Response, StatusCode};
use hyper_static::serve::static_file;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic;
use std::sync::Arc;

lazy_static! {
    static ref MIME_TYPES: OnceCell<HashMap<String, String>> = <_>::default();
    static ref MIRROR_PATH: OnceCell<PathBuf> = <_>::default();
}

static BUF_SIZE: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Mirror service";

const ERR_INVALID_IP: &str = "Invalid IP address";

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

#[inline]
fn default_buf_size() -> usize {
    65_536
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: SocketAddr,
    real_ip_header: Option<String>,
    mime_types: Option<String>,
    //gzip_types: Option<Vec<String>>,
    mirror_path: String,
    #[serde(default = "default_buf_size")]
    buf_size: usize,
}

async fn handler(
    req: Request<Body>,
    ip: IpAddr,
    real_ip_header: Arc<Option<String>>,
) -> Result<Response<Body>, http::Error> {
    macro_rules! unwrap_or_invalid_ip {
        ($result: expr) => {
            match $result {
                Ok(v) => v,
                Err(e) => {
                    let message = format!("{}: {}", ERR_INVALID_IP, e);
                    error!("{}", message);
                    return hyper_response!(StatusCode::BAD_REQUEST, message);
                }
            }
        };
    }
    let (parts, _body) = req.into_parts();
    let ip_addr = if let Some(ref ip_header) = *real_ip_header {
        if let Some(s) = parts.headers.get(ip_header) {
            unwrap_or_invalid_ip!(unwrap_or_invalid_ip!(s.to_str()).parse::<IpAddr>())
        } else {
            ip
        }
    } else {
        ip
    };
    let uri = parts.uri.path();
    if parts.method == Method::GET {
        if let Some(file_path) = uri.strip_prefix('/') {
            let mut path = MIRROR_PATH.get().unwrap().clone();
            path.push(file_path);
            if path.is_dir() {
                path.push("index.html");
            }
            let mime_type = if let Some(ext) = path.extension().and_then(std::ffi::OsStr::to_str) {
                MIME_TYPES.get().unwrap().get(ext)
            } else {
                None
            };
            //return match serve::file(
            //&path,
            //mime_type.map(String::as_str),
            //&parts.headers,
            //BUF_SIZE.load(atomic::Ordering::SeqCst),
            //)
            //.await
            //{
            //Ok(v) => v,
            //Err(e) => e.into(),
            //};
            return match static_file(
                &path,
                mime_type.map(String::as_str),
                &parts.headers,
                BUF_SIZE.load(atomic::Ordering::SeqCst),
            )
            .await
            {
                Ok(v) => {
                    debug!(
                        r#"{} "GET {}" {}"#,
                        ip_addr,
                        uri,
                        v.as_ref().map_or(0, |res| res.status().as_u16())
                    );
                    v
                }
                Err(e) => {
                    let resp: Result<Response<Body>, http::Error> = e.into();
                    warn!(
                        r#"{} "GET {}" {}"#,
                        ip_addr,
                        uri,
                        resp.as_ref().map_or(0, |res| res.status().as_u16())
                    );
                    resp
                }
            };
        }
    }
    warn!("[{}] 405 {}", ip, parts.method);
    hyper_response!(StatusCode::METHOD_NOT_ALLOWED)
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let eva_dir = initial.eva_dir();
    let rpc = initial
        .init_rpc(Handlers {
            info: ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION),
        })
        .await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    let mime_types = if let Some(mime_types_path) = config.mime_types {
        let types = tokio::fs::read_to_string(eva_common::tools::format_path(
            eva_dir,
            Some(&mime_types_path),
            None,
        ))
        .await
        .map_err(|e| {
            error!("unable to read {}", mime_types_path);
            Into::<Error>::into(e)
        })?;
        serde_yaml::from_str(&types).map_err(|e| {
            error!("unable to parse {}", mime_types_path);
            Error::invalid_data(e)
        })?
    } else {
        HashMap::new()
    };
    BUF_SIZE.store(config.buf_size, atomic::Ordering::SeqCst);
    MIME_TYPES
        .set(mime_types)
        .map_err(|_| Error::core("Unable to set UI DIR"))?;
    //if let Some(gzip_types) = config.gzip_types {
    //hyper_static::serve::set_gzip_mime_types(
    //gzip_types
    //.iter()
    //.map(String::as_str)
    //.collect::<Vec<&str>>()
    //.as_slice(),
    //);
    //}
    MIRROR_PATH
        .set({
            let mirror_path =
                eva_common::tools::format_path(eva_dir, Some(&config.mirror_path), None);
            debug!("mirror path: {}", mirror_path);
            Path::new(&mirror_path).to_owned()
        })
        .map_err(|_| Error::core("Unable to set PVT_PATH"))?;
    let real_ip_header = Arc::new(config.real_ip_header.clone());
    debug!("http api {}", config.listen);
    debug!("real ip header: {:?}", real_ip_header);
    let make_svc = make_service_fn(move |conn: &AddrStream| {
        let client_addr = conn.remote_addr();
        let ip_header = real_ip_header.clone();
        let service = service_fn(move |req| handler(req, client_addr.ip(), ip_header.clone()));
        async move { Ok::<_, Infallible>(service) }
    });
    tokio::spawn(async move {
        info!("Starting mirror server at: {}", config.listen);
        let server = Server::bind(&config.listen).serve(make_svc);
        if let Err(e) = server.await {
            error!("API server exited with the error: {}", e);
            std::process::exit(1);
        }
    });
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
