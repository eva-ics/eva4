use crate::items::{Inventory, ItemState};
use eva_common::events::DbState;
use eva_common::prelude::*;
use futures::TryStreamExt;
use log::{debug, warn};
use once_cell::sync::OnceCell;
use sqlx::{
    any::{AnyConnectOptions, AnyKind, AnyPool, AnyPoolOptions, AnyRow},
    sqlite, ConnectOptions, Row, Transaction,
};
use std::str::FromStr;
use std::time::Duration;

lazy_static! {
    static ref POOL: OnceCell<AnyPool> = <_>::default();
    static ref DB_KIND: OnceCell<AnyKind> = <_>::default();
}

#[inline]
pub fn get_pool() -> Option<&'static AnyPool> {
    POOL.get()
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
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS inventory(
                    oid VARCHAR(768),
                    cfg VARCHAR(4096),
                    state VARCHAR(8192),
                    PRIMARY KEY(oid))
                "#,
    )
    .execute(&pool)
    .await?;
    POOL.set(pool)
        .map_err(|_| Error::core("unable to set db pool"))
}

macro_rules! create {
    ($oid: expr, $ex: expr) => {{
        let q = match DB_KIND.get().unwrap() {
            AnyKind::Sqlite | AnyKind::MySql => "INSERT INTO inventory(oid) VALUES(?)",
            AnyKind::Postgres => "INSERT INTO inventory(oid) VALUES($1) ON CONFLICT DO NOTHING",
        };
        let _r = sqlx::query(q).bind($oid.as_str()).execute($ex).await;
    }};
}

pub async fn load(inventory: &mut Inventory, pool: &AnyPool, boot_id: u64) -> EResult<()> {
    let mut rows = sqlx::query("SELECT oid,cfg,state FROM inventory").fetch(pool);
    while let Some(row) = rows.try_next().await? {
        if let Err(e) = append_item(row, inventory, boot_id) {
            warn!("invalid inventory record: {}", e);
        }
    }
    Ok(())
}

fn append_item(row: AnyRow, inventory: &mut Inventory, boot_id: u64) -> EResult<()> {
    let oid_str: String = row.try_get("oid")?;
    let oid: OID = oid_str.parse()?;
    debug!("loading {}", oid);
    let cfg_str: String = row.try_get("cfg")?;
    let config_value: serde_json::Value = serde_json::from_str(&cfg_str)?;
    let state_str: Option<String> = row.try_get("state")?;
    let state: Option<ItemState> = if let Some(ref st) = state_str {
        Some(serde_json::from_str(st)?)
    } else {
        None
    };
    inventory.append_item_from_value(&oid, config_value, state, boot_id)?;
    Ok(())
}

macro_rules! delete {
    ($oid: expr, $ex: expr) => {{
        let q = match DB_KIND.get().unwrap() {
            AnyKind::Sqlite | AnyKind::MySql => "DELETE FROM inventory WHERE oid=?",
            AnyKind::Postgres => "DELETE FROM inventory WHERE oid=$1",
        };
        sqlx::query(q).bind($oid.as_str()).execute($ex).await?;
    }};
}

pub async fn destroy(oid: &OID, pool: &AnyPool) -> EResult<()> {
    delete!(oid, pool);
    Ok(())
}

pub async fn destroy_tx(oid: &OID, tx: &mut Transaction<'_, sqlx::Any>) -> EResult<()> {
    delete!(oid, tx);
    Ok(())
}

macro_rules! set_config {
    ($oid: expr, $config: expr, $ex: expr) => {
        let q = match DB_KIND.get().unwrap() {
            AnyKind::Sqlite | AnyKind::MySql => "UPDATE inventory SET cfg=? WHERE oid=?",
            AnyKind::Postgres => "UPDATE inventory SET cfg=$1 WHERE oid=$2",
        };
        sqlx::query(q)
            .bind(serde_json::to_string(&$config)?)
            .bind($oid.as_str())
            .execute($ex)
            .await?;
    };
}

pub async fn save_config(oid: &OID, config: Value, pool: &AnyPool) -> EResult<()> {
    create!(oid, pool);
    set_config!(oid, config, pool);
    Ok(())
}

pub async fn create_tx(oid: &OID, tx: &mut Transaction<'_, sqlx::Any>) -> EResult<()> {
    create!(oid, tx);
    Ok(())
}

/// Warning: does not create item automatically if missing
pub async fn save_config_tx(
    oid: &OID,
    config: Value,
    tx: &mut Transaction<'_, sqlx::Any>,
) -> EResult<()> {
    set_config!(oid, config, tx);
    Ok(())
}

macro_rules! set_state {
    ($oid: expr, $state: expr, $ex: expr) => {
        let q = match DB_KIND.get().unwrap() {
            AnyKind::Sqlite | AnyKind::MySql => "UPDATE inventory SET state=? WHERE oid=?",
            AnyKind::Postgres => "UPDATE inventory SET state=$1 WHERE oid=$2",
        };
        sqlx::query(q)
            .bind(serde_json::to_string(&$state)?)
            .bind($oid.as_str())
            .execute($ex)
            .await?;
    };
}

pub async fn save_state(oid: &OID, state: DbState, pool: &AnyPool) -> EResult<()> {
    set_state!(oid, state, pool);
    Ok(())
}

pub async fn save_state_tx(
    oid: &OID,
    state: DbState,
    tx: &mut Transaction<'_, sqlx::Any>,
) -> EResult<()> {
    set_state!(oid, state, tx);
    Ok(())
}
