use eva_common::common_payloads::ValueOrList;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use lettre::{
    message::Mailbox,
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
        PoolConfig,
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{hash_map, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ttl_cache::TtlCache;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Mailer service";
const CACHE_USER_EMAILS_TTL: Duration = Duration::from_secs(10);
const CACHE_USER_EMAILS_SIZE: usize = 10_000;

const MAX_PARALLEL_RESOLVERS: usize = 10;
const MAX_PARALLEL_SENDS_DELAYED: usize = 10;
const MAX_PARALLEL_TASKS: usize = 100;

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();
static AUTH_SVCS: OnceCell<Vec<String>> = OnceCell::new();

static USER_EMAILS: Lazy<Mutex<TtlCache<String, String>>> =
    Lazy::new(|| Mutex::new(TtlCache::new(CACHE_USER_EMAILS_SIZE)));

type DelayedEmailsMap = HashMap<(Vec<String>, Option<String>), DelayedMail>;

static DELAYED_EMAILS: Lazy<Mutex<DelayedEmailsMap>> = Lazy::new(<_>::default);

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

struct DelayedMail {
    delayed_until: Instant,
    body: String,
}

async fn send_delayed_emails(mailer: Arc<Mailer>) {
    let mut int = tokio::time::interval(Duration::from_secs(1));
    loop {
        int.tick().await;
        let Ok(_permit) = mailer.tasks_sem.acquire().await.log_err() else {
            continue;
        };
        let mut emails = Vec::new();
        {
            let mut keys_to_remove = Vec::new();
            let mut delayed_emails = DELAYED_EMAILS.lock();
            let now = Instant::now();
            for (k, v) in &*delayed_emails {
                if v.delayed_until < now {
                    keys_to_remove.push(k.clone());
                }
            }
            for k in keys_to_remove {
                if let Some(d) = delayed_emails.remove(&k) {
                    emails.push((k, d.body));
                }
            }
        }
        let mut tasks = Vec::new();
        let pool = tokio_task_pool::Pool::bounded(MAX_PARALLEL_SENDS_DELAYED);
        for email in emails {
            let mailer_c = mailer.clone();
            let Ok(task) = pool
                .spawn(async move {
                    mailer_c
                        .send(
                            &email.0 .0.iter().map(String::as_str).collect::<Vec<&str>>(),
                            email.0 .1.as_deref(),
                            email.1,
                            None,
                        )
                        .await
                        .log_ef();
                })
                .await
                .log_err()
            else {
                continue;
            };
            tasks.push(task);
        }
        for task in tasks {
            task.await.log_ef();
        }
    }
}

#[derive(Deserialize)]
struct UserProfileField {
    value: Option<String>,
}

#[derive(Serialize)]
struct UserProfileReq<'a> {
    i: &'a str,
    field: &'a str,
}

async fn resolve_email(user: &str) -> EResult<Option<String>> {
    if let Some(addr) = USER_EMAILS.lock().get(user) {
        return Ok(Some(addr.clone()));
    }
    let payload = Arc::new(pack(&UserProfileReq {
        i: user,
        field: "email",
    })?);
    let rpc = RPC.get().unwrap();
    let timeout = *TIMEOUT.get().unwrap();
    for svc in AUTH_SVCS.get().unwrap() {
        if let Ok(result) = safe_rpc_call(
            rpc,
            svc,
            "user.get_profile_field",
            payload.clone().into(),
            QoS::No,
            timeout,
        )
        .await
        {
            match unpack::<UserProfileField>(result.payload()) {
                Ok(v) => {
                    if let Some(addr) = v.value {
                        if !addr.is_empty() {
                            USER_EMAILS.lock().insert(
                                user.to_owned(),
                                addr.clone(),
                                CACHE_USER_EMAILS_TTL,
                            );
                            return Ok(Some(addr));
                        }
                    }
                }
                Err(e) => {
                    error!("svc {} invalid response: {}", svc, e);
                }
            }
        }
    }
    Ok(None)
}

struct Mailer {
    mailer: AsyncSmtpTransport<Tokio1Executor>,
    timeout: Duration,
    default_rcp: Vec<Mailbox>,
    from: Mailbox,
    tasks_sem: tokio::sync::Semaphore,
}

impl Mailer {
    fn new(
        config: &SmtpConfig,
        timeout: Duration,
        default_rcp: &[&str],
        from: &str,
    ) -> EResult<Self> {
        let mut rcps: Vec<Mailbox> = Vec::new();
        for s in default_rcp {
            let m = s.parse().map_err(Error::invalid_data)?;
            rcps.push(m);
        }
        let mut b = if config.tls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
        }
        .map_err(Error::invalid_data)?;
        b = b
            .port(config.port)
            .pool_config(PoolConfig::new().max_size(config.pool_size));
        if !config.ssl && config.tls {
            b = b.tls(Tls::Required(
                TlsParameters::new_native(config.host.clone()).map_err(Error::invalid_data)?,
            ));
        } else if !config.tls && !config.ssl {
            b = b.tls(Tls::None);
        } else {
            b = b.tls(Tls::Wrapper(
                TlsParameters::new_native(config.host.clone()).map_err(Error::invalid_data)?,
            ));
        }
        if let Some(ref username) = config.username {
            b = b.credentials(Credentials::new(
                username.clone(),
                config
                    .password
                    .as_ref()
                    .map_or_else(String::new, Clone::clone),
            ));
        }
        let mailer = b.build();
        let tasks_sem = tokio::sync::Semaphore::new(MAX_PARALLEL_TASKS);
        Ok(Self {
            mailer,
            timeout,
            default_rcp: rcps,
            from: from.parse().map_err(Error::invalid_data)?,
            tasks_sem,
        })
    }
    fn is_busy(&self) -> bool {
        self.tasks_sem.available_permits() < MAX_PARALLEL_TASKS
    }
    async fn send(
        &self,
        rcp: &[&str],
        subject: Option<&str>,
        body: String,
        delayed: Option<Duration>,
    ) -> EResult<()> {
        let _permit = self.tasks_sem.acquire().await.map_err(Error::failed)?;
        if let Some(d) = delayed {
            // if the email is delayed - schedule
            let subj = subject.map(ToOwned::to_owned);
            let rcps = rcp.iter().map(|&v| v.to_owned()).collect::<Vec<String>>();
            match DELAYED_EMAILS.lock().entry((rcps, subj)) {
                hash_map::Entry::Occupied(mut o) => {
                    let d_e = o.get_mut();
                    if !body.is_empty() {
                        if !d_e.body.is_empty() && !d_e.body.ends_with('\n') {
                            writeln!(d_e.body)?;
                        }
                        write!(d_e.body, "{}", body)?;
                    }
                }
                hash_map::Entry::Vacant(v) => {
                    v.insert(DelayedMail {
                        delayed_until: Instant::now() + d,
                        body,
                    });
                }
            }
            return Ok(());
        }
        let rcps = if rcp.is_empty() {
            self.default_rcp.clone()
        } else {
            let mut rcps: Vec<Mailbox> = Vec::with_capacity(rcp.len());
            for s in rcp {
                match s.parse() {
                    Ok(m) => rcps.push(m),
                    Err(e) => error!("invalid email address {}: {}", s, e),
                }
            }
            rcps
        };
        for rcp in &rcps {
            debug!("sending mail to {}, subject: {:?}", rcp, subject);
        }
        if rcps.is_empty() {
            return Err(Error::failed("no rcp specified, no defaults set"));
        }
        let mut ebuilder = Message::builder().from(self.from.clone());
        for rcp in rcps {
            ebuilder = ebuilder.to(rcp);
        }
        if let Some(subj) = subject {
            ebuilder = ebuilder.subject(subj);
        }
        let message = ebuilder.body(body).map_err(Error::invalid_data)?;
        tokio::time::timeout(self.timeout, self.mailer.send(message))
            .await?
            .map_err(Error::failed)?;
        Ok(())
    }
}

struct Handlers {
    info: ServiceInfo,
    mailer: Arc<Mailer>,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "send" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct ParamsSend {
                        i: Option<ValueOrList<String>>,
                        #[serde(default)]
                        rcp: Option<ValueOrList<String>>,
                        #[serde(default)]
                        subject: Option<String>,
                        #[serde(default)]
                        text: Option<String>,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        delayed: Option<Duration>,
                    }
                    let p: ParamsSend = unpack(payload)?;
                    let mut rcp = p.rcp.unwrap_or_default().into_vec();
                    let logins = p.i.unwrap_or_default();
                    rcp.reserve(logins.len());
                    let pool = tokio_task_pool::Pool::bounded(MAX_PARALLEL_RESOLVERS);
                    let mut futs = Vec::new();
                    let has_logins = !logins.is_empty();
                    if has_logins {
                        let (tx, rx) = async_channel::bounded(logins.len());
                        for i in logins {
                            let tx_c = tx.clone();
                            let fut = pool
                                .spawn(async move {
                                    if let Ok(Some(addr)) = resolve_email(&i).await.log_err() {
                                        let _ = tx_c.send(addr).await;
                                    } else {
                                        warn!("unable to resolve email address for {}", i);
                                    }
                                })
                                .await
                                .map_err(Error::failed)?;
                            futs.push(fut);
                        }
                        drop(tx);
                        while let Ok(v) = rx.recv().await {
                            rcp.push(v);
                        }
                    }
                    if has_logins && rcp.is_empty() {
                        return Err(Error::failed("no addresses have been resolved").into());
                    }
                    self.mailer
                        .send(
                            &rcp.iter().map(String::as_str).collect::<Vec<&str>>(),
                            p.subject.as_deref(),
                            p.text.unwrap_or_default(),
                            p.delayed,
                        )
                        .await
                        .log_err()?;
                    Ok(None)
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[inline]
fn default_port() -> u16 {
    25
}

#[inline]
fn default_pool_size() -> u32 {
    5
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmtpConfig {
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    tls: bool,
    #[serde(default)]
    ssl: bool,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default = "default_pool_size")]
    pool_size: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    smtp: SmtpConfig,
    #[serde(default)]
    default_rcp: ValueOrList<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    auth_svcs: Vec<String>,
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("send")
            .optional("i")
            .optional("rcp")
            .optional("subject")
            .optional("text")
            .optional("delayed"),
    );
    let mailer = Arc::new(Mailer::new(
        &config.smtp,
        initial.timeout(),
        &config
            .default_rcp
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
        config
            .from
            .as_deref()
            .unwrap_or(&format!("eva@{}", initial.system_name())),
    )?);
    let rpc = initial
        .init_rpc(Handlers {
            info,
            mailer: mailer.clone(),
        })
        .await?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("Unable to set RPC"))?;
    TIMEOUT
        .set(initial.timeout())
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    AUTH_SVCS
        .set(config.auth_svcs)
        .map_err(|_| Error::core("Unable to set AUTH_SVCS"))?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    tokio::spawn(send_delayed_emails(mailer.clone()));
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    let now = Instant::now();
    for d in DELAYED_EMAILS.lock().values_mut() {
        d.delayed_until = now;
    }
    while mailer.is_busy() || !DELAYED_EMAILS.lock().is_empty() {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
    Ok(())
}
