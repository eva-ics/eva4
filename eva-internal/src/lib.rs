use std::{path::Path, str::FromStr, time::Duration};

use eva_common::{Error, time::Time};

pub struct RtcSyncedInterval {
    inner: rtc_interval::AsyncRtcInterval,
}

impl RtcSyncedInterval {
    pub fn new(d: Duration) -> Self {
        Self {
            inner: rtc_interval::AsyncRtcInterval::new(d),
        }
    }
    pub async fn tick(&mut self) -> Time {
        let t = self.inner.tick().await;
        Time::from_timestamp_ns(u64::try_from(t.as_nanos()).unwrap_or(u64::MAX))
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
