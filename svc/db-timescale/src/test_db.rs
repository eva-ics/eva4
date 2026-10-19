use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, process};

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

pub const TEST_URL_ENV: &str = "EVA_DB_TIMESCALE_TEST_URL";

static DB_SEQ: AtomicU32 = AtomicU32::new(0);

pub struct TestDb {
    pub pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDb {
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

    pub async fn drop_db(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name))
            .execute(&self.admin)
            .await
            .unwrap();
    }
}
