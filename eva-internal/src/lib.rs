use std::{path::Path, str::FromStr, time::Duration};

use eva_common::{
    Error,
    time::{Time, now},
};
use tokio::time::Interval;

pub struct RtcSyncedInterval {
    interval: Interval,
    d: Duration,
    tick_prev: u64,
}

impl RtcSyncedInterval {
    pub fn new(d: Duration) -> Self {
        let mut interval = tokio::time::interval(if d >= Duration::from_secs(1) {
            Duration::from_millis(200)
        } else {
            d
        });
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            interval,
            d,
            tick_prev: 0,
        }
    }
    pub async fn tick(&mut self) -> Time {
        if self.d >= Duration::from_secs(1) {
            loop {
                self.interval.tick().await;
                let t_secs = now();
                if t_secs.is_multiple_of(self.d.as_secs()) && t_secs != self.tick_prev {
                    self.tick_prev = t_secs;
                    #[allow(clippy::cast_precision_loss)]
                    break Time::from_timestamp(t_secs as f64);
                }
            }
        } else {
            self.interval.tick().await;
            Time::now()
        }
    }
}

/// for legacy storage implementations only
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbKind {
    Sqlite,
    Postgres,
}

impl DbKind {
    pub async fn create_if_missing(&self, uri: &str) -> Result<(), Error> {
        match self {
            DbKind::Sqlite => {
                let path = Path::new(
                    uri.split_once("://")
                        .ok_or_else(|| Error::unsupported("invalid sqlite uri"))?
                        .1,
                );
                if !path.exists() {
                    tokio::fs::File::create(path).await?;
                }
                Ok(())
            }
            DbKind::Postgres => Ok(()),
        }
    }
}

impl FromStr for DbKind {
    type Err = Error;

    fn from_str(uri: &str) -> Result<Self, Self::Err> {
        let s_lower = uri.to_lowercase();
        if s_lower.starts_with("sqlite:") {
            Ok(DbKind::Sqlite)
        } else if s_lower.starts_with("postgres:") || s_lower.starts_with("postgresql:") {
            Ok(DbKind::Postgres)
        } else {
            Err(Error::unsupported("unsupported database kind"))
        }
    }
}
