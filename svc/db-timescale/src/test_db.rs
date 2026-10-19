use std::env;
use std::process;
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Environment variable with the admin Postgres URL for integration tests.
pub const TEST_URL_ENV: &str = "EVA_DB_TIMESCALE_TEST_URL";

static DB_SEQ: AtomicU32 = AtomicU32::new(0);

/// Throwaway database with the timescaledb extension.
///
/// Each test gets its own database: the service filters timescale catalog
/// views by table name only, so schemas in a shared database would see each
/// other's hypertables.
pub struct TestDb {
    /// Pool connected to the throwaway database.
    pub pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDb {
    /// Creates a new throwaway database with the timescaledb extension.
    ///
    /// # Panics
    ///
    /// Panics if [`TEST_URL_ENV`] is unset or database setup fails.
    pub async fn new() -> Self {
        let url = env::var(TEST_URL_ENV).unwrap_or_else(|_| panic!("{TEST_URL_ENV} is not set"));
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let name = format!(
            "eva_ts_test_{}_{}",
            process::id(),
            DB_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
        let opts = PgConnectOptions::from_str(&url).unwrap().database(&name);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query("CREATE EXTENSION IF NOT EXISTS timescaledb")
            .execute(&pool)
            .await
            .unwrap();
        Self { pool, admin, name }
    }

    /// Drops the throwaway database.
    ///
    /// # Panics
    ///
    /// Panics if `DROP DATABASE` fails.
    pub async fn drop_db(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name))
            .execute(&self.admin)
            .await
            .unwrap();
    }
}
