use crate::db::naive_to_ts;
use chrono::NaiveDateTime;
use eva_common::{acl::OIDMask, prelude::*};
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
    fn prepare(&self, s: &str) -> String {
        match self {
            ValueFunction::Mean => {
                format!("locf(avg({}))", s,)
            }
            ValueFunction::Sum => {
                format!("sum({})", s,)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub async fn state_history_combined(
    oid_str: &[&str],
    t_start: f64,
    t_end: Option<f64>,
    fill: Fill,
    precision: Option<u32>,
    mut xopts: BTreeMap<String, Value>,
) -> EResult<BTreeMap<String, Vec<Value>>> {
    let pool = crate::db::POOL.get().unwrap();
    let vfn = if let Some(v) = xopts.remove("vfn") {
        ValueFunction::deserialize(v)?
    } else {
        ValueFunction::default()
    };
    let mut q = "SELECT id, oid FROM state_history_oids WHERE FALSE".to_owned();
    let mut oids = Vec::new();
    for s in oid_str {
        if let Ok(oid) = s.parse::<OID>() {
            oids.push(oid);
        } else if !crate::eva_pg_enabled() {
            return Err(Error::unsupported(
                "OID masks are not supported without eva_pg extension",
            ));
        } else if let Ok(oid_mask) = s.parse::<OIDMask>() {
            write!(q, " OR oid_match(oid, '{}')", oid_mask)?;
        } else {
            return Err(Error::invalid_data("invalid OID/mask"));
        };
    }
    if !oids.is_empty() {
        q = format!(
            "{} OR oid IN ({})",
            q,
            oids.iter()
                .map(|oid| format!("'{}'", oid))
                .collect::<Vec<_>>()
                .join(",")
        );
    };
    let mut rows = sqlx::query(&q).fetch(pool);
    let mut cols = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let id: i32 = row.try_get(0)?;
        let oid: String = row.try_get(1)?;
        cols.push((id, oid));
    }
    if cols.is_empty() {
        return Ok(BTreeMap::new());
    }
    let te = if let Some(t) = t_end {
        t + fill.as_secs_f64()
    } else {
        eva_common::time::now_ns_float()
    };
    let mut subq = String::new();
    for col in &cols {
        if !subq.is_empty() {
            subq.push(',');
        }
        write!(
            subq,
            "{} AS \"{}\"",
            vfn.prepare(&format!("CASE WHEN oid_id={} THEN value END", col.0)),
            col.1
        )?;
    }
    let q = format!(
        r"SELECT CAST(bucket AS TIMESTAMP),* FROM (SELECT
    time_bucket_gapfill(
        '{} seconds'::interval,
        t,
        start=>to_timestamp({}),
        finish=>to_timestamp({})
        ) as bucket,
        {}
FROM state_history_events
WHERE oid_id IN ({})
GROUP BY bucket
ORDER BY bucket
)
",
        fill.as_secs(),
        t_start,
        te,
        subq,
        cols.iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
    let mut rows = sqlx::query(&q).fetch(pool);
    let mut result = BTreeMap::new();
    while let Some(row) = rows.try_next().await? {
        let t_n: NaiveDateTime = row.try_get(0)?;
        let t = naive_to_ts(t_n);
        if let Some(e) = t_end {
            if t > e {
                break;
            }
        }
        result
            .entry("t".to_owned())
            .or_insert_with(Vec::new)
            .push(Value::F64(t));
        for (i, (_, oid)) in cols.iter().enumerate() {
            let val: Option<f64> = row.try_get(i + 2)?;
            if let Some(v) = val {
                result
                    .entry(oid.to_owned())
                    .or_insert_with(Vec::new)
                    .push(Value::F64(v).rounded(precision)?);
            } else {
                result
                    .entry(oid.to_owned())
                    .or_insert_with(Vec::new)
                    .push(Value::Unit);
            }
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub async fn state_history_filled(
    oid_str: &str,
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
    let mut where_oid_q = String::new();
    // try to parse OID
    if let Ok(oid) = oid_str.parse::<OID>() {
        write!(where_oid_q, "WHERE oid='{}'", oid)?;
    } else if !crate::eva_pg_enabled() {
        return Err(Error::unsupported(
            "OID masks are not supported without eva_pg extension",
        ));
    } else if let Ok(oid_mask) = oid_str.parse::<OIDMask>() {
        // try to use OID mask
        write!(where_oid_q, "WHERE oid_match(oid, '{}')", oid_mask)?;
    } else {
        return Err(Error::invalid_data("invalid OID/mask"));
    }
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
    {} AND t>=to_timestamp({}) and t<=to_timestamp({}) GROUP BY period"#,
        cols,
        fill.as_secs(),
        t_start,
        te,
        pq,
        where_oid_q,
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
