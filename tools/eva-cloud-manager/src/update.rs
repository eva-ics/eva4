use crate::manifest::Manifest;
use crate::safe_http;
use clap::Parser;
use colored::Colorize;
use ecm::tools::svc_version_info;
use ecm::{EVA_DIR, REPOSITORY_URL};
use eva_client::VersionInfo;
use eva_common::prelude::*;
use log::{error, info, warn};
use std::cmp::Ordering;
use std::env;
use std::path::Path;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;

#[derive(Parser, Clone, Debug)]
pub struct Options {
    #[clap(short = 'u', long = "repository-url")]
    repository_url: Option<String>,
    #[clap(short = 't', long = "timeout", default_value = "10")]
    pub timeout: f64,
    #[clap(long = "download-timeout")]
    pub download_timeout: Option<f64>,
    #[clap(long = "YES", help = "update without any confirmations")]
    yes: bool,
    #[clap(short = 'i', long = "info-only", help = "get update info only")]
    info_only: bool,
    #[clap(long = "test", help = "update to a build marked as test one")]
    test: bool,
}

#[allow(clippy::too_many_lines)]
pub async fn update(opts: Options) -> EResult<()> {
    let repository_url = opts
        .repository_url
        .unwrap_or_else(|| REPOSITORY_URL.get().unwrap().clone());
    let timeout = Duration::from_secs_f64(opts.timeout);
    info!("using repository {}", repository_url);
    let current_version: VersionInfo = svc_version_info().await?;
    let mut http_client = safe_http::Client::new(timeout);
    let new_version: VersionInfo = if let Ok(ver) = env::var("EVA_UPDATE_FORCE_VERSION") {
        let mut sp = ver.splitn(2, ':');
        let version = sp.next().unwrap().to_owned();
        let build_str = sp
            .next()
            .ok_or_else(|| Error::invalid_params("no build in env var"))?;
        let build: u64 = build_str.parse()?;
        VersionInfo { build, version }
    } else {
        let url = if opts.test {
            format!("{}/update_info_test.json", repository_url)
        } else {
            format!("{}/update_info.json", repository_url)
        };
        info!("fetching the newest version info");
        let res = http_client.fetch_animated(&url).await?;
        serde_json::from_slice(&res)?
    };
    if opts.info_only {
        println!("Current build: {}", format!("{}", current_version).cyan());
        println!(
            "Latest available build: {}",
            format!("{}", new_version).yellow()
        );
        if new_version.build > current_version.build {
            println!("{}", "Update available".green().bold());
        } else {
            println!("{}", "Update not available".yellow().bold());
        }
        return Ok(());
    }
    match new_version.build.cmp(&current_version.build) {
        Ordering::Greater => {}
        Ordering::Less => {
            warn!("Your build is newer than the update server has");
            return Ok(());
        }
        Ordering::Equal => {
            info!("no update available");
            return Ok(());
        }
    }
    let mut lock_path = EVA_DIR.clone();
    lock_path.push("var/update.lock");
    if lock_path.exists() {
        return Err(Error::busy("update is already active"));
    }
    crate::FILE_CLEANER.append(&lock_path);
    fs::write(&lock_path, &[]).await?;
    info!("downloading the update manifest");
    let res = http_client
        .fetch(&format!(
            "{}/{}/nightly/manifest-{}.json",
            repository_url, new_version.version, new_version.build
        ))
        .await?;
    let mut manifest: Manifest = serde_json::from_slice(&res)?;
    if manifest.version_info() != &new_version {
        return Err(Error::invalid_data("manifest build/version mismatch"));
    }
    let mut pub_key_path = EVA_DIR.clone();
    pub_key_path.push("share/eva.key");
    match fs::read(&pub_key_path).await {
        Ok(key) => manifest.set_pub_key(key),
        Err(e) => return Err(Error::failed(format!("unable to read eva.key: {}", e))),
    }
    http_client.set_manifest(manifest);
    info!("current build: {}", current_version);
    info!("new build: {}", new_version);
    if !opts.yes {
        if current_version.version != new_version.version {
            let res = http_client
                .fetch(&format!(
                    "{}/{}/nightly/UPDATE.rst",
                    repository_url, new_version.version
                ))
                .await?;
            let update_info_text = std::str::from_utf8(&res)?;
            println!("\n{}", update_info_text);
        }
        ecm::tools::confirm("update").await?;
    }
    info!("Downloading the distribution");
    let update_script = format!("update-{}.sh", new_version.build);
    let distro = format!(
        "eva-{}-{}-{}.tgz",
        new_version.version,
        new_version.build,
        crate::arch_sfx()
    );
    env::set_current_dir(&*EVA_DIR)?;
    let download_timeout = opts
        .download_timeout
        .map_or_else(|| timeout * 30, Duration::from_secs_f64);
    for file in &[&update_script, &distro] {
        let f = Path::new(file);
        crate::FILE_CLEANER.append(f);
        info!("downloading {}", file);
        ttycarousel::tokio1::spawn0("Downloading").await;
        let res = http_client
            .safe_download(
                &format!(
                    "{}/{}/nightly/{}",
                    repository_url, new_version.version, file
                ),
                f,
                download_timeout,
                true,
            )
            .await;
        ttycarousel::tokio1::stop_clear().await;
        res?;
    }
    warn!("STARTING UPDATE, DO NOT INTERRUPT");
    let mut p = Command::new("bash")
        .arg(&update_script)
        .env("EVA_REPOSITORY_URL", repository_url)
        .env("ARCH_SFX", crate::arch_sfx())
        .spawn()?;
    let res = p.wait().await?;
    if res.code().unwrap_or(0) == 0 {
        Ok(())
    } else {
        error!("FAILED TO APPLY UPDATE! TRYING TO BRING THE NODE BACK ONLINE");
        let mut eva_control = EVA_DIR.clone();
        eva_control.push("sbin/eva-control");
        Command::new(&eva_control)
            .arg("restart")
            .spawn()?
            .wait()
            .await?;
        Err(Error::failed("update failed"))
    }
}
