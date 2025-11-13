use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use eva_common::{EResult, Error};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use log::{debug, error};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinHandle;

async fn fetch_jwks(path: &str) -> EResult<JWKeys> {
    if path.starts_with("http://") || path.starts_with("https://") {
        let res = reqwest::get(path).await.map_err(Error::access)?;
        let status = res.status();
        if !status.is_success() {
            return Err(Error::access(format!(
                "Failed to fetch JWKs: HTTP {}",
                status
            )));
        }
        let data = res.text().await.map_err(Error::access)?;
        let jwks: JWKeys = serde_json::from_str(&data).map_err(Error::invalid_data)?;
        return Ok(jwks);
    }
    let data = tokio::fs::read_to_string(path).await?;
    let jwks: JWKeys = serde_json::from_str(&data).map_err(Error::invalid_data)?;
    Ok(jwks)
}

async fn safe_fetch_jwks(path: &str, timeout: Duration) -> EResult<JWKeys> {
    tokio::time::timeout(timeout, fetch_jwks(path)).await?
}

async fn fetcher(
    jwks: Arc<Mutex<Option<JWKeys>>>,
    path: String,
    timeout: Duration,
    refresh_interval: Duration,
    retry_delay: Duration,
    failed_after: Duration,
) {
    let mut int = tokio::time::interval(refresh_interval);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        int.tick().await;
        let ts = Instant::now();
        loop {
            match safe_fetch_jwks(&path, timeout).await {
                Ok(v) => {
                    let mut guard = jwks.lock();
                    *guard = Some(v);
                    debug!("Successfully updated JWKs from {}", path);
                    break;
                }
                Err(e) => {
                    if failed_after > Duration::ZERO && ts.elapsed() > failed_after {
                        jwks.lock().take();
                    }
                    error!("Failed to fetch JWKs from {}: {}", path, e);
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }
}

pub struct Verifier {
    sub_field: String,
    jwk: Arc<Mutex<Option<JWKeys>>>,
    fetcher: JoinHandle<()>,
}

impl Verifier {
    pub fn create(
        path: &str,
        timeout: Duration,
        refresh_interval: Duration,
        retry_delay: Duration,
        failed_after: Duration,
        sub_field: &str,
    ) -> Self {
        let jwk = Arc::new(Mutex::new(None));
        let handle = tokio::spawn({
            let jwk = jwk.clone();
            let path = path.to_string();
            async move {
                fetcher(
                    jwk,
                    path,
                    timeout,
                    refresh_interval,
                    retry_delay,
                    failed_after,
                )
                .await
            }
        });
        Self {
            sub_field: sub_field.to_string(),
            jwk,
            fetcher: handle,
        }
    }
    pub fn verify(&self, token: &str) -> EResult<String> {
        let guard = self.jwk.lock();
        let jwk = guard
            .as_ref()
            .ok_or_else(|| Error::not_ready("JWKs not loaded"))?;
        check(jwk, token, &self.sub_field).ok_or_else(|| Error::access("JWT verification failed"))
    }
}

impl Drop for Verifier {
    fn drop(&mut self) {
        self.fetcher.abort();
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JWKeys {
    Multiple { keys: Vec<Value> },
    Single(Value),
}

fn check(keys: &JWKeys, token: &str, sub_field: &str) -> Option<String> {
    match keys {
        JWKeys::Multiple { keys } => {
            for jwk in keys {
                match check_jwt(jwk, token, sub_field) {
                    Ok(sub) => return Some(sub),
                    Err(e) => {
                        debug!("JWK {} did not validate token: {}", jwk, e);
                    }
                }
            }
            None
        }
        JWKeys::Single(jwk) => match check_jwt(jwk, token, sub_field) {
            Ok(sub) => Some(sub),
            Err(e) => {
                debug!("JWK did not validate token: {}", e);
                None
            }
        },
    }
}

fn check_jwt(jwk: &Value, token: &str, sub_field: &str) -> EResult<String> {
    let header = decode_header(token).map_err(Error::invalid_data)?;
    let alg = header.alg;

    let decoding_key = match jwk["kty"].as_str() {
        Some("RSA") => {
            let n = jwk["n"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing n"))?;
            let e = jwk["e"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing e"))?;
            DecodingKey::from_rsa_components(n, e).map_err(Error::failed)?
        }
        Some("EC") => {
            let crv = jwk["crv"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing crv"))?;
            let x = jwk["x"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing x"))?;
            let y = jwk["y"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing y"))?;
            match crv {
                "P-256" | "P-384" => {
                    DecodingKey::from_ec_components(x, y).map_err(Error::failed)?
                }
                _ => return Err(Error::failed(format!("Unsupported EC curve: {}", crv))),
            }
        }
        Some("oct") => {
            let k = jwk["k"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing k"))?;
            DecodingKey::from_base64_secret(k).map_err(Error::failed)?
        }
        Some("OKP") => {
            let crv = jwk["crv"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing crv"))?;
            let x = jwk["x"]
                .as_str()
                .ok_or_else(|| Error::invalid_data("JWK missing x"))?;
            match crv {
                "Ed25519" => DecodingKey::from_ed_der(
                    base64_url::decode(x)
                        .map_err(Error::invalid_data)?
                        .as_slice(),
                ),
                _ => {
                    return Err(Error::invalid_data(format!(
                        "Unsupported OKP curve: {}",
                        crv
                    )));
                }
            }
        }
        other => {
            return Err(Error::unsupported(format!(
                "Unsupported key type: {:?}",
                other
            )));
        }
    };

    let validation = Validation::new(alg);

    let claims = decode::<Value>(token, &decoding_key, &validation)
        .map_err(Error::access)?
        .claims;
    let Value::Object(obj) = claims else {
        return Err(Error::invalid_data("Claims is not an object"));
    };
    let sub_value = obj
        .get(sub_field)
        .ok_or_else(|| Error::invalid_data(format!("Missing field: {}", sub_field)))?;
    let sub = String::deserialize(sub_value)
        .map_err(|_| Error::invalid_data(format!("Field {} is not a string", sub_field)))?;
    Ok(sub)
}
