use crate::{db::naive_to_ts, history_table_name};
use chrono::NaiveDateTime;
use eva_common::{acl::OIDMask, prelude::*};
use eva_sdk::types::{Fill, HistoricalState, StateHistoryData, StateProp};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Write as _};

use sqlx::Row;

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ValueFunction {
    #[default]
    Mean,
    Sum,
}

#[derive(Deserialize, Default, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum FillNull {
    #[default]
    None,
    Zero,
    NaN,
    Previous,
}

impl From<FillNull> for NullFiller {
    fn from(kind: FillNull) -> Self {
        NullFiller {
            kind,
            previous: None,
        }
    }
}

struct NullFiller {
    kind: FillNull,
    previous: Option<Value>,
}

impl NullFiller {
    fn format_status(&mut self, maybe_status: Option<i16>, is_first: bool) -> EResult<Value> {
        match self.kind {
            FillNull::None | FillNull::NaN => {
                if let Some(s) = maybe_status {
                    Ok(Value::I16(s))
                } else {
                    Ok(Value::Unit)
                }
            }
            FillNull::Zero => {
                if let Some(s) = maybe_status {
                    Ok(Value::I16(s))
                } else if is_first {
                    Ok(Value::Unit)
                } else {
                    Ok(Value::I16(0))
                }
            }
            FillNull::Previous => {
                if let Some(s) = maybe_status {
                    self.previous = Some(Value::I16(s));
                    Ok(Value::I16(s))
                } else if let Some(ref v) = self.previous {
                    Ok(v.clone())
                } else if is_first {
                    Ok(Value::Unit)
                } else {
                    Ok(Value::I16(0))
                }
            }
        }
    }
    fn format_value(
        &mut self,
        maybe_value: Option<f64>,
        is_first: bool,
        precision: Option<u32>,
    ) -> EResult<Value> {
        match self.kind {
            FillNull::None => {
                if let Some(v) = maybe_value {
                    Ok(Value::F64(v).rounded(precision)?)
                } else {
                    Ok(Value::Unit)
                }
            }
            _ => {
                if let Some(v) = maybe_value {
                    if self.kind == FillNull::Previous {
                        self.previous = Some(Value::F64(v).rounded(precision)?);
                    }
                    Ok(Value::F64(v).rounded(precision)?)
                } else if is_first {
                    Ok(Value::Unit)
                } else {
                    match self.kind {
                        FillNull::Zero => Ok(Value::F64(0.0)),
                        FillNull::NaN => Ok(Value::F64(f64::NAN)),
                        FillNull::Previous => {
                            if let Some(ref v) = self.previous {
                                Ok(v.clone())
                            } else {
                                Ok(Value::F64(0.0))
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
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

#[derive(Serialize, Default)]
pub struct StateHistoryCombinedData {
    t: Vec<f64>,
    data: BTreeMap<String, Vec<Value>>,
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
) -> EResult<StateHistoryCombinedData> {
    let pool = crate::db::POOL.get().unwrap();
    let vfn = if let Some(v) = xopts.remove("vfn") {
        ValueFunction::deserialize(v)?
    } else {
        ValueFunction::default()
    };
    let mut null_filler: NullFiller = if let Some(v) = xopts.remove("fill_null") {
        FillNull::deserialize(v)?.into()
    } else {
        FillNull::default().into()
    };
    let table_name = history_table_name(&xopts)?;
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
        return Ok(<_>::default());
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
FROM {}
WHERE oid_id IN ({})
AND t>=to_timestamp({}) and t<=to_timestamp({})
GROUP BY bucket
ORDER BY bucket
)
",
        fill.as_secs(),
        t_start,
        te,
        subq,
        table_name,
        cols.iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(","),
        t_start,
        te
    );
    let mut rows = sqlx::query(&q).fetch(pool);
    let mut result = StateHistoryCombinedData::default();
    let mut is_first = true;
    while let Some(row) = rows.try_next().await? {
        let t_n: NaiveDateTime = row.try_get(0)?;
        let t = naive_to_ts(t_n);
        if let Some(e) = t_end {
            if t > e {
                break;
            }
        }
        result.t.push(t);
        for (i, (_, oid)) in cols.iter().enumerate() {
            let val: Option<f64> = row.try_get(i + 2)?;
            result
                .data
                .entry(format!("{}/value", oid))
                .or_insert_with(Vec::new)
                .push(null_filler.format_value(val, is_first, precision)?);
        }
        is_first = false;
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
    let mut null_filler: NullFiller = if let Some(v) = xopts.remove("fill_null") {
        FillNull::deserialize(v)?.into()
    } else {
        FillNull::default().into()
    };
    let table_name = history_table_name(&xopts)?;
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
    finish=>to_timestamp({})) AS period, {} FROM {} AS she
    JOIN state_history_oids
    ON she.oid_id=state_history_oids.id
    {} AND t>=to_timestamp({}) and t<=to_timestamp({}) GROUP BY period"#,
        cols,
        fill.as_secs(),
        t_start,
        te,
        pq,
        table_name,
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
    let mut is_first = true;
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
            Some(null_filler.format_status(s, is_first)?)
        } else {
            None
        };
        let value = if need_value {
            let val: Option<f64> = row.try_get("value")?;
            Some(null_filler.format_value(val, is_first, precision)?)
        } else {
            None
        };
        let state = HistoricalState {
            set_time: t,
            status,
            value,
        };
        data.push(state);
        is_first = false;
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
