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
use serde::Deserialize;
use std::time::Duration;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Mailer service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

struct Mailer {
    mailer: AsyncSmtpTransport<Tokio1Executor>,
    timeout: Duration,
    default_rcp: Vec<Mailbox>,
    from: Mailbox,
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
        Ok(Self {
            mailer,
            timeout,
            default_rcp: rcps,
            from: from.parse().map_err(Error::invalid_data)?,
        })
    }
    async fn send(&self, rcp: Option<&[&str]>, subject: Option<&str>, body: String) -> EResult<()> {
        let rcps = if let Some(recipients) = rcp {
            let mut rcps: Vec<Mailbox> = Vec::new();
            for s in recipients {
                let m = s.parse().map_err(Error::invalid_data)?;
                rcps.push(m);
            }
            rcps
        } else {
            self.default_rcp.clone()
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
    mailer: Mailer,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Recipient {
    Single(String),
    Multiple(Vec<String>),
}

impl Recipient {
    fn as_vec(&self) -> Vec<&str> {
        match self {
            Recipient::Single(ref r) => vec![r],
            Recipient::Multiple(ref r) => r.iter().map(String::as_str).collect(),
        }
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
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
                        #[serde(default)]
                        rcp: Option<Recipient>,
                        #[serde(default)]
                        subject: Option<String>,
                        #[serde(default)]
                        text: Option<String>,
                    }
                    let p: ParamsSend = unpack(payload)?;
                    let rcp = p.rcp.as_ref().map(Recipient::as_vec);
                    self.mailer
                        .send(
                            rcp.as_deref(),
                            p.subject.as_deref(),
                            p.text.unwrap_or_default(),
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
    default_rcp: Vec<String>,
    #[serde(default)]
    from: Option<String>,
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
            .optional("rcp")
            .optional("subject")
            .optional("text"),
    );
    let rpc = initial
        .init_rpc(Handlers {
            info,
            mailer: Mailer::new(
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
            )?,
        })
        .await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
