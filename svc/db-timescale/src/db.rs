use chrono::DateTime;
use chrono::naive::NaiveDateTime;
use eva_common::acl::OIDMask;
use eva_common::prelude::*;
use eva_sdk::prelude::err_logger;
use eva_sdk::types::{HistoricalState, ItemState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use log::{info, warn};
use sqlx::{
    ConnectOptions, PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use crate::history_table_name;

err_logger!();

pub static POOL: OnceLock<PgPool> = OnceLock::new();

const DEFAULT_COMPRESSION_POLICY: &str = "1d";
const DEFAULT_SLOW_STATEMENT_THRESHOLD: Duration = Duration::from_secs(2);
const EVENTS_TABLE_NAME: &str = "state_history_events";
// Inserts and OID cleanup both take it exclusive, so a batch insert and a cleanup pass never
// interleave: either the OIDs a batch needs already exist before cleanup runs, or cleanup waits
// for the batch to finish inserting its events first.
const OIDS_LOCK_KEY: i64 = 0x4556_4154_534f_4944;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageMode {
    Hypertable,
    Plain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventStorage {
    pub mode: StorageMode,
    pub schema: Box<str>,
}

static EVENT_STORAGE: OnceLock<EventStorage> = OnceLock::new();

#[derive(Debug, Eq, PartialEq)]
pub enum CleanupOutcome {
    Nothing,
    DroppedChunks(usize),
    DeletedRows(u64),
}

pub fn cleanup_cutoff(now: f64, keep: f64) -> EResult<NaiveDateTime> {
    ts_to_naive(now - keep)
}

fn slow_statement_log(threshold: Option<Duration>) -> (log::LevelFilter, Duration) {
    match threshold {
        None => (log::LevelFilter::Warn, DEFAULT_SLOW_STATEMENT_THRESHOLD),
        Some(t) if t.is_zero() => (log::LevelFilter::Off, DEFAULT_SLOW_STATEMENT_THRESHOLD),
        Some(t) => (log::LevelFilter::Warn, t),
    }
}

#[allow(clippy::cast_sign_loss)]
#[allow(clippy::cast_possible_truncation)]
pub fn ts_to_naive(ts: f64) -> EResult<NaiveDateTime> {
    let d = DateTime::from_timestamp(ts.trunc() as i64, (ts.fract() * 1_000_000_000_f64) as u32)
        .ok_or_else(|| Error::invalid_data("unable to convert timestamp"))?;
    Ok(d.naive_utc())
}

#[allow(clippy::cast_precision_loss)]
pub fn naive_to_ts(t: NaiveDateTime) -> f64 {
    let u = t.and_utc();
    u.timestamp() as f64 + f64::from(u.timestamp_subsec_nanos()) / 1_000_000_000.0
}

#[allow(clippy::too_many_lines)]
pub async fn init(
    conn: &str,
    size: u32,
    timeout: Duration,
    slow_statement_threshold: Option<Duration>,
) -> EResult<()> {
    let (slow_level, slow_threshold) = slow_statement_log(slow_statement_threshold);
    let opts = PgConnectOptions::from_str(conn)?
        .log_statements(log::LevelFilter::Trace)
        .log_slow_statements(slow_level, slow_threshold);
    let pool = PgPoolOptions::new()
        .max_connections(size)
        .acquire_timeout(timeout)
        .connect_with(opts)
        .await?;
    create_schema(&pool).await?;
    let schema = resolve_events_schema(&pool).await?;
    let mode = if is_hypertable(&pool, &schema).await? {
        info!("events hypertable active");
        StorageMode::Hypertable
    } else {
        match init_hypertable(&pool, &schema).await {
            Ok(()) => {
                warn!(
                    "events hypertable initialization completed, default compression policy: {}",
                    DEFAULT_COMPRESSION_POLICY
                );
                StorageMode::Hypertable
            }
            Err(e) => {
                warn!("Can not initialize events hypertable: {}", e);
                warn!("Read more at: https://info.bma.ai/en/actual/eva4/svc/eva-db-timescale.html");
                StorageMode::Plain
            }
        }
    };
    POOL.set(pool)
        .map_err(|_| Error::core("unable to set db pool"))?;
    EVENT_STORAGE
        .set(EventStorage { mode, schema })
        .map_err(|_| Error::core("unable to set event storage"))?;
    Ok(())
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn qualified_events_name(schema: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(EVENTS_TABLE_NAME))
}

// `state_history_events` is created unqualified in `create_schema`, so it lands in whichever
// schema is first on the connection's `search_path`. Resolve that schema via the catalog instead
// of assuming `public`, so hypertable detection below matches the table the connection actually
// uses.
async fn resolve_events_schema(pool: &PgPool) -> EResult<Box<str>> {
    let schema: Option<String> = sqlx::query_scalar(
        r"SELECT n.nspname FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.oid = to_regclass($1)",
    )
    .bind(EVENTS_TABLE_NAME)
    .fetch_optional(pool)
    .await?;
    schema
        .map(Into::into)
        .ok_or_else(|| Error::core("state_history_events relation not found after creation"))
}

async fn is_hypertable(pool: &PgPool, schema: &str) -> EResult<bool> {
    sqlx::query_scalar(
        r"SELECT EXISTS (
            SELECT 1 FROM timescaledb_information.hypertables
            WHERE hypertable_schema = $1 AND hypertable_name = $2
        )",
    )
    .bind(schema)
    .bind(EVENTS_TABLE_NAME)
    .fetch_one(pool)
    .await
    .map_err(|e| {
        Error::failed(format!(
            "unable to get timescale hypertable information. is the timescale extension enabled? ({})",
            e
        ))
    })
}

fn insert_fn_sql(name: &str) -> String {
    format!(
        r"CREATE OR REPLACE FUNCTION {name}(
ts TIMESTAMP[], oids VARCHAR[], statuses SMALLINT[], vals DOUBLE PRECISION[]) RETURNS void
LANGUAGE plpgsql
AS $$
BEGIN
  PERFORM pg_advisory_xact_lock({OIDS_LOCK_KEY});
  INSERT INTO state_history_oids(oid)
    SELECT DISTINCT u.oid FROM UNNEST(oids) AS u(oid)
    WHERE NOT EXISTS (SELECT 1 FROM state_history_oids o WHERE o.oid = u.oid)
    ORDER BY u.oid
    ON CONFLICT (oid) DO NOTHING;
  INSERT INTO state_history_events(t, oid_id, status, value)
    SELECT e.t, o.id, e.status, e.value
    FROM UNNEST(ts, oids, statuses, vals) AS e(t, oid, status, value)
    LEFT JOIN state_history_oids o ON o.oid = e.oid
    ON CONFLICT (t, oid_id) DO NOTHING;
END
$$;"
    )
}

async fn create_schema(pool: &PgPool) -> EResult<()> {
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS state_history_oids
        (id SERIAL,
        oid VARCHAR(256) NOT NULL UNIQUE,
        PRIMARY KEY (id))",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS state_history_events
        (t TIMESTAMP,
        oid_id INT,
        status SMALLINT,
        value DOUBLE PRECISION,
        PRIMARY KEY (t, oid_id),
        FOREIGN KEY(oid_id) REFERENCES state_history_oids(id) ON DELETE CASCADE)",
    )
    .execute(pool)
    .await?;
    let _ = sqlx::query("DROP INDEX IF EXISTS state_history_events_t")
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DROP FUNCTION IF EXISTS
                insert_state_history_events(
                    t TIMESTAMP[], oids VARCHAR[], statuses SMALLINT[], vals DOUBLE PRECISION[])",
    )
    .execute(pool)
    .await;
    for name in [
        "insert_state_history_events",
        "try_insert_state_history_events",
    ] {
        sqlx::query(&insert_fn_sql(name)).execute(pool).await?;
    }
    Ok(())
}

async fn init_hypertable(pool: &PgPool, schema: &str) -> EResult<()> {
    // Relation args for Timescale helpers are bound and built with format('%I.%I', …)::regclass
    // so schema names like tenant'o do not break a SQL string literal. ALTER TABLE still needs a
    // SQL identifier, so that path uses quote_ident (identifier context, not a string embed).
    sqlx::query("SELECT create_hypertable(format('%I.%I', $1::text, $2::text)::regclass, 't')")
        .bind(schema)
        .bind(EVENTS_TABLE_NAME)
        .execute(pool)
        .await?;
    let qualified = qualified_events_name(schema);
    sqlx::query(&format!(
        r"ALTER TABLE {qualified}
                SET (
                    timescaledb.compress,
                    timescaledb.compress_orderby = 't DESC',
                    timescaledb.compress_segmentby = 'oid_id')"
    ))
    .execute(pool)
    .await?;
    sqlx::query(
        "SELECT add_compression_policy(\
            format('%I.%I', $1::text, $2::text)::regclass, $3::interval)",
    )
    .bind(schema)
    .bind(EVENTS_TABLE_NAME)
    .bind(DEFAULT_COMPRESSION_POLICY)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn submit(state: ItemState) -> EResult<()> {
    if let Some(val) = state.value
        && let Ok(value) = f64::try_from(val)
    {
        sqlx::query("SELECT insert_state_history_events($1, $2, $3, $4)")
            .bind([ts_to_naive(state.set_time)?])
            .bind([state.oid.as_str()])
            .bind([state.status])
            .bind([value])
            .execute(POOL.get().unwrap())
            .await?;
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
    let mut values = Vec::with_capacity(state.len());
    for s in &state {
        if let Some(ref val) = s.value
            && let Ok(value) = f64::try_from(val)
        {
            ts.push(ts_to_naive(s.set_time)?);
            oids.push(s.oid.as_str());
            statuses.push(s.status);
            values.push(value);
        }
    }
    let q = if replace {
        sqlx::query("SELECT insert_state_history_events($1, $2, $3, $4)")
    } else {
        sqlx::query("SELECT try_insert_state_history_events($1, $2, $3, $4)")
    };
    q.bind(ts)
        .bind(oids)
        .bind(statuses)
        .bind(values)
        .execute(POOL.get().unwrap())
        .await?;
    Ok(())
}

#[allow(clippy::cast_possible_truncation, clippy::too_many_arguments)]
#[allow(clippy::cast_sign_loss)]
pub async fn state_history(
    oid_str: &str,
    t_start: f64,
    t_end: Option<f64>,
    precision: Option<u32>,
    limit: Option<usize>,
    prop: Option<StateProp>,
    xopts: BTreeMap<String, Value>,
    compact: bool,
) -> EResult<StateHistoryData> {
    let pool = POOL.get().unwrap();
    let table_name = history_table_name(&xopts)?;
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
        "status,value"
    } else if need_status {
        "status"
    } else {
        "value"
    };
    let query = format!(
        r"SELECT t,{pq} FROM {table_name} AS she
            JOIN state_history_oids
            ON she.oid_id=state_history_oids.id {q}"
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
    xopts: BTreeMap<String, Value>,
) -> EResult<Vec<ItemState>> {
    let pool = POOL.get().unwrap();
    let table_name = history_table_name(&xopts)?;
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
        JOIN state_history_oids
        ON she.oid_id=state_history_oids.id {q}"
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
    cleanup_events_now(keep).await.log_ef();
}

async fn cleanup_events_now(keep: f64) -> EResult<()> {
    let cutoff = cleanup_cutoff(eva_common::time::now_ns_float(), keep)?;
    let storage = EVENT_STORAGE.get().unwrap();
    match cleanup_event_records(POOL.get().unwrap(), storage, cutoff).await? {
        CleanupOutcome::Nothing => {}
        CleanupOutcome::DroppedChunks(n) => {
            info!("dropped {} events chunk(s) older than {}", n, cutoff);
        }
        CleanupOutcome::DeletedRows(n) => {
            log::debug!("deleted {} event records older than {}", n, cutoff);
        }
    }
    Ok(())
}

async fn cleanup_event_records(
    pool: &PgPool,
    storage: &EventStorage,
    cutoff: NaiveDateTime,
) -> EResult<CleanupOutcome> {
    match storage.mode {
        StorageMode::Hypertable => {
            let dropped: Vec<String> = sqlx::query_scalar(
                "SELECT drop_chunks(\
                    format('%I.%I', $1::text, $2::text)::regclass, \
                    older_than => $3::timestamp)::text",
            )
            .bind(&*storage.schema)
            .bind(EVENTS_TABLE_NAME)
            .bind(cutoff)
            .fetch_all(pool)
            .await?;
            Ok(if dropped.is_empty() {
                CleanupOutcome::Nothing
            } else {
                CleanupOutcome::DroppedChunks(dropped.len())
            })
        }
        StorageMode::Plain => {
            // FROM/DELETE need a SQL identifier; quote_ident is safe here (not inside a literal).
            let qualified = qualified_events_name(&storage.schema);
            let has_old: bool = sqlx::query_scalar(&format!(
                "SELECT EXISTS (SELECT 1 FROM {qualified} WHERE t < $1)"
            ))
            .bind(cutoff)
            .fetch_one(pool)
            .await?;
            if !has_old {
                return Ok(CleanupOutcome::Nothing);
            }
            let res = sqlx::query(&format!("DELETE FROM {qualified} WHERE t < $1"))
                .bind(cutoff)
                .execute(pool)
                .await?;
            Ok(CleanupOutcome::DeletedRows(res.rows_affected()))
        }
    }
}

pub async fn cleanup_oids() {
    log::trace!("cleaning oids");
    cleanup_event_oids(POOL.get().unwrap()).await.log_ef();
}

async fn cleanup_event_oids(pool: &PgPool) -> EResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(OIDS_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r"DELETE FROM state_history_oids
        WHERE id NOT IN (SELECT DISTINCT oid_id FROM state_history_events)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use chrono::{NaiveDate, NaiveDateTime};
    use sqlx::PgPool;
    use tokio::time;

    use super::{
        CleanupOutcome, EVENTS_TABLE_NAME, EventStorage, OIDS_LOCK_KEY, StorageMode,
        cleanup_cutoff, cleanup_event_oids, cleanup_event_records, create_schema, init_hypertable,
        insert_fn_sql, is_hypertable, qualified_events_name, quote_ident, resolve_events_schema,
        slow_statement_log,
    };
    use crate::test_db::TestDb;

    const INSERT_FNS: [&str; 2] = [
        "insert_state_history_events",
        "try_insert_state_history_events",
    ];

    fn ts(month: u32, day: u32, hour: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2024, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
    }

    fn storage(mode: StorageMode, schema: &str) -> EventStorage {
        EventStorage {
            mode,
            schema: schema.into(),
        }
    }

    async fn setup(pool: &PgPool, hypertable: bool) {
        create_schema(pool).await.unwrap();
        if hypertable {
            init_hypertable(pool, "public").await.unwrap();
            sqlx::query("SELECT remove_compression_policy('state_history_events')")
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("SELECT set_chunk_time_interval('state_history_events', INTERVAL '1 day')")
                .execute(pool)
                .await
                .unwrap();
        }
    }

    async fn call_insert(
        pool: &PgPool,
        func: &str,
        rows: &[(NaiveDateTime, &str, i16, f64)],
    ) -> Result<(), sqlx::Error> {
        let t: Vec<NaiveDateTime> = rows.iter().map(|r| r.0).collect();
        let oids: Vec<&str> = rows.iter().map(|r| r.1).collect();
        let statuses: Vec<i16> = rows.iter().map(|r| r.2).collect();
        let vals: Vec<f64> = rows.iter().map(|r| r.3).collect();
        sqlx::query(&format!("SELECT {func}($1, $2, $3, $4)"))
            .bind(t)
            .bind(oids)
            .bind(statuses)
            .bind(vals)
            .execute(pool)
            .await
            .map(|_| ())
    }

    async fn count(pool: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    #[test]
    fn slow_statement_log_default_is_warn_2s() {
        assert_eq!(
            slow_statement_log(None),
            (log::LevelFilter::Warn, Duration::from_secs(2)),
        );
    }

    #[test]
    fn slow_statement_log_custom_threshold() {
        assert_eq!(
            slow_statement_log(Some(Duration::from_millis(500))),
            (log::LevelFilter::Warn, Duration::from_millis(500)),
        );
    }

    #[test]
    fn slow_statement_log_zero_disables() {
        let (level, _) = slow_statement_log(Some(Duration::ZERO));

        assert_eq!(level, log::LevelFilter::Off);
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn insert_resolves_new_and_existing_oids() {
        for func in INSERT_FNS {
            let db = TestDb::new().await;
            setup(&db.pool, true).await;
            call_insert(
                &db.pool,
                func,
                &[
                    (ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0),
                    (ts(2, 1, 0, 0), "sensor:t/b", 1, 2.0),
                ],
            )
            .await
            .unwrap();
            call_insert(
                &db.pool,
                func,
                &[
                    (ts(2, 1, 0, 5), "sensor:t/b", 1, 3.0),
                    (ts(2, 1, 0, 5), "sensor:t/c", 1, 4.0),
                ],
            )
            .await
            .unwrap();

            assert_eq!(
                count(&db.pool, "SELECT COUNT(*) FROM state_history_oids").await,
                3,
            );
            assert_eq!(
                count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
                4,
            );
            assert_eq!(
                count(
                    &db.pool,
                    "SELECT COUNT(*) FROM state_history_events e
                     JOIN state_history_oids o ON o.id = e.oid_id
                     WHERE o.oid = 'sensor:t/b'"
                )
                .await,
                2,
            );
            db.drop_db().await;
        }
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn insert_skips_duplicate_t_oid() {
        for func in INSERT_FNS {
            let db = TestDb::new().await;
            setup(&db.pool, true).await;
            call_insert(
                &db.pool,
                func,
                &[
                    (ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0),
                    (ts(2, 1, 0, 0), "sensor:t/a", 2, 2.0),
                ],
            )
            .await
            .unwrap();

            assert_eq!(
                count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
                1,
            );

            let status_before: i16 = sqlx::query_scalar("SELECT status FROM state_history_events")
                .fetch_one(&db.pool)
                .await
                .unwrap();
            call_insert(&db.pool, func, &[(ts(2, 1, 0, 0), "sensor:t/a", 9, 9.0)])
                .await
                .unwrap();
            let status_after: i16 = sqlx::query_scalar("SELECT status FROM state_history_events")
                .fetch_one(&db.pool)
                .await
                .unwrap();

            assert_eq!(status_after, status_before);
            db.drop_db().await;
        }
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn insert_existing_oids_keep_sequence() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        let seq = "SELECT last_value FROM state_history_oids_id_seq";
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[(ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0)],
        )
        .await
        .unwrap();
        let before = count(&db.pool, seq).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[(ts(2, 1, 0, 5), "sensor:t/a", 1, 1.0)],
        )
        .await
        .unwrap();

        assert_eq!(count(&db.pool, seq).await, before);
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn insert_concurrent_batches_share_new_oids() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        let oids: Vec<String> = (0..100).map(|i| format!("sensor:t/c{i}")).collect();
        let batch = |t: NaiveDateTime| -> Vec<(NaiveDateTime, &str, i16, f64)> {
            oids.iter().map(|o| (t, o.as_str(), 1, 1.0)).collect()
        };
        let (b1, b2) = (batch(ts(2, 1, 0, 0)), batch(ts(2, 1, 0, 5)));
        let (r1, r2) = tokio::join!(
            call_insert(&db.pool, INSERT_FNS[0], &b1),
            call_insert(&db.pool, INSERT_FNS[1], &b2),
        );
        r1.unwrap();
        r2.unwrap();

        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_oids").await,
            100,
        );
        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
            200,
        );
        db.drop_db().await;
    }

    #[test]
    fn insert_fn_sql_is_set_based() {
        let sql = insert_fn_sql("try_insert_state_history_events");

        assert!(sql.contains("FUNCTION try_insert_state_history_events("));
        assert!(!sql.contains("LOCK TABLE"));
        assert!(!sql.contains("LOOP"));
        assert!(!sql.contains("array_append"));
    }

    #[test]
    fn insert_fn_sql_holds_exclusive_oids_lock() {
        let sql = insert_fn_sql("insert_state_history_events");
        let lock = format!("PERFORM pg_advisory_xact_lock({OIDS_LOCK_KEY});");

        assert!(sql.contains(&lock));
    }

    #[test]
    fn quote_ident_keeps_apostrophe_inside_identifier() {
        assert_eq!(quote_ident("tenant'o"), "\"tenant'o\"");
        assert_eq!(
            qualified_events_name("tenant'o"),
            "\"tenant'o\".\"state_history_events\"",
        );
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_oids_deletes_only_unused() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[(ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0)],
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO state_history_oids(oid) VALUES ('sensor:t/orphan')")
            .execute(&db.pool)
            .await
            .unwrap();
        cleanup_event_oids(&db.pool).await.unwrap();

        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_oids").await,
            1,
        );
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_oids_waits_for_in_flight_insert() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        // Reproduces insert function state after OID insert and before events insert, holding
        // the same exclusive lock the insert function itself takes.
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(OIDS_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .unwrap();
        let oid_id: i32 = sqlx::query_scalar(
            "INSERT INTO state_history_oids(oid) VALUES ('sensor:t/a') RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let pool = db.pool.clone();
        let cleanup = tokio::spawn(async move { cleanup_event_oids(&pool).await });
        time::sleep(Duration::from_millis(300)).await;

        assert!(!cleanup.is_finished());

        sqlx::query(
            "INSERT INTO state_history_events(t, oid_id, status, value) \
             VALUES ($1, $2, 1, 1.0)",
        )
        .bind(ts(2, 1, 0, 0))
        .bind(oid_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        cleanup.await.unwrap().unwrap();

        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_oids").await,
            1,
        );
        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
            1,
        );
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn insert_empty_batch_is_noop() {
        for func in INSERT_FNS {
            let db = TestDb::new().await;
            setup(&db.pool, true).await;
            call_insert(&db.pool, func, &[]).await.unwrap();

            assert_eq!(
                count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
                0
            );
            db.drop_db().await;
        }
    }

    #[test]
    fn cleanup_cutoff_subtracts_keep() {
        let now = 1_706_745_600.0;

        assert_eq!(cleanup_cutoff(now, 86_400.0).unwrap(), ts(1, 31, 0, 0));
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_hypertable_nothing_old_is_noop() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[(ts(2, 2, 12, 0), "sensor:t/a", 1, 1.0)],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &db.pool,
            &storage(StorageMode::Hypertable, "public"),
            ts(2, 1, 0, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::Nothing);
        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
            1
        );
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_hypertable_drops_only_fully_old_chunks() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[
                (ts(1, 5, 12, 0), "sensor:t/a", 1, 1.0),
                (ts(1, 31, 23, 0), "sensor:t/a", 1, 1.0),
                (ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0),
                (ts(2, 2, 12, 0), "sensor:t/a", 1, 1.0),
            ],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &db.pool,
            &storage(StorageMode::Hypertable, "public"),
            ts(2, 1, 0, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::DroppedChunks(2));
        assert_eq!(
            count(
                &db.pool,
                "SELECT COUNT(*) FROM state_history_events WHERE t >= '2024-02-01 00:00:00'"
            )
            .await,
            2,
        );
        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
            2,
        );
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_hypertable_keeps_partially_old_chunk() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[
                (ts(2, 1, 1, 0), "sensor:t/a", 1, 1.0),
                (ts(2, 1, 23, 0), "sensor:t/a", 1, 1.0),
            ],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &db.pool,
            &storage(StorageMode::Hypertable, "public"),
            ts(2, 1, 12, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::Nothing);
        assert_eq!(
            count(&db.pool, "SELECT COUNT(*) FROM state_history_events").await,
            2,
        );
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_plain_nothing_old_skips_delete() {
        let db = TestDb::new().await;
        setup(&db.pool, false).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[(ts(2, 2, 0, 0), "sensor:t/a", 1, 1.0)],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &db.pool,
            &storage(StorageMode::Plain, "public"),
            ts(2, 1, 0, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::Nothing);
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn cleanup_plain_deletes_rows_older_than_cutoff() {
        let db = TestDb::new().await;
        setup(&db.pool, false).await;
        call_insert(
            &db.pool,
            INSERT_FNS[0],
            &[
                (ts(1, 31, 23, 59), "sensor:t/a", 1, 1.0),
                (ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0),
            ],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &db.pool,
            &storage(StorageMode::Plain, "public"),
            ts(2, 1, 0, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::DeletedRows(1));
        assert_eq!(
            count(
                &db.pool,
                "SELECT COUNT(*) FROM state_history_events WHERE t = '2024-02-01 00:00:00'"
            )
            .await,
            1,
        );
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn storage_mode_is_schema_aware_when_search_path_shadows_public_hypertable() {
        let db = TestDb::new().await;
        setup(&db.pool, true).await;
        sqlx::query("CREATE SCHEMA tenant")
            .execute(&db.pool)
            .await
            .unwrap();
        let tenant_pool = db.pool_with_search_path("tenant, public").await;
        create_schema(&tenant_pool).await.unwrap();

        let schema = resolve_events_schema(&tenant_pool).await.unwrap();

        assert_eq!(&*schema, "tenant");
        assert!(!is_hypertable(&tenant_pool, &schema).await.unwrap());
        assert!(is_hypertable(&db.pool, "public").await.unwrap());

        call_insert(
            &tenant_pool,
            INSERT_FNS[0],
            &[
                (ts(1, 31, 23, 59), "sensor:t/a", 1, 1.0),
                (ts(2, 1, 0, 0), "sensor:t/a", 1, 1.0),
            ],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &tenant_pool,
            &storage(StorageMode::Plain, &schema),
            ts(2, 1, 0, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::DeletedRows(1));
        tenant_pool.close().await;
        db.drop_db().await;
    }

    #[tokio::test]
    #[ignore = "needs TimescaleDB: set EVA_DB_TIMESCALE_TEST_URL"]
    async fn hypertable_ops_accept_schema_with_apostrophe() {
        let db = TestDb::new().await;
        sqlx::query(r#"CREATE SCHEMA "tenant'o""#)
            .execute(&db.pool)
            .await
            .unwrap();
        let pool = db.pool_with_search_path(r#""tenant'o", public"#).await;
        create_schema(&pool).await.unwrap();
        init_hypertable(&pool, "tenant'o").await.unwrap();
        sqlx::query(
            "SELECT remove_compression_policy(\
                format('%I.%I', $1::text, $2::text)::regclass)",
        )
        .bind("tenant'o")
        .bind(EVENTS_TABLE_NAME)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "SELECT set_chunk_time_interval(\
                format('%I.%I', $1::text, $2::text)::regclass, INTERVAL '1 day')",
        )
        .bind("tenant'o")
        .bind(EVENTS_TABLE_NAME)
        .execute(&pool)
        .await
        .unwrap();
        call_insert(
            &pool,
            INSERT_FNS[0],
            &[
                (ts(1, 5, 12, 0), "sensor:t/a", 1, 1.0),
                (ts(2, 2, 12, 0), "sensor:t/a", 1, 1.0),
            ],
        )
        .await
        .unwrap();
        let res = cleanup_event_records(
            &pool,
            &storage(StorageMode::Hypertable, "tenant'o"),
            ts(2, 1, 0, 0),
        )
        .await
        .unwrap();

        assert_eq!(res, CleanupOutcome::DroppedChunks(1));
        pool.close().await;
        db.drop_db().await;
    }
}
