use crate::svcmgr::ServiceStatus;
use crate::{EResult, Error};
use busrt::client::AsyncClient;
use busrt::rpc::{self, Rpc, RpcClient, RpcError, RpcEvent, RpcHandlers, RpcResult};
use busrt::tools::pubsub;
use busrt::{Frame, FrameKind, QoS};
use eva_common::err_logger;
use eva_common::events::SERVICE_STATUS_TOPIC;
use eva_common::payload::{pack, unpack};
use eva_common::services::SERVICE_PAYLOAD_INITIAL;
use eva_common::services::SERVICE_PAYLOAD_PING;
use eva_common::services::{Initial, RealtimeConfig};
use eva_common::tools::get_eva_dir;
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic;
use std::time::Duration;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::io::{BufReader, BufWriter};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;

err_logger!();

pub const LAUNCHER_CLIENT_PFX: &str = "eva.launcher.";

struct ServiceRuntimeData {
    pid: u32,
    tasks: Vec<JoinHandle<()>>,
    shutdown_timeout: Duration,
    status: Arc<atomic::AtomicU8>,
}

impl Drop for ServiceRuntimeData {
    fn drop(&mut self) {
        for fut in &self.tasks {
            fut.abort();
        }
    }
}

struct Service {
    id: String,
    mem_warn: u64,
    data: Arc<Mutex<Option<ServiceRuntimeData>>>,
    active: Arc<atomic::AtomicBool>,
    runner_fut: Option<JoinHandle<()>>,
}

impl Service {
    fn new(id: &str, mem_warn: u64) -> Self {
        Self {
            id: id.to_owned(),
            mem_warn,
            data: <_>::default(),
            active: <_>::default(),
            runner_fut: None,
        }
    }
    #[allow(clippy::too_many_lines)]
    async fn launch(
        id: &str,
        initial: &Initial,
        rpc: Arc<RwLock<Option<RpcClient>>>,
        service_data: &Mutex<Option<ServiceRuntimeData>>,
        active_beacon: Arc<atomic::AtomicBool>,
    ) -> EResult<()> {
        let mut sp = initial.command().split(' ');
        let command = sp
            .next()
            .ok_or_else(|| Error::invalid_data("command not specified"))?;
        let args: Vec<&str> = sp.collect();
        initial.set_fail_mode(false);
        let mut child = Command::new(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(initial.env())
            .kill_on_drop(false)
            .args(&args)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::io("unable to get stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::io("unable to get stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::io("unable to get stderr"))?;
        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();
        let pid = child
            .id()
            .ok_or_else(|| Error::io("unable to get process id"))?;
        let mut stdin_writer = BufWriter::new(stdin);
        let payload = pack(initial)?;
        let payload_len: u32 = payload.len().try_into().map_err(Error::failed)?;
        stdin_writer.write_all(&[SERVICE_PAYLOAD_INITIAL]).await?;
        stdin_writer.flush().await?;
        stdin_writer.write_all(&payload_len.to_le_bytes()).await?;
        stdin_writer.flush().await?;
        stdin_writer.write_all(&payload).await?;
        stdin_writer.flush().await?;
        let startup_timeout = initial.startup_timeout();
        let shutdown_timeout = initial.shutdown_timeout();
        let timeout = initial.timeout();
        let svc_id = id.to_owned();
        {
            let mut r_data_c = service_data.lock().await;
            let a_beacon = active_beacon.clone();
            let pinger_fut = tokio::spawn(async move {
                async fn ping(
                    id: &str,
                    rpc: &RwLock<Option<RpcClient>>,
                    timeout: Duration,
                ) -> EResult<()> {
                    if let Some(rpc) = rpc.read().await.as_ref() {
                        tokio::time::timeout(
                            timeout,
                            rpc.call(id, "test", busrt::empty_payload!(), QoS::Processed),
                        )
                        .await??;
                    }
                    Ok(())
                }
                sleep(startup_timeout).await;
                macro_rules! terminate {
                    ($e: expr) => {
                        if a_beacon.load(atomic::Ordering::Relaxed) {
                            warn!("service {} heartbeat error: {}", svc_id, $e);
                        }
                        bmart::process::kill_pstree(pid, Some(shutdown_timeout), true).await;
                        break;
                    };
                }
                loop {
                    if let Err(e) = ping(&svc_id, &rpc, timeout).await {
                        terminate!(e);
                    }
                    if let Err(e) = stdin_writer.write_all(&[SERVICE_PAYLOAD_PING]).await {
                        terminate!(e);
                    }
                    if let Err(e) = stdin_writer.flush().await {
                        terminate!(e);
                    }
                    sleep(timeout).await;
                }
            });
            let svc_id = id.to_owned();
            let stdout_fut = tokio::spawn(async move {
                while let Ok(Some(line)) = stdout_reader.next_line().await {
                    info!("{} {}", svc_id, line);
                }
            });
            let svc_id = id.to_owned();
            let stderr_fut = tokio::spawn(async move {
                while let Ok(Some(line)) = stderr_reader.next_line().await {
                    error!("{} {}", svc_id, line);
                }
            });
            let status_beacon: Arc<atomic::AtomicU8> = <_>::default();
            let b = status_beacon.clone();
            let svc_id = id.to_owned();
            let ready_fut = tokio::spawn(async move {
                sleep(startup_timeout).await;
                if b.load(atomic::Ordering::Relaxed) == 0 {
                    error!("service {} is not ready, terminating", svc_id);
                    bmart::process::kill_pstree(pid, None, true).await;
                }
            });
            let r_data: ServiceRuntimeData = ServiceRuntimeData {
                pid,
                tasks: vec![pinger_fut, stdout_fut, stderr_fut, ready_fut],
                shutdown_timeout,
                status: status_beacon,
            };
            r_data_c.replace(r_data);
        }
        let result = child.wait().await;
        service_data.lock().await.take();
        let code = result?.code().unwrap_or(-1);
        if code == 0 || !active_beacon.load(atomic::Ordering::Relaxed) {
            Ok(())
        } else {
            if initial.can_rtf() {
                info!("launching service {} react-to-fail mode", id);
                initial.set_fail_mode(true);
                let mut buf = vec![SERVICE_PAYLOAD_INITIAL];
                let payload = pack(&initial)?;
                let payload_len: u32 = payload.len().try_into().map_err(Error::failed)?;
                buf.extend(payload_len.to_le_bytes());
                buf.extend(&payload);
                match bmart::process::command(
                    command,
                    args,
                    initial.timeout(),
                    bmart::process::Options::new().input(Cow::Owned(buf)),
                )
                .await
                {
                    Ok(v) => {
                        for line in v.out {
                            info!("{} {}", id, line);
                        }
                        for line in v.err {
                            error!("{} {}", id, line);
                        }
                        let code = v.code.unwrap_or(-1);
                        if code != 0 {
                            error!(
                                "service {} react-to-fail mode exited with code {}",
                                id, code
                            );
                        }
                    }
                    Err(e) => {
                        error!("unable to launch service {} react-to-fail mode: {}", id, e);
                    }
                }
            }
            Err(Error::failed(format!(
                "service {} exited with code {}",
                id, code
            )))
        }
    }
    async fn runner(
        id: &str,
        initial: Initial,
        rpc: Arc<RwLock<Option<RpcClient>>>,
        active_beacon: Arc<atomic::AtomicBool>,
        service_data: Arc<Mutex<Option<ServiceRuntimeData>>>,
    ) -> EResult<()> {
        while active_beacon.load(atomic::Ordering::Relaxed) {
            Self::launch(
                id,
                &initial,
                rpc.clone(),
                &service_data,
                active_beacon.clone(),
            )
            .await
            .map_err(|e| Error::io(format!("unable to launch {} service: {}", id, e)))
            .log_ef();
            sleep(initial.restart_delay()).await;
        }
        Ok(())
    }
    fn start(&mut self, initial: Initial, rpc: Arc<RwLock<Option<RpcClient>>>) {
        self.active.store(true, atomic::Ordering::Relaxed);
        let id = self.id.clone();
        let active = self.active.clone();
        let data = self.data.clone();
        let fut = tokio::spawn(async move {
            Self::runner(&id, initial, rpc, active, data).await.log_ef();
        });
        self.runner_fut.replace(fut);
    }
    async fn stop(&self) {
        self.active.store(false, atomic::Ordering::Relaxed);
        let data = self.data.lock().await.take();
        if let Some(service_data) = data {
            bmart::process::kill_pstree(
                service_data.pid,
                Some(service_data.shutdown_timeout),
                true,
            )
            .await;
        }
        if let Some(ref fut) = self.runner_fut {
            fut.abort();
        }
    }
}

struct Handlers {
    services: Arc<Mutex<HashMap<String, Arc<Service>>>>,
    rpc_server: Arc<RwLock<Option<RpcClient>>>,
    rpc: Arc<RwLock<Option<RpcClient>>>,
    topic_broker: pubsub::TopicBroker,
    default_realtime: RealtimeConfig,
}

impl Handlers {
    fn new(realtime: RealtimeConfig) -> Self {
        Self {
            services: <_>::default(),
            rpc_server: <_>::default(),
            rpc: <_>::default(),
            topic_broker: <_>::default(),
            default_realtime: realtime,
        }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        macro_rules! stop_svc {
            ($id: expr, $services: expr) => {{
                let svc = $services.remove($id);
                if let Some(service) = svc {
                    service.stop().await;
                }
            }};
        }
        match event.parse_method()? {
            "list" => {
                let mut result: HashMap<&str, ServiceStatus> = HashMap::new();
                let services = self.services.lock().await;
                for (s, v) in &*services {
                    result.insert(
                        s,
                        v.data.lock().await.as_ref().map_or_else(
                            || ServiceStatus {
                                pid: None,
                                status: crate::svcmgr::Status::Starting,
                            },
                            |d| ServiceStatus {
                                pid: Some(d.pid),
                                status: d.status.load(atomic::Ordering::Relaxed).into(),
                            },
                        ),
                    );
                }
                Ok(Some(pack(&result)?))
            }
            "get" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ParamsGet<'a> {
                    #[serde(borrow)]
                    i: &'a str,
                }
                let p: ParamsGet = unpack(event.payload())?;
                let services = self.services.lock().await;
                if let Some(srv) = services.get(p.i) {
                    let res = srv.data.lock().await.as_ref().map_or_else(
                        || ServiceStatus {
                            pid: None,
                            status: crate::svcmgr::Status::Starting,
                        },
                        |d| ServiceStatus {
                            pid: Some(d.pid),
                            status: d.status.load(atomic::Ordering::Relaxed).into(),
                        },
                    );
                    Ok(Some(pack(&res)?))
                } else {
                    Err(Error::not_found("no such service").into())
                }
            }
            "start" => {
                if self.rpc.read().await.is_none() {
                    return Err(Error::failed("rpc not initialized yet").into());
                }
                let mut services = self.services.lock().await;
                let mut p: crate::svcmgr::PayloadStartStop = unpack(event.payload())?;
                let mut realtime = p.initial.realtime().clone();
                if realtime.priority.is_none() {
                    debug!(
                        "overriding realtime priority for {} to {:?}",
                        p.id, self.default_realtime.priority
                    );
                    realtime.priority = self.default_realtime.priority;
                }
                if realtime.cpu_ids.is_empty() {
                    debug!(
                        "overriding CPU affinity for {} to {:?}",
                        p.id, self.default_realtime.cpu_ids
                    );
                    realtime.cpu_ids.clone_from(&self.default_realtime.cpu_ids);
                }
                p.initial = p.initial.with_realtime(realtime);
                debug!("starting service {}", p.id);
                stop_svc!(&p.id, services);
                if let Some(b) = p.binary_to_reflash
                    && !b.is_empty()
                {
                    let mut sp = p.initial.command().split(' ');
                    let program_path = Path::new(
                        sp.next()
                            .ok_or_else(|| Error::invalid_data("command not specified"))?
                            .trim(),
                    );
                    let program_dir = program_path
                        .parent()
                        .ok_or_else(|| Error::io("unable to get svc directory"))
                        .log_err()?
                        .canonicalize()
                        .log_err()?;
                    warn!(
                        "reflashing service {} binary {}",
                        p.id,
                        program_path.display()
                    );
                    let venv_bin = Path::new(&get_eva_dir()).join("venv/bin");
                    let forbidden_paths = [
                        Path::new("/"),
                        Path::new("/bin"),
                        Path::new("/sbin"),
                        Path::new("/usr/bin"),
                        Path::new("/usr/sbin"),
                        Path::new("/usr/local/bin"),
                        Path::new("/usr/local/sbin"),
                        &venv_bin,
                    ];
                    if forbidden_paths.contains(&program_dir.as_path()) {
                        error!(
                            "refusing to reflash service {} binary {} to system path",
                            p.id,
                            program_path.display()
                        );
                        return Err(Error::invalid_data(
                            "refusing to reflash binary to system path",
                        )
                        .into());
                    }
                    if program_dir == Path::new(&get_eva_dir()).join("svc") {
                        error!(
                            "refusing to reflash service {} binary {} for the internal service",
                            p.id,
                            program_path.display()
                        );
                        return Err(Error::invalid_data(
                            "refusing to reflash binary for the internal service",
                        )
                        .into());
                    }
                    if program_path.exists() {
                        fs::remove_file(&program_path).await.log_err()?;
                    }
                    fs::write(&program_path, &b).await?;
                    let mut perms = fs::metadata(&program_path).await.log_err()?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&program_path, perms).await.log_err()?;
                }
                let mut service = Service::new(&p.id, p.mem_warn);
                if let Some(dp) = p.initial.data_path() {
                    let data_path = Path::new(dp);
                    if !data_path.exists() {
                        fs::create_dir_all(data_path).await?;
                        let mut mp = fs::metadata(data_path).await?.permissions();
                        mp.set_mode(0o100_700);
                        fs::set_permissions(data_path, mp).await?;
                        let x_user = if let Some(user) = p.initial.user() {
                            Some(eva_common::services::get_system_user(user)?)
                        } else {
                            None
                        };
                        if let Some(u) = x_user
                            && nix::unistd::getuid() != u.uid
                        {
                            nix::unistd::chown(data_path, Some(u.uid), Some(u.gid)).map_err(|e| {
                            Error::failed(format!(
                                "Failed to change the service data directory permisssions for {}: {}", p.id,
                                e
                            ))
                        })?;
                        }
                    }
                }
                service.start(p.initial, self.rpc.clone());
                services.insert(p.id, Arc::new(service));
                Ok(None)
            }
            "stop" => {
                let p: crate::svcmgr::PayloadStartStop = unpack(event.payload())?;
                debug!("stopping service {}", p.id);
                stop_svc!(&p.id, self.services.lock().await);
                Ok(None)
            }
            "stop_and_purge" => {
                let p: crate::svcmgr::PayloadStartStop = unpack(event.payload())?;
                debug!("stopping service {}", p.id);
                stop_svc!(&p.id, self.services.lock().await);
                debug!("purging service data {}", p.id);
                if let Err(e) = fs::remove_dir_all(&p.initial.planned_data_path()).await
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(Into::<Error>::into(e).into());
                }
                Ok(None)
            }
            "shutdown" => {
                let mut futs = Vec::new();
                for svc in self.services.lock().await.values() {
                    let svc = svc.clone();
                    let fut = tokio::spawn(async move {
                        svc.stop().await;
                    });
                    futs.push(fut);
                }
                for fut in futs {
                    fut.await.map_err(Error::failed)?;
                }
                Ok(None)
            }
            _ => Err(RpcError::method(None)),
        }
    }
    async fn handle_frame(&self, frame: Frame) {
        if frame.kind() == FrameKind::Publish {
            self.topic_broker.process(frame).await.log_ef();
        }
    }
}

async fn status_handler(
    services: &Mutex<HashMap<String, Arc<Service>>>,
    rx: async_channel::Receiver<pubsub::Publication>,
) {
    while let Ok(frame) = rx.recv().await {
        if let Ok(status) =
            unpack::<eva_common::services::ServiceStatusBroadcastEvent>(frame.payload())
            && let Some(svc) = services.lock().await.get(frame.primary_sender())
            && let Some(data) = svc.data.lock().await.as_ref()
        {
            data.status
                .store(status.status as u8, atomic::Ordering::Relaxed);
        }
    }
}

async fn mem_warn_handler(services: &Mutex<HashMap<String, Arc<Service>>>) {
    let mut int = tokio::time::interval(crate::SYSINFO_CHECK_INTERVAL);
    loop {
        int.tick().await;
        let mut to_check: Vec<(String, u32, u64)> = Vec::new();
        for (id, svc) in &*services.lock().await {
            if let Some(data) = svc.data.lock().await.as_ref() {
                to_check.push((id.clone(), data.pid, svc.mem_warn));
            }
        }
        for (id, pid, mem_warn) in to_check {
            let pid = sysinfo::Pid::from(pid as usize);
            // safe lock
            let system = loop {
                let Some(system) = crate::SYSTEM_INFO.try_lock() else {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    continue;
                };
                break system;
            };

            let Some(process) = system.process(pid) else {
                continue;
            };
            let mut total_memory = process.memory();
            for process in system.processes().values() {
                if process.parent() == Some(pid) {
                    total_memory += process.memory();
                }
            }
            drop(system);
            crate::check_memory_usage(&id, total_memory, mem_warn);
        }
    }
}

pub async fn init<C>(
    mut client: C,
    client_secondary: C,
    channel_size: usize,
    realtime: RealtimeConfig,
) -> EResult<()>
where
    C: AsyncClient + 'static,
{
    let mut handlers = Handlers::new(realtime);
    let handlers_rpc = handlers.rpc.clone();
    let handlers_rpc_server = handlers.rpc_server.clone();
    let (_, rx) = handlers
        .topic_broker
        .register_topic(SERVICE_STATUS_TOPIC, channel_size)?;
    let services = handlers.services.clone();
    tokio::spawn(async move {
        status_handler(&services, rx).await;
    });
    let services = handlers.services.clone();
    tokio::spawn(async move {
        mem_warn_handler(&services).await;
    });
    client
        .subscribe(SERVICE_STATUS_TOPIC, QoS::Processed)
        .await?;
    let rpc = RpcClient::create(client, handlers, rpc::Options::new().blocking_frames());
    handlers_rpc_server.write().await.replace(rpc);
    let rpc_secondary = RpcClient::create0(client_secondary, rpc::Options::new().blocking_frames());
    handlers_rpc.write().await.replace(rpc_secondary);
    Ok(())
}
