use crate::manifest::Manifest;
use crate::safe_http;
use clap::Parser;
use ecm::tools::svc_version_info;
use ecm::{EVA_DIR, REPOSITORY_URL};
use eva_client::VersionInfo;
use eva_common::prelude::*;
use log::info;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;
use tokio::fs;

#[derive(Parser, Clone, Debug)]
pub struct Options {
    #[clap(short = 'u', long = "repository-url", help = "source repository URL")]
    repository_url: Option<String>,
    #[clap(short = 't', long = "timeout", default_value = "10")]
    pub timeout: f64,
    #[clap(long = "download-timeout")]
    pub download_timeout: Option<f64>,
    #[clap(
        long = "dest",
        help = "destination directory (default: EVA_DIR/mirror)"
    )]
    dest: Option<String>,
    #[clap(long = "force", help = "force download existing files")]
    force: bool,
    #[clap(
        short = 'o',
        long = "current-arch-only",
        help = "download files for the current CPU architecture only"
    )]
    current_arch_only: bool,
}

#[derive(Parser, Clone, Debug)]
pub struct SetOptions {
    #[clap(help = r#"EVA ICS v4 mirror url as http://<ip/host>:port or
"default" to restore the default settings"#)]
    url: String,
}

async fn download(
    client: &safe_http::Client,
    base: &str,
    dest: &Path,
    download_timeout: Duration,
    check: bool,
) -> EResult<()> {
    let fname = dest
        .file_name()
        .ok_or_else(|| Error::invalid_params("invalid file name"))?
        .to_string_lossy();
    let url = format!("{}/{}", base, fname);
    info!("{}", url);
    ttycarousel::tokio1::spawn0("Downloading").await;
    let res = client
        .safe_download(&url, dest, download_timeout, check)
        .await;
    ttycarousel::tokio1::stop_clear().await;
    res
}

#[allow(clippy::large_futures)]
async fn verify_local(path: &Path, manifest: &Manifest) -> EResult<()> {
    ttycarousel::tokio1::spawn0(&format!(
        "Verifying the local file {}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
    .await;
    let res = manifest.verify0(path).await;
    ttycarousel::tokio1::stop_clear().await;
    res
}

#[allow(clippy::large_futures)]
pub async fn update(opts: Options) -> EResult<()> {
    let mut repository_url = opts
        .repository_url
        .unwrap_or_else(|| REPOSITORY_URL.get().unwrap().clone());
    info!("using repository {}", repository_url);
    let timeout = Duration::from_secs_f64(opts.timeout);
    let download_timeout = opts
        .download_timeout
        .map_or_else(|| timeout * 30, Duration::from_secs_f64);
    let dest = opts.dest.map_or_else(
        || Path::new(&format!("{}/mirror", eva_common::tools::get_eva_dir())).to_path_buf(),
        |d| Path::new(&d).to_path_buf(),
    );
    info!("mirror dir: {}", dest.to_string_lossy());
    let ver: VersionInfo = svc_version_info().await?;
    info!("local version: {}", ver);
    write!(repository_url, "/{}/nightly", ver.version).map_err(Error::failed)?;
    let mut dest_eva = dest.clone();
    dest_eva.push("eva");
    let mut update_info_file = dest_eva.clone();
    update_info_file.push("update_info.json");
    dest_eva.push(&ver.version);
    dest_eva.push("nightly");
    fs::create_dir_all(&dest_eva).await?;
    let mut update_rst = dest_eva.clone();
    update_rst.push("UPDATE.rst");
    let mut manifest_file = dest_eva.clone();
    manifest_file.push(format!("manifest-{}.json", ver.build));
    let mut http_client = safe_http::Client::new(timeout);
    for f in &[&update_rst, &manifest_file] {
        if !opts.force && f.exists() {
            info!(
                "{} - exists, skipped",
                f.file_name().unwrap_or_default().to_string_lossy()
            );
        } else {
            download(&http_client, &repository_url, f, download_timeout, false).await?;
        }
    }
    info!("reading the manifest");
    let mut manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_file).await?)?;
    if manifest.version_info() != &ver {
        return Err(Error::invalid_data("manifest build/version mismatch"));
    }
    let mut pub_key_path = EVA_DIR.clone();
    pub_key_path.push("share/eva.key");
    match fs::read(&pub_key_path).await {
        Ok(key) => manifest.set_pub_key(key),
        Err(e) => return Err(Error::failed(format!("unable to read eva.key: {}", e))),
    }
    http_client.set_manifest(manifest.clone());
    let distro = format!("eva-{}-{}-{}.tgz", ver.version, ver.build, crate::ARCH_SFX);
    let distro_p = Path::new(&distro);
    let mut files = manifest.files();
    files.sort();
    for f in files {
        if !opts.current_arch_only
            || !f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with("eva-")
            || f == distro_p
        {
            let mut path = dest_eva.clone();
            path.push(f);
            if !opts.force && path.exists() && verify_local(&path, &manifest).await.is_ok() {
                info!(
                    "{} - exists, skipped",
                    f.file_name().unwrap_or_default().to_string_lossy()
                );
            } else {
                download(&http_client, &repository_url, &path, download_timeout, true).await?;
            }
        }
    }
    info!("creating update_info.json");
    fs::write(update_info_file, serde_json::to_vec(&ver)?).await?;
    let mut banner = dest.clone();
    banner.push("index.html");
    if !banner.exists() {
        info!("creating index.html");
        fs::write(banner, "EVA ICS v4 mirror".as_bytes()).await?;
    }
    Ok(())
}

pub async fn set(opts: SetOptions) -> EResult<()> {
    let mut url: &str = &opts.url;
    let eva_url = if url == "default" {
        url = "https://pub.bma.ai (default)";
        crate::DEFAULT_REPOSITORY_URL.to_owned()
    } else {
        while url.ends_with('/') {
            url = &url[..url.len() - 1];
        }
        let m_url = format!("{}/eva", url);
        if let Err(e) = verify_mirror_url(&m_url).await {
            return Err(Error::failed(format!(
                "Mirror URL verification failed: {}",
                e
            )));
        }
        m_url
    };
    ecm::tools::set_config_field("repository_url", &Value::String(eva_url)).await?;
    info!("mirror URL has been set to {}", url);
    Ok(())
}

async fn verify_mirror_url(url: &str) -> EResult<()> {
    let client = safe_http::Client::new(crate::DEFAULT_TIMEOUT);
    serde_json::from_slice::<VersionInfo>(
        &client.fetch(&format!("{url}/update_info.json")).await?,
    )?;
    Ok(())
}
