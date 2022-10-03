use eva_common::prelude::*;
use eva_common::time::{ts_from_ns, ts_to_ns};
use eva_sdk::types::{Fill, HistoricalState, ItemState, StateHistoryData, StateProp};
use futures::stream::{StreamExt, TryStreamExt};
use hyper::{
    client::connect::HttpConnector, header::HeaderMap, Body, Client, Method, Request, StatusCode,
};
use hyper_tls::HttpsConnector;
use log::trace;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ApiVersion {
    V1,
    V2,
}

impl TryFrom<u16> for ApiVersion {
    type Error = Error;
    fn try_from(ver: u16) -> EResult<ApiVersion> {
        match ver {
            1 => Ok(ApiVersion::V1),
            2 => Ok(ApiVersion::V2),
            v => Err(Error::unsupported(format!(
                "api version {} is not supported",
                v
            ))),
        }
    }
}

trait InfluxFillX {
    fn to_influx_fill(&self) -> String;
}

impl InfluxFillX for Fill {
    fn to_influx_fill(&self) -> String {
        match self {
            Fill::Seconds(v) => format!("{}s", v),
            Fill::Minutes(v) => format!("{}m", v),
            Fill::Hours(v) => format!("{}h", v),
            Fill::Days(v) => format!("{}d", v),
            Fill::Weeks(v) => format!("{}w", v),
        }
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone)]
pub struct InfluxClient {
    client: Client<HttpsConnector<HttpConnector>>,
    submit_headers: HeaderMap,
    query_headers: HeaderMap,
    submit_uri: String,
    query_uri: String,
    api_version: ApiVersion,
    v2_from_bucket_range: Option<String>,
}

impl InfluxClient {
    pub fn create(config: &crate::common::Config, timeout: Duration) -> EResult<Self> {
        let influx_api_version = config.api_version.try_into()?;
        let https = HttpsConnector::new();
        let client: Client<_> = Client::builder()
            .http2_only(false)
            .pool_idle_timeout(timeout)
            .build(https);
        let (submit_uri, mut query_headers, query_uri, v2_from_bucket_range) =
            match influx_api_version {
                ApiVersion::V1 => {
                    let mut query_headers = HeaderMap::new();
                    query_headers.insert(
                        hyper::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded"
                            .parse()
                            .map_err(Error::invalid_data)?,
                    );
                    query_headers.insert(
                        hyper::header::ACCEPT,
                        "application/json".parse().map_err(Error::invalid_data)?,
                    );
                    (
                        format!("{}/write?db={}", config.url, config.db),
                        query_headers,
                        format!("{}/query?db={}", config.url, config.db),
                        None,
                    )
                }
                ApiVersion::V2 => {
                    if let Some(ref org) = config.org {
                        let submit_uri = format!(
                            "{}/api/v2/write?bucket={}&org={}&precision=ns",
                            config.url, config.db, org
                        );
                        let query_uri = format!("{}/api/v2/query?org={}", config.url, org);
                        let mut query_headers = HeaderMap::new();
                        query_headers.insert(
                            hyper::header::CONTENT_TYPE,
                            "application/vnd.flux"
                                .parse()
                                .map_err(Error::invalid_data)?,
                        );
                        query_headers.insert(
                            hyper::header::ACCEPT,
                            "application/csv".parse().map_err(Error::invalid_data)?,
                        );
                        (
                            submit_uri,
                            query_headers,
                            query_uri,
                            Some(format!("from(bucket:\"{}\")\n |> range(", config.db)),
                        )
                    } else {
                        return Err(Error::invalid_params("org is not set"));
                    }
                }
            };
        let mut submit_headers = HeaderMap::new();
        submit_headers.insert(
            hyper::header::CONTENT_TYPE,
            "application/octet-stream"
                .to_owned()
                .parse()
                .map_err(Error::invalid_data)?,
        );
        let mut auth_headers = HeaderMap::new();
        if let Some(ref token) = config.token {
            auth_headers.insert(
                hyper::header::AUTHORIZATION,
                format!("Token {}", token)
                    .parse()
                    .map_err(Error::invalid_data)?,
            );
        } else if let Some(ref username) = config.username {
            let b = format!(
                "{}:{}",
                username,
                config
                    .password
                    .as_ref()
                    .map_or(<_>::default(), String::as_str)
            );
            auth_headers.insert(
                hyper::header::AUTHORIZATION,
                format!("Basic {}", base64::encode(b))
                    .parse()
                    .map_err(Error::invalid_data)?,
            );
        }
        query_headers.extend(auth_headers.clone());
        submit_headers.extend(auth_headers);
        Ok(InfluxClient {
            client,
            submit_headers,
            query_headers,
            submit_uri,
            query_uri,
            api_version: influx_api_version,
            v2_from_bucket_range,
        })
    }
    #[allow(clippy::similar_names)]
    pub async fn submit(&self, q: String) -> EResult<()> {
        trace!("submitting data to {}: {}", self.submit_uri, q);
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(&self.submit_uri)
            .body(Body::from(q))
            .map_err(Error::failed)?;
        req.headers_mut().extend(self.submit_headers.clone());
        let status = self
            .client
            .request(req)
            .await
            .map_err(Error::failed)?
            .status();
        if status == StatusCode::OK || status == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(Error::failed(format!(
                "influx server http error code: {}",
                status
            )))
        }
    }
    #[allow(clippy::similar_names)]
    #[allow(clippy::too_many_lines)]
    async fn query(
        &self,
        q: String,
        limit: Option<usize>,
        prop: Option<StateProp>,
    ) -> EResult<Vec<InfluxState>> {
        let mut data = Vec::new();
        trace!("querying data from {}: {}", self.query_uri, q);
        match self.api_version {
            ApiVersion::V1 => {
                #[derive(Deserialize)]
                struct V1Series {
                    values: Option<Vec<Vec<Option<Value>>>>,
                }
                #[derive(Deserialize)]
                struct V1Results {
                    error: Option<Value>,
                    series: Option<Vec<V1Series>>,
                }
                #[derive(Deserialize)]
                struct V1Res {
                    results: Vec<V1Results>,
                }
                let mut req = Request::builder()
                    .method(Method::POST)
                    .uri(&self.query_uri)
                    .body(Body::from(format!(
                        "q={}&epoch=ns",
                        urlencoding::encode(&q)
                    )))
                    .map_err(Error::failed)?;
                req.headers_mut().extend(self.query_headers.clone());
                let res = self.client.request(req).await.map_err(Error::failed)?;
                let status = res.status();
                if status == StatusCode::OK {
                } else {
                    return Err(Error::failed(format!(
                        "influx server http error code: {}",
                        status
                    )));
                }
                let mut result: V1Res = serde_json::from_slice(
                    &hyper::body::to_bytes(res).await.map_err(Error::failed)?,
                )?;
                if result.results.is_empty() {
                    return Err(Error::failed("influx server empty result"));
                }
                let mut r0 = result.results.remove(0);
                if let Some(err) = r0.error.as_ref() {
                    return Err(Error::failed(format!("influx server error: {:?}", err)));
                }
                macro_rules! epop {
                    ($data: expr) => {
                        $data
                            .pop()
                            .ok_or_else(|| Error::new0(ErrorKind::InvalidData))?
                    };
                }
                macro_rules! pop_status {
                    ($data: expr) => {
                        if let Some(z) = epop!($data) {
                            Some(ItemStatus::deserialize(z)?)
                        } else {
                            None
                        }
                    };
                }
                macro_rules! pop_timestamp {
                    ($data: expr) => {
                        u64::deserialize(
                            epop!($data).ok_or_else(|| Error::new0(ErrorKind::InvalidData))?,
                        )?
                    };
                }
                if let Some(mut series) = r0.series.take() {
                    if !series.is_empty() {
                        if let Some(values) = series.remove(0).values {
                            for mut v in values {
                                let (timestamp, status, value) = if let Some(p) = prop {
                                    let (status, value) = match p {
                                        StateProp::Status => (pop_status!(v), None),
                                        StateProp::Value => (None, epop!(v)),
                                    };
                                    let timestamp = pop_timestamp!(v);
                                    (timestamp, status, value)
                                } else {
                                    let value = epop!(v);
                                    let status = pop_status!(v);
                                    let timestamp = pop_timestamp!(v);
                                    (timestamp, status, value)
                                };
                                data.push(InfluxState {
                                    status,
                                    value,
                                    timestamp,
                                });
                            }
                        }
                    }
                }
            }
            ApiVersion::V2 => {
                let mut req = Request::builder()
                    .method(Method::POST)
                    .uri(&self.query_uri)
                    .body(Body::from(q))
                    .map_err(Error::failed)?;
                req.headers_mut().extend(self.query_headers.clone());
                let res = self.client.request(req).await.map_err(Error::failed)?;
                let status = res.status();
                if status == StatusCode::OK {
                } else {
                    return Err(Error::failed(format!(
                        "influx server http error code: {}",
                        status
                    )));
                }
                let stream = res
                    .into_body()
                    .map(|result| {
                        result.map_err(|_| {
                            std::io::Error::new(std::io::ErrorKind::Other, "body read error")
                        })
                    })
                    .into_async_read();
                let rdr = csv_async::AsyncDeserializer::from_reader(stream);
                let mut records = rdr.into_deserialize::<InfluxState>();
                while let Some(record) = records.next().await {
                    let record = record.map_err(Error::invalid_data)?;
                    data.push(record);
                }
            }
        }
        if let Some(l) = limit {
            if data.len() > l {
                data.drain(..data.len() - l - 1);
            }
        }
        Ok(data)
    }
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    pub async fn state_history(
        &self,
        oid: OID,
        t_start: f64,
        t_end: Option<f64>,
        fill: Option<Fill>,
        precision: Option<u32>,
        limit: Option<usize>,
        prop: Option<StateProp>,
        mut xopts: HashMap<String, Value>,
        compact: bool,
    ) -> EResult<StateHistoryData> {
        let mut data = Vec::new();
        let mut sfr = false;
        let vfn = if let Some(v) = xopts.remove("vfn") {
            v.to_alphanumeric_string().map_err(Error::invalid_params)?
        } else {
            "mean".to_owned()
        };
        let q = match self.api_version {
            ApiVersion::V1 => {
                let rp_opt = if let Some(rp) = xopts.remove("rp") {
                    format!("\"{}\".", rp.to_alphanumeric_string()?)
                } else {
                    String::new()
                };
                let (ts, col_str, fill_cond) = if let Some(ref f) = fill {
                    sfr = true;
                    let cstr = if let Some(ref p) = prop {
                        match p {
                            StateProp::Status => "last(status)".to_owned(),
                            StateProp::Value => format!("{}(value)", vfn),
                        }
                    } else {
                        format!("last(status),{}(value)", vfn)
                    };
                    (
                        t_start - f.as_secs_f64(),
                        cstr,
                        format!(" group by time({}) fill(previous)", f.to_influx_fill()),
                    )
                } else {
                    (
                        t_start,
                        prop.as_ref()
                            .map_or("status,value", StateProp::as_str)
                            .to_owned(),
                        String::new(),
                    )
                };
                let mut cond = format!("where time>={}", ts_to_ns(ts));
                if let Some(t_e) = t_end {
                    write!(cond, " and time<={}", ts_to_ns(t_e)).map_err(Error::failed)?;
                }
                format!(
                    "select {} from {}\"{}\" {}{}",
                    col_str, rp_opt, oid, cond, fill_cond
                )
            }
            ApiVersion::V2 => {
                let mut q = self.v2_from_bucket_range.clone().unwrap_or_default();
                let (ts, fill_cond) = if let Some(ref f) = fill {
                    let fill_cond = format!(
                        r#"
 |> aggregateWindow(every: {}, fn: {})
 |> fill(usePrevious: true)"#,
                        f.to_influx_fill(),
                        vfn
                    );
                    ((t_start - f.as_secs_f64()).trunc(), fill_cond)
                } else {
                    (t_start.trunc(), String::new())
                };
                write!(q, "start:{}", ts).map_err(Error::failed)?;
                if let Some(t_e) = t_end {
                    write!(q, ",stop:{}", t_e.trunc()).map_err(Error::failed)?;
                }
                let prop_cond = if let Some(p) = prop {
                    format!(" and r._field == \"{}\"", p.as_str())
                } else {
                    String::new()
                };
                write!(
                    q,
                    ")\n |> filter(fn: (r) => r._measurement == \"{}\"{}){}",
                    oid, prop_cond, fill_cond
                )
                .map_err(Error::failed)?;
                q += r#"
 |> map(fn: (r) => ({ r with timestamp: uint(v: r._time) }))
 |> pivot(rowKey:["timestamp"], columnKey:["_field"], valueColumn:"_value")
 |> drop(columns:["_measurement","_start","_stop"])"#;
                q
            }
        };
        let (need_status, need_value) = prop.as_ref().map_or((true, true), |p| {
            (*p == StateProp::Status, *p == StateProp::Value)
        });
        for s in self.query(q, limit, prop).await? {
            if sfr {
                sfr = false;
            } else {
                data.push(s.into_historical_state(need_status, need_value, precision)?);
            }
        }
        // shift values for influxV2
        if fill.is_some() && self.api_version == ApiVersion::V2 {
            if data.len() < 2 {
                data.pop();
            } else {
                for i in (0..data.len() - 1).rev() {
                    data[i + 1].set_time = data[i].set_time;
                }
                data.remove(0);
            }
        }
        Ok(if compact {
            StateHistoryData::new_compact(data, need_status, need_value)
        } else {
            StateHistoryData::new_regular(data)
        })
    }
    #[allow(clippy::too_many_lines)]
    pub async fn state_log(
        &self,
        oid: OID,
        t_start: f64,
        t_end: Option<f64>,
        limit: Option<usize>,
        mut xopts: HashMap<String, Value>,
    ) -> EResult<Vec<ItemState>> {
        let mut data = Vec::new();
        let q = match self.api_version {
            ApiVersion::V1 => {
                let mut cond = format!("where time>={}", ts_to_ns(t_start));
                if let Some(t_e) = t_end {
                    write!(cond, " and time<={}", ts_to_ns(t_e)).map_err(Error::failed)?;
                }
                let rp_opt = if let Some(rp) = xopts.remove("rp") {
                    format!("\"{}\".", rp.to_alphanumeric_string()?)
                } else {
                    String::new()
                };
                format!("select status,value from {}\"{}\" {}", rp_opt, oid, cond)
            }
            ApiVersion::V2 => {
                let mut q = self.v2_from_bucket_range.clone().unwrap_or_default();
                write!(q, "start:{}", t_start.trunc()).map_err(Error::failed)?;
                if let Some(t_e) = t_end {
                    write!(q, ",stop:{}", t_e.trunc()).map_err(Error::failed)?;
                }
                write!(q, ")\n |> filter(fn: (r) => r._measurement == \"{}\")", oid)
                    .map_err(Error::failed)?;
                q += r#"
 |> map(fn: (r) => ({ r with timestamp: uint(v: r._time) }))
 |> pivot(rowKey:["timestamp"], columnKey:["_field"], valueColumn:"_value")
 |> drop(columns:["_measurement","_start","_stop"])"#;
                q
            }
        };
        for s in self.query(q, limit, None).await? {
            data.push(s.into_item_state(&oid)?);
        }
        Ok(data)
    }
}

// state, collected from InfluxDB (with timestamp in nanoseconds)
#[derive(Deserialize)]
struct InfluxState {
    status: Option<ItemStatus>,
    value: Option<Value>,
    timestamp: u64,
}

impl InfluxState {
    #[inline]
    fn into_item_state(self, oid: &OID) -> EResult<ItemState> {
        Ok(ItemState {
            oid: oid.clone(),
            status: self
                .status
                .ok_or_else(|| Error::invalid_data("item status is null"))?,
            value: self.value,
            set_time: ts_from_ns(self.timestamp),
        })
    }
    #[inline]
    fn into_historical_state(
        self,
        need_status: bool,
        need_value: bool,
        precision: Option<u32>,
    ) -> EResult<HistoricalState> {
        let status = if let Some(status) = self.status {
            Some(Value::I16(status))
        } else if need_status {
            Some(Value::Unit)
        } else {
            None
        };
        let value = if let Some(v) = self.value {
            Some(v.rounded(precision)?)
        } else if need_value {
            Some(Value::Unit)
        } else {
            None
        };
        Ok(HistoricalState {
            status,
            value,
            set_time: ts_from_ns(self.timestamp),
        })
    }
}
