use eva_common::events;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic;
use std::time::{Duration, Instant};

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Item state expiration checker";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct ItemWatch {
    oid: OID,
    expires: Duration,
    last_event: Mutex<Instant>,
    expired: atomic::AtomicBool,
}

impl ItemWatch {
    #[inline]
    fn is_expired(&self) -> bool {
        (*self.last_event.lock().unwrap()).elapsed() >= self.expires
    }
    #[inline]
    fn is_marked_expired(&self) -> bool {
        self.expired.load(atomic::Ordering::SeqCst)
    }
    #[inline]
    fn handle_event(&self) {
        *self.last_event.lock().unwrap() = Instant::now();
        self.expired.store(false, atomic::Ordering::SeqCst);
    }
    #[inline]
    fn mark_expired(&self) {
        self.expired.store(true, atomic::Ordering::SeqCst);
    }
}

async fn checker<C>(
    client: Arc<tokio::sync::Mutex<C>>,
    watchers: Arc<HashMap<String, ItemWatch>>,
    sleep_step: Duration,
) where
    C: ?Sized + AsyncClient,
{
    let ev = pack(&events::RawStateEvent::new0(-1)).unwrap();
    let mut next_iter = Instant::now();
    loop {
        let mut expired: Vec<&str> = Vec::new();
        for (p, w) in &*watchers {
            if !w.is_marked_expired() && w.is_expired() {
                log::log!(
                    if w.oid.kind() == ItemKind::Lvar {
                        log::Level::Debug
                    } else {
                        log::Level::Warn
                    },
                    "marking {} as expired",
                    w.oid
                );
                expired.push(p);
                w.mark_expired();
            }
        }
        if !expired.is_empty() {
            {
                let mut c = client.lock().await;
                for exp in expired {
                    c.publish(
                        &format!("{}{}", events::RAW_STATE_TOPIC, exp),
                        (ev.as_slice()).into(),
                        QoS::No,
                    )
                    .await
                    .log_ef();
                }
            }
        }
        next_iter += sleep_step;
        let now = Instant::now();
        if svc_is_terminating() {
            break;
        } else if now > next_iter {
            warn!("checker loop timeout");
            while next_iter < now {
                next_iter += sleep_step;
            }
        }
        let to_sleep = next_iter - now;
        async_io::Timer::after(to_sleep).await;
    }
}

struct Handlers {
    me: String,
    watchers: Arc<HashMap<String, ItemWatch>>,
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_handle_default_rpc(event.parse_method()?, &self.info)
    }
    async fn handle_frame(&self, frame: Frame) {
        if frame.kind() == busrt::FrameKind::Publish && frame.primary_sender() != self.me {
            if let Some(topic) = frame.topic() {
                if let Some(oid_path) = topic.strip_prefix(events::RAW_STATE_TOPIC) {
                    if let Some(i) = self.watchers.get(oid_path) {
                        i.handle_event();
                    }
                } else if let Some(oid_path) = topic.strip_prefix(events::LOCAL_STATE_TOPIC) {
                    // handle for lvars
                    if let Some(i) = self.watchers.get(oid_path) {
                        i.handle_event();
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(
        default = "eva_common::get_default_sleep_step",
        deserialize_with = "eva_common::tools::de_float_as_duration"
    )]
    interval: Duration,
    #[serde(default)]
    items: HashMap<OID, f64>,
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    // get config from initial and deserialize it
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    // process the config
    let now = Instant::now();
    let mut topics: Vec<String> = Vec::new();
    let mut watchers = HashMap::new();
    for (oid, t) in config.items {
        let oid_path = oid.as_path().to_owned();
        topics.push(format!(
            "{}{}",
            if oid.kind() == ItemKind::Lvar {
                events::LOCAL_STATE_TOPIC
            } else {
                events::RAW_STATE_TOPIC
            },
            oid_path
        ));
        let i = ItemWatch {
            oid,
            expires: Duration::from_secs_f64(t),
            last_event: Mutex::new(now),
            expired: atomic::AtomicBool::new(false),
        };
        watchers.insert(oid_path, i);
    }
    let watchers = Arc::new(watchers);
    // init rpc
    let handlers = Handlers {
        me: initial.id().to_owned(),
        watchers: watchers.clone(),
        info: ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION),
    };
    let rpc = initial.init_rpc_blocking(handlers).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    // init logs
    svc_init_logs(&initial, client.clone())?;
    // start sigterm handler
    svc_start_signal_handlers();
    // work
    client
        .lock()
        .await
        .subscribe_bulk(
            &topics.iter().map(AsRef::as_ref).collect::<Vec<&str>>(),
            QoS::RealtimeProcessed,
        )
        .await?;
    let c = client.clone();
    let rpc_c = rpc.clone();
    svc_mark_ready(&client).await?;
    let startup_timeout = initial.startup_timeout();
    let checker_fut = tokio::spawn(async move {
        let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
        checker(c, watchers, config.interval).await;
    });
    info!("item expiration watcher started ({})", initial.id());
    svc_block(&rpc).await;
    checker_fut.abort();
    svc_mark_terminating(&client).await?;
    Ok(())
}
