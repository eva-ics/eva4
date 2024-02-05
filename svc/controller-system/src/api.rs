use crate::metric::Metric;
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
use std::time::Duration;
use tokio::net::TcpListener;

err_logger!();

static HOSTS: OnceCell<BTreeMap<String, String>> = OnceCell::new();
static OID_PREFIX: OnceCell<String> = OnceCell::new();
static REAL_IP_HEADER: OnceCell<String> = OnceCell::new();

fn default_max_clients() -> usize {
    128
}

pub fn set_oid_prefix(prefix: String) -> EResult<()> {
    format!("{}/id", prefix).parse::<OID>()?;
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

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ClientMetric {
    i: String,
    #[serde(alias = "s")]
    status: ItemStatus,
    #[serde(default, alias = "v")]
    value: ValueOptionOwned,
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
    let Some(host_header) = headers.get("x-system-name") else {
        return Err(Error::access("X-System-Name not set"));
    };
    let Some(key) = headers.get("x-auth-key") else {
        return Err(Error::access("X-Auth-Key not set"));
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
        let metric = Metric::new_for_host(OID_PREFIX.get().unwrap(), host, &client_metric.i);
        let ev = if let ValueOptionOwned::Value(ref v) = client_metric.value {
            RawStateEvent::new(client_metric.status, v)
        } else {
            RawStateEvent::new0(client_metric.status)
        };
        metric
            .send_event(ev)
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
                    return Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .body(Full::new(Bytes::from(e.to_string())));
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
