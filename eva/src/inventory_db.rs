use crate::items::{Inventory, ItemState};
use async_trait::async_trait;
use eva_common::prelude::*;
use eva_common::{events::DbState, SLEEP_STEP};
use futures::TryStreamExt;
use log::{error, warn};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, Sqlite};
use sqlx::{FromRow, Pool, Postgres};
use std::borrow::Cow;
use std::{collections::BTreeMap, mem, time::Duration};
use std::{str::FromStr, sync::Arc};
use tokio::task::JoinHandle;

static DB: OnceCell<Arc<Database>> = OnceCell::new();

const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
const SLOW_STATEMENT: Duration = Duration::from_secs(2);

#[async_trait]
trait Storage: Send + Sync + 'static {
    async fn init(&self) -> EResult<()>;
    async fn load_inventory(&self, inventory: &mut Inventory, boot_id: u64) -> EResult<()>;
    async fn save_items(&self, items: Vec<(OID, Value)>) -> EResult<()>;
    async fn save_states(&self, states: BTreeMap<OID, DbState>) -> EResult<()>;
    async fn destroy_items(&self, oids: Vec<OID>) -> EResult<()>;
}

macro_rules! crate_tables {
    ($db: expr) => {
        sqlx::query(
            r"CREATE TABLE IF NOT EXISTS inventory(
                    oid VARCHAR(768),
                    cfg VARCHAR(4096) NOT NULL,
                    state VARCHAR(8192),
                    PRIMARY KEY(oid))
                ",
        )
        .execute($db)
        .await?;
    };
}

fn append_item(raw_item: RawItem, inventory: &mut Inventory, boot_id: u64) -> EResult<()> {
    let config_value: serde_json::Value = serde_json::from_str(&raw_item.cfg)?;
    let state: Option<ItemState> = if let Some(st) = &raw_item.state {
        Some(serde_json::from_str(st)?)
    } else {
        None
    };
    inventory.append_item_from_value(&raw_item.oid, config_value, state, boot_id)?;
    Ok(())
}

macro_rules! load_inventory {
    ($db: expr, $inventory: expr, $boot_id: expr) => {
        let mut rows = sqlx::query_as("SELECT oid,cfg,state FROM inventory").fetch($db);
        while let Some(row) = rows.try_next().await? {
            let r: RawItem = RawItem::from(row);
            if let Err(e) = append_item(r, $inventory, $boot_id) {
                warn!("invalid inventory record: {}", e);
            }
        }
    };
}

#[derive(FromRow)]
struct RawItem {
    oid: OID,
    cfg: String,
    state: Option<String>,
}

#[async_trait]
impl Storage for Pool<Postgres> {
    async fn init(&self) -> EResult<()> {
        crate_tables!(self);
        Ok(())
    }
    async fn save_items(&self, items: Vec<(OID, Value)>) -> EResult<()> {
        let mut oids = Vec::with_capacity(items.len());
        let mut serialized_cfgs = Vec::with_capacity(items.len());
        for (o, c) in items {
            match serde_json::to_string(&c) {
                Ok(v) => {
                    oids.push(o);
                    serialized_cfgs.push(v);
                }
                Err(e) => error!("unable to serialize state for {}, not saved: {}", o, e),
            }
        }
        sqlx::query(
            r"INSERT INTO inventory(oid, cfg)
        SELECT UNNEST($1), UNNEST($2)
        ON CONFLICT (oid) DO UPDATE SET cfg=excluded.cfg",
        )
        .bind(oids)
        .bind(serialized_cfgs)
        .execute(self)
        .await?;
        Ok(())
    }
    async fn load_inventory(&self, inventory: &mut Inventory, boot_id: u64) -> EResult<()> {
        load_inventory!(self, inventory, boot_id);
        Ok(())
    }
    async fn save_states(&self, states: BTreeMap<OID, DbState>) -> EResult<()> {
        let mut oids = Vec::with_capacity(states.len());
        let mut serialized_states = Vec::with_capacity(states.len());
        for (o, s) in states {
            match serde_json::to_string(&s) {
                Ok(v) => {
                    oids.push(o);
                    serialized_states.push(v);
                }
                Err(e) => error!("unable to serialize state for {}, not saved: {}", o, e),
            }
        }
        sqlx::query(
            r"UPDATE inventory SET state=dt.new_state
                FROM (SELECT UNNEST($1) AS oid, UNNEST($2) as new_state) AS dt
                    WHERE inventory.oid = dt.oid",
        )
        .bind(oids)
        .bind(serialized_states)
        .execute(self)
        .await?;
        Ok(())
    }
    async fn destroy_items(&self, oids: Vec<OID>) -> EResult<()> {
        sqlx::query("DELETE FROM inventory WHERE oid IN (SELECT UNNEST($1))")
            .bind(oids)
            .execute(self)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Storage for Pool<Sqlite> {
    async fn init(&self) -> EResult<()> {
        crate_tables!(self);
        Ok(())
    }
    async fn load_inventory(&self, inventory: &mut Inventory, boot_id: u64) -> EResult<()> {
        load_inventory!(self, inventory, boot_id);
        Ok(())
    }
    async fn save_items(&self, items: Vec<(OID, Value)>) -> EResult<()> {
        let mut qb = sqlx::QueryBuilder::<Sqlite>::new("REPLACE INTO inventory (oid, cfg) ");
        qb.push_values(items, |mut b, (oid, cfg)| {
            b.push_bind(oid);
            b.push_bind(cfg);
        });
        qb.build().execute(self).await?;
        Ok(())
    }
    async fn save_states(&self, states: BTreeMap<OID, DbState>) -> EResult<()> {
        let mut tx = self.begin().await?;
        for (oid, s) in states {
            match serde_json::to_string(&s) {
                Ok(v) => {
                    if let Err(e) = sqlx::query("UPDATE inventory SET state=? WHERE oid=?")
                        .bind(v)
                        .bind(&oid)
                        .execute(&mut tx)
                        .await
                    {
                        error!("unable to save state for {}: {}", oid, e);
                    }
                }
                Err(e) => error!("unable to serialize state for {}, not saved: {}", oid, e),
            }
        }
        tx.commit().await?;
        Ok(())
    }
    async fn destroy_items(&self, oids: Vec<OID>) -> EResult<()> {
        let mut tx = self.begin().await?;
        for oid in oids {
            sqlx::query("DELETE FROM inventory WHERE oid=?")
                .bind(oid)
                .execute(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

struct Database {
    engine: Box<dyn Storage>,
    state_buf: Mutex<BTreeMap<OID, DbState>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    busy: tokio::sync::Mutex<()>,
}

impl Database {
    async fn connect(db_uri: &str, pool_size: u32, timeout: Duration) -> EResult<Arc<Database>> {
        let mut sp = db_uri.split("://");
        macro_rules! db {
            ($engine: expr) => {
                Arc::new(Database {
                    engine: Box::new($engine),
                    state_buf: <_>::default(),
                    worker: <_>::default(),
                    busy: <_>::default(),
                })
            };
        }
        let db: Arc<Database> = match sp.next().unwrap() {
            "postgres" => {
                let mut opts = PgConnectOptions::from_str(db_uri)?;
                opts.log_statements(log::LevelFilter::Trace)
                    .log_slow_statements(log::LevelFilter::Warn, SLOW_STATEMENT);
                let pool = PgPoolOptions::new()
                    .max_connections(pool_size)
                    .acquire_timeout(timeout)
                    .connect_with(opts)
                    .await?;
                db!(pool)
            }
            "sqlite" => {
                let Some(db_path) = sp.next() else {
                    return Err(Error::invalid_params("database path not specified"));
                };
                let sqlite_db_uri: Cow<str> = if db_path.starts_with('/') {
                    db_uri.into()
                } else {
                    format!(
                        "sqlite://{}/runtime/{}",
                        eva_common::tools::get_eva_dir(),
                        db_path
                    )
                    .into()
                };
                let mut opts = SqliteConnectOptions::from_str(&sqlite_db_uri)?;
                opts.log_statements(log::LevelFilter::Trace)
                    .log_slow_statements(log::LevelFilter::Warn, SLOW_STATEMENT);
                opts = opts
                    .create_if_missing(true)
                    .synchronous(SqliteSynchronous::Extra)
                    .busy_timeout(timeout);
                let pool = SqlitePoolOptions::new()
                    .max_connections(pool_size)
                    .acquire_timeout(timeout)
                    .connect_with(opts)
                    .await?;
                db!(pool)
            }
            v => return Err(Error::unsupported(format!("unsupported db kind: {}", v))),
        };
        db.engine.init().await?;
        db.worker.lock().replace(tokio::spawn(db.clone().worker()));
        Ok(db)
    }
    async fn worker(self: Arc<Self>) {
        let mut int = tokio::time::interval(FLUSH_INTERVAL);
        int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            int.tick().await;
            let _lock = self.busy.lock().await;
            let states: BTreeMap<OID, DbState> = mem::take(&mut self.state_buf.lock());
            if !states.is_empty() {
                if let Err(e) = self.engine.save_states(states).await {
                    error!("unable to save: {}, states dropped", e);
                }
            }
        }
    }
    fn is_busy(&self) -> bool {
        self.busy.try_lock().is_err() || !self.state_buf.lock().is_empty()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if let Some(fut) = self.worker.lock().take() {
            fut.abort();
        }
    }
}

pub(crate) async fn init(conn: &str, size: u32, timeout: Duration) -> EResult<()> {
    let db = Database::connect(conn, size, timeout).await?;
    DB.set(db).map_err(|_| Error::core("Unable to set DB"))?;
    Ok(())
}

pub(crate) async fn load(inventory: &mut Inventory, boot_id: u64) -> EResult<()> {
    DB.get()
        .unwrap()
        .engine
        .load_inventory(inventory, boot_id)
        .await?;
    Ok(())
}

pub(crate) fn push_state(oid: OID, state: DbState) {
    DB.get().unwrap().state_buf.lock().insert(oid, state);
}

pub(crate) async fn shutdown() {
    if let Some(db) = DB.get() {
        while db.is_busy() {
            tokio::time::sleep(SLEEP_STEP).await;
        }
    }
}

#[inline]
pub(crate) fn is_initialized() -> bool {
    DB.get().is_some()
}

#[inline]
pub(crate) async fn save_item(oid: OID, config: Value) -> EResult<()> {
    DB.get()
        .unwrap()
        .engine
        .save_items(vec![(oid, config)])
        .await
}

#[inline]
pub(crate) async fn save_items_bulk(configs: Vec<(OID, Value)>) -> EResult<()> {
    DB.get().unwrap().engine.save_items(configs).await
}

#[inline]
pub(crate) async fn destroy_item(oid: OID) -> EResult<()> {
    DB.get().unwrap().engine.destroy_items(vec![oid]).await
}

#[inline]
pub(crate) async fn destroy_items_bulk(oids: Vec<OID>) -> EResult<()> {
    DB.get().unwrap().engine.destroy_items(oids).await
}

//#[async_trait]
//impl Storage for Pool<Sqlite> {
//async fn init(&self) -> EResult<()> {
//crate_tables!(self);
//Ok(())
//}
//}

//lazy_static! {
//static ref POOL: OnceCell<AnyPool> = <_>::default();
//static ref DB_KIND: OnceCell<AnyKind> = <_>::default();
//}

//#[inline]
//pub fn get_pool() -> Option<&'static AnyPool> {
//POOL.get()
//}

//pub async fn init(conn: &str, size: u32, timeout: Duration) -> EResult<()> {
//let mut opts = AnyConnectOptions::from_str(conn)?;
//opts.log_statements(log::LevelFilter::Trace)
//.log_slow_statements(log::LevelFilter::Warn, Duration::from_secs(2));
//if let Some(o) = opts.as_sqlite_mut() {
//opts = o
//.clone()
//.create_if_missing(true)
//.synchronous(sqlite::SqliteSynchronous::Extra)
//.busy_timeout(timeout)
//.into();
//}
//let kind = opts.kind();
//DB_KIND
//.set(kind)
//.map_err(|_| Error::core("unable to set db kind"))?;
//let pool = AnyPoolOptions::new()
//.max_connections(size)
//.acquire_timeout(timeout)
//.connect_with(opts)
//.await?;
//sqlx::query(
//r"CREATE TABLE IF NOT EXISTS inventory(
//oid VARCHAR(768),
//cfg VARCHAR(4096),
//state VARCHAR(8192),
//PRIMARY KEY(oid))
//",
//)
//.execute(&pool)
//.await?;
//POOL.set(pool)
//.map_err(|_| Error::core("unable to set db pool"))
//}

//macro_rules! create {
//($oid: expr, $ex: expr) => {{
//let q = match DB_KIND.get().unwrap() {
//AnyKind::Sqlite | AnyKind::MySql => "INSERT INTO inventory(oid) VALUES(?)",
//AnyKind::Postgres => "INSERT INTO inventory(oid) VALUES($1) ON CONFLICT DO NOTHING",
//};
//let _r = sqlx::query(q).bind($oid.as_str()).execute($ex).await;
//}};
//}
//fn append_item(row: AnyRow, inventory: &mut Inventory, boot_id: u64) -> EResult<()> {
//let oid_str: String = row.try_get("oid")?;
//let oid: OID = oid_str.parse()?;
//debug!("loading {}", oid);
//let cfg_str: String = row.try_get("cfg")?;
//let config_value: serde_json::Value = serde_json::from_str(&cfg_str)?;
//let state_str: Option<String> = row.try_get("state")?;
//let state: Option<ItemState> = if let Some(ref st) = state_str {
//Some(serde_json::from_str(st)?)
//} else {
//None
//};
//inventory.append_item_from_value(&oid, config_value, state, boot_id)?;
//Ok(())
//}

//macro_rules! delete {
//($oid: expr, $ex: expr) => {{
//let q = match DB_KIND.get().unwrap() {
//AnyKind::Sqlite | AnyKind::MySql => "DELETE FROM inventory WHERE oid=?",
//AnyKind::Postgres => "DELETE FROM inventory WHERE oid=$1",
//};
//sqlx::query(q).bind($oid.as_str()).execute($ex).await?;
//}};
//}

//pub async fn destroy(oid: &OID, pool: &AnyPool) -> EResult<()> {
//delete!(oid, pool);
//Ok(())
//}

//pub async fn destroy_tx(oid: &OID, tx: &mut Transaction<'_, sqlx::Any>) -> EResult<()> {
//delete!(oid, tx);
//Ok(())
//}

//macro_rules! set_config {
//($oid: expr, $config: expr, $ex: expr) => {
//let q = match DB_KIND.get().unwrap() {
//AnyKind::Sqlite | AnyKind::MySql => "UPDATE inventory SET cfg=? WHERE oid=?",
//AnyKind::Postgres => "UPDATE inventory SET cfg=$1 WHERE oid=$2",
//};
//sqlx::query(q)
//.bind(serde_json::to_string(&$config)?)
//.bind($oid.as_str())
//.execute($ex)
//.await?;
//};
//}

//pub async fn save_config(oid: &OID, config: Value, pool: &AnyPool) -> EResult<()> {
//create!(oid, pool);
//set_config!(oid, config, pool);
//Ok(())
//}

//pub async fn create_tx(oid: &OID, tx: &mut Transaction<'_, sqlx::Any>) -> EResult<()> {
//create!(oid, tx);
//Ok(())
//}

// Warning: does not create item automatically if missing
//pub async fn save_config_tx(
//oid: &OID,
//config: Value,
//tx: &mut Transaction<'_, sqlx::Any>,
//) -> EResult<()> {
//set_config!(oid, config, tx);
//Ok(())
//}

//macro_rules! set_state {
//($oid: expr, $state: expr, $ex: expr) => {
//let q = match DB_KIND.get().unwrap() {
//AnyKind::Sqlite | AnyKind::MySql => "UPDATE inventory SET state=? WHERE oid=?",
//AnyKind::Postgres => "UPDATE inventory SET state=$1 WHERE oid=$2",
//};
//sqlx::query(q)
//.bind(serde_json::to_string(&$state)?)
//.bind($oid.as_str())
//.execute($ex)
//.await?;
//};
//}

//pub async fn save_state(oid: &OID, state: DbState, pool: &AnyPool) -> EResult<()> {
//set_state!(oid, state, pool);
//Ok(())
//}

//pub async fn save_state_tx(
//oid: &OID,
//state: DbState,
//tx: &mut Transaction<'_, sqlx::Any>,
//) -> EResult<()> {
//set_state!(oid, state, tx);
//Ok(())
//}
