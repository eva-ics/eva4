use crate::db::naive_to_ts;
use chrono::NaiveDateTime;
use eva_common::prelude::*;
use eva_sdk::types::{Fill, HistoricalState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use serde::Deserialize;
use std::{collections::BTreeMap, fmt::Write as _};

use sqlx::Row;

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ValueFunction {
    #[default]
    Mean,
    Sum,
}

impl ValueFunction {
    fn as_str(&self) -> &str {
        match self {
            ValueFunction::Mean => "locf(avg(value)) as value",
            ValueFunction::Sum => "sum(value) as value",
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub async fn state_history_filled(
    oid: OID,
    t_start: f64,
    t_end: Option<f64>,
    fill: Fill,
    precision: Option<u32>,
    limit: Option<usize>,
    prop: Option<StateProp>,
    mut xopts: BTreeMap<String, Value>,
    compact: bool,
) -> EResult<StateHistoryData> {
    let pool = crate::db::POOL.get().unwrap();
    let vfn = if let Some(v) = xopts.remove("vfn") {
        ValueFunction::deserialize(v)?
    } else {
        ValueFunction::default()
    };
    let (cols, pq, need_status, need_value) = if let Some(p) = prop {
        match p {
            StateProp::Status => (
                "status::smallint",
                "locf(avg(status)) as status".to_owned(),
                true,
                false,
            ),
            StateProp::Value => ("value", vfn.as_str().to_owned(), false, true),
        }
    } else {
        (
            "status::smallint,value",
            format!("locf(avg(status)) as status,{}", vfn.as_str()),
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
        r#"SELECT CAST(period AS TIMESTAMP) AS t,{} FROM
(SELECT time_bucket_gapfill(
    '{} seconds'::interval,
    t,
    start=>to_timestamp({}),
    finish=>to_timestamp({})) AS period, {} FROM state_history_events
    JOIN state_history_oids
    ON state_history_events.oid_id=state_history_oids.id
    WHERE oid='{}' AND t>=to_timestamp({}) and t<=to_timestamp({}) GROUP BY period"#,
        cols,
        fill.as_secs(),
        t_start,
        te,
        pq,
        oid,
        t_start,
        te
    );
    if let Some(l) = limit {
        write!(query, " ORDER BY period DESC LIMIT {}", l).map_err(Error::failed)?;
    }
    query += ") AS q1";
    log::trace!("executing query {}", query);
    let mut rows = sqlx::query(&query).fetch(pool);
    let mut data = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let t_n: NaiveDateTime = row.try_get("t")?;
        let t = naive_to_ts(t_n);
        if let Some(e) = t_end {
            if t > e {
                break;
            }
        }
        let status: Option<Value> = if need_status {
            let s: Option<i16> = row.try_get("status")?;
            Some(if let Some(st) = s {
                Value::I16(st)
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
            t_end.unwrap_or_else(eva_common::time::now_ns_float),
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
