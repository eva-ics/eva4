use super::GeneratorSource;
use crate::target::{Target, notify};
use chrono::{Datelike, Local, Timelike, Utc};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
enum Clock {
    #[default]
    Local,
    Utc,
    Monotonic,
}

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
enum Format {
    #[default]
    Timestamp,
    TimestampFloat,
    TimestampNanos,
    Rfc3339,
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default)]
    clock: Clock,
    #[serde(default)]
    format: Format,
}

pub struct GenSource {}

macro_rules! time_val {
    ($format: expr, $clock: ty) => {
        match $format {
            Format::Timestamp => Value::U64(eva_common::time::now()),
            Format::TimestampFloat => Value::F64(eva_common::time::now_ns_float()),
            Format::TimestampNanos => Value::U64(eva_common::time::now_ns()),
            _ => {
                let now = <$clock>::now();
                match $format {
                    Format::Rfc3339 => Value::String(now.to_rfc3339()),
                    Format::Year => Value::I32(now.year()),
                    Format::Month => Value::U32(now.month()),
                    Format::Day => Value::U32(now.day()),
                    Format::Hour => Value::U32(now.hour()),
                    Format::Minute => Value::U32(now.minute()),
                    Format::Second => Value::U32(now.second()),
                    // never reaches
                    Format::Timestamp | Format::TimestampFloat | Format::TimestampNanos => {
                        Value::Unit
                    }
                }
            }
        }
    };
}

#[async_trait::async_trait]
impl GeneratorSource for GenSource {
    async fn start(
        &self,
        name: &str,
        params: Value,
        sampling: f64,
        targets: Arc<Vec<Target>>,
    ) -> EResult<JoinHandle<()>> {
        let params = Params::deserialize(params)?;
        let name = name.to_owned();
        let fut = tokio::spawn(async move {
            let mut int = tokio::time::interval(Duration::from_secs_f64(1.0 / sampling));
            while !svc_is_terminating() {
                int.tick().await;
                let value = match params.clock {
                    Clock::Monotonic => match params.format {
                        // for monotonic clock - output nanoseconds
                        Format::TimestampNanos => Value::U64(eva_common::time::monotonic_ns()),
                        // monotonic float
                        Format::TimestampFloat =>
                        {
                            #[allow(clippy::cast_precision_loss)]
                            Value::F64(eva_common::time::monotonic_ns() as f64 / 1_000_000_000.0)
                        }
                        // or monotonic int for all others
                        _ => Value::U64(eva_common::time::monotonic()),
                    },
                    Clock::Local => time_val!(params.format, Local),
                    Clock::Utc => time_val!(params.format, Utc),
                };
                notify(&name, &targets, value).await;
            }
        });
        Ok(fut)
    }
}
