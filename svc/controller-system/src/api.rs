use crate::common::ClientMetric;
use crate::metric::Metric;
use crate::{HEADER_API_AUTH_KEY, HEADER_API_SYSTEM_NAME, VAR_HOST};
use eva_common::events::RawStateEvent;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::HeaderMap;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::error;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::atomic;
use std::time::Duration;
use tokio::net::TcpListener;

err_logger!();

static HOSTS: OnceCell<BTreeMap<String, String>> = OnceCell::new();
static OID_PREFIX: OnceCell<String> = OnceCell::new();
static PREFIX_CONTAINS_HOST: atomic::AtomicBool = atomic::AtomicBool::new(false);
static REAL_IP_HEADER: OnceCell<String> = OnceCell::new();
static HEADER_TRUSTED_API_SYSTEM: OnceCell<String> = OnceCell::new();

fn default_max_clients() -> usize {
    128
}

pub fn set_oid_prefix(prefix: String) -> EResult<()> {
    format!("{}/id", prefix.replace(VAR_HOST, "localhost")).parse::<OID>()?;
    PREFIX_CONTAINS_HOST.store(prefix.contains(VAR_HOST), atomic::Ordering::Relaxed);
    OID_PREFIX
        .set(prefix)
        .map_err(|_| Error::core("Unable to set OID_PREFIX"))
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub client_oid_prefix: String,
    listen: SocketAddr,
    real_ip_header: Option<String>,
    trusted_system_header: Option<String>,
    #[serde(default = "default_max_clients")]
    max_clients: usize,
    #[serde(default)]
    hosts: Vec<Host>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Host {
    name: String,
    key: String,
}

fn get_real_ip(headers: &HeaderMap, remote_ip: IpAddr) -> EResult<IpAddr> {
    if let Some(real_ip_header) = REAL_IP_HEADER.get() {
        if let Some(val) = headers.get(real_ip_header) {
            return val
                .to_str()
                .map_err(Error::invalid_data)?
                .parse()
                .map_err(Error::invalid_data);
        }
    }
    Ok(remote_ip)
}

fn authorize_request(headers: &HeaderMap) -> EResult<String> {
    if let Some(trusted_api_system) = HEADER_TRUSTED_API_SYSTEM.get() {
        if let Some(val) = headers.get(trusted_api_system) {
            let host = val.to_str().map_err(Error::invalid_data)?;
            if host.contains('=') {
                for part in host.split(',') {
                    let mut parts = part.splitn(2, '=');
                    let key = parts.next().unwrap();
                    if key.trim() != "CN" {
                        continue;
                    }
                    let value = parts
                        .next()
                        .ok_or_else(|| Error::invalid_data("missing common name value"))?;
                    return Ok(value.trim().to_owned());
                }
                return Err(Error::access("no trusted system common name"));
            }
            return Ok(host.trim().to_owned());
        }
    }
    let Some(host_header) = headers.get(HEADER_API_SYSTEM_NAME) else {
        return Err(Error::access(format!("{} not set", HEADER_API_SYSTEM_NAME)));
    };
    let Some(key) = headers.get(HEADER_API_AUTH_KEY) else {
        return Err(Error::access(format!("{} not set", HEADER_API_AUTH_KEY)));
    };
    let host = host_header.to_str().map_err(Error::invalid_data)?;
    if HOSTS.get().unwrap().get(host).map_or(false, |v| v == key) {
        Ok(host.to_owned())
    } else {
        Err(Error::access("invalid credentials"))
    }
}

async fn handle_request(request: Request<hyper::body::Incoming>, host: &str) -> EResult<()> {
    let body = request.collect().await.map_err(Error::failed)?.to_bytes();
    let client_metrics: Vec<ClientMetric> = serde_json::from_slice(&body)?;
    for client_metric in client_metrics {
        let metric = Metric::new_for_host(
            OID_PREFIX.get().unwrap(),
            host,
            &client_metric.i,
            PREFIX_CONTAINS_HOST.load(atomic::Ordering::Relaxed),
        );
        let ev = if let ValueOptionOwned::Value(ref v) = client_metric.value {
            RawStateEvent::new(client_metric.status, v)
        } else {
            RawStateEvent::new0(client_metric.status)
        };
        metric
            .send_bus_event(ev)
            .await
            .log_ef_with("unable to send metric event");
    }
    Ok(())
}

async fn serve(
    request: Request<hyper::body::Incoming>,
    remote_ip: IpAddr,
) -> Result<Response<Full<Bytes>>, hyper::http::Error> {
    macro_rules! respond {
        ($code: expr) => {
            Response::builder()
                .status($code)
                .body(Full::new(Bytes::new()))
        };
    }
    if request.uri() == "/report" {
        if request.method() == Method::POST {
            let Ok(ip) = get_real_ip(request.headers(), remote_ip)
                .log_err_with("unable to get API client real IP")
            else {
                return respond!(StatusCode::BAD_REQUEST);
            };
            match authorize_request(request.headers()) {
                Ok(host) => {
                    if let Err(e) = handle_request(request, &host).await {
                        error!("client {} API report error: {}", ip, e);
                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Full::new(Bytes::from(e.to_string())))
                    } else {
                        respond!(StatusCode::NO_CONTENT)
                    }
                }
                Err(e) => {
                    error!("client {} API access denied: {}", ip, e);
                    Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .body(Full::new(Bytes::from(e.to_string())))
                }
            }
        } else {
            respond!(StatusCode::METHOD_NOT_ALLOWED)
        }
    } else {
        respond!(StatusCode::NOT_FOUND)
    }
}

pub async fn launch_server(config: Config, timeout: Duration) -> EResult<()> {
    if HOSTS.get().is_none() {
        // first time initialization
        let hosts: BTreeMap<String, String> =
            config.hosts.into_iter().map(|v| (v.name, v.key)).collect();
        HOSTS
            .set(hosts)
            .map_err(|_| Error::core("Unable to set HOSTS"))?;
        if let Some(real_ip_header) = config.real_ip_header {
            REAL_IP_HEADER
                .set(real_ip_header)
                .map_err(|_| Error::core("Unable to set REAL_IP_HEADER"))?;
        }
        if let Some(trusted_api_system_name) = config.trusted_system_header {
            HEADER_TRUSTED_API_SYSTEM
                .set(trusted_api_system_name)
                .map_err(|_| Error::core("Unable to set HEADER_TRUSTED_API_SYSTEM_NAME"))?;
        }
    }
    let listener = TcpListener::bind(config.listen).await?;
    let client_pool =
        tokio_task_pool::Pool::bounded(config.max_clients).with_spawn_timeout(timeout);
    info!("API server listening at: {}", config.listen);
    loop {
        let Ok((stream, client)) = listener
            .accept()
            .await
            .log_err_with("unable to accept API connection")
        else {
            continue;
        };
        let io = TokioIo::new(stream);
        client_pool
            .spawn(async move {
                http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(|req: Request<hyper::body::Incoming>| async move {
                            serve(req, client.ip()).await
                        }),
                    )
                    .await
                    .log_ef_with("Error serving API connection");
            })
            .await
            .log_ef_with("unable to spawn API client task");
    }
}
