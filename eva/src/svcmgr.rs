use crate::eapi::EAPI_VERSION;
use crate::{EResult, Error};
use crate::{BUILD, VERSION};
use busrt::rpc::{Rpc, RpcClient};
use busrt::QoS;
use eva_common::err_logger;
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use eva_common::registry;
use eva_common::services::{BusConfig, CoreInfo, Initial, Timeout};
use eva_common::tools::{format_path, get_eva_dir};
use log::{debug, error, info, trace, warn};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use yedb::Database;

err_logger!();

pub const DEFAULT_LAUNCHER: &str = "eva.launcher.main";

#[inline]
fn default_launcher() -> Arc<String> {
    Arc::new(DEFAULT_LAUNCHER.to_owned())
}

#[derive(Default)]
pub struct Manager {
    services: Mutex<HashMap<String, Params>>,
    rpc: OnceCell<Arc<RpcClient>>,
    core_active: OnceCell<Arc<atomic::AtomicBool>>,
}

#[derive(Serialize, Deserialize)]
pub struct PayloadStartStop {
    pub id: String,
    pub initial: Initial,
}

async fn start_stop_service(
    launcher: &str,
    method: &str,
    id: &str,
    initial: Initial,
    timeout: Duration,
    rpc: &RpcClient,
) -> EResult<()> {
    let payload = PayloadStartStop {
        id: id.to_owned(),
        initial,
    };
    tokio::time::timeout(
        // give it 1 sec more to let the bus call pass
        timeout + Duration::from_secs(1),
        rpc.call(launcher, method, pack(&payload)?.into(), QoS::Processed),
    )
    .await??;
    Ok(())
}

async fn get_service_status(
    name: &str,
    launcher: &str,
    rpc: &RpcClient,
    timeout: Duration,
) -> EResult<ServiceStatus> {
    #[derive(Serialize)]
    struct ParamsGet<'a> {
        i: &'a str,
    }
    let result = tokio::time::timeout(
        timeout,
        rpc.call(
            launcher,
            "get",
            pack(&ParamsGet { i: name })?.into(),
            QoS::Processed,
        ),
    )
    .await??;
    let status: ServiceStatus = unpack(result.payload())?;
    Ok(status)
}

async fn collect_service_info(
    launcher: &str,
    rpc: &RpcClient,
    info: Arc<Mutex<HashMap<String, ServiceStatus>>>,
    timeout: Duration,
) -> EResult<()> {
    let result = tokio::time::timeout(
        timeout,
        rpc.call(launcher, "list", busrt::empty_payload!(), QoS::Processed),
    )
    .await??;
    let data: HashMap<String, ServiceStatus> = unpack(result.payload())?;
    let mut info = info.lock().unwrap();
    for (n, v) in data {
        info.insert(n, v);
    }
    Ok(())
}

impl Manager {
    #[inline]
    pub fn set_rpc(&self, rpc: Arc<RpcClient>) -> EResult<()> {
        self.rpc
            .set(rpc)
            .map_err(|_| Error::core("unable to set RPC"))
    }
    #[inline]
    pub fn set_core_active_beacon(
        &self,
        core_active_beacon: Arc<atomic::AtomicBool>,
    ) -> EResult<()> {
        self.core_active
            .set(core_active_beacon)
            .map_err(|_| Error::core("unable to set core active beacon"))
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub fn load(&self, db: &mut Database) -> EResult<()> {
        info!("loading services");
        let s_key = registry::format_top_key(registry::R_SERVICE);
        let s_offs = s_key.len() + 1;
        let mut services = self.services.lock().unwrap();
        for (n, v) in db.key_get_recursive(&s_key)? {
            let id = &n[s_offs..];
            debug!("loading service {}", id);
            services.insert(id.to_owned(), serde_json::from_value(v)?);
        }
        Ok(())
    }
    #[inline]
    fn core_active(&self) -> bool {
        self.core_active
            .get()
            .unwrap()
            .load(atomic::Ordering::SeqCst)
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn get_service_init(
        &self,
        id: &str,
        system_name: &str,
        default_timeout: Duration,
        allow_any: bool,
    ) -> EResult<(Initial, Arc<String>)> {
        self.services.lock().unwrap().get(id).map_or_else(
            || Err(Error::not_found(format!("no such service: {}", id))),
            |c| {
                if c.enabled || allow_any {
                    Ok((
                        c.to_initial(id, system_name, default_timeout, self.core_active()),
                        c.launcher.clone(),
                    ))
                } else {
                    Err(Error::failed("the service is disabled"))
                }
            },
        )
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn get_service_launcher(&self, id: &str) -> EResult<Arc<String>> {
        self.services.lock().unwrap().get(id).map_or_else(
            || Err(Error::not_found(format!("no such service: {}", id))),
            |c| Ok(c.launcher.clone()),
        )
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn is_service_enabled(&self, id: &str) -> EResult<bool> {
        self.services.lock().unwrap().get(id).map_or_else(
            || Err(Error::not_found(format!("no such service: {}", id))),
            |c| Ok(c.enabled),
        )
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn get_service_params(&self, id: &str) -> EResult<Params> {
        self.services.lock().unwrap().get(id).map_or_else(
            || Err(Error::not_found(format!("no such service: {}", id))),
            |c| Ok(c.clone()),
        )
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned or RPC is not set
    pub async fn is_service_online(&self, id: &str, timeout: Duration) -> EResult<bool> {
        let rpc = self.rpc.get().unwrap();
        let launcher = self.get_service_launcher(id)?;
        if let Ok(status) = get_service_status(id, &launcher, rpc, timeout).await {
            Ok(status.status == Status::Online)
        } else {
            Ok(false)
        }
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned or RPC is not set
    pub async fn get_service_info(&self, id: &str, timeout: Duration) -> EResult<Info> {
        let rpc = self.rpc.get().unwrap();
        let launcher = self.get_service_launcher(id)?;
        let enabled = self.is_service_enabled(id)?;
        let status = get_service_status(id, &launcher, rpc, timeout).await?;
        Ok(Info::from_params(id, Some(&status), launcher, enabled))
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned or RPC is not set
    pub async fn list_services(&self, timeout: Duration, log_not_reg: bool) -> Vec<Info> {
        let rpc = self.rpc.get().unwrap();
        let svc_info: Arc<Mutex<HashMap<String, ServiceStatus>>> = <_>::default();
        let mut futs = Vec::new();
        #[allow(clippy::single_element_loop)]
        let launchers = self
            .services
            .lock()
            .unwrap()
            .values()
            .map(|c| c.launcher.clone())
            .collect::<Vec<Arc<String>>>();
        for launcher in launchers {
            let rpc = rpc.clone();
            let svc_info = svc_info.clone();
            trace!("collecting service info from {}", launcher);
            let f = tokio::spawn(async move {
                collect_service_info(&launcher, &rpc, svc_info, timeout)
                    .await
                    .map_err(|e| {
                        if log_not_reg || e.kind() != ErrorKind::BusClientNotRegistered {
                            error!("unable to collect info from {}: {}", launcher, e);
                        }
                    })
            });
            futs.push(f);
        }
        for f in futs {
            f.await.log_ef();
        }
        let svc_info = svc_info.lock().unwrap();
        let mut result: Vec<Info> = self
            .services
            .lock()
            .unwrap()
            .iter()
            .map(|(n, c)| Info::from_params(n, svc_info.get(n), c.launcher.clone(), c.enabled))
            .collect();
        result.sort();
        result
    }
    pub async fn start(&self, system_name: &str, default_timeout: Duration, log_not_reg: bool) {
        self.start_stop("start", system_name, default_timeout, None, log_not_reg)
            .await;
    }
    pub async fn stop(&self, system_name: &str, default_timeout: Duration, log_not_reg: bool) {
        self.start_stop("stop", system_name, default_timeout, None, log_not_reg)
            .await;
    }
    async fn start_stop(
        &self,
        method: &str,
        system_name: &str,
        default_timeout: Duration,
        launcher: Option<&str>,
        log_not_reg: bool,
    ) {
        let mut futs = Vec::new();
        let mut srv = HashMap::new();
        let rpc = self.rpc.get().unwrap();
        for (id, config) in self
            .services
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, c)| c.enabled && launcher.map_or(true, |l| l == c.launcher.as_str()))
        {
            srv.insert(
                id.clone(),
                (
                    config.to_initial(id, system_name, default_timeout, self.core_active()),
                    config.launcher.clone(),
                ),
            );
        }
        for (id, (init, launcher)) in srv {
            let rpc = rpc.clone();
            let method = method.to_owned();
            let fut = tokio::spawn(async move {
                if let Err(e) =
                    start_stop_service(&launcher, &method, &id, init, default_timeout, &rpc).await
                {
                    if log_not_reg || e.kind() != ErrorKind::BusClientNotRegistered {
                        error!("unable to {} {} with {}: {}", method, id, launcher, e);
                    }
                }
            });
            futs.push(fut);
        }
        for f in futs {
            let _r = f.await;
        }
    }
    pub async fn start_by_launcher(
        &self,
        launcher: &str,
        system_name: &str,
        default_timeout: Duration,
    ) {
        self.start_stop("start", system_name, default_timeout, Some(launcher), true)
            .await;
    }
    pub async fn stop_by_launcher(
        &self,
        launcher: &str,
        system_name: &str,
        default_timeout: Duration,
    ) {
        self.start_stop("stop", system_name, default_timeout, Some(launcher), true)
            .await;
    }
    /// # Panics
    ///
    /// Will panic if the local RPC is not set
    #[inline]
    pub async fn restart_service(
        &self,
        id: &str,
        system_name: &str,
        default_timeout: Duration,
    ) -> EResult<()> {
        trace!("restarting service {}", id);
        let rpc = self.rpc.get().unwrap();
        let (init, launcher) = self.get_service_init(id, system_name, default_timeout, false)?;
        start_stop_service(&launcher, "start", id, init, default_timeout, rpc)
            .await
            .map_err(|e| Error::failed(format!("unable to restart {}: {}", id, e)))
            .log_err()
    }
    /// # Panics
    ///
    /// Will panic if the local RPC is not set
    #[inline]
    pub async fn stop_service(
        &self,
        id: &str,
        system_name: &str,
        default_timeout: Duration,
    ) -> EResult<()> {
        trace!("stopping service {}", id);
        let rpc = self.rpc.get().unwrap();
        let (init, launcher) = self.get_service_init(id, system_name, default_timeout, true)?;
        start_stop_service(&launcher, "stop", id, init, default_timeout, rpc)
            .await
            .map_err(|e| Error::failed(format!("unable to stop {}: {}", id, e)))
            .log_err()
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub async fn deploy_service(
        &self,
        id: &str,
        params: Params,
        system_name: &str,
        default_timeout: Duration,
    ) -> EResult<()> {
        info!("deploying service {}", id);
        match self.stop_service(id, system_name, default_timeout).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::ResourceNotFound => {}
            Err(e) => error!("unable top stop service {}: {}", id, e),
        }
        registry::key_set(registry::R_SERVICE, id, &params, self.rpc.get().unwrap()).await?;
        let enabled = params.enabled;
        self.services.lock().unwrap().insert(id.to_owned(), params);
        if enabled {
            self.restart_service(id, system_name, default_timeout)
                .await?;
        }
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the local RPC is not set
    pub async fn purge_service(
        &self,
        id: &str,
        system_name: &str,
        default_timeout: Duration,
    ) -> EResult<()> {
        warn!("purging service {}", id);
        let rpc = self.rpc.get().unwrap();
        match self.get_service_init(id, system_name, default_timeout, true) {
            Ok((initial, launcher)) => {
                start_stop_service(
                    &launcher,
                    "stop_and_purge",
                    id,
                    initial,
                    default_timeout,
                    rpc,
                )
                .await?;
                self.services.lock().unwrap().remove(id);
            }
            Err(e) if e.kind() == ErrorKind::ResourceNotFound => {}
            Err(e) => return Err(e),
        }
        registry::key_delete(registry::R_SERVICE, id, rpc).await?;
        registry::key_delete_recursive(registry::R_SERVICE_DATA, id, rpc).await?;
        registry::key_delete_recursive(registry::R_CACHE, id, rpc).await?;
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub async fn undeploy_service(
        &self,
        id: &str,
        system_name: &str,
        default_timeout: Duration,
    ) -> EResult<()> {
        let (init, launcher) = self.get_service_init(id, system_name, default_timeout, true)?;
        info!("undeploying service {}", id);
        let rpc = self.rpc.get().unwrap();
        start_stop_service(&launcher, "stop", id, init, default_timeout, rpc).await?;
        registry::key_delete(registry::R_SERVICE, id, rpc).await?;
        self.services.lock().unwrap().remove(id);
        Ok(())
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub async fn wait_all_started(&self, timeout: Duration) -> EResult<()> {
        trace!("waiting until all services are started");
        let mut max_timeout = None;
        for service in self.services.lock().unwrap().values() {
            if service.enabled {
                let startup_timeout = service.startup_timeout().unwrap_or(timeout);
                if let Some(max) = max_timeout {
                    if startup_timeout > max {
                        max_timeout.replace(startup_timeout);
                    }
                } else {
                    max_timeout = Some(startup_timeout);
                }
            }
        }
        if let Some(max_t) = max_timeout {
            let wait_until = Instant::now() + max_t;
            while Instant::now() < wait_until {
                tokio::time::sleep(eva_common::SLEEP_STEP).await;
                let states: Vec<Info> = self.list_services(timeout, false).await;
                let mut online: HashSet<String> = HashSet::new();
                for s in states {
                    if s.status == Status::Online {
                        online.insert(s.id);
                    }
                }
                let mut all_started = true;
                for (s, v) in self.services.lock().unwrap().iter() {
                    if v.enabled && !online.contains(s) {
                        all_started = false;
                        break;
                    }
                }
                if all_started {
                    trace!("all services are online");
                    return Ok(());
                }
            }
            Err(Error::failed("some services failed to start"))
        } else {
            Ok(())
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ServiceStatus {
    pub pid: Option<u32>,
    pub status: Status,
}

#[inline]
fn default_workers() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    #[serde(default = "eva_common::tools::default_true")]
    enabled: bool,
    command: String,
    #[serde(default)]
    prepare_command: Option<String>,
    #[serde(default)]
    timeout: Timeout,
    bus: BusConfig,
    #[serde(default)]
    config: Option<Value>,
    #[serde(default = "default_workers")]
    workers: u32,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    react_to_fail: bool,
    #[serde(default = "default_launcher")]
    launcher: Arc<String>,
}

impl Params {
    fn to_initial(
        &self,
        id: &str,
        system_name: &str,
        default_timeout: Duration,
        core_active: bool,
    ) -> Initial {
        let dir_eva = get_eva_dir();
        let mut timeout = self.timeout.clone();
        timeout.offer(default_timeout.as_secs_f64());
        let mut bus = self.bus.clone();
        bus.offer_timeout(busrt::DEFAULT_TIMEOUT.as_secs_f64());
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        if crate::is_unix_socket!(bus.path()) {
            bus.set_path(&format_path(&dir_eva, Some(bus.path()), None));
        }
        Initial::new(
            id,
            system_name,
            &format_path(&dir_eva, Some(&self.command), None),
            self.prepare_command.as_deref(),
            &format!("{}/runtime/svc_data/{}", dir_eva, id),
            &timeout,
            CoreInfo::new(
                BUILD,
                VERSION,
                EAPI_VERSION,
                &dir_eva,
                crate::logs::get_min_log_level().0,
                core_active,
            ),
            bus,
            self.config.as_ref(),
            self.workers,
            self.user.as_deref(),
            self.react_to_fail,
            crate::FIPS.load(atomic::Ordering::SeqCst),
        )
    }
    fn startup_timeout(&self) -> Option<Duration> {
        self.timeout.startup()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, bmart::tools::Sorting)]
#[serde(deny_unknown_fields)]
pub struct Info {
    id: String,
    launcher: Arc<String>,
    status: Status,
    enabled: bool,
    pid: Option<u32>,
}

/// Used by API to show the actial service state (Unknown = unable to contact the launcher)
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Unknown,
    Offline,
    Starting,
    Terminating,
    Online,
    Private,
    Disabled,
}

impl From<u8> for Status {
    fn from(s: u8) -> Status {
        match s {
            0 => Status::Starting,
            1 => Status::Online,
            0xef => Status::Terminating,
            _ => Status::Private,
        }
    }
}

impl Info {
    fn from_params(
        id: &str,
        status: Option<&ServiceStatus>,
        launcher: Arc<String>,
        enabled: bool,
    ) -> Self {
        let (ss, pid) = if let Some(st) = status {
            if st.pid.is_some() {
                (st.status, st.pid)
            } else {
                (st.status, None)
            }
        } else if enabled {
            (Status::Unknown, None)
        } else {
            (Status::Disabled, None)
        };
        Self {
            id: id.to_owned(),
            launcher,
            status: ss,
            enabled,
            pid,
        }
    }
}
