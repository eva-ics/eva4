use eva_common::prelude::*;
use eva_common::time::{ts_from_ns, ts_to_ns};
use eva_sdk::prelude::err_logger;
use eva_sdk::types::{HistoricalState, ItemState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use once_cell::sync::OnceCell;
use sqlx::{
    any::{AnyConnectOptions, AnyKind, AnyPool, AnyPoolOptions},
    sqlite, ConnectOptions, Row,
};
use std::fmt::Write as _;
use std::str::FromStr;
use std::time::Duration;

err_logger!();

pub static POOL: OnceCell<AnyPool> = OnceCell::new();
static DB_KIND: OnceCell<AnyKind> = OnceCell::new();

fn safe_sql_string(s: &str) -> EResult<&str> {
    for ch in s.chars() {
        if ch == '\'' {
            return Err(Error::invalid_data("unsupported quote in SQL string"));
        }
    }
    Ok(s)
}

pub async fn init(conn: &str, size: u32, timeout: Duration) -> EResult<()> {
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
        .max_connections(size)
        .acquire_timeout(timeout)
        .connect_with(opts)
        .await?;
    match kind {
        AnyKind::Sqlite => {
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS state_history
                (t INTEGER NOT NULL,
                oid VARCHAR(256) NOT NULL,
                status INTEGER,
                value VARCHAR(8192),
                PRIMARY KEY (t, oid))"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS state_history_i_t ON state_history(t)")
                .execute(&pool)
                .await?;
        }
        AnyKind::MySql | AnyKind::Postgres => {
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS state_history
                (t BIGINT NOT NULL,
                oid VARCHAR(256) NOT NULL,
                status INTEGER,
                value VARCHAR(8192),
                PRIMARY KEY (t, oid))"#,
            )
            .execute(&pool)
            .await?;
            sqlx::query("CREATE INDEX IF NOT EXISTS state_history_i_t ON state_history(t)")
                .execute(&pool)
                .await?;
        }
    }
    POOL.set(pool)
        .map_err(|_| Error::core("unable to set db pool"))
}

macro_rules! insert {
    ($x: expr, $kind: expr, $state: expr) => {{
        log::trace!("submitting state for {} at {}", $state.oid, $state.set_time);
        let q = match $kind {
            AnyKind::Sqlite => {
                "INSERT OR REPLACE INTO state_history(t,oid,status,value) VALUES(?,?,?,?)"
            }
            AnyKind::MySql => {
                "REPLACE INTO state_history(t,oid,status,value) VALUES(?,?,?,?)"
            }
            AnyKind::Postgres => {
                "INSERT INTO state_history(t,oid,status,value) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING"
            }
        };
        sqlx::query(q)
            .bind(TryInto::<i64>::try_into(ts_to_ns($state.set_time)).map_err(Error::failed)?)
            .bind($state.oid.as_str())
            .bind($state.status as i32)
            .bind(if let Some(v) = $state.value {
                Some(v.to_string_or_pack()?)
            } else {
                None
            })
            .execute($x)
            .await?;
    }};
}

pub async fn submit(state: ItemState) -> EResult<()> {
    let pool = POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    insert!(pool, kind, state);
    Ok(())
}

pub async fn submit_bulk(state: Vec<ItemState>) -> EResult<()> {
    if state.is_empty() {
        return Ok(());
    }
    let pool = POOL.get().unwrap();
    let kind = DB_KIND.get().unwrap();
    if *kind == AnyKind::Postgres {
        // TODO move to bind when switch out from Any
        let mut ts = String::new();
        let mut oids = String::new();
        let mut statuses = String::new();
        let mut values = String::new();
        let last_record = state.len() - 1;
        for (i, s) in state.into_iter().enumerate() {
            write!(ts, "{}", ts_to_ns(s.set_time))?;
            write!(oids, "'{}'", safe_sql_string(s.oid.as_str())?)?;
            write!(statuses, "{}", s.status)?;
            if let Some(v) = s.value {
                write!(
                    values,
                    "'{}'",
                    safe_sql_string(&v.to_string_or_pack().unwrap_or_default())?
                )?;
            } else {
                write!(values, "NULL")?;
            }
            if i != last_record {
                write!(ts, ",")?;
                write!(oids, ",")?;
                write!(statuses, ",")?;
                write!(values, ",")?;
            }
        }
        let q = format!(
            r#"INSERT INTO state_history(t,oid,status,value)
            VALUES(
                UNNEST(ARRAY[{}]),
                UNNEST(ARRAY[{}]),
                UNNEST(ARRAY[{}]),
                UNNEST(ARRAY[{}])
                ) ON CONFLICT DO NOTHING;"#,
            ts, oids, statuses, values
        );
        sqlx::query(&q).execute(pool).await?;
    } else {
        let mut tx = pool.begin().await?;
        for s in state {
            insert!(&mut tx, kind, s);
        }
        tx.commit().await?;
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub async fn state_history(
    oid: OID,
    t_start: f64,
    t_end: Option<f64>,
    precision: Option<u32>,
    limit: Option<usize>,
    prop: Option<StateProp>,
    compact: bool,
) -> EResult<StateHistoryData> {
    let pool = POOL.get().unwrap();
    let mut q = format!("WHERE oid='{}'", oid);
    write!(q, " and t>={}", ts_to_ns(t_start)).map_err(Error::failed)?;
    if let Some(te) = t_end {
        write!(q, " and t<={}", ts_to_ns(te)).map_err(Error::failed)?;
    }
    q += " ORDER BY t";
    if let Some(l) = limit {
        q += " DESC";
        write!(q, " limit {}", l).map_err(Error::failed)?;
    }
    let (need_status, need_value) = prop.as_ref().map_or((true, true), |p| {
        (*p == StateProp::Status, *p == StateProp::Value)
    });
    let pq = if need_status && need_value {
        "status,value"
    } else if need_status {
        "status"
    } else {
        "value"
    };
    let query = format!("SELECT t,{pq} FROM state_history {q}");
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut result = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let t: i64 = row.try_get("t")?;
        let status: Option<Value> = if need_status {
            let s: i32 = row.try_get("status")?;
            Some(Value::I16(s as ItemStatus))
        } else {
            None
        };
        let value = if need_value {
            let val: Option<String> = row.try_get("value")?;
            if let Some(value_str) = val {
                if let Ok(v) = value_str.parse::<u64>() {
                    Some(Value::U64(v))
                } else if let Ok(v) = value_str.parse::<f64>() {
                    Some(Value::F64(v).rounded(precision)?)
                } else {
                    Some(Value::String(value_str).unpack()?)
                }
            } else {
                Some(Value::Unit)
            }
        } else {
            None
        };
        let state = HistoricalState {
            set_time: ts_from_ns(t as u64),
            status,
            value,
        };
        result.push(state);
    }
    if limit.is_some() {
        result.reverse();
    }
    Ok(if compact {
        StateHistoryData::new_compact(result, need_status, need_value)
    } else {
        StateHistoryData::new_regular(result)
    })
}

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub async fn state_log(
    oid: OID,
    t_start: f64,
    t_end: Option<f64>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> EResult<Vec<ItemState>> {
    let pool = POOL.get().unwrap();
    let mut q = "WHERE oid".to_owned();
    if oid.is_wildcard() {
        write!(q, " like '{}'", oid.to_wildcard_str("%")).map_err(Error::failed)?;
    } else {
        write!(q, "='{}'", oid).map_err(Error::failed)?;
    }
    write!(q, " and t>={}", ts_to_ns(t_start)).map_err(Error::failed)?;
    if let Some(te) = t_end {
        write!(q, " and t<={}", ts_to_ns(te)).map_err(Error::failed)?;
    }
    q += " ORDER BY t";
    if let Some(l) = limit {
        q += " DESC, oid";
        write!(q, " limit {}", l).map_err(Error::failed)?;
        if let Some(o) = offset {
            write!(q, " offset {}", o).map_err(Error::failed)?;
        }
    } else {
        q += ", oid";
    }
    let query = format!("SELECT oid,t,status,value FROM state_history {q}");
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut result = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let oid_str: String = row.try_get("oid")?;
        let oid: OID = oid_str.parse()?;
        let t: i64 = row.try_get("t")?;
        let status: i32 = row.try_get("status")?;
        let val: Option<String> = row.try_get("value")?;
        let value = if let Some(value_str) = val {
            if let Ok(v) = value_str.parse::<u64>() {
                Some(Value::U64(v))
            } else if let Ok(v) = value_str.parse::<f64>() {
                Some(Value::F64(v))
            } else {
                Some(Value::String(value_str).unpack()?)
            }
        } else {
            Some(Value::Unit)
        };
        let state = ItemState {
            oid,
            set_time: ts_from_ns(t as u64),
            status: status as ItemStatus,
            value,
        };
        result.push(state);
    }
    if limit.is_some() {
        result.reverse();
    }
    Ok(result)
}

pub async fn cleanup(keep: u64, simple: bool) {
    log::trace!("cleaning db records");
    cleanup_records(keep, simple).await.log_ef();
}

async fn cleanup_records(keep: u64, simple: bool) -> EResult<()> {
    let pool = POOL.get().unwrap();
    let ut = eva_common::time::now_ns() - keep * 1_000_000_000;
    if simple {
        let q = format!("DELETE FROM STATE_HISTORY WHERE t < {}", ut);
        sqlx::query(&q).execute(pool).await?;
    } else {
        let q = format!(
            "SELECT oid, max(t) AS maxt FROM state_history WHERE t<{} GROUP BY oid",
            ut
        );
        let mut rows = sqlx::query(&q).fetch(pool);
        let mut tx = pool.begin().await?;
        while let Some(row) = rows.try_next().await? {
            let oid_str: String = row.try_get("oid")?;
            let oid: OID = oid_str.parse()?;
            let maxt: i64 = row.try_get("maxt")?;
            let q = format!(
                "DELETE FROM state_history WHERE oid='{}' AND t<{}",
                oid, maxt
            );
            sqlx::query(&q).execute(&mut tx).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}
