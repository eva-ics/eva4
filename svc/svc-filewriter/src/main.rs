use chrono::{DateTime, Local, NaiveDateTime, SecondsFormat, Utc};
use cron::Schedule;
use eva_common::acl::OIDMaskList;
use eva_common::common_payloads::ParamsIdListOwned;
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::{ItemState, ShortItemState, State};
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Deserializer};
use std::io::SeekFrom;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

err_logger!();

lazy_static! {
    static ref RPC: OnceCell<Arc<RpcClient>> = <_>::default();
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "File writer service";

const CSV_HEADER: &[&str] = &[
    "OID",
    "Type",
    "Group",
    "Id",
    "Timestamp",
    "Time",
    "Status",
    "Value",
];

const CSV_ESCAPED_CHARS: &str = ",\r\n\"";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

struct Handlers {
    info: ServiceInfo,
    tx: async_channel::Sender<Event>,
}

async fn process_state(
    topic: &str,
    path: &str,
    payload: &[u8],
    tx: &async_channel::Sender<Event>,
) -> EResult<()> {
    match OID::from_path(path) {
        Ok(oid) => match unpack::<State>(payload) {
            Ok(v) => {
                tx.send(Event::State(ItemState {
                    oid,
                    status: v.status,
                    value: v.value,
                    set_time: v.set_time,
                }))
                .await
                .map_err(Error::core)?;
            }
            Err(e) => {
                warn!("invalid state event payload {}: {}", topic, e);
            }
        },
        Err(e) => warn!("invalid OID in state event {}: {}", topic, e),
    }
    Ok(())
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        match method {
            "flush" => {
                if event.payload().is_empty() {
                    self.tx.send(Event::Flush).await.map_err(Error::core)?;
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            "rotate" => {
                if event.payload().is_empty() {
                    self.tx.send(Event::Rotate).await.map_err(Error::core)?;
                    Ok(None)
                } else {
                    Err(RpcError::params(None))
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, frame: Frame) {
        if frame.kind() == busrt::FrameKind::Publish {
            if let Some(topic) = frame.topic() {
                if let Some(o) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), &self.tx)
                        .await
                        .log_ef();
                } else if let Some(o) = topic.strip_prefix(REMOTE_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), &self.tx)
                        .await
                        .log_ef();
                } else if let Some(o) = topic.strip_prefix(REMOTE_ARCHIVE_STATE_TOPIC) {
                    process_state(topic, o, frame.payload(), &self.tx)
                        .await
                        .log_ef();
                }
            }
        }
    }
}

#[inline]
fn default_queue_size() -> usize {
    8192
}

#[derive(Deserialize, Copy, Clone)]
#[serde(rename_all = "lowercase")]
enum DataFormat {
    Json,
    Csv,
}

#[inline]
pub fn de_opt_schedule_from_str<'de, D>(deserializer: D) -> Result<Option<Schedule>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf: Option<String> = Option::deserialize(deserializer)?;
    if let Some(b) = buf {
        Ok(Some(b.parse().map_err(serde::de::Error::custom)?))
    } else {
        Ok(None)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    file_path: String,
    #[serde(default)]
    auto_flush: bool,
    #[serde(default)]
    dos_cr: bool,
    format: DataFormat,
    #[serde(default = "default_queue_size")]
    queue_size: usize,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    interval: Option<Duration>,
    #[serde(default, deserialize_with = "de_opt_schedule_from_str")]
    auto_rotate: Option<Schedule>,
    #[serde(default)]
    ignore_events: bool,
    oids: OIDMaskList,
}

enum Event {
    State(ItemState),
    BulkState(Vec<ItemState>),
    Flush,
    Rotate,
}

fn generate_file_path(base_file_path: &str) -> String {
    if base_file_path.contains('%') {
        let dt = Local::now();
        dt.format(base_file_path).to_string()
    } else {
        base_file_path.to_owned()
    }
}

async fn file_handler(
    rx: &async_channel::Receiver<Event>,
    base_file_path: &str,
    gen: &DataGen,
    auto_flush: bool,
) -> EResult<()> {
    let mut file_path = generate_file_path(base_file_path);
    let mut fx: Option<tokio::fs::File> = None;
    while let Ok(event) = rx.recv().await {
        let mut fd_opt = None;
        if let Some(ref mut existing_fh) = fx {
            if let Ok(path_metadata) = tokio::fs::metadata(&file_path).await {
                let metadata = existing_fh.metadata().await?;
                if metadata.dev() == path_metadata.dev() && metadata.ino() == path_metadata.ino() {
                    fd_opt = Some(existing_fh);
                }
            }
        }
        let fd = if let Some(fd) = fd_opt {
            fd
        } else {
            let mut f = tokio::fs::OpenOptions::new()
                .read(false)
                .write(true)
                .truncate(false)
                .append(true)
                .create(true)
                .open(&file_path)
                .await?;
            f.seek(SeekFrom::End(0)).await?;
            fx.replace(f);
            fx.as_mut().unwrap()
        };
        let pos = fd.seek(SeekFrom::Current(0)).await?;
        if pos == 0 {
            if let Some(header) = gen.header() {
                fd.write_all(&header).await?;
            }
        }
        match event {
            Event::State(state) => fd.write_all(&gen.line(state)?).await?,
            Event::BulkState(states) => {
                let mut buf = Vec::new();
                for state in states {
                    buf.extend(&gen.line(state)?);
                }
                fd.write_all(&buf).await?;
            }
            Event::Flush => {
                fd.flush().await?;
                continue;
            }
            Event::Rotate => {
                fd.flush().await?;
                fx.take();
                if base_file_path.contains('%') {
                    file_path = generate_file_path(base_file_path);
                } else {
                    let dt = Local::now();
                    let rotated_path = format!(
                        "{}.{}",
                        &file_path,
                        dt.to_rfc3339_opts(SecondsFormat::Secs, false)
                    );
                    tokio::fs::rename(&file_path, rotated_path).await?;
                }
                continue;
            }
        }
        if auto_flush {
            fd.flush().await?;
        }
    }
    Ok(())
}

struct DataGen {
    format: DataFormat,
    dos_cr: bool,
}

impl DataGen {
    #[inline]
    fn new(format: DataFormat, dos_cr: bool) -> Self {
        Self { format, dos_cr }
    }
    #[inline]
    fn header(&self) -> Option<Vec<u8>> {
        match self.format {
            DataFormat::Json => None,
            DataFormat::Csv => Some(self.prepare(CSV_HEADER.join(","))),
        }
    }
    #[inline]
    fn line(&self, state: ItemState) -> EResult<Vec<u8>> {
        match self.format {
            DataFormat::Json => Ok(self.prepare(serde_json::to_string(&state)?)),
            DataFormat::Csv => {
                let s = format!(
                    "{},{},{},{},{},{},{},{}",
                    state.oid,
                    state.oid.kind(),
                    state.oid.group().unwrap_or_default(),
                    state.oid.id(),
                    state.set_time,
                    date_str(state.set_time),
                    state.status,
                    Self::escape_csv(state.value.unwrap_or_default().to_string())
                );
                Ok(self.prepare(s))
            }
        }
    }
    #[inline]
    fn prepare(&self, mut s: String) -> Vec<u8> {
        if self.dos_cr {
            s += "\r";
        }
        s += "\n";
        s.as_bytes().to_vec()
    }
    #[inline]
    fn escape_csv(mut s: String) -> String {
        s = s.replace('"', "\"\"");
        for c in s.chars() {
            if CSV_ESCAPED_CHARS.contains(c) {
                s = format!("\"{}\"", s);
                break;
            }
        }
        s
    }
}

async fn collect_periodic(
    oids: &OIDMaskList,
    interval: Duration,
    tx: &async_channel::Sender<Event>,
) -> EResult<()> {
    let i: Vec<String> = oids.oid_masks().iter().map(ToString::to_string).collect();
    let p = ParamsIdListOwned { i };
    let payload = pack(&p)?;
    let mut int = tokio::time::interval(interval);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let rpc = RPC.get().unwrap();
    loop {
        int.tick().await;
        let data = rpc
            .call(
                "eva.core",
                "item.state",
                (payload.as_slice()).into(),
                QoS::Processed,
            )
            .await?;
        let states: Vec<ShortItemState> = unpack(data.payload())?;
        if !states.is_empty() {
            let t = eva_common::time::now_ns_float();
            tx.send(Event::BulkState(
                states
                    .into_iter()
                    .map(|s| ItemState {
                        oid: s.oid,
                        status: s.status,
                        value: s.value,
                        set_time: t,
                    })
                    .collect(),
            ))
            .await
            .map_err(Error::core)?;
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
#[inline]
fn date_str(t: f64) -> String {
    let dt_utc = DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(t.round() as i64, 0), Utc);
    let dt: DateTime<Local> = DateTime::from(dt_utc);
    dt.to_rfc3339_opts(SecondsFormat::Secs, false)
}

async fn auto_rotate(schedule: Schedule, tx: async_channel::Sender<Event>) -> EResult<()> {
    for next_launch in schedule.upcoming(Local) {
        let now: DateTime<Local> = Local::now();
        if next_launch > now {
            let run_in = next_launch - now;
            tokio::time::sleep(run_in.to_std().map_err(Error::core)?).await;
            if svc_is_terminating() {
                return Ok(());
            }
            debug!("auto-rotating");
            tx.send(Event::Rotate).await.map_err(Error::core)?;
        } else {
            warn!("auto-rotation skipped");
        }
    }
    warn!("auto-rotation finished, no upcoming schedules");
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let (tx, rx) = async_channel::bounded::<Event>(config.queue_size);
    let file_path = config.file_path;
    let auto_flush = config.auto_flush;
    let gen = DataGen::new(config.format, config.dos_cr);
    tokio::spawn(async move {
        loop {
            file_handler(&rx, &file_path, &gen, auto_flush)
                .await
                .log_ef();
        }
    });
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("flush"));
    info.add_method(ServiceMethod::new("rotate"));
    let rpc = initial
        .init_rpc_blocking(Handlers {
            info,
            tx: tx.clone(),
        })
        .await?;
    initial.drop_privileges()?;
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("unable to set RPC"))?;
    if !config.ignore_events {
        eva_sdk::service::subscribe_oids(
            rpc.as_ref(),
            &config.oids,
            eva_sdk::service::EventKind::Any,
        )
        .await?;
    }
    let client = rpc.client().clone();
    let txc = tx.clone();
    if let Some(interval) = config.interval {
        let rpc_c = rpc.clone();
        let startup_timeout = initial.startup_timeout();
        tokio::spawn(async move {
            let _r = svc_wait_core(&rpc_c, startup_timeout, true).await;
            loop {
                collect_periodic(&config.oids, interval, &txc)
                    .await
                    .log_ef();
            }
        });
    }
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    if let Some(sched) = config.auto_rotate {
        let txc = tx.clone();
        tokio::spawn(async move { auto_rotate(sched, txc).await.log_ef() });
    }
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    while !tx.is_empty() {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
    Ok(())
}
