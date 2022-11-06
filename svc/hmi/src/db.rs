use crate::aaa::{Auth, Token, TokenId};
use eva_common::prelude::*;
use futures::TryStreamExt;
use lazy_static::lazy_static;
use log::error;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use sqlx::{
    any::{AnyConnectOptions, AnyKind, AnyPool, AnyPoolOptions, AnyRow},
    sqlite, ConnectOptions, Row,
};
use std::fmt::Write as _;
//use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

//type QueryRows = Pin<Box<dyn futures::Stream<Item = Result<AnyRow, sqlx::Error>> + Send>>;

lazy_static! {
    pub(crate) static ref DB_POOL: OnceCell<AnyPool> = <_>::default();
    static ref DB_KIND: OnceCell<AnyKind> = <_>::default();
}

macro_rules! not_impl {
    ($kind: expr) => {
        return Err(Error::not_implemented(format!(
            "database type {:?} is not supported",
            $kind
        )))
    };
}

#[derive(Deserialize, Default)]
pub struct ApiLogFilter {
    t_start: Option<Value>,
    t_end: Option<Value>,
    user: Option<String>,
    acl: Option<String>,
    method: Option<String>,
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
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql | AnyKind::Postgres => {
            let q = format!(
                r#"SELECT id, t, auth, login, acl, source, method, code, msg, elapsed
            FROM api_log {} ORDER BY t"#,
                filter.condition()?
            );
            Ok(dbg!(q))
        }
        AnyKind::Mssql => not_impl!(kind),
    }
}

pub async fn api_log_clear(keep: u32) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let keep_from =
        i64::try_from(eva_common::time::now() - u64::from(keep)).map_err(Error::failed)?;
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("DELETE FROM api_log WHERE t<?")
                .bind(keep_from)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("DELETE FROM api_log WHERE t<$1")
                .bind(keep_from)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn api_log_insert(id_str: &str, auth: &Auth, source: &str, method: &str) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query(
                r#"INSERT INTO api_log (id, t, login, auth, acl, source, method)
                VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(id_str)
            .bind(i64::try_from(eva_common::time::now()).map_err(Error::failed)?)
            .bind(auth.user())
            .bind(auth.as_str())
            .bind(auth.acl_id())
            .bind(source)
            .bind(method)
            .execute(pool)
            .await?;
        }
        AnyKind::Postgres => {
            sqlx::query(
                r#"INSERT INTO api_log (id, t, login, auth, acl, source, method)
                VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            )
            .bind(id_str)
            .bind(i64::try_from(eva_common::time::now()).map_err(Error::failed)?)
            .bind(auth.user())
            .bind(auth.as_str())
            .bind(auth.acl_id())
            .bind(source)
            .bind(method)
            .execute(pool)
            .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn api_log_mark_success(id_str: &str, elapsed: f64) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("UPDATE api_log set code=0, elapsed=? WHERE id=?")
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("UPDATE api_log set code=0, elapsed=$1 WHERE id=$2")
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn api_log_mark_error(id_str: &str, elapsed: f64, err: &Error) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("UPDATE api_log set code=?, msg=?, elapsed=? WHERE id=?")
                .bind(i64::from(err.kind() as i16))
                .bind(err.message())
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("UPDATE api_log set code=$1, msg=$2, elapsed=$3 WHERE id=$4")
                .bind(i64::from(err.kind() as i16))
                .bind(err.message())
                .bind(elapsed)
                .bind(id_str)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn clear_idle_tokens(expiration: Duration) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let t = i64::try_from(eva_common::time::now() - expiration.as_secs()).map_err(Error::failed)?;
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("DELETE FROM tokens WHERE t<?")
                .bind(t)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("DELETE FROM tokens WHERE t<$1")
                .bind(t)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn clear_token_acls() -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql | AnyKind::Postgres => {
            sqlx::query("DELETE FROM token_acls WHERE token_id NOT IN (SELECT id FROM tokens)")
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn clear_tokens_by_user(user: &str) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("DELETE FROM tokens WHERE login=?")
                .bind(user)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("DELETE FROM tokens WHERE login=$1")
                .bind(user)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn delete_token(token_id: &TokenId) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token_id.to_short_string();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("DELETE FROM tokens WHERE id=?")
                .bind(&token_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM token_acls WHERE token_id=?")
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("DELETE FROM tokens WHERE id=$1")
                .bind(&token_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM token_acls WHERE token_id=$1")
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn set_token_time(token_id: &TokenId, t: i64) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token_id.to_short_string();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("UPDATE tokens SET t=? WHERE id=?")
                .bind(t)
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("UPDATE tokens SET t=$1 WHERE id=$2")
                .bind(t)
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn set_token_mode(token_id: &TokenId, mode: u8) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let token_id = token_id.to_short_string();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("UPDATE tokens SET md=? WHERE id=?")
                .bind(i64::from(mode))
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        AnyKind::Postgres => {
            sqlx::query("UPDATE tokens SET md=$1 WHERE id=$2")
                .bind(i64::from(mode))
                .bind(&token_id)
                .execute(pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
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

pub async fn load_all_tokens() -> EResult<Vec<Token>> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    let mut result = Vec::new();
    let mut rows = match kind {
        AnyKind::Sqlite | AnyKind::MySql | AnyKind::Postgres => {
            sqlx::query("SELECT id, md, t, login, acl, auth_svc, ip FROM tokens ORDER BY t DESC")
                .fetch(pool)
        }
        AnyKind::Mssql => not_impl!(kind),
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
        AnyKind::Sqlite | AnyKind::MySql | AnyKind::Postgres => {
            sqlx::query("SELECT id FROM tokens").fetch(pool)
        }
        AnyKind::Mssql => not_impl!(kind),
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
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query("SELECT md, t, login, acl, auth_svc, ip FROM tokens WHERE id=?")
                .bind(token_id_str)
                .fetch(pool)
        }
        AnyKind::Postgres => {
            sqlx::query("SELECT md, t, login, acl, auth_svc, ip FROM tokens WHERE id=$1")
                .bind(token_id_str)
                .fetch(pool)
        }
        AnyKind::Mssql => not_impl!(kind),
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
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query(
                r#"INSERT INTO tokens
                (id, md, t, login, acl, auth_svc, ip)
                VALUES
                (?, ?, ?, ?, ?, ?, ?)
                 "#,
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
                    r#"INSERT INTO token_acls
                    (token_id, acl_id)
                    VALUES
                    (?, ?)
                     "#,
                )
                .bind(&token_id)
                .bind(acl_id)
                .execute(pool)
                .await?;
            }
        }
        AnyKind::Postgres => {
            sqlx::query(
                r#"INSERT INTO tokens
                (id, md, t, login, acl, auth_svc, ip)
                VALUES
                ($1, $2, $3, $4, $5, $6, $7)
                 "#,
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
                    r#"INSERT INTO token_acls
                    (token_id, acl_id)
                    VALUES
                    ($1, $2)
                     "#,
                )
                .bind(&token_id)
                .bind(acl_id)
                .execute(pool)
                .await?;
            }
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

pub async fn clear_tokens_by_acl_id(acl_id: &str) -> EResult<()> {
    let pool = DB_POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    match kind {
        AnyKind::Sqlite | AnyKind::MySql => {
            sqlx::query(
                "DELETE FROM tokens WHERE id IN (SELECT token_id FROM token_acls WHERE acl_id=?)",
            )
            .bind(acl_id)
            .execute(pool)
            .await?;
        }
        AnyKind::Postgres => {
            sqlx::query(
                "DELETE FROM tokens WHERE id IN (SELECT token_id FROM token_acls WHERE acl_id=$1)",
            )
            .bind(acl_id)
            .execute(pool)
            .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn init(conn: &str, pool_size: u32, timeout: Duration) -> EResult<()> {
    let mut opts = AnyConnectOptions::from_str(conn)?;
    opts.log_statements(log::LevelFilter::Trace)
        .log_slow_statements(log::LevelFilter::Warn, Duration::from_secs(2));
    if let Some(o) = opts.as_sqlite_mut() {
        opts = o
            .clone()
            .create_if_missing(true)
            .synchronous(sqlite::SqliteSynchronous::Extra)
            .busy_timeout(timeout)
            .into();
    }
    let kind = opts.kind();
    DB_KIND
        .set(kind)
        .map_err(|_| Error::core("unable to set db kind"))?;
    let pool = AnyPoolOptions::new()
        .max_connections(pool_size)
        .connect_timeout(timeout)
        .connect_with(opts)
        .await?;
    match kind {
        AnyKind::Sqlite => {
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
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
            PRIMARY KEY(id)
        )"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS api_log_t ON api_log(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
        tokens(
            id CHAR(48),
            md INT,
            t INT,
            login VARCHAR(256),
            acl VARCHAR(12000),
            auth_svc VARCHAR(256),
            ip VARCHAR(39),
            PRIMARY KEY(id)
        )"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS tokens_t ON tokens(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
        token_acls(
            token_id CHAR(48),
            acl_id VARCHAR(256),
            PRIMARY KEY(token_id, acl_id)
        )"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS token_acls_acl_id ON token_acls(acl_id)")
                .execute(&pool)
                .await?;
        }
        AnyKind::MySql => {
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
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
            PRIMARY KEY(id)
        )"#,
            )
            .execute(&pool)
            .await?;
            let _r = sqlx::query("CREATE INDEX api_log_t ON api_log(t)")
                .execute(&pool)
                .await;
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
        tokens(
            id CHAR(48),
            md INT,
            t INT,
            login VARCHAR(256),
            acl VARCHAR(12000),
            auth_svc VARCHAR(256),
            ip VARCHAR(39),
            PRIMARY KEY(id)
        )"#,
            )
            .execute(&pool)
            .await?;
            let _r = sqlx::query("CREATE INDEX tokens_t ON tokens(t)")
                .execute(&pool)
                .await;
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
        token_acls(
            token_id CHAR(48),
            acl_id VARCHAR(256),
            PRIMARY KEY(token_id, acl_id)
        )"#,
            )
            .execute(&pool)
            .await?;
            let _r = sqlx::query("CREATE INDEX token_acls_acl_id ON token_acls(acl_id)")
                .execute(&pool)
                .await;
        }
        AnyKind::Postgres => {
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
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
            PRIMARY KEY(id)
        )"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS api_log_t ON api_log(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
        tokens(
            id CHAR(48),
            md BIGINT,
            t BIGINT,
            login VARCHAR(256),
            acl VARCHAR(12000),
            auth_svc VARCHAR(256),
            ip VARCHAR(39),
            PRIMARY KEY(id)
        )"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS tokens_t ON tokens(t)")
                .execute(&pool)
                .await?;
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS
        token_acls(
            token_id CHAR(48),
            acl_id VARCHAR(256),
            PRIMARY KEY(token_id, acl_id)
        )"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS token_acls_acl_id ON token_acls(acl_id)")
                .execute(&pool)
                .await?;
        }
        AnyKind::Mssql => not_impl!(kind),
    }
    DB_POOL
        .set(pool)
        .map_err(|_| Error::core("unable to set DB_POOL"))?;
    Ok(())
}
