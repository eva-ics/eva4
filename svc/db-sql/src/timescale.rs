use eva_common::prelude::*;
use eva_common::time::ts_to_ns;
use eva_sdk::types::{Fill, HistoricalState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use std::fmt::Write as _;

use sqlx::Row;

#[allow(clippy::too_many_arguments)]
pub async fn state_history_filled(
    oid: OID,
    t_start: f64,
    t_end: Option<f64>,
    fill: Fill,
    precision: Option<u32>,
    limit: Option<usize>,
    prop: Option<StateProp>,
    compact: bool,
) -> EResult<StateHistoryData> {
    let pool = crate::db::POOL.get().unwrap();
    let (cols, pq, need_status, need_value) = if let Some(p) = prop {
        match p {
            StateProp::Status => ("status::int", "locf(avg(status)) as status", true, false),
            StateProp::Value => (
                "value",
                "locf(avg(cast(value as double precision))) as value",
                false,
                true,
            ),
        }
    } else {
        (
            "status::int,value",
            "locf(avg(status)) as status,locf(avg(cast(value as double precision))) as value",
            true,
            true,
        )
    };
    let te = if let Some(t) = t_end {
        t + fill.as_secs_f64()
    } else {
        eva_common::time::now_ns_float()
    };
    let mut query = format!(
        r#"SELECT EXTRACT(EPOCH FROM period) AS t,{} FROM
(SELECT time_bucket_gapfill(
    '{} seconds'::interval,
    to_timestamp(t/1000000000),
    start=>to_timestamp({}),
    finish=>to_timestamp({})) AS period, {} FROM state_history
    WHERE oid='{}' AND t>={} and t<={} GROUP BY period"#,
        cols,
        fill.as_secs(),
        t_start,
        te,
        pq,
        oid,
        ts_to_ns(t_start),
        ts_to_ns(te)
    );
    if let Some(l) = limit {
        write!(query, " ORDER BY period DESC LIMIT {}", l).map_err(Error::failed)?;
    }
    query += ") AS q1";
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut data = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let t: f64 = row.try_get("t")?;
        if let Some(e) = t_end {
            if t > e {
                break;
            }
        }
        let status: Option<Value> = if need_status {
            let s: Option<i32> = row.try_get("status")?;
            Some(if let Some(st) = s {
                #[allow(clippy::cast_possible_truncation)]
                Value::I16(st as ItemStatus)
            } else {
                Value::Unit
            })
        } else {
            None
        };
        let value = if need_value {
            let val: Option<f64> = row.try_get("value")?;
            if let Some(v) = val {
                Some(Value::F64(v).rounded(precision)?)
            } else {
                Some(Value::Unit)
            }
        } else {
            None
        };
        let state = HistoricalState {
            set_time: t,
            status,
            value,
        };
        data.push(state);
    }
    if data.is_empty() {
        data = fill.fill_na(
            t_start,
            t_end.unwrap_or_else(|| eva_common::time::now_ns_float()),
            limit,
            need_status,
            need_value,
        );
    } else if limit.is_some() {
        data.reverse();
    }
    Ok(if compact {
        StateHistoryData::new_compact(data, need_status, need_value)
    } else {
        StateHistoryData::new_regular(data)
    })
}
