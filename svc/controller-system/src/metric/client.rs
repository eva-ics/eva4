use super::Metric;
use crate::common::ClientMetric;
use eva_common::err_logger;
use eva_common::events::EventBuffer;
use eva_common::prelude::*;
use log::{error, info, warn};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[cfg(target_os = "windows")]
mod cert_resolver {
    use eva_common::prelude::*;
    use rustls::{
        client::ResolvesClientCert, pki_types::CertificateDer, sign::CertifiedKey, SignatureScheme,
    };
    use rustls_cng::{signer::CngSigningKey, store::CertStore};
    use std::sync::Arc;

    #[derive(Debug)]
    pub struct ClientCertResolver {
        pub store: CertStore,
        pub cert_name: String,
        pub pin: Option<String>,
    }

    fn get_chain(
        store: &CertStore,
        name: &str,
    ) -> EResult<(Vec<CertificateDer<'static>>, CngSigningKey)> {
        let contexts = store.find_by_subject_name(name).map_err(Error::failed)?;
        let context = contexts
            .first()
            .ok_or_else(|| Error::failed("No client certificate"))?;
        let key = context.acquire_key().map_err(Error::failed)?;
        let signing_key = CngSigningKey::new(key).map_err(Error::failed)?;
        let chain = context
            .as_chain_der()
            .map_err(Error::failed)?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok((chain, signing_key))
    }

    impl ResolvesClientCert for ClientCertResolver {
        fn resolve(
            &self,
            _acceptable_issuers: &[&[u8]],
            sigschemes: &[SignatureScheme],
        ) -> Option<Arc<CertifiedKey>> {
            let (chain, signing_key) = get_chain(&self.store, &self.cert_name).ok()?;
            if let Some(ref pin) = self.pin {
                signing_key.key().set_pin(pin).ok()?;
            }
            for scheme in signing_key.supported_schemes() {
                if sigschemes.contains(scheme) {
                    return Some(Arc::new(CertifiedKey {
                        cert: chain,
                        key: Arc::new(signing_key),
                        ocsp: None,
                    }));
                }
            }
            log::warn!("client cert NOT FOUND");
            None
        }

        fn has_certs(&self) -> bool {
            true
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    server_url: String,
    timeout: Option<f64>,
    #[serde(default)]
    pub fips: bool,
    auth: Auth,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Auth {
    NameKey(NameKeyAuth),
    X509(X509Auth),
}

impl Auth {
    fn add_request_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Auth::NameKey(auth) => req
                .header(crate::HEADER_API_SYSTEM_NAME, &auth.name)
                .header(crate::HEADER_API_AUTH_KEY, &auth.key),
            Auth::X509(_) => req,
        }
    }
    async fn build_client(&self) -> EResult<reqwest::Client> {
        match self {
            Auth::NameKey(_) => Ok(reqwest::Client::new()),
            #[cfg(target_os = "windows")]
            Auth::X509(auth) => {
                let store = rustls_cng::store::CertStore::open(
                    rustls_cng::store::CertStoreType::LocalMachine,
                    auth.store.as_str(),
                )
                .map_err(Error::failed)?;

                let mut root_store = rustls::RootCertStore::empty();

                let load_results = rustls_native_certs::load_native_certs();
                for cert in load_results.certs {
                    if let Err(e) = root_store.add(cert.into()) {
                        warn!("rustls failed to parse DER certificate: {e:?}");
                    }
                }

                let client_config = rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_client_cert_resolver(std::sync::Arc::new(
                        cert_resolver::ClientCertResolver {
                            store,
                            cert_name: format!("CN = {}", auth.cert),
                            pin: None,
                        },
                    ));

                reqwest::ClientBuilder::new()
                    .use_preconfigured_tls(client_config)
                    .build()
                    .map_err(Error::failed)
            }
            #[cfg(not(target_os = "windows"))]
            Auth::X509(auth) => {
                let mut builder = reqwest::Client::builder();
                info!("Loading client TLS certificate from {}", auth.cert_file);
                let cert = tokio::fs::read(&auth.cert_file).await?;
                info!("Loading client TLS key from {}", auth.key_file);
                let key = tokio::fs::read(&auth.key_file).await?;
                builder =
                    builder.identity(reqwest::Identity::from_pkcs8_pem(&cert, &key).map_err(
                        |e| Error::failed(format!("Unable to create TLS idenity: {}", e)),
                    )?);
                builder.build().map_err(Error::failed)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameKeyAuth {
    name: String,
    key: String,
}

#[cfg(target_os = "windows")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct X509Auth {
    store: WindowsCertStore,
    cert: String,
}

#[cfg(target_os = "windows")]
#[derive(Deserialize, Copy, Clone, Debug)]
enum WindowsCertStore {
    My,
    Trust,
}

#[cfg(target_os = "windows")]
impl WindowsCertStore {
    fn as_str(self) -> &'static str {
        match self {
            WindowsCertStore::My => "My",
            WindowsCertStore::Trust => "Trust",
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct X509Auth {
    cert_file: String,
    key_file: String,
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
            warn!("clearing the event buffer");
            EVENT_BUFFER.take();
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
    let client = config.auth.build_client().await?;
    loop {
        int.tick().await;
        let data = EVENT_BUFFER.take();
        if data.is_empty() {
            continue;
        }
        let req = config.auth.add_request_auth(client.post(&server_url_full));
        let response = tokio::time::timeout(timeout, req.json(&data).send())
            .await
            .map_err(|_| Error::timeout())?
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
