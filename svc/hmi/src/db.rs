use crate::aaa::{Auth, Token, TokenId};
use eva_common::err_logger;
use eva_common::prelude::*;
use eva_common::time::now;
use eva_internal::DbKind;
use futures::TryStreamExt;
use log::error;
use serde::{Deserialize, Serialize};
use sqlx::AnyPool;
use sqlx::{
    ConnectOptions, Row,
    any::{AnyConnectOptions, AnyPoolOptions, AnyRow},
};
use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::OnceLock;
use std::sync::atomic;
use std::time::Duration;

err_logger!();

pub(crate) static DB_POOL: OnceLock<AnyPool> = OnceLock::new();
static DB_KIND: OnceLock<DbKind> = OnceLock::new();
static MAX_USER_DATA_RECORDS: atomic::AtomicU32 = atomic::AtomicU32::new(0);
static MAX_USER_DATA_RECORD_LEN: atomic::AtomicU32 = atomic::AtomicU32::new(0);

#[inline]
pub fn set_max_user_data_records(max_records: u32) {
    MAX_USER_DATA_RECORDS.store(max_records, atomic::Ordering::Relaxed);
}

#[inline]
pub fn set_max_user_data_record_len(max_len: u32) {
    MAX_USER_DATA_RECORD_LEN.store(max_len, atomic::Ordering::Relaxed);
}

#[inline]
pub fn max_user_data_records() -> u32 {
    MAX_USER_DATA_RECORDS.load(atomic::Ordering::Relaxed)
}

#[inline]
pub fn max_user_data_record_len() -> u32 {
    MAX_USER_DATA_RECORD_LEN.load(atomic::Ordering::Relaxed)
}

#[derive(Deserialize, Default)]
pub struct ApiLogFilter {
    t_start: Option<Value>,
    t_end: Option<Value>,
    pub(crate) user: Option<String>,
    acl: Option<String>,
    method: Option<String>,
    oid: Option<OID>,
    note: Option<String>,
    source: Option<String>,
    code: Option<i64>,
    success: Option<bool>,
}

fn quoted_sql_str(s: &str) -> EResult<String> {
    for ch in s.chars() {
        if ch == '\'' {
            return Err(Error::invalid_data("invalid character in request"));
        }
    }
    Ok(format!("'{}'", s))
}

impl ApiLogFilter {
    pub fn condition(&self) -> EResult<String> {
        let mut cond = "WHERE 1".to_owned();
        if let Some(ref t_start) = self.t_start {
            write!(cond, " AND t>={}", t_start.as_timestamp()?)?;
        }
        if let Some(ref t_end) = self.t_end {
            write!(cond, " AND t<={}", t_end.as_timestamp()?)?;
        }
        if let Some(ref user) = self.user {
            write!(cond, " AND login={}", quoted_sql_str(user)?)?;
        }
        if let Some(ref acl) = self.acl {
            write!(cond, " AND acl={}", quoted_sql_str(acl)?)?;
        }
        if let Some(ref method) = self.method {
            write!(cond, " AND method={}", quoted_sql_str(method)?)?;
        }
        if let Some(ref oid) = self.oid {
            write!(cond, " AND oid={}", quoted_sql_str(oid.as_str())?)?;
        }
        if let Some(ref note) = self.note {
            write!(cond, " AND note={}", quoted_sql_str(note.as_str())?)?;
        }
        if let Some(ref source) = self.source {
            write!(cond, " AND source={}", quoted_sql_str(source)?)?;
        }
        if let Some(code) = self.code {
            write!(cond, " AND code={}", code)?;
        }
        if let Some(success) = self.success {
            if success {
                write!(cond, " AND code=0")?;
            } else {
                write!(cond, " AND code!=0")?;
            }
        }
        Ok(cond)
    }
}

pub fn api_log_query(filter: &ApiLogFilter) -> EResult<String> {
    //let pool = DB_POOL.get().unwrap();
    let q = format!(
        r"SELECT id, t, auth, login, acl, source, method, code, msg, elapsed, params, oid, note
            FROM api_log {} ORDER BY t",
        filter.condition()?
    );
    Ok(q)
}

pub async fn api_log_clear(keep: u32) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let keep_from = i64::try_from(now() - u64::from(keep)).map_err(Error::failed)?;
    match kind {
        DbKind::Sqlite => {
            sqlx::query("DELETE FROM api_log WHERE t<?")
                .bind(keep_from)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("DELETE FROM api_log WHERE t<$1")
                .bind(keep_from)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

#[allow(clippy::similar_names)]
pub async fn api_log_insert<T>(
    id_str: &str,
    auth: &Auth,
    source: &str,
    method: &str,
    params: Option<T>,
    oid: Option<OID>,
    note: Option<&str>,
) -> EResult<()>
where
    T: Serialize,
{
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let oid_str: Option<&str> = oid.as_ref().map(OID::as_str);
    match kind {
        DbKind::Sqlite => {
            sqlx::query(
                r"INSERT INTO api_log (id, t, login, auth, acl, source, method, params, oid, note)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id_str)
            .bind(i64::try_from(now()).map_err(Error::failed)?)
            .bind(auth.user())
            .bind(auth.as_str())
            .bind(auth.acl_id())
            .bind(source)
            .bind(method)
            .bind(if let Some(ref p) = params {
                Some(serde_json::to_string(p)?)
            } else {
                None
            })
            .bind(oid_str)
            .bind(note)
            .execute(pool)
            .await?;
        }
        DbKind::Postgres => {
            sqlx::query(
                r"INSERT INTO api_log (id, t, login, auth, acl, source, method, params, oid, note)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(id_str)
            .bind(i64::try_from(now()).map_err(Error::failed)?)
            .bind(auth.user())
            .bind(auth.as_str())
            .bind(auth.acl_id())
            .bind(source)
            .bind(method)
            .bind(if let Some(ref p) = params {
                Some(serde_json::to_string(p)?)
            } else {
                None
            })
            .bind(oid_str)
            .bind(note)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn api_log_mark_success(id_str: &str, elapsed: f64) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("UPDATE api_log set code=0, elapsed=? WHERE id=?")
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("UPDATE api_log set code=0, elapsed=$1 WHERE id=$2")
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn api_log_mark_error(id_str: &str, elapsed: f64, err: &Error) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("UPDATE api_log set code=?, msg=?, elapsed=? WHERE id=?")
                .bind(i64::from(err.kind() as i16))
                .bind(err.message())
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("UPDATE api_log set code=$1, msg=$2, elapsed=$3 WHERE id=$4")
                .bind(i64::from(err.kind() as i16))
                .bind(err.message())
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn clear_idle_tokens(expiration: Duration) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let t = i64::try_from(now() - expiration.as_secs()).map_err(Error::failed)?;
    match kind {
        DbKind::Sqlite => {
            sqlx::query("DELETE FROM tokens WHERE t<?")
                .bind(t)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("DELETE FROM tokens WHERE t<$1")
                .bind(t)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn clear_token_acls() -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        DbKind::Sqlite | DbKind::Postgres => {
            sqlx::query("DELETE FROM token_acls WHERE token_id NOT IN (SELECT id FROM tokens)")
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn clear_tokens_by_user(user: &str) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("DELETE FROM tokens WHERE login=?")
                .bind(user)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("DELETE FROM tokens WHERE login=$1")
                .bind(user)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn delete_token(token_id: &TokenId) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token_id.to_short_string();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("DELETE FROM tokens WHERE id=?")
                .bind(&token_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM token_acls WHERE token_id=?")
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("DELETE FROM tokens WHERE id=$1")
                .bind(&token_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM token_acls WHERE token_id=$1")
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn set_token_time(token_id: &TokenId, t: i64) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token_id.to_short_string();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("UPDATE tokens SET t=? WHERE id=?")
                .bind(t)
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("UPDATE tokens SET t=$1 WHERE id=$2")
                .bind(t)
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn set_token_mode(token_id: &TokenId, mode: u8) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token_id.to_short_string();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("UPDATE tokens SET md=? WHERE id=?")
                .bind(i64::from(mode))
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("UPDATE tokens SET md=$1 WHERE id=$2")
                .bind(i64::from(mode))
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

fn parse_token(row: AnyRow, token_id: Option<TokenId>) -> EResult<Token> {
    let mode: i64 = row.try_get("md")?;
    let t: i64 = row.try_get("t")?;
    let user: String = row.try_get("login")?;
    let acl: String = row.try_get("acl")?;
    let auth_svc: String = row.try_get("auth_svc")?;
    let ip: Option<String> = row.try_get("ip")?;
    let id = if let Some(id) = token_id {
        id
    } else {
        let id: String = row.try_get("id")?;
        id.as_str().try_into()?
    };
    Ok(Token::new(
        id,
        user,
        serde_json::from_str(&acl)?,
        auth_svc,
        if let Some(i) = ip {
            Some(i.parse().map_err(Error::invalid_data)?)
        } else {
            None
        },
        Some(u8::try_from(mode).map_err(Error::invalid_data)?),
        Some(t),
    ))
}

#[inline]
fn parse_token_id(row: AnyRow) -> EResult<TokenId> {
    let id: String = row.try_get("id")?;
    id.as_str().try_into()
}

pub async fn is_active_session_for(login: &str) -> EResult<bool> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut rows = match kind {
        DbKind::Sqlite => sqlx::query("SELECT COUNT(id) FROM tokens WHERE login=?")
            .bind(login)
            .fetch(pool),
        DbKind::Postgres => sqlx::query("SELECT COUNT(id) FROM tokens WHERE login=$1")
            .bind(login)
            .fetch(pool),
    };
    if let Some(row) = rows.try_next().await? {
        let count: i64 = row.try_get(0)?;
        Ok(count > 0)
    } else {
        Ok(false)
    }
}

pub async fn load_all_tokens() -> EResult<Vec<Token>> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut result = Vec::new();
    let mut rows = match kind {
        DbKind::Sqlite | DbKind::Postgres => {
            sqlx::query("SELECT id, md, t, login, acl, auth_svc, ip FROM tokens ORDER BY t DESC")
                .fetch(pool)
        }
    };
    while let Some(row) = rows.try_next().await? {
        match parse_token(row, None) {
            Ok(v) => result.push(v),
            Err(e) => error!("token parse error: {}", e),
        }
    }
    Ok(result)
}

pub async fn load_token_ids() -> EResult<Vec<TokenId>> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut result = Vec::new();
    let mut rows = match kind {
        DbKind::Sqlite | DbKind::Postgres => sqlx::query("SELECT id FROM tokens").fetch(pool),
    };
    while let Some(row) = rows.try_next().await? {
        match parse_token_id(row) {
            Ok(v) => result.push(v),
            Err(e) => error!("token id parse error: {}", e),
        }
    }
    Ok(result)
}

pub async fn load_token(token_id: TokenId) -> EResult<Option<Token>> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id_str = token_id.to_short_string();
    let mut rows = match kind {
        DbKind::Sqlite => {
            sqlx::query("SELECT md, t, login, acl, auth_svc, ip FROM tokens WHERE id=?")
                .bind(token_id_str)
                .fetch(pool)
        }
        DbKind::Postgres => {
            sqlx::query("SELECT md, t, login, acl, auth_svc, ip FROM tokens WHERE id=$1")
                .bind(token_id_str)
                .fetch(pool)
        }
    };
    if let Some(row) = rows.try_next().await? {
        Ok(Some(parse_token(row, Some(token_id))?))
    } else {
        Ok(None)
    }
}

pub async fn save_token(token: &Token) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token.id().to_short_string();
    let token_acl = serde_json::to_string(token.acl())?;
    match kind {
        DbKind::Sqlite => {
            sqlx::query(
                r"INSERT INTO tokens
                (id, md, t, login, acl, auth_svc, ip)
                VALUES
                (?, ?, ?, ?, ?, ?, ?)
                 ",
            )
            .bind(&token_id)
            .bind(i64::from(token.mode()))
            .bind(token.time())
            .bind(token.user())
            .bind(token_acl)
            .bind(token.auth_svc())
            .bind(token.ip().map(ToString::to_string))
            .execute(pool)
            .await?;
            for acl_id in token.acl().from() {
                sqlx::query(
                    r"INSERT INTO token_acls
                    (token_id, acl_id)
                    VALUES
                    (?, ?)
                     ",
                )
                .bind(&token_id)
                .bind(acl_id)
                .execute(pool)
                .await?;
            }
        }
        DbKind::Postgres => {
            sqlx::query(
                r"INSERT INTO tokens
                (id, md, t, login, acl, auth_svc, ip)
                VALUES
                ($1, $2, $3, $4, $5, $6, $7)
                 ",
            )
            .bind(&token_id)
            .bind(i64::from(token.mode()))
            .bind(token.time())
            .bind(token.user())
            .bind(token_acl)
            .bind(token.auth_svc())
            .bind(token.ip().map(ToString::to_string))
            .execute(pool)
            .await?;
            for acl_id in token.acl().from() {
                sqlx::query(
                    r"INSERT INTO token_acls
                    (token_id, acl_id)
                    VALUES
                    ($1, $2)
                     ",
                )
                .bind(&token_id)
                .bind(acl_id)
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(())
}

pub async fn clear_tokens_by_acl_id(acl_id: &str) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        DbKind::Sqlite => {
            sqlx::query(
                "DELETE FROM tokens WHERE id IN (SELECT token_id FROM token_acls WHERE acl_id=?)",
            )
            .bind(acl_id)
            .execute(pool)
            .await?;
        }
        DbKind::Postgres => {
            sqlx::query(
                "DELETE FROM tokens WHERE id IN (SELECT token_id FROM token_acls WHERE acl_id=$1)",
            )
            .bind(acl_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn insert_ehmi_config(
    token_id: &str,
    config_key: &str,
    payload: impl Serialize,
) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        DbKind::Sqlite => {
            sqlx::query("INSERT INTO ehmi_configs(k, token_id, config, t) VALUES (?, ?, ?, ?)")
                .bind(config_key)
                .bind(token_id)
                .bind(serde_json::to_string(&payload)?)
                .bind(i64::try_from(now()).map_err(Error::failed)?)
                .execute(pool)
                .await?;
        }
        DbKind::Postgres => {
            sqlx::query("INSERT INTO ehmi_configs(k, token_id, config, t) VALUES ($1, $2, $3, $4)")
                .bind(config_key)
                .bind(token_id)
                .bind(serde_json::to_string(&payload)?)
                .bind(i64::try_from(now()).map_err(Error::failed)?)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn ehmi_config_token_id(config_key: &str) -> EResult<Option<String>> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut rows = match kind {
        DbKind::Sqlite => sqlx::query("SELECT token_id FROM ehmi_configs WHERE k = ?")
            .bind(config_key)
            .fetch(pool),
        DbKind::Postgres => sqlx::query("SELECT token_id FROM ehmi_configs WHERE k = $1")
            .bind(config_key)
            .fetch(pool),
    };
    if let Some(row) = rows.try_next().await? {
        Ok(Some(row.try_get(0)?))
    } else {
        Ok(None)
    }
}

pub async fn get_ehmi_config(config_key: &str) -> EResult<Value> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut rows = match kind {
        DbKind::Sqlite => sqlx::query("SELECT config FROM ehmi_configs WHERE k = ?")
            .bind(config_key)
            .fetch(pool),
        DbKind::Postgres => sqlx::query("SELECT config FROM ehmi_configs WHERE k = $1")
            .bind(config_key)
            .fetch(pool),
    };
    if let Some(row) = rows.try_next().await? {
        let config: String = row.try_get("config")?;
        Ok(serde_json::from_str(&config)?)
    } else {
        Err(Error::not_found("no such ehmi config"))
    }
}

pub async fn get_user_data(login: &str, key: &str) -> EResult<Value> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let q = match kind {
        DbKind::Sqlite => "SELECT v FROM user_data WHERE login = ? AND k = ?",
        DbKind::Postgres => "SELECT v FROM user_data WHERE login = $1 AND k = $2",
    };
    let mut rows = sqlx::query(q).bind(login).bind(key).fetch(pool);
    if let Some(row) = rows.try_next().await? {
        let v: Option<String> = row.try_get(0)?;
        Ok(v.map_or(Ok(Value::Unit), |s| serde_json::from_str(&s))?)
    } else {
        Err(Error::not_found("no such data record"))
    }
}

pub async fn delete_user_data(login: &str, key: &str) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let q = match kind {
        DbKind::Sqlite => "DELETE FROM user_data WHERE login = ? AND k = ?",
        DbKind::Postgres => "DELETE FROM user_data WHERE login = $1 AND k = $2",
    };
    sqlx::query(q).bind(login).bind(key).execute(pool).await?;
    Ok(())
}

pub async fn set_user_data(login: &str, key: &str, value: Value) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut tr = pool.begin().await?;
    match kind {
        DbKind::Sqlite => {
            // no need to lock
        }
        DbKind::Postgres => {
            // lock in share update
            sqlx::query("LOCK TABLE user_data IN SHARE UPDATE EXCLUSIVE MODE")
                .execute(&mut *tr)
                .await?;
        }
    }
    let q = match kind {
        DbKind::Sqlite => "SELECT COUNT(*) FROM user_data WHERE login = ?",
        DbKind::Postgres => "SELECT COUNT(*) FROM user_data WHERE login = $1",
    };
    let count: i64 = sqlx::query(q)
        .bind(login)
        .bind(key)
        .fetch_one(&mut *tr)
        .await?
        .try_get(0)?;
    if count >= i64::from(max_user_data_records()) {
        return Err(Error::access(format!(
            "user data records ({count}) exceed the limit"
        )));
    }
    let data_str = serde_json::to_string(&value)?;
    if data_str.len() > usize::try_from(max_user_data_record_len()).unwrap() {
        return Err(Error::access(format!(
            "the record length ({}) exceeds the limit",
            data_str.len()
        )));
    }
    let q = match kind {
        DbKind::Sqlite => "INSERT OR REPLACE INTO user_data(login, k, v) VALUES (?, ?, ?)",
        DbKind::Postgres => {
            r"INSERT INTO user_data(login, k, v) VALUES ($1, $2, $3)
ON CONFLICT ON CONSTRAINT user_data_pkey DO UPDATE SET v=$3"
        }
    };
    sqlx::query(q)
        .bind(login)
        .bind(key)
        .bind(data_str)
        .execute(&mut *tr)
        .await?;
    tr.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn init(conn: &str, pool_size: u32, timeout: Duration) -> EResult<()> {
    let kind = DbKind::from_str(conn)?;
    kind.create_if_missing(conn).await?;
    DB_KIND
        .set(kind)
        .map_err(|_| Error::core("unable to set db kind"))?;

    let opts = AnyConnectOptions::from_str(conn)?
        .log_statements(log::LevelFilter::Trace)
        .log_slow_statements(log::LevelFilter::Warn, Duration::from_secs(2));
    let pool = AnyPoolOptions::new()
        .max_connections(pool_size)
        .acquire_timeout(timeout)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                if kind == DbKind::Sqlite {
                    sqlx::query("PRAGMA synchronous = EXTRA; PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                        .execute(conn)
                        .await?;
                }
                Ok(())
            })
        })
        .connect_with(opts)
        .await?;
    match kind {
        DbKind::Sqlite => {
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        api_log(
            id CHAR(36),
            t INT,
            auth CHAR(5),
            login VARCHAR(64),
            acl VARCHAR(256),
            source VARCHAR(100),
            method VARCHAR(100),
            code INT,
            msg VARCHAR(256),
            elapsed REAL,
            params VARCHAR(4096),
            oid VARCHAR(1024),
            PRIMARY KEY(id)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS api_log_t ON api_log(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        tokens(
            id CHAR(48),
            md INT,
            t INT,
            login VARCHAR(256),
            acl VARCHAR(12000),
            auth_svc VARCHAR(256),
            ip VARCHAR(39),
            PRIMARY KEY(id)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS tokens_t ON tokens(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        token_acls(
            token_id CHAR(48),
            acl_id VARCHAR(256),
            PRIMARY KEY(token_id, acl_id)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS token_acls_acl_id ON token_acls(acl_id)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        ehmi_configs(
            k VARCHAR(128) NOT NULL,
            token_id CHAR(48) NOT NULL,
            config VARCHAR(16384),
            t INTEGER NOT NULL,
            PRIMARY KEY(k)
            FOREIGN KEY(token_id) REFERENCES tokens(id) ON DELETE CASCADE
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        user_data(
        login VARCHAR(64) NOT NULL,
        k VARCHAR(128) NOT NULL,
        v VARCHAR(16384) NOT NULL,
        PRIMARY KEY(login, k)
        )",
            )
            .execute(&pool)
            .await?;
        }
        DbKind::Postgres => {
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        api_log(
            id CHAR(36),
            t BIGINT,
            auth CHAR(5),
            login VARCHAR(64),
            acl VARCHAR(256),
            source VARCHAR(100),
            method VARCHAR(100),
            code BIGINT,
            msg VARCHAR(256),
            elapsed DOUBLE PRECISION,
            params VARCHAR(4096),
            oid VARCHAR(1024),
            PRIMARY KEY(id)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS api_log_t ON api_log(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        tokens(
            id CHAR(48),
            md BIGINT,
            t BIGINT,
            login VARCHAR(256),
            acl VARCHAR(12000),
            auth_svc VARCHAR(256),
            ip VARCHAR(39),
            PRIMARY KEY(id)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS tokens_t ON tokens(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        token_acls(
            token_id CHAR(48),
            acl_id VARCHAR(256),
            PRIMARY KEY(token_id, acl_id)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS token_acls_acl_id ON token_acls(acl_id)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        ehmi_configs(
            k VARCHAR(128),
            token_id CHAR(48) NOT NULL,
            config VARCHAR(16384),
            t BIGINT,
            PRIMARY KEY(k),
            FOREIGN KEY(token_id) REFERENCES tokens(id) ON DELETE CASCADE
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query(
                r"CREATE TABLE IF NOT EXISTS
        user_data(
        login VARCHAR(64) NOT NULL,
        k VARCHAR(128) NOT NULL,
        v VARCHAR(16384),
        PRIMARY KEY(login, k)
        )",
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS i_user_data_login ON user_data(login)")
                .execute(&pool)
                .await?;
        }
    }
    let _r = sqlx::query("ALTER TABLE api_log ADD oid VARCHAR(1024)")
        .execute(&pool)
        .await;
    let _r = sqlx::query("ALTER TABLE api_log ADD params VARCHAR(4096)")
        .execute(&pool)
        .await;
    let _r = sqlx::query("ALTER TABLE api_log ADD note VARCHAR(1024)")
        .execute(&pool)
        .await;
    DB_POOL
        .set(pool)
        .map_err(|_| Error::core("unable to set DB_POOL"))?;
    Ok(())
}
