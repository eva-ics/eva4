use chrono::naive::NaiveDateTime;
use eva_common::acl::OIDMask;
use eva_common::prelude::*;
use eva_sdk::prelude::err_logger;
use eva_sdk::types::{HistoricalState, ItemState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use once_cell::sync::OnceCell;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    ConnectOptions, PgPool, Row,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::str::FromStr;
use std::time::Duration;

use crate::path::jsonpath_to_pg;

err_logger!();

pub static POOL: OnceCell<PgPool> = OnceCell::new();

#[allow(clippy::cast_sign_loss)]
#[allow(clippy::cast_possible_truncation)]
pub fn ts_to_naive(ts: f64) -> EResult<NaiveDateTime> {
    NaiveDateTime::from_timestamp_opt(ts.trunc() as i64, (ts.fract() * 1_000_000_000_f64) as u32)
        .ok_or_else(|| Error::invalid_data("unable to convert timestamp"))
}

#[allow(clippy::cast_precision_loss)]
pub fn naive_to_ts(t: NaiveDateTime) -> f64 {
    t.timestamp() as f64 + f64::from(t.timestamp_subsec_nanos()) / 1_000_000_000.0
}

#[allow(clippy::too_many_lines)]
pub async fn init(conn: &str, size: u32, timeout: Duration) -> EResult<()> {
    let mut opts = PgConnectOptions::from_str(conn)?;
    opts.log_statements(log::LevelFilter::Trace)
        .log_slow_statements(log::LevelFilter::Warn, Duration::from_secs(2));
    let pool = PgPoolOptions::new()
        .max_connections(size)
        .acquire_timeout(timeout)
        .connect_with(opts)
        .await?;
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS d_state_history_oids
        (id SERIAL,
        oid VARCHAR(256) NOT NULL UNIQUE,
        PRIMARY KEY (id))",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS d_state_history_events
        (t TIMESTAMP,
        oid_id INT,
        status SMALLINT,
        value JSONB,
        PRIMARY KEY (t, oid_id),
        FOREIGN KEY(oid_id) REFERENCES d_state_history_oids(id) ON DELETE CASCADE)",
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("DROP INDEX IF EXISTS d_state_history_events_t")
        .execute(&pool)
        .await;
    let _ = sqlx::query(
        "DROP FUNCTION IF EXISTS
                insert_d_state_history_events(
                    t TIMESTAMP[], oids VARCHAR[], statuses SMALLINT[], vals JSONB[])",
    )
    .execute(&pool)
    .await;
    sqlx::query(
        r"CREATE OR REPLACE FUNCTION insert_d_state_history_events(
ts TIMESTAMP[], oids VARCHAR[], statuses SMALLINT[], vals JSONB[]) RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
 oid_id_x INT;
 oid_ids INT[];
BEGIN
  LOCK TABLE d_state_history_oids IN SHARE UPDATE EXCLUSIVE MODE;
  FOR i IN array_lower(ts, 1)..array_upper(ts, 1) LOOP
    SELECT id FROM d_state_history_oids WHERE oid=oids[i] into oid_id_x;
    IF oid_id_x IS NULL THEN
        INSERT INTO d_state_history_oids(oid) VALUES(oids[i]) RETURNING id INTO oid_id_x;
    END IF;
    oid_ids := array_append(oid_ids, oid_id_x);
  END LOOP;
  INSERT INTO d_state_history_events(t, oid_id, status, value) VALUES (
      UNNEST(ts),
      UNNEST(oid_ids),
      UNNEST(statuses),
      UNNEST(vals)
    ) ON CONFLICT (t, oid_id) DO UPDATE SET status=EXCLUDED.status, value=EXCLUDED.value;
  END
$$;",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r"CREATE OR REPLACE FUNCTION try_insert_d_state_history_events(
ts TIMESTAMP[], oids VARCHAR[], statuses SMALLINT[], vals JSONB[]) RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
 oid_id_x INT;
 oid_ids INT[];
BEGIN
  LOCK TABLE d_state_history_oids IN SHARE UPDATE EXCLUSIVE MODE;
  FOR i IN array_lower(ts, 1)..array_upper(ts, 1) LOOP
    SELECT id FROM d_state_history_oids WHERE oid=oids[i] into oid_id_x;
    IF oid_id_x IS NULL THEN
        INSERT INTO d_state_history_oids(oid) VALUES(oids[i]) RETURNING id INTO oid_id_x;
    END IF;
    oid_ids := array_append(oid_ids, oid_id_x);
  END LOOP;
  INSERT INTO d_state_history_events(t, oid_id, status, value) VALUES (
      UNNEST(ts),
      UNNEST(oid_ids),
      UNNEST(statuses),
      UNNEST(vals)
    ) ON CONFLICT (t, oid_id) DO NOTHING;
  END
$$;",
    )
    .execute(&pool)
    .await?;
    POOL.set(pool)
        .map_err(|_| Error::core("unable to set db pool"))
}

pub async fn submit(state: ItemState) -> EResult<()> {
    if let Some(val) = state.value {
        if let Ok(value) = serde_json::to_value(val) {
            sqlx::query("SELECT insert_d_state_history_events($1, $2, $3, $4)")
                .bind([ts_to_naive(state.set_time)?])
                .bind([state.oid.as_str()])
                .bind([state.status])
                .bind([value])
                .execute(POOL.get().unwrap())
                .await?;
        }
    }
    Ok(())
}

pub async fn submit_bulk(state: Vec<ItemState>, replace: bool) -> EResult<()> {
    if state.is_empty() {
        return Ok(());
    }
    let mut ts = Vec::with_capacity(state.len());
    let mut oids = Vec::with_capacity(state.len());
    let mut statuses = Vec::with_capacity(state.len());
    let mut values: Vec<serde_json::Value> = Vec::with_capacity(state.len());
    for s in state {
        let Some(val) = s.value else {
            continue;
        };
        let Ok(val) = serde_json::to_value(val) else {
            continue;
        };
        ts.push(ts_to_naive(s.set_time)?);
        oids.push(s.oid.as_str().to_owned());
        statuses.push(s.status);
        values.push(val);
    }
    let q = if replace {
        sqlx::query("SELECT insert_d_state_history_events($1, $2, $3, $4)")
    } else {
        sqlx::query("SELECT try_insert_d_state_history_events($1, $2, $3, $4)")
    };
    q.bind(ts)
        .bind(oids)
        .bind(statuses)
        .bind(values)
        .execute(POOL.get().unwrap())
        .await?;
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub async fn state_history(
    oid_str: &str,
    t_start: f64,
    t_end: Option<f64>,
    _precision: Option<u32>,
    limit: Option<usize>,
    prop: Option<StateProp>,
    mut xopts: BTreeMap<String, Value>,
    compact: bool,
) -> EResult<StateHistoryData> {
    let pool = POOL.get().unwrap();
    let table_name = "d_state_history_events";
    let mut q = String::new();
    // try to parse OID
    if let Ok(oid) = oid_str.parse::<OID>() {
        write!(q, "WHERE oid='{}'", oid)?;
    } else if !crate::eva_pg_enabled() {
        return Err(Error::unsupported(
            "OID masks are not supported without eva_pg extension",
        ));
    } else if let Ok(oid_mask) = oid_str.parse::<OIDMask>() {
        // try to use OID mask
        write!(q, "WHERE oid_match(oid, '{}')", oid_mask)?;
    } else {
        return Err(Error::invalid_data("invalid OID/mask"));
    }
    write!(q, " and t>=to_timestamp({})", t_start).map_err(Error::failed)?;
    if let Some(te) = t_end {
        write!(q, " and t<=to_timestamp({})", te).map_err(Error::failed)?;
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
        format!(
            "status,{} as value",
            jsonpath_to_pg("value", xopts.remove("path"))?
        )
    } else if need_status {
        "status".to_owned()
    } else {
        format!(
            "{} as value",
            jsonpath_to_pg("value", xopts.remove("path"))?
        )
    };
    let query = format!(
        r"SELECT t,{pq} FROM {table_name} AS she
            JOIN d_state_history_oids
            ON she.oid_id=d_state_history_oids.id {q}"
    );
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut result = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let t: NaiveDateTime = row.try_get("t")?;
        let status: Option<Value> = if need_status {
            let s: i16 = row.try_get("status")?;
            Some(Value::I16(s))
        } else {
            None
        };
        let value = if need_value {
            let val: Option<serde_json::Value> = row.try_get("value")?;
            eva_common::value::to_value(val.unwrap_or_default()).ok()
        } else {
            None
        };
        let state = HistoricalState {
            set_time: naive_to_ts(t),
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
    _xopts: BTreeMap<String, Value>,
) -> EResult<Vec<ItemState>> {
    let pool = POOL.get().unwrap();
    let table_name = "d_state_history_events";
    let mut q = "WHERE oid".to_owned();
    if oid.is_wildcard() {
        write!(q, " like '{}'", oid.to_wildcard_str("%")).map_err(Error::failed)?;
    } else {
        write!(q, "='{}'", oid).map_err(Error::failed)?;
    }
    write!(q, " and t>=to_timestamp({})", t_start).map_err(Error::failed)?;
    if let Some(te) = t_end {
        write!(q, " and t<=to_timestamp({})", te).map_err(Error::failed)?;
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
    let query = format!(
        r"SELECT oid,t,status,value FROM {table_name} AS she
        JOIN d_state_history_oids
        ON she.oid_id=d_state_history_oids.id {q}"
    );
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut result = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let oid_str: String = row.try_get("oid")?;
        let oid: OID = oid_str.parse()?;
        let t: NaiveDateTime = row.try_get("t")?;
        let status: i16 = row.try_get("status")?;
        let val: Option<serde_json::Value> = row.try_get("value")?;
        let value = eva_common::value::to_value(val.unwrap_or_default()).ok();
        let state = ItemState {
            oid,
            set_time: naive_to_ts(t),
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

pub async fn cleanup_events(keep: f64) {
    log::trace!("cleaning db records");
    cleanup_event_records(keep).await.log_ef();
}

async fn cleanup_event_records(keep: f64) -> EResult<()> {
    let pool = POOL.get().unwrap();
    let ut = eva_common::time::now_ns_float() - keep;
    let q = format!(
        "DELETE FROM d_state_history_events WHERE t < to_timestamp({})",
        ut
    );
    sqlx::query(&q).execute(pool).await?;
    Ok(())
}

pub async fn cleanup_oids() {
    log::trace!("cleaning oids");
    cleanup_event_oids().await.log_ef();
}

async fn cleanup_event_oids() -> EResult<()> {
    let pool = POOL.get().unwrap();
    sqlx::query(
        r"DELETE FROM d_state_history_oids
        WHERE id NOT IN (SELECT DISTINCT oid_id FROM d_state_history_events)",
    )
    .execute(pool)
    .await?;
    Ok(())
}
