use eva_common::acl::Acl;
use eva_common::events::{
    AAA_ACL_TOPIC, AAA_KEY_TOPIC, AAA_USER_TOPIC, ANY_STATE_TOPIC, LOG_EVENT_TOPIC,
};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::Server;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;

err_logger!();

trait ApiKeyId {
    fn api_key_id(&self) -> Option<&str>;
}

impl ApiKeyId for Acl {
    fn api_key_id(&self) -> Option<&str> {
        if let Some(Value::Map(map)) = self.meta() {
            if let Some(Value::Seq(s)) = map.get(&Value::String("api_key_id".to_owned())) {
                if let Some(Value::String(val)) = s.get(0) {
                    return Some(val);
                }
            }
        }
        None
    }
}

macro_rules! ok {
    () => {{
        #[derive(Serialize)]
        struct OK {
            ok: bool,
        }
        Ok(to_value(OK { ok: true })?)
    }};
}

mod aaa;
mod aci;
mod api;
mod call_parser;
mod convert;
mod db;
mod eapi;
mod handler;
mod lang;
mod serve;
mod tpl;
mod upload;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Default HMI service";

lazy_static! {
    static ref UI_PATH: OnceCell<Option<String>> = <_>::default();
    static ref PVT_PATH: OnceCell<Option<String>> = <_>::default();
    static ref VENDORED_APPS_PATH: OnceCell<String> = <_>::default();
    static ref REG: OnceCell<Registry> = <_>::default();
    static ref MIME_TYPES: OnceCell<HashMap<String, String>> = <_>::default();
    static ref SYSTEM_NAME: OnceCell<String> = <_>::default();
    static ref DEFAULT_HISTORY_DB_SVC: OnceCell<String> = <_>::default();
    static ref I18N: OnceCell<lang::Converter> = <_>::default();
    static ref HTTP_CLIENT: OnceCell<eva_sdk::http::Client> = <_>::default();
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref SVC_ID: OnceCell<String> = <_>::default();
}

static BUF_SIZE: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

static PUBLIC_API_LOG: atomic::AtomicBool = atomic::AtomicBool::new(false);

static DEVELOPMEPT_MODE: atomic::AtomicBool = atomic::AtomicBool::new(false);

#[inline]
fn set_rpc(rpc: Arc<RpcClient>) -> EResult<()> {
    RPC.set(rpc).map_err(|_| Error::core("unable to set RPC"))
}

#[inline]
fn set_timeout(timeout: Duration) -> EResult<()> {
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("unable to set timeout"))
}

#[inline]
fn buf_size() -> usize {
    BUF_SIZE.load(atomic::Ordering::Relaxed)
}

#[inline]
fn public_api_log() -> bool {
    PUBLIC_API_LOG.load(atomic::Ordering::Relaxed)
}

#[inline]
fn development_mode() -> bool {
    DEVELOPMEPT_MODE.load(atomic::Ordering::Relaxed)
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ApiProto {
    Http,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiBind {
    proto: ApiProto,
    listen: SocketAddr,
    real_ip_header: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionConfig {
    timeout: Option<f64>,
    #[serde(default)]
    prolong: bool,
    #[serde(default)]
    stick_ip: bool,
    #[serde(default)]
    allow_list_neighbors: bool,
}

#[inline]
fn default_history_db_svc() -> String {
    "default".to_owned()
}

#[inline]
fn default_buf_size() -> usize {
    16384
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    api: Vec<ApiBind>,
    db: Option<String>,
    #[serde(default)]
    auth_svcs: Vec<String>,
    #[serde(default)]
    session: SessionConfig,
    #[serde(default)]
    keep_api_log: u32,
    #[serde(default)]
    public_api_log: bool,
    mime_types: Option<String>,
    #[serde(default = "default_buf_size")]
    buf_size: usize,
    ui_path: Option<String>,
    pvt_path: Option<String>,
    #[serde(default = "default_history_db_svc")]
    default_history_db_svc: String,
    #[serde(default)]
    development: bool,
    #[serde(default = "eva_common::tools::default_true")]
    vendored_apps: bool,
    #[serde(default)]
    user_data: UserDataConfig,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct UserDataConfig {
    #[serde(default)]
    max_records: u32,
    #[serde(default)]
    max_record_length: u32,
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let timeout = initial.timeout();
    let workers = initial.workers();
    SYSTEM_NAME
        .set(initial.system_name().to_owned())
        .map_err(|_| Error::core("Unable to set SYSTEM_NAME"))?;
    SVC_ID
        .set(initial.id().to_owned())
        .map_err(|_| Error::core("Unable to set SVC_ID"))?;
    HTTP_CLIENT
        .set(eva_sdk::http::Client::new(
            (workers * 100).try_into().map_err(Error::failed)?,
            timeout,
        ))
        .map_err(|_| Error::core("Unable to set HTTP_CLIENT"))?;
    let mut i18n = lang::Converter::default();
    let eva_dir = initial.eva_dir();
    PVT_PATH
        .set(config.pvt_path.map(|p| {
            let pvt_path = eva_common::tools::format_path(eva_dir, Some(&p), None);
            debug!("pvt_path: {}", pvt_path);
            i18n.append_dir(&format!("{pvt_path}/locales"));
            pvt_path
        }))
        .map_err(|_| Error::core("Unable to set PVT_PATH"))?;
    UI_PATH
        .set(config.ui_path.as_ref().map(|p| {
            let ui_path = eva_common::tools::format_path(eva_dir, Some(p), None);
            debug!("ui_path: {}", ui_path);
            i18n.append_dir(&format!("{ui_path}/locales"));
            ui_path
        }))
        .map_err(|_| Error::core("Unable to set UI_PATH"))?;
    if config.vendored_apps {
        let vendored_apps_path =
            eva_common::tools::format_path(eva_dir, Some("vendored-apps"), None);
        VENDORED_APPS_PATH
            .set(vendored_apps_path)
            .map_err(|_| Error::core("Unable to set VENDORED_APPS_PATH"))?;
    }
    db::set_max_user_data_records(config.user_data.max_records);
    db::set_max_user_data_record_len(config.user_data.max_record_length);
    I18N.set(i18n)
        .map_err(|_| Error::core("Unable to set I18N"))?;
    DEFAULT_HISTORY_DB_SVC
        .set(config.default_history_db_svc)
        .map_err(|_| Error::core("Unable to set DEFAULT_DB"))?;
    PUBLIC_API_LOG.store(config.public_api_log, atomic::Ordering::Relaxed);
    BUF_SIZE.store(config.buf_size, atomic::Ordering::Relaxed);
    set_timeout(timeout)?;
    aaa::set_auth_svcs(config.auth_svcs)?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("tpl.reload"));
    info.add_method(ServiceMethod::new("i18n.cache_purge"));
    info.add_method(
        ServiceMethod::new("api_log.get")
            .optional("t_start")
            .optional("t_end")
            .optional("user")
            .optional("acl")
            .optional("method")
            .optional("source")
            .optional("code")
            .optional("success"),
    );
    info.add_method(ServiceMethod::new("session.broadcast.reload"));
    info.add_method(ServiceMethod::new("session.broadcast.restart"));
    info.add_method(ServiceMethod::new("session.list"));
    info.add_method(ServiceMethod::new("session.destroy").required("i"));
    info.add_method(ServiceMethod::new("ws.stats"));
    info.add_method(
        ServiceMethod::new("authenticate")
            .required("key")
            .optional("ip"),
    );
    info.add_method(
        ServiceMethod::new("user_data.get")
            .required("login")
            .required("key"),
    );
    let handlers = eapi::Handlers::new(info);
    let rpc: Arc<RpcClient> = initial.init_rpc(handlers).await?;
    initial.drop_privileges()?;
    aaa::set_session_config(
        config.session.timeout,
        config.session.prolong,
        config.session.stick_ip,
        config.session.allow_list_neighbors,
    );
    let db_path = if let Some(path) = config.db {
        path
    } else {
        let data_path = initial.data_path().ok_or_else(|| {
            Error::failed("unable to get service data path, is the user restricted?")
        })?;
        format!("sqlite://{data_path}/hmi.db")
    };
    db::init(&db_path, workers, timeout).await?;
    aaa::start().await?;
    aci::start(config.keep_api_log).await?;
    set_rpc(rpc.clone())?;
    let client = rpc.client().clone();
    let mut topics = vec![
        format!("{AAA_ACL_TOPIC}#"),
        format!("{AAA_USER_TOPIC}#"),
        format!("{AAA_KEY_TOPIC}#"),
        format!("{ANY_STATE_TOPIC}#"),
    ];
    for t in handler::get_log_topics(0) {
        topics.push(format!("{LOG_EVENT_TOPIC}{}", t));
    }
    client
        .lock()
        .await
        .subscribe_bulk(
            &topics.iter().map(String::as_str).collect::<Vec<&str>>(),
            QoS::No,
        )
        .await?;
    svc_init_logs(&initial, client.clone())?;
    if config.development {
        warn!("development mode started");
        DEVELOPMEPT_MODE.store(true, atomic::Ordering::Relaxed);
    }
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
    MIME_TYPES
        .set(mime_types)
        .map_err(|_| Error::core("Unable to set UI DIR"))?;
    for api in config.api {
        match api.proto {
            ApiProto::Http => {
                let real_ip_header = Arc::new(api.real_ip_header.clone());
                debug!("http api {}", api.listen);
                debug!("real ip header: {:?}", real_ip_header);
                let make_svc = make_service_fn(move |conn: &AddrStream| {
                    let client_addr = conn.remote_addr();
                    let ip_header = real_ip_header.clone();
                    let service = service_fn(move |req| {
                        handler::web(req, client_addr.ip(), ip_header.clone())
                    });
                    async move { Ok::<_, Infallible>(service) }
                });
                tokio::spawn(async move {
                    info!("Starting HTTP API at: {}", api.listen);
                    let server = Server::bind(&api.listen).serve(make_svc);
                    if let Err(e) = server.await {
                        error!("API server exited with the error: {}", e);
                        std::process::exit(1);
                    }
                });
            }
        }
    }
    let registry = initial.init_registry(&rpc);
    REG.set(registry)
        .map_err(|_| Error::core("unable to set registry object"))?;
    svc_start_signal_handlers();
    if let Some(ui_path) = UI_PATH.get().unwrap() {
        tokio::task::spawn_blocking(move || {
            tpl::reload_ui(ui_path).log_ef();
        })
        .await
        .map_err(Error::core)?;
    }
    if let Some(pvt_path) = PVT_PATH.get().unwrap() {
        tokio::task::spawn_blocking(move || {
            tpl::reload_pvt(pvt_path).log_ef();
        })
        .await
        .map_err(Error::core)?;
    }
    svc_mark_ready(&client).await?;
    info!("Default HMI service started ({})", initial.id());
    svc_block(&rpc).await;
    handler::notify_server_event(handler::ServerEvent::Restart);
    svc_mark_terminating(&client).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
