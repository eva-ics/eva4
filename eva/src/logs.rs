use crate::{EResult, Error};
use busrt::rpc::{Rpc, RpcClient};
use busrt::QoS;
use bytes::{BufMut, BytesMut};
use chrono::prelude::*;
use colored::Colorize;
use eva_common::events::LOG_EVENT_TOPIC;
use eva_common::payload::pack;
use eva_common::prelude::*;
use eva_common::tools::{format_path, SocketPath};
use log::trace;
use log::{Level, LevelFilter, Log, Metadata, Record};
use once_cell::sync::OnceCell;
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::HashMap;
use std::io::stdout;
use std::io::Write;
use std::str::FromStr;
use std::sync::atomic;
use std::sync::Arc;
use std::sync::Mutex;

const DEFAULT_KEEP: i64 = 86_400_i64;
const INTERVAL_CLEAN_MEMORY_LOGS: std::time::Duration = std::time::Duration::from_secs(60);
const DEFAULT_GET_RECORD_LIMIT: u32 = 50;

static CAN_LOG_CONSOLE: atomic::AtomicBool = atomic::AtomicBool::new(true);
static MIN_LOG_LEVEL: atomic::AtomicU8 = atomic::AtomicU8::new(eva_common::LOG_LEVEL_ERROR);

pub fn disable_console_log() {
    CAN_LOG_CONSOLE.store(false, atomic::Ordering::SeqCst);
}

type BusReceiver = async_channel::Receiver<(&'static String, Vec<u8>)>;

lazy_static! {
    static ref KEEP_MEM: OnceCell<chrono::Duration> = <_>::default();
    static ref KEEP_MEM_MAX_RECORDS: OnceCell<usize> = <_>::default();
    static ref MEMORY_LOG: Mutex<Vec<Arc<LogRecord>>> = Mutex::new(Vec::new());
    static ref CONSOLE_LOCK: Mutex<()> = Mutex::new(());
    static ref BUS_RPC: OnceCell<Arc<RpcClient>> = <_>::default();
    static ref BUS_RX: Mutex<Option<BusReceiver>> = <_>::default();
    static ref BUS_LOG_TOPIC: HashMap<Level, String> = {
        let mut h = HashMap::new();
        h.insert(Level::Trace, format!("{LOG_EVENT_TOPIC}trace"));
        h.insert(Level::Debug, format!("{LOG_EVENT_TOPIC}debug"));
        h.insert(Level::Info, format!("{LOG_EVENT_TOPIC}info"));
        h.insert(Level::Warn, format!("{LOG_EVENT_TOPIC}warn"));
        h.insert(Level::Error, format!("{LOG_EVENT_TOPIC}error"));
        h
    };
}

#[inline]
pub fn set_rpc(rpc: Arc<RpcClient>) -> EResult<()> {
    BUS_RPC
        .set(rpc)
        .map_err(|_| Error::core("unable to set RPC"))
}

#[derive(Debug)]
pub struct RecordFilter<'a> {
    level: LogLevel,
    module: Option<&'a str>,
    regex_filter: Option<&'a regex::Regex>,
    not_before: Option<chrono::DateTime<chrono::Local>>,
    limit: u32,
}

impl<'a> RecordFilter<'a> {
    pub fn new(
        level: Option<LogLevel>,
        module: Option<&'a str>,
        regex_filter: Option<&'a regex::Regex>,
        t: Option<u32>,
        limit: Option<u32>,
    ) -> Self {
        Self {
            level: level.unwrap_or(LogLevel(eva_common::LOG_LEVEL_INFO)),
            module,
            regex_filter,
            not_before: t.map(|v| Local::now() - chrono::Duration::seconds(i64::from(v))),
            limit: limit.unwrap_or(DEFAULT_GET_RECORD_LIMIT),
        }
    }
    #[inline]
    pub fn matches(&self, record: &LogRecord) -> bool {
        if record.l < self.level {
            return false;
        }
        if let Some(not_before) = self.not_before {
            if record.time < not_before {
                return false;
            }
        }
        if let Some(module) = self.module {
            if record.module != module {
                return false;
            }
        }
        if let Some(regex_filter) = self.regex_filter {
            if !regex_filter.is_match(&record.msg.to_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// # Panics
///
/// Will panic if the mutex is poisoned
pub fn get_log_records(filter: RecordFilter) -> Vec<Arc<LogRecord>> {
    let records = MEMORY_LOG
        .lock()
        .unwrap()
        .iter()
        .filter(|r| filter.matches(r))
        .cloned()
        .collect::<Vec<Arc<LogRecord>>>();
    let len = records.len();
    records
        .iter()
        .skip(if len > filter.limit as usize {
            len - filter.limit as usize
        } else {
            0
        })
        .map(|v| (*v).clone())
        .collect()
}

/// # Panics
///
/// Will panic if the mutex is poisoned
#[inline]
pub fn purge_log_records() {
    MEMORY_LOG.lock().unwrap().clear();
}

#[inline]
pub fn get_min_log_level() -> LogLevel {
    LogLevel(MIN_LOG_LEVEL.load(atomic::Ordering::SeqCst))
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct LogLevel(pub u8);

impl LogLevel {
    pub fn as_code(&self) -> u8 {
        self.0
    }
    pub fn as_str(&self) -> &str {
        #[allow(clippy::match_same_arms)]
        match self.0 {
            eva_common::LOG_LEVEL_TRACE => "trace",
            eva_common::LOG_LEVEL_DEBUG => "debug",
            eva_common::LOG_LEVEL_INFO => "info",
            eva_common::LOG_LEVEL_WARN => "warn",
            eva_common::LOG_LEVEL_ERROR => "error",
            // 3.x compat
            50 => "error",
            _ => "unknown",
        }
    }
}

impl FromStr for LogLevel {
    type Err = Error;
    fn from_str(level: &str) -> Result<Self, Self::Err> {
        match level.to_lowercase().as_str() {
            "trace" | "t" => Ok(LogLevel(eva_common::LOG_LEVEL_TRACE)),
            "debug" | "d" => Ok(LogLevel(eva_common::LOG_LEVEL_DEBUG)),
            "info" | "i" => Ok(LogLevel(eva_common::LOG_LEVEL_INFO)),
            "warning" | "warn" | "w" => Ok(LogLevel(eva_common::LOG_LEVEL_WARN)),
            "error" | "e" | "err" | "critical" | "crit" | "c" => {
                Ok(LogLevel(eva_common::LOG_LEVEL_ERROR))
            }
            _ => Err(Error::invalid_data(format!("Invalid log level: {}", level))),
        }
    }
}

impl From<Level> for LogLevel {
    fn from(l: Level) -> LogLevel {
        let level = match l {
            Level::Trace => eva_common::LOG_LEVEL_TRACE,
            Level::Debug => eva_common::LOG_LEVEL_DEBUG,
            Level::Info => eva_common::LOG_LEVEL_INFO,
            Level::Warn => eva_common::LOG_LEVEL_WARN,
            Level::Error => eva_common::LOG_LEVEL_ERROR,
        };
        LogLevel(level)
    }
}

impl From<LogLevel> for Level {
    fn from(l: LogLevel) -> Level {
        match l.0 {
            eva_common::LOG_LEVEL_TRACE => Level::Trace,
            eva_common::LOG_LEVEL_DEBUG => Level::Debug,
            //eva_common::LOG_LEVEL_INFO => Level::Info,
            eva_common::LOG_LEVEL_WARN => Level::Warn,
            eva_common::LOG_LEVEL_ERROR => Level::Error,
            _ => Level::Info,
        }
    }
}

trait LevelX {
    fn as_code(&self) -> u8;
    fn as_str_lower(&self) -> &str;
}

#[derive(bmart::tools::EnumStr)]
enum LogFormat {
    Regular,
    Json,
}

struct MemoryLogger {
    system_name: String,
    filter: LevelFilter,
}

impl MemoryLogger {
    fn new(system_name: &str, filter: LevelFilter) -> Self {
        Self {
            system_name: system_name.to_owned(),
            filter,
        }
    }
}

impl Log for MemoryLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.filter
    }
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            MEMORY_LOG
                .lock()
                .unwrap()
                .push(Arc::new(record.to_log_record(&self.system_name)));
        }
    }
    fn flush(&self) {}
}

#[allow(clippy::unused_async)]
async fn cleanup_memory_log(keep: Option<chrono::Duration>, max: Option<usize>) {
    trace!("cleaning up memory log records");
    let mut mem_log = MEMORY_LOG.lock().unwrap();
    if let Some(k) = keep {
        let t = Local::now();
        let not_before = t - k;
        mem_log.retain(|r| r.time > not_before);
    }
    if let Some(m) = max {
        let len = mem_log.len();
        if len > m {
            mem_log.drain(0..len - m);
        }
    }
}

struct ConsoleLogger {
    system_name: String,
    filter: LevelFilter,
    format: LogFormat,
}

impl ConsoleLogger {
    fn new(system_name: &str, filter: LevelFilter, format: LogFormat) -> Self {
        Self {
            system_name: system_name.to_owned(),
            filter,
            format,
        }
    }
}

macro_rules! format_log_string {
    ($record: expr, $system_name: expr, $format: expr) => {
        match $format {
            LogFormat::Regular => format!(
                "{} {}  {} {} {}",
                Local::now().to_rfc3339_opts(SecondsFormat::Secs, false),
                $system_name,
                $record.level(),
                $record.target(),
                $record.args()
            ),
            LogFormat::Json => $record.as_json(&$system_name),
        }
    };
}

impl Log for ConsoleLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.filter
            && !metadata.target().starts_with("busrt::")
            && !metadata.target().starts_with("mio::")
    }
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) && CAN_LOG_CONSOLE.load(atomic::Ordering::SeqCst) {
            let log_string = format_log_string!(record, self.system_name, self.format);
            let _console = CONSOLE_LOCK.lock().unwrap();
            match record.level() {
                Level::Trace => println!("{}", log_string.bright_black().dimmed()),
                Level::Debug => println!("{}", log_string.dimmed()),
                Level::Info => println!("{}", log_string.normal()),
                Level::Warn => eprintln!("{}", log_string.yellow().bold()),
                Level::Error => eprintln!("{}", log_string.red()),
            }
        }
    }

    fn flush(&self) {
        let _r = stdout().flush();
    }
}

struct BusLogger {
    system_name: String,
    filter: LevelFilter,
    tx: async_channel::Sender<(&'static String, Vec<u8>)>,
}

impl BusLogger {
    fn new(
        system_name: &str,
        tx: async_channel::Sender<(&'static String, Vec<u8>)>,
        filter: LevelFilter,
    ) -> Self {
        Self {
            system_name: system_name.to_owned(),
            filter,
            tx,
        }
    }
}

impl Log for BusLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.filter
            && !metadata.target().starts_with("busrt::")
            && !metadata.target().starts_with("mio::")
    }
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) && CAN_LOG_CONSOLE.load(atomic::Ordering::SeqCst) {
            if let Some(topic) = BUS_LOG_TOPIC.get(&record.level()) {
                let payload = record.as_msgpack(&self.system_name);
                let _r = self.tx.try_send((topic, payload));
            }
        }
    }

    fn flush(&self) {
        let _r = stdout().flush();
    }
}

struct FileLogger {
    system_name: String,
    path: String,
    filter: LevelFilter,
    format: LogFormat,
    fd: std::sync::Mutex<Option<std::fs::File>>,
}

impl FileLogger {
    fn new(system_name: &str, path: &str, filter: LevelFilter, format: LogFormat) -> Self {
        Self {
            system_name: system_name.to_owned(),
            path: path.to_owned(),
            filter,
            format,
            fd: std::sync::Mutex::new(None),
        }
    }
}

impl Log for FileLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.filter
    }
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            use std::os::unix::fs::MetadataExt;
            let mut fd_guard = self.fd.lock().unwrap();
            let mut fd_opt = None;
            if let Some(ref mut fd) = *fd_guard {
                if let Ok(path_metadata) = std::fs::metadata(&self.path) {
                    let metadata = fd.metadata().unwrap();
                    if metadata.dev() == path_metadata.dev()
                        && metadata.ino() == path_metadata.ino()
                    {
                        fd_opt = Some(fd);
                    }
                }
            }
            #[allow(clippy::option_if_let_else)]
            let fd = if let Some(fd) = fd_opt {
                fd
            } else {
                let fd = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .unwrap();
                fd_guard.replace(fd);
                fd_guard.as_mut().unwrap()
            };
            let log_string = format_log_string!(record, self.system_name, self.format);
            let mut buf = BytesMut::with_capacity(log_string.len() + 1);
            buf.put(log_string.as_bytes());
            buf.put_u8(0x0a);
            fd.write_all(&buf).unwrap();
        }
    }

    fn flush(&self) {
        if let Some(ref mut fd) = *self.fd.lock().unwrap() {
            fd.flush().unwrap();
        }
    }
}

struct SysLogger {
    logger: std::sync::Mutex<syslog::Logger<syslog::LoggerBackend, syslog::Formatter3164>>,
    filter: LevelFilter,
}

impl SysLogger {
    fn new(
        logger: syslog::Logger<syslog::LoggerBackend, syslog::Formatter3164>,
        filter: LevelFilter,
    ) -> Self {
        Self {
            logger: std::sync::Mutex::new(logger),
            filter,
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
impl TryFrom<Value> for LogLevel {
    type Error = Error;
    fn try_from(v: Value) -> EResult<LogLevel> {
        match v {
            Value::String(s) => s.parse(),
            n => {
                let level: u64 = n.try_into()?;
                Ok(LogLevel(level as u8))
            }
        }
    }
}

impl Log for SysLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.filter
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let message = record.args().to_string();
            let mut logger = self.logger.lock().unwrap();
            let _r = match record.level() {
                Level::Error => logger.err(message),
                Level::Warn => logger.warning(message),
                Level::Info => logger.info(message),
                Level::Debug | Level::Trace => logger.debug(message),
            };
        }
    }

    fn flush(&self) {}
}

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub time: chrono::DateTime<chrono::Local>,
    pub h: String,
    pub l: LogLevel,
    pub module: String,
    pub msg: String,
}

impl Serialize for LogRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let l = LogRecordS {
            time: self.time,
            h: &self.h,
            l: self.l,
            module: &self.module,
            msg: &self.msg,
        };
        l.serialize(serializer)
    }
}

struct LogRecordS<'a> {
    time: chrono::DateTime<chrono::Local>,
    h: &'a str,
    l: LogLevel,
    module: &'a str,
    msg: &'a str,
}

impl<'a> Serialize for LogRecordS<'a> {
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(8))?;
        map.serialize_entry(
            "dt",
            &self.time.to_rfc3339_opts(SecondsFormat::Millis, false),
        )?;
        map.serialize_entry("h", self.h)?;
        map.serialize_entry("l", &self.l.0)?;
        map.serialize_entry("lvl", self.l.as_str())?;
        map.serialize_entry("mod", self.module)?;
        map.serialize_entry("msg", self.msg)?;
        map.serialize_entry(
            "t",
            &(f64::from(self.time.timestamp() as u32)
                + f64::from(self.time.timestamp_subsec_micros() as u32) / 1_000_000.0),
        )?;
        map.serialize_entry("th", &None::<&str>)?;
        map.end()
    }
}

trait RecordX {
    fn as_json(&self, system_name: &str) -> String;
    fn as_msgpack(&self, system_name: &str) -> Vec<u8>;
    fn to_log_record(&self, system_name: &str) -> LogRecord;
}

impl<'a> RecordX for Record<'a> {
    #[inline]
    fn as_json(&self, system_name: &str) -> String {
        let record = LogRecordS {
            time: Local::now(),
            h: system_name,
            l: self.level().into(),
            module: self.target(),
            msg: &self.args().to_string(),
        };
        serde_json::to_string(&record).expect("Unable to serialize log record")
    }
    #[inline]
    fn as_msgpack(&self, system_name: &str) -> Vec<u8> {
        let record = LogRecordS {
            time: Local::now(),
            h: system_name,
            l: self.level().into(),
            module: self.target(),
            msg: &self.args().to_string(),
        };
        pack(&record).expect("Unable to serialize log record")
    }
    #[inline]
    fn to_log_record(&self, system_name: &str) -> LogRecord {
        LogRecord {
            time: Local::now(),
            h: system_name.to_owned(),
            l: self.level().into(),
            module: self.target().to_owned(),
            msg: self.args().to_string(),
        }
    }
}

fn parse_level(logger_config: &mut HashMap<String, Value>) -> LevelFilter {
    let level = logger_config.remove("level").map_or_else(
        || "info".to_owned(),
        |v| v.deserialize_into().expect("logger level is not a string"),
    );
    LevelFilter::from_str(&level).unwrap_or_else(|_| panic!("unsupported log level: {}", level))
}

fn parse_format(logger_config: &mut HashMap<String, Value>) -> LogFormat {
    let format = logger_config.remove("format").map_or_else(
        || "regular".to_owned(),
        |v| v.deserialize_into().expect("logger format is not a string"),
    );
    format.parse().unwrap_or_else(|e| {
        panic!("{}", e);
    })
}

fn parse_path(logger_config: &mut HashMap<String, Value>) -> Option<String> {
    logger_config.remove("path").map(|v| {
        v.deserialize_into()
            .expect("logger keep option is not an integer")
    })
}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::implicit_hasher)]
#[allow(clippy::too_many_lines)]
/// # Panics
///
/// Will panic on config errors
pub fn init(
    system_name: &str,
    process_name: &str,
    logs_config: Vec<HashMap<String, Value>>,
    dir_eva: &str,
    enable_bus: bool,
) {
    let mut loggers: Vec<Box<(dyn Log + 'static)>> = Vec::new();
    let mut max_filter = LevelFilter::Error;
    let mut keep_mem: Option<chrono::Duration> = None;
    let mut keep_mem_max_records: Option<usize> = None;
    let verbose_mode = std::env::var("VERBOSE").unwrap_or_default() == "1";
    let mut console_ready = false;
    let mut bus_configured = false;
    for mut logger_config in logs_config {
        macro_rules! parse_level {
            () => {{
                let level_filter = parse_level(&mut logger_config);
                if level_filter > max_filter {
                    max_filter = level_filter;
                }
                level_filter
            }};
        }
        let output: String = logger_config
            .remove("output")
            .expect("logger output not specified")
            .deserialize_into()
            .expect("logger output is not a string");
        match output.as_str() {
            "console" => {
                loggers.push(Box::new(ConsoleLogger::new(
                    system_name,
                    if verbose_mode {
                        max_filter = LevelFilter::Trace;
                        LevelFilter::Trace
                    } else {
                        parse_level!()
                    },
                    parse_format(&mut logger_config),
                )));
                console_ready = true;
            }
            "memory" => {
                assert!(
                    keep_mem.is_none(),
                    "Multiple memory loggers can not be defined"
                );
                let keep = logger_config.remove("keep").map_or(DEFAULT_KEEP, |v| {
                    v.deserialize_into()
                        .expect("logger keep option is not an integer")
                });
                keep_mem_max_records = logger_config
                    .remove("max_records")
                    .map(|v| v.deserialize_into().expect("max records is not an integer"));
                if keep > 0 {
                    loggers.push(Box::new(MemoryLogger::new(system_name, parse_level!())));
                    keep_mem = Some(chrono::Duration::seconds(keep));
                }
            }
            "bus" => {
                if enable_bus {
                    assert!(!bus_configured, "Multiple bus loggers can not be defined");
                    let (tx, rx) = async_channel::unbounded();
                    loggers.push(Box::new(BusLogger::new(system_name, tx, parse_level!())));
                    BUS_RX.lock().unwrap().replace(rx);
                    bus_configured = true;
                } else {
                    eprintln!("bus loggers disabled");
                }
            }
            "syslog" => {
                let path = parse_path(&mut logger_config);
                let mut formatter = syslog::Formatter3164 {
                    facility: syslog::Facility::LOG_USER,
                    hostname: None,
                    process: process_name.to_owned(),
                    pid: std::process::id(),
                };
                macro_rules! set_hostname {
                    ($f: expr) => {{
                        $f.hostname = Some(system_name.to_owned());
                        $f
                    }};
                }
                let logger = if let Some(p) = path {
                    match SocketPath::from_str(&p).unwrap() {
                        // TODO fix tcp
                        SocketPath::Tcp(ref v) => syslog::tcp(set_hostname!(formatter), v),
                        SocketPath::Udp(ref v) => {
                            syslog::udp(set_hostname!(formatter), "0.0.0.0:0", v)
                        }
                        SocketPath::Unix(ref v) => syslog::unix_custom(formatter, v),
                    }
                } else {
                    syslog::unix(formatter)
                };
                match logger {
                    Ok(l) => loggers.push(Box::new(SysLogger::new(l, parse_level!()))),
                    Err(e) => println!("Unable to init syslog logger: {}", e),
                }
            }
            "file" => {
                let path = format_path(
                    dir_eva,
                    Some(&parse_path(&mut logger_config).expect("log path not specified")),
                    None,
                );
                loggers.push(Box::new(FileLogger::new(
                    system_name,
                    &path,
                    parse_level!(),
                    parse_format(&mut logger_config),
                )));
            }
            v => eprintln!("logger output {} not implemented", v),
        }
    }
    if verbose_mode && !console_ready {
        loggers.push(Box::new(ConsoleLogger::new(
            system_name,
            LevelFilter::Trace,
            LogFormat::Regular,
        )));
        max_filter = LevelFilter::Trace;
    }
    if loggers.is_empty() {
        println!("{}", "logs not configured, running muted".yellow());
    } else {
        multi_log::MultiLogger::init(
            loggers,
            max_filter.to_level().expect("unable to set logging level"),
        )
        .expect("Unable to init logging system");
        MIN_LOG_LEVEL.store(
            Into::<LogLevel>::into(max_filter.to_level().unwrap()).as_code(),
            atomic::Ordering::SeqCst,
        );
        if let Some(keep) = keep_mem {
            KEEP_MEM.set(keep).expect("Unable to set KEEP_MEM");
        }
        if let Some(max) = keep_mem_max_records {
            KEEP_MEM_MAX_RECORDS
                .set(max)
                .expect("Unable to set KEEP_MEM_MAX_RECORDS");
        }
    }
}

/// # Panics
///
/// Will panic if the mutex is poisoned
pub async fn start_bus_logger() {
    if let Some(rx) = BUS_RX.lock().unwrap().take() {
        if let Some(rpc) = BUS_RPC.get() {
            let client = rpc.client();
            tokio::spawn(async move {
                while let Ok((topic, msg)) = rx.recv().await {
                    let _r = client
                        .lock()
                        .await
                        .publish(topic, msg.into(), QoS::No)
                        .await;
                }
            });
        }
    }
}

/// # Panics
///
/// Will panic if the mutex is poisoned
///
/// # Errors
///
/// Will return Err if failed to start workers
pub async fn start() -> EResult<()> {
    eva_common::cleaner!(
        "logmem",
        cleanup_memory_log,
        INTERVAL_CLEAN_MEMORY_LOGS,
        KEEP_MEM.get().map(Clone::clone),
        KEEP_MEM_MAX_RECORDS.get().map(Clone::clone)
    );
    Ok(())
}
