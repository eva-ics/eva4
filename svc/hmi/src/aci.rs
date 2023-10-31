use crate::aaa::{Auth, Token};
use crate::db;
use chrono::{DateTime, Local, NaiveDateTime, SecondsFormat, Utc};
use eva_common::acl::Acl;
use eva_common::err_logger;
use eva_common::prelude::*;
use futures::TryStreamExt;
use log::{error, trace};
use serde::Deserialize;
use serde::{ser::SerializeMap, Serialize, Serializer};
use sqlx::Row;
use std::collections::BTreeMap;
use std::sync::atomic;
use std::time::Duration;
use std::time::Instant;
use uuid::Uuid;

err_logger!();

const API_LOG_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
static NEED_API_LOG: atomic::AtomicBool = atomic::AtomicBool::new(false);

#[inline]
fn api_log_enabled() -> bool {
    NEED_API_LOG.load(atomic::Ordering::Relaxed)
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
    params: Option<Value>,
    t: i64,
}

pub async fn log_get(filter: &db::ApiLogFilter) -> EResult<Vec<ApiCallInfo>> {
    if api_log_enabled() {
        let mut result = Vec::new();
        let q = db::api_log_query(filter)?;
        let mut rows = sqlx::query(&q).fetch(db::DB_POOL.get().unwrap());
        while let Some(row) = rows.try_next().await? {
            let t: i64 = row.try_get("t")?;
            let code: i64 = row.try_get("code")?;
            #[allow(deprecated)]
            let dt_utc = DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(t, 0), Utc);
            let dt: DateTime<Local> = DateTime::from(dt_utc);
            let params: Option<String> = row.try_get("params")?;
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
                params: if let Some(p) = params {
                    let val: Option<serde_json::Value> = serde_json::from_str(&p)?;
                    if let Some(v) = val {
                        Some(Value::deserialize(v)?)
                    } else {
                        None
                    }
                } else {
                    None
                },
                t,
            };
            result.push(call_info);
        }
        Ok(result)
    } else {
        Err(Error::not_ready("API call log is not configured"))
    }
}

pub async fn start(keep_api_log: u32) -> EResult<()> {
    NEED_API_LOG.store(keep_api_log > 0, atomic::Ordering::Relaxed);
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
    params: Option<BTreeMap<String, Value>>,
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
        } else if let Auth::Key(_, _) = self.auth {
            map.serialize_entry("token_mode", &Value::Unit)?;
            map.serialize_entry("u", &self.auth.user())?;
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
            params: None,
            log_level: log::Level::Info,
            t: Instant::now(),
        }
    }
    #[inline]
    #[allow(dead_code)]
    pub fn user(&self) -> Option<&str> {
        if let Auth::Token(ref token) = self.auth {
            Some(token.user())
        } else if let Auth::Key(_, _) = self.auth {
            self.auth.user()
        } else {
            None
        }
    }
    /// set ACL ID for Auth::Login and Auth::LoginKey
    #[inline]
    pub fn set_acl_id(&mut self, acl_id: &str) {
        if let Auth::Login(_, ref mut a) = self.auth {
            a.replace(acl_id.to_owned());
        } else if let Auth::LoginKey(_, ref mut a) = self.auth {
            a.replace(acl_id.to_owned());
        }
    }
    /// set Key ID for Auth::LoginKey
    #[inline]
    pub fn set_key_id(&mut self, key_id: &str) {
        if let Auth::LoginKey(ref mut a, _) = self.auth {
            a.replace(key_id.to_owned());
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
    pub fn log_param<T>(&mut self, name: &str, val: T) -> EResult<()>
    where
        T: Serialize,
    {
        if let Some(ref mut p) = self.params {
            p.insert(name.to_owned(), to_value(val)?);
        } else {
            let mut p = BTreeMap::new();
            p.insert(name.to_owned(), to_value(val)?);
            self.params.replace(p);
        }
        Ok(())
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
            if let Err(e) = db::api_log_insert(
                &self.id_str,
                &self.auth,
                &self.source,
                &self.method,
                self.params.take(),
            )
            .await
            {
                error!("db log req error: {}", e);
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
                error!("db log mark success error: {}", e);
            }
        }
    }
    pub async fn log_error(&self, err: &Error) {
        #[allow(clippy::cast_precision_loss)]
        let elapsed = self.t.elapsed().as_millis() as f64 / 1000.0;
        error!("API request {} failed. {} ({} sec)", self.id, err, elapsed);
        if api_log_enabled() && self.log_level <= log::Level::Info {
            if let Err(e) = db::api_log_mark_error(&self.id_str, elapsed, err).await {
                error!("db log mark err error: {}", e);
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
