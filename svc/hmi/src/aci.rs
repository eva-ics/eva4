use crate::aaa::{Auth, Token};
use crate::db;
use chrono::{DateTime, Local, NaiveDateTime, SecondsFormat, Utc};
use eva_common::acl::Acl;
use eva_common::err_logger;
use eva_common::prelude::*;
use futures::TryStreamExt;
use log::{error, trace};
use serde::{ser::SerializeMap, Serialize, Serializer};
use sqlx::Row;
use std::sync::atomic;
use std::time::Duration;
use std::time::Instant;
use uuid::Uuid;

err_logger!();

const API_LOG_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
static NEED_API_LOG: atomic::AtomicBool = atomic::AtomicBool::new(false);

#[inline]
fn api_log_enabled() -> bool {
    NEED_API_LOG.load(atomic::Ordering::SeqCst)
}

#[derive(Serialize)]
pub struct ApiCallInfo {
    id: String,
    dt: String,
    auth: String,
    user: Option<String>,
    acl: String,
    source: String,
    method: String,
    code: i16,
    msg: Option<String>,
    elapsed: Option<f64>,
    t: i64,
}

pub async fn log_get() -> EResult<Vec<ApiCallInfo>> {
    if api_log_enabled() {
        let mut result = Vec::new();
        let mut rows = db::api_log_get()?;
        while let Some(row) = rows.try_next().await? {
            let t: i64 = row.try_get("t")?;
            let code: i64 = row.try_get("code")?;
            let dt_utc = DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(t, 0), Utc);
            let dt: DateTime<Local> = DateTime::from(dt_utc);
            let call_info = ApiCallInfo {
                id: row.try_get("id")?,
                dt: dt.to_rfc3339_opts(SecondsFormat::Secs, false),
                auth: row.try_get("auth")?,
                user: row.try_get("login")?,
                acl: row.try_get("acl")?,
                source: row.try_get("source")?,
                method: row.try_get("method")?,
                code: i16::try_from(code).map_err(Error::invalid_data)?,
                msg: row.try_get("msg")?,
                elapsed: row.try_get("elapsed")?,
                t,
            };
            result.push(call_info);
        }
        Ok(result)
    } else {
        Err(Error::not_ready("API call log not configured"))
    }
}

pub async fn start(keep_api_log: u32) -> EResult<()> {
    NEED_API_LOG.store(keep_api_log > 0, atomic::Ordering::SeqCst);
    if keep_api_log > 0 {
        eva_common::cleaner!(
            "api_log",
            clear_api_log,
            API_LOG_CLEANUP_INTERVAL,
            keep_api_log
        );
    }
    Ok(())
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug)]
pub struct ACI {
    id: Uuid,
    id_str: String,
    auth: Auth,
    source: String,
    method: String,
    log_level: log::Level,
    t: Instant,
}

impl Serialize for ACI {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("auth", self.auth.as_str())?;
        if let Auth::Token(ref token) = self.auth {
            map.serialize_entry("token_mode", token.mode_as_str())?;
            map.serialize_entry("u", token.user())?;
        } else {
            map.serialize_entry("token_mode", &Value::Unit)?;
            map.serialize_entry("u", &Value::Unit)?;
        }
        map.serialize_entry("acl", &self.auth.acl().id())?;
        map.end()
    }
}

impl ACI {
    pub fn new(auth: Auth, method: &str, source: String) -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            id_str: id.to_string(),
            auth,
            source,
            method: method.to_owned(),
            log_level: log::Level::Info,
            t: Instant::now(),
        }
    }
    #[inline]
    pub fn writable(&self) -> bool {
        if let Auth::Token(ref token) = self.auth {
            if token.is_readonly() {
                return false;
            }
        }
        true
    }
    #[inline]
    pub fn check_write(&self) -> EResult<()> {
        if self.writable() {
            Ok(())
        } else {
            Err(Error::access("Session is in the read-only mode"))
        }
    }
    pub async fn log_request(&mut self, level: log::Level) -> EResult<()> {
        self.log_level = level;
        log::log!(
            self.log_level,
            "API request {} {}@{} {}",
            self.id_str,
            self.auth,
            self.source,
            self.method
        );
        if api_log_enabled() && level <= log::Level::Info {
            if let Err(e) =
                db::api_log_insert(&self.id_str, &self.auth, &self.source, &self.method).await
            {
                error!("db log error: {}", e);
            }
        }
        Ok(())
    }
    pub async fn log_success(&self) {
        #[allow(clippy::cast_precision_loss)]
        let elapsed = self.t.elapsed().as_millis() as f64 / 1000.0;
        log::log!(
            self.log_level,
            "API request {} successful ({} sec)",
            self.id,
            elapsed
        );
        if api_log_enabled() && self.log_level <= log::Level::Info {
            if let Err(e) = db::api_log_mark_success(&self.id_str, elapsed).await {
                error!("db log error: {}", e);
            }
        }
    }
    pub async fn log_error(&self, err: &Error) {
        #[allow(clippy::cast_precision_loss)]
        let elapsed = self.t.elapsed().as_millis() as f64 / 1000.0;
        error!("API request {} failed. {} ({} sec)", self.id, err, elapsed);
        if api_log_enabled() && self.log_level <= log::Level::Info {
            if let Err(e) = db::api_log_mark_error(&self.id_str, elapsed, err).await {
                error!("db log error: {}", e);
            }
        }
    }
    #[inline]
    pub fn acl(&self) -> &Acl {
        self.auth.acl()
    }
    #[inline]
    pub fn token(&self) -> Option<&Token> {
        self.auth.token()
    }
}

async fn clear_api_log(keep: u32) {
    do_clear_api_log(keep).await.log_ef();
}

async fn do_clear_api_log(keep: u32) -> EResult<()> {
    trace!("cleaning up API call log");
    if let Err(e) = db::api_log_clear(keep).await {
        error!("db log cleaner error: {}", e);
    }
    Ok(())
}
