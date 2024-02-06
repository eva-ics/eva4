use super::Metric;
use crate::common::ClientMetric;
use eva_common::err_logger;
use eva_common::events::EventBuffer;
use eva_common::prelude::*;
use log::error;
use log::info;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    server_url: String,
    timeout: Option<f64>,
    auth: Auth,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    name: String,
    key: String,
}

const CLIENT_STEP: Duration = Duration::from_millis(200);
const SLEEP_STEP_ERR: Duration = Duration::from_secs(1);

const EVENT_BUFFER_SIZE: usize = 100_000;

static EVENT_BUFFER: Lazy<EventBuffer<ClientMetric>> =
    Lazy::new(|| EventBuffer::bounded(EVENT_BUFFER_SIZE));

err_logger!();

#[cfg(feature = "agent")]
impl Metric {
    #[inline]
    pub fn new0(group: &str, resource: &str) -> Self {
        let i = format!("{}/{}", group, resource);
        Self { i, status: 1 }
    }
    #[inline]
    pub fn new(group: &str, subgroup: &str, resource: &str) -> Self {
        let i = format!("{}/{}/{}", group, subgroup, resource);
        Self { i, status: 1 }
    }
    #[inline]
    pub async fn report<S: Serialize>(&self, value: S) {
        if let Err(e) = self.send_report(value).await {
            error!("unable to bufferize metric event for {}: {}", self.i, e);
            crate::abort();
        }
    }
    #[allow(clippy::unused_async)]
    #[inline]
    pub async fn send_report<S: Serialize>(&self, value: S) -> EResult<()> {
        EVENT_BUFFER.push(self.try_into_client_metric(value)?)?;
        Ok(())
    }
    #[inline]
    fn try_into_client_metric<S: Serialize>(&self, value: S) -> EResult<ClientMetric> {
        Ok(if self.status < 0 {
            ClientMetric {
                i: self.i.clone(),
                status: self.status,
                value: ValueOptionOwned::No,
            }
        } else {
            ClientMetric {
                i: self.i.clone(),
                status: self.status,
                value: ValueOptionOwned::Value(to_value(value)?),
            }
        })
    }
}

pub fn spawn_worker(config: Config) {
    tokio::spawn(async move {
        loop {
            submit_worker(&config)
                .await
                .log_ef_with("submit worker error");
            tokio::time::sleep(SLEEP_STEP_ERR).await;
        }
    });
}

async fn submit_worker(config: &Config) -> EResult<()> {
    let timeout = config
        .timeout
        .map_or(eva_common::DEFAULT_TIMEOUT, Duration::from_secs_f64);
    let server_url = config.server_url.trim_end_matches('/');
    info!("submit worker started, url: {}", server_url);
    let server_url_full = format!("{}/report", server_url);
    let mut int = tokio::time::interval(CLIENT_STEP);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let client = reqwest::Client::new();
    loop {
        int.tick().await;
        let data = EVENT_BUFFER.take();
        if data.is_empty() {
            continue;
        }
        let response = tokio::time::timeout(
            timeout,
            client
                .post(&server_url_full)
                .header(crate::HEADER_API_SYSTEM_NAME, &config.auth.name)
                .header(crate::HEADER_API_AUTH_KEY, &config.auth.key)
                .json(&data)
                .send(),
        )
        .await?
        .map_err(Error::failed)?;
        if !response.status().is_success() {
            return Err(Error::failed(format!(
                "server error HTTP code {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }
    }
}
