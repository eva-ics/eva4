use crate::manifest::Manifest;
use eva_common::op::Op;
use eva_common::{EResult, Error};
use hyper::{body::HttpBody, client::HttpConnector, Body, Response, StatusCode, Uri};
use hyper_tls::HttpsConnector;
use log::info;
use openssl::sha::Sha256;
use std::path::Path;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub trait IsUrl {
    fn is_url(&self) -> bool;
}

impl IsUrl for std::path::PathBuf {
    #[inline]
    fn is_url(&self) -> bool {
        self.starts_with("http://") || self.starts_with("https://")
    }
}

impl IsUrl for std::path::Path {
    #[inline]
    fn is_url(&self) -> bool {
        self.starts_with("http://") || self.starts_with("https://")
    }
}

pub struct Client {
    timeout: Duration,
    client: hyper::Client<HttpsConnector<HttpConnector>>,
    manifest: Option<Manifest>,
    max_redirects: usize,
}

impl Client {
    pub fn new(timeout: Duration) -> Self {
        let https = HttpsConnector::new();
        let client: hyper::Client<_> = hyper::Client::builder()
            .pool_idle_timeout(timeout)
            .build(https);
        Self {
            timeout,
            client,
            manifest: None,
            max_redirects: 0,
        }
    }
    pub fn set_manifest(&mut self, manifest: Manifest) {
        self.manifest = Some(manifest);
    }
    pub fn allow_redirects(&mut self) {
        self.max_redirects = 10;
    }
    async fn get_url(&self, url: &str, timeout: Duration) -> EResult<Response<Body>> {
        let op = Op::new(timeout);
        let mut target_uri: Uri = {
            if url.starts_with("http://") || url.starts_with("https://") {
                url.parse()
            } else {
                format!("http://{url}").parse()
            }
        }
        .map_err(|e| Error::invalid_params(format!("invalid url {}: {}", url, e)))?;
        let mut rdr = 0;
        loop {
            let res = tokio::time::timeout(op.timeout()?, self.client.get(target_uri.clone()))
                .await?
                .map_err(Error::io)?;
            if self.max_redirects > 0
                && (res.status() == StatusCode::MOVED_PERMANENTLY
                    || res.status() == StatusCode::TEMPORARY_REDIRECT
                    || res.status() == StatusCode::FOUND)
            {
                if rdr > self.max_redirects {
                    return Err(Error::io("too many redirects"));
                }
                rdr += 1;
                if let Some(loc) = res.headers().get(hyper::header::LOCATION) {
                    let location_uri: Uri = loc
                        .to_str()
                        .map_err(|e| Error::invalid_params(format!("invalid redirect url: {e}")))?
                        .parse()
                        .map_err(|e| Error::invalid_params(format!("invalid redirect url: {e}")))?;
                    let loc_parts = location_uri.into_parts();
                    let mut parts = target_uri.into_parts();
                    if loc_parts.scheme.is_some() {
                        parts.scheme = loc_parts.scheme;
                    }
                    if loc_parts.authority.is_some() {
                        parts.authority = loc_parts.authority;
                    }
                    parts.path_and_query = loc_parts.path_and_query;
                    target_uri = Uri::from_parts(parts)
                        .map_err(|e| Error::invalid_params(format!("invalid redirect url: {e}")))?;
                } else {
                    return Err(Error::io("invalid redirect"));
                }
            } else {
                return Ok(res);
            }
        }
    }
    pub async fn fetch_animated(&self, url: &str) -> EResult<Vec<u8>> {
        info!("GET {}", url);
        ttycarousel::tokio1::spawn0("Downloading").await;
        let res = self.fetch(url).await;
        ttycarousel::tokio1::stop_clear().await;
        res
    }
    pub async fn fetch(&self, url: &str) -> EResult<Vec<u8>> {
        let op = eva_common::op::Op::new(self.timeout);
        match self.get_url(url, op.timeout()?).await {
            Ok(v) => {
                if v.status() == StatusCode::OK {
                    Ok(
                        tokio::time::timeout(op.timeout()?, hyper::body::to_bytes(v))
                            .await?
                            .map_err(Error::io)?
                            .to_vec(),
                    )
                } else {
                    Err(Error::io(format!("Server response: {}", v.status())))
                }
            }
            Err(e) => Err(Error::io(format!("unable to fetch {}: {}", url, e))),
        }
    }
    pub async fn safe_download(
        &self,
        url: &str,
        dest: &Path,
        download_timeout: Duration,
        check: bool,
    ) -> EResult<()> {
        let manifest = if check {
            if let Some(ref mc) = self.manifest {
                if !mc.contains(dest)? {
                    return Err(Error::failed(format!(
                        "no entry for {} in the manifest",
                        dest.to_string_lossy()
                    )));
                }
                Some(mc)
            } else {
                return Err(Error::invalid_params("manifest is not set in client"));
            }
        } else {
            None
        };
        let op = eva_common::op::Op::new(download_timeout);
        match self.get_url(url, op.timeout()?).await {
            Ok(mut res) => {
                if res.status() == StatusCode::OK {
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(dest)
                        .await?;
                    let mut size = 0;
                    let mut hasher = Sha256::new();
                    while let Some(chunk) =
                        tokio::time::timeout(op.timeout()?, res.body_mut().data()).await?
                    {
                        let chunk = chunk.map_err(Error::io)?;
                        size += chunk.len() as u64;
                        hasher.update(&chunk);
                        file.write_all(&chunk).await?;
                    }
                    if let Some(m) = manifest {
                        m.verify(dest, size, &hasher.finish())?;
                    }
                    Ok(())
                } else {
                    Err(Error::io(format!("Server response: {}", res.status())))
                }
            }
            Err(e) => Err(Error::io(format!("unable to download {}: {}", url, e))),
        }
    }
}
