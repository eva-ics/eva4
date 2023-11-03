use chrono::{DateTime, Local, NaiveDateTime, SecondsFormat, Utc};
use cron::Schedule;
use eva_common::acl::OIDMaskList;
use eva_common::common_payloads::ParamsIdListOwned;
use eva_common::events::{LOCAL_STATE_TOPIC, REMOTE_ARCHIVE_STATE_TOPIC, REMOTE_STATE_TOPIC};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::{ItemState, ShortItemStateConnected, State};
use once_cell::sync::{Lazy, OnceCell};
use openssl::sha::Sha256;
use parking_lot::Mutex;
use serde::{Deserialize, Deserializer};
use std::collections::{btree_map, BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::SeekFrom;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{atomic, Arc};
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

err_logger!();

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static NEED_DEDUP_LINES: atomic::AtomicBool = atomic::AtomicBool::new(false);
static LINE_CACHE: Lazy<Mutex<BTreeMap<[u8; 32], Instant>>> = Lazy::new(<_>::default);
static SKIP_DISCONNECTED: atomic::AtomicBool = atomic::AtomicBool::new(false);

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "File writer service";

const DEDUP_CLEANER_INTERVAL: Duration = Duration::from_secs(30);

fn default_fields(format: DataFormat) -> Vec<FieldKind> {
    match format {
        DataFormat::Json => vec![
            FieldKind::OID,
            FieldKind::Timestamp,
            FieldKind::Status,
            FieldKind::Value,
        ],
        DataFormat::Csv => vec![
            FieldKind::OID,
            FieldKind::Type,
            FieldKind::Group,
            FieldKind::Id,
            FieldKind::Timestamp,
            FieldKind::Time,
            FieldKind::Status,
            FieldKind::Value,
        ],
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Deserialize, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
enum FieldKind {
    OID,
    #[serde(alias = "kind")]
    Type,
    Group,
    Id,
    Timestamp,
    Time,
    Status,
    Value,
}

impl FieldKind {
    fn default_name(self, format: DataFormat) -> &'static str {
        match format {
            DataFormat::Json => match self {
                FieldKind::OID => "oid",
                FieldKind::Type => "type",
                FieldKind::Group => "group",
                FieldKind::Id => "id",
                FieldKind::Timestamp => "t",
                FieldKind::Time => "dt",
                FieldKind::Status => "status",
                FieldKind::Value => "value",
            },
            DataFormat::Csv => match self {
                FieldKind::OID => "OID",
                FieldKind::Type => "Type",
                FieldKind::Group => "Group",
                FieldKind::Id => "Id",
                FieldKind::Timestamp => "Timestamp",
                FieldKind::Time => "Time",
                FieldKind::Status => "Status",
                FieldKind::Value => "Value",
            },
        }
    }
    fn data_value_from(self, state: &ItemState) -> Value {
        match self {
            FieldKind::OID => Value::String(state.oid.as_str().to_owned()),
            FieldKind::Type => Value::String(state.oid.kind().to_string()),
            FieldKind::Group => state
                .oid
                .group()
                .map_or(Value::Unit, |g| Value::String(g.to_owned())),
            FieldKind::Id => Value::String(state.oid.id().to_owned()),
            FieldKind::Timestamp => Value::F64(state.set_time),
            FieldKind::Time => Value::String(date_str(state.set_time)),
            FieldKind::Status => Value::I16(state.status),
            FieldKind::Value => state.value.clone().unwrap_or_default(),
        }
    }
    fn write_csv_string_from(self, s: &mut String, state: &ItemState) -> EResult<()> {
        match self {
            FieldKind::OID => write!(s, "{}", state.oid.as_str())?,
            FieldKind::Type => write!(s, "{}", state.oid.kind())?,
            FieldKind::Group => {
                if let Some(g) = state.oid.group() {
                    write!(s, "{}", g)?;
                }
            }
            FieldKind::Id => write!(s, "{}", state.oid.id())?,
            FieldKind::Timestamp => write!(s, "{}", state.set_time)?,
            FieldKind::Time => write!(s, "{}", date_str(state.set_time))?,
            FieldKind::Status => write!(s, "{}", state.status)?,
            FieldKind::Value => {
                if let Some(ref value) = state.value {
                    write!(s, "{}", escape_csv(value.to_string()))?;
                }
            }
        }
        Ok(())
    }
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

const CSV_ESCAPED_CHARS: &str = ",\r\n\"";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[inline]
fn sha256sum(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finish()
}

fn can_write_line(line: &[u8]) -> bool {
    if NEED_DEDUP_LINES.load(atomic::Ordering::Relaxed) {
        let checksum = sha256sum(line);
        let mut cache = LINE_CACHE.lock();
        if let btree_map::Entry::Vacant(v) = cache.entry(checksum) {
            v.insert(Instant::now());
            true
        } else {
            false
        }
    } else {
        true
    }
}

#[inline]
fn clean_dedup_cache(d: Duration) {
    LINE_CACHE.lock().retain(|_, v| v.elapsed() < d);
}

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
#[allow(clippy::struct_excessive_bools)]
struct Config {
    file_path: String,
    rotated_path: Option<String>,
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
    #[serde(default)]
    skip_disconnected: bool,
    #[serde(default, deserialize_with = "de_opt_schedule_from_str")]
    auto_rotate: Option<Schedule>,
    #[serde(default)]
    ignore_events: bool,
    dedup_lines: Option<u64>,
    fields: Option<Vec<FieldKind>>,
    oids: OIDMaskList,
}

enum Event {
    State(ItemState),
    BulkState(Vec<ItemState>),
    Flush,
    Rotate,
    Quit,
}

fn generate_file_path(base_file_path: &str, dt: DateTime<Local>) -> String {
    if base_file_path.contains('%') {
        dt.format(base_file_path).to_string()
    } else {
        base_file_path.to_owned()
    }
}

async fn sync_dirs(paths: &[&str]) -> EResult<()> {
    let mut dirs = BTreeSet::new();
    for p in paths {
        let p = Path::new(p).to_owned();
        if let Some(parent) = p.parent() {
            dirs.insert(parent.to_owned());
        }
    }
    for d in dirs {
        tokio::fs::File::open(&d)
            .await
            .map_err(|e| {
                Error::io(format!(
                    "unable to open directory {} for sync: {}",
                    d.to_string_lossy(),
                    e
                ))
            })?
            .sync_all()
            .await
            .map_err(|e| {
                Error::io(format!(
                    "unable to sync directory {}: {}",
                    d.to_string_lossy(),
                    e
                ))
            })?;
    }
    Ok(())
}

async fn truncate_last_line(f: &mut tokio::fs::File) -> Result<(), std::io::Error> {
    let mut pos = f.stream_position().await?;
    loop {
        if pos < 2 {
            f.set_len(0).await?;
            break;
        }
        pos = f.seek(SeekFrom::Current(-2)).await?;
        let mut buf = [0u8; 1];
        f.read_exact(&mut buf).await?;
        if buf[0] == b'\n' {
            f.set_len(pos + 1).await?;
            break;
        }
    }
    f.flush().await?;
    Ok(())
}

async fn truncate_if_broken(f: &mut tokio::fs::File) -> Result<(), std::io::Error> {
    let pos = f.stream_position().await?;
    if pos > 0 {
        f.seek(SeekFrom::End(-1)).await?;
        let mut buf = [0u8; 1];
        f.read_exact(&mut buf).await?;
        if buf[0] != b'\n' {
            warn!("output file broken, removing the last line");
            truncate_last_line(f).await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn file_handler(
    rx: &async_channel::Receiver<Event>,
    base_file_path: &str,
    base_rotated_path: Option<&str>,
    gen: &DataGen,
    auto_flush: bool,
) -> EResult<()> {
    macro_rules! explain_io {
        ($msg: expr, $err: expr) => {
            Error::io(format!("{} {}", $msg, $err))
        };
    }
    let mut dt = Local::now();
    let mut file_path = generate_file_path(base_file_path, dt);
    let mut fx: Option<tokio::fs::File> = None;
    while let Ok(event) = rx.recv().await {
        let mut fd_opt = None;
        if let Some(ref mut existing_fh) = fx {
            if let Ok(path_metadata) = tokio::fs::metadata(&file_path).await {
                let metadata = existing_fh
                    .metadata()
                    .await
                    .map_err(|e| explain_io!("unable to get metadata", e))?;
                if metadata.dev() == path_metadata.dev() && metadata.ino() == path_metadata.ino() {
                    fd_opt = Some(existing_fh);
                }
            }
        }
        let fd = if let Some(fd) = fd_opt {
            fd
        } else {
            let mut f = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .truncate(false)
                .append(true)
                .create(true)
                .open(&file_path)
                .await
                .map_err(|e| explain_io!("unable to create file", e))?;
            sync_dirs(&[&file_path]).await?;
            f.seek(SeekFrom::End(0))
                .await
                .map_err(|e| explain_io!("unable to seek file from end", e))?;
            truncate_if_broken(&mut f)
                .await
                .map_err(|e| explain_io!("unable to truncate last line", e))?;
            fx.replace(f);
            dt = Local::now();
            fx.as_mut().unwrap()
        };
        let pos = fd
            .seek(SeekFrom::Current(0))
            .await
            .map_err(|e| explain_io!("unable to seek file from current", e))?;
        if pos == 0 {
            if let Some(header) = gen.header() {
                fd.write_all(&header)
                    .await
                    .map_err(|e| explain_io!("unable to write file header", e))?;
            }
        }
        match event {
            Event::State(state) => {
                let line = gen.line(state)?;
                if !can_write_line(&line) {
                    continue;
                }
                fd.write_all(&line)
                    .await
                    .map_err(|e| explain_io!("unable to write state data", e))?;
            }
            Event::BulkState(states) => {
                let mut buf = Vec::new();
                for state in states {
                    let line = gen.line(state)?;
                    buf.extend(line);
                }
                if buf.is_empty() {
                    continue;
                }
                fd.write_all(&buf)
                    .await
                    .map_err(|e| explain_io!("unable to write bulk state data", e))?;
            }
            Event::Quit => {
                fd.flush()
                    .await
                    .map_err(|e| explain_io!("unable to flush", e))?;
                break;
            }
            Event::Flush => {
                fd.flush()
                    .await
                    .map_err(|e| explain_io!("unable to flush", e))?;
                continue;
            }
            Event::Rotate => {
                fd.flush()
                    .await
                    .map_err(|e| explain_io!("unable to flush", e))?;
                fx.take();
                if let Some(path) = base_rotated_path {
                    let rotated_path = generate_file_path(path, dt);
                    tokio::fs::rename(&file_path, &rotated_path)
                        .await
                        .map_err(|e| {
                            explain_io!(
                                format!("unable to rename {} to {}", file_path, rotated_path),
                                e
                            )
                        })?;
                    sync_dirs(&[&file_path, &rotated_path]).await?;
                    file_path = generate_file_path(base_file_path, dt);
                } else if base_file_path.contains('%') {
                    file_path = generate_file_path(base_file_path, dt);
                } else {
                    let rotated_path = format!(
                        "{}.{}",
                        &file_path,
                        dt.to_rfc3339_opts(SecondsFormat::Secs, false)
                    );
                    tokio::fs::rename(&file_path, &rotated_path)
                        .await
                        .map_err(|e| {
                            explain_io!(
                                format!("unable to rename {} to {}", file_path, rotated_path),
                                e
                            )
                        })?;
                    sync_dirs(&[&file_path, &rotated_path]).await?;
                }
                continue;
            }
        }
        if auto_flush {
            fd.flush()
                .await
                .map_err(|e| explain_io!("unable to flush", e))?;
        }
    }
    Ok(())
}

struct DataGen {
    format: DataFormat,
    dos_cr: bool,
    fields: Vec<FieldKind>,
}

impl DataGen {
    #[inline]
    fn new(format: DataFormat, dos_cr: bool, fields: Option<Vec<FieldKind>>) -> Self {
        Self {
            format,
            dos_cr,
            fields: fields.unwrap_or_else(|| default_fields(format)),
        }
    }
    #[inline]
    fn header(&self) -> Option<Vec<u8>> {
        match self.format {
            DataFormat::Json => None,
            DataFormat::Csv => Some(
                self.prepare(
                    self.fields
                        .iter()
                        .map(|v| v.default_name(self.format))
                        .collect::<Vec<&str>>()
                        .join(","),
                ),
            ),
        }
    }
    #[inline]
    fn line(&self, state: ItemState) -> EResult<Vec<u8>> {
        match self.format {
            DataFormat::Json => {
                let mut data = BTreeMap::new();
                for field in &self.fields {
                    data.insert(
                        field.default_name(DataFormat::Json),
                        field.data_value_from(&state),
                    );
                }
                Ok(self.prepare(serde_json::to_string(&data)?))
            }
            DataFormat::Csv => {
                let mut s = String::new();
                for field in &self.fields {
                    if !s.is_empty() {
                        write!(s, ",")?;
                    }
                    field.write_csv_string_from(&mut s, &state)?;
                }
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
        let mut states: Vec<ShortItemStateConnected> = unpack(data.payload())?;
        let skip_disconnected = SKIP_DISCONNECTED.load(atomic::Ordering::Relaxed);
        states.retain(|s| !skip_disconnected || s.connected);
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
    #[allow(deprecated)]
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
    SKIP_DISCONNECTED.store(config.skip_disconnected, atomic::Ordering::Relaxed);
    let (tx, rx) = async_channel::bounded::<Event>(config.queue_size);
    let file_path = config.file_path;
    let rotated_path = config.rotated_path;
    let auto_flush = config.auto_flush;
    let gen = DataGen::new(config.format, config.dos_cr, config.fields);
    if let Some(dedup_lines) = config.dedup_lines {
        NEED_DEDUP_LINES.store(true, atomic::Ordering::Relaxed);
        let d = Duration::from_secs(dedup_lines);
        tokio::spawn(async move {
            let mut int = tokio::time::interval(DEDUP_CLEANER_INTERVAL);
            loop {
                int.tick().await;
                clean_dedup_cache(d);
            }
        });
    }
    let fh_fut = tokio::spawn(async move {
        loop {
            if file_handler(&rx, &file_path, rotated_path.as_deref(), &gen, auto_flush)
                .await
                .log_err()
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
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
                tokio::time::sleep(Duration::from_secs(1)).await;
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
    tx.send(Event::Quit).await.map_err(Error::failed)?;
    fh_fut.await?;
    Ok(())
}
