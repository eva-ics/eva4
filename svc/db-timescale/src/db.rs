use chrono::naive::NaiveDateTime;
use eva_common::prelude::*;
use eva_sdk::prelude::err_logger;
use eva_sdk::types::{HistoricalState, ItemState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use log::{info, warn};
use once_cell::sync::OnceCell;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    ConnectOptions, PgPool, Row,
};
use std::fmt::Write as _;
use std::str::FromStr;
use std::time::Duration;

err_logger!();

pub static POOL: OnceCell<PgPool> = OnceCell::new();

const DEFAULT_COMPRESSION_POLICY: &str = "1d";

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
        r#"CREATE TABLE IF NOT EXISTS state_history_oids
        (id SERIAL,
        oid VARCHAR(256) NOT NULL UNIQUE,
        PRIMARY KEY (id))"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS state_history_events
        (t TIMESTAMP,
        oid_id INT,
        status SMALLINT,
        value DOUBLE PRECISION,
        PRIMARY KEY (t, oid_id),
        FOREIGN KEY(oid_id) REFERENCES state_history_oids(id) ON DELETE CASCADE)"#,
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("DROP INDEX IF EXISTS state_history_events_t")
        .execute(&pool)
        .await;
    sqlx::query(
        r#"CREATE OR REPLACE FUNCTION insert_state_history_events(
t TIMESTAMP[], oids VARCHAR[], statuses SMALLINT[], vals DOUBLE PRECISION[]) RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
 oid_id INT;
 oid_ids INT[];
BEGIN
  LOCK TABLE state_history_oids IN SHARE UPDATE EXCLUSIVE MODE;
  FOR i IN array_lower(t, 1)..array_upper(t, 1) LOOP
    SELECT id FROM state_history_oids WHERE oid=oids[i] into oid_id;
    IF oid_id IS NULL THEN
        INSERT INTO state_history_oids(oid) VALUES(oids[i]) RETURNING id INTO oid_id;
    END IF;
    oid_ids := array_append(oid_ids, oid_id);
  END LOOP;
  INSERT INTO state_history_events(t, oid_id, status, value) VALUES (
      UNNEST(t),
      UNNEST(oid_ids),
      UNNEST(statuses),
      UNNEST(vals)
    ) ON CONFLICT DO NOTHING;
  END
$$;"#,
    )
    .execute(&pool)
    .await?;
    let c: i64 = sqlx::query(
        r#"SELECT COUNT(*) FROM timescaledb_information.hypertables
        WHERE hypertable_name='state_history_events'"#,
    )
    .fetch_one(&pool)
    .await.map_err(|e| Error::failed(format!("unable to get timescale hypertable information. is the timescale extension enabled? ({})", e)))?
    .try_get(0)?;
    if c == 0 {
        match init_hypertable(&pool).await {
            Ok(()) => {
                warn!(
                    "events hypertable initialization completed, default compression policy: {}",
                    DEFAULT_COMPRESSION_POLICY
                );
            }
            Err(e) => {
                warn!("Can not initialize events hypertable: {}", e);
                warn!("Read more at: https://info.bma.ai/en/actual/eva4/svc/eva-db-timescale.html");
            }
        }
    } else {
        info!("events hypertable active");
    }
    POOL.set(pool)
        .map_err(|_| Error::core("unable to set db pool"))
}

async fn init_hypertable(pool: &PgPool) -> EResult<()> {
    sqlx::query("SELECT create_hypertable('state_history_events', 't')")
        .execute(pool)
        .await?;
    sqlx::query(
        r#"ALTER TABLE state_history_events
                SET (
                    timescaledb.compress,
                    timescaledb.compress_orderby = 't DESC',
                    timescaledb.compress_segmentby = 'oid_id')"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(&format!(
        "SELECT add_compression_policy('state_history_events', INTERVAL '{}')",
        DEFAULT_COMPRESSION_POLICY
    ))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn submit(state: ItemState) -> EResult<()> {
    if let Some(val) = state.value {
        if let Ok(value) = f64::try_from(val) {
            sqlx::query("SELECT insert_state_history_events($1, $2, $3, $4)")
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

pub async fn submit_bulk(state: Vec<ItemState>) -> EResult<()> {
    if state.is_empty() {
        return Ok(());
    }
    let mut ts = Vec::with_capacity(state.len());
    let mut oids = Vec::with_capacity(state.len());
    let mut statuses = Vec::with_capacity(state.len());
    let mut values = Vec::with_capacity(state.len());
    for s in &state {
        if let Some(ref val) = s.value {
            if let Ok(value) = f64::try_from(val) {
                ts.push(ts_to_naive(s.set_time)?);
                oids.push(s.oid.as_str());
                statuses.push(s.status);
                values.push(value);
            }
        }
    }
    sqlx::query("SELECT insert_state_history_events($1, $2, $3, $4)")
        .bind(ts)
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
        "status,value"
    } else if need_status {
        "status"
    } else {
        "value"
    };
    let query = format!(
        r#"SELECT t,{pq} FROM state_history_events
            JOIN state_history_oids
            ON state_history_events.oid_id=state_history_oids.id {q}"#
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
            let val: Option<f64> = row.try_get("value")?;
            Some(if let Some(v) = val {
                Value::F64(v).rounded(precision)?
            } else {
                Value::Unit
            })
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
) -> EResult<Vec<ItemState>> {
    let pool = POOL.get().unwrap();
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
        r#"SELECT oid,t,status,value FROM state_history_events
        JOIN state_history_oids
        ON state_history_events.oid_id=state_history_oids.id {q}"#
    );
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut result = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let oid_str: String = row.try_get("oid")?;
        let oid: OID = oid_str.parse()?;
        let t: NaiveDateTime = row.try_get("t")?;
        let status: i16 = row.try_get("status")?;
        let val: Option<f64> = row.try_get("value")?;
        let value = Some(if let Some(v) = val {
            Value::F64(v)
        } else {
            Value::Unit
        });
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
        "DELETE FROM state_history_events WHERE t < to_timestamp({})",
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
        r#"DELETE FROM state_history_oids
        WHERE id NOT IN (SELECT DISTINCT oid_id FROM state_history_events)"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}
