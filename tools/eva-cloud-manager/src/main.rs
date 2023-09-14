use log::{error, info, warn};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};

mod args;
mod cloud_update;
mod deploy;
mod dump;
mod manifest;
mod mirror;
mod safe_http;
mod update;

pub const DEFAULT_REPOSITORY_URL: &str = "https://pub.bma.ai/eva4";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

const ARCH_SFX: &str = env!("ARCH_SFX");

#[derive(Default)]
pub struct FileCleaner {
    paths: Mutex<HashSet<PathBuf>>,
}

impl FileCleaner {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    #[inline]
    pub fn append(&self, path: &Path) {
        self.paths.lock().unwrap().insert(path.to_owned());
    }
    /// # Panics
    ///
    /// Will panic if the mutex is poisoned
    pub fn clear(&self) {
        let mut paths = self.paths.lock().unwrap();
        for p in &*paths {
            let _r = std::fs::remove_file(p);
        }
        paths.clear();
    }
}

lazy_static::lazy_static! {
    pub static ref FILE_CLEANER: FileCleaner = <_>::default();
    pub static ref DEFAULT_CONNECTION_PATH: String = {
        let mut p = ecm::EVA_DIR.clone();
        p.push("var/bus.ipc");
        p.to_string_lossy().to_string()
    };
}

#[tokio::main(worker_threads = 2)]
async fn main() {
    macro_rules! handle_signal {
        ($signal: expr) => {{
            tokio::spawn(async move {
                signal($signal).unwrap().recv().await;
                FILE_CLEANER.clear();
                ttycarousel::tokio1::stop_clear().await;
                warn!("operation aborted");
                std::process::exit(1);
            });
        }};
    }
    handle_signal!(SignalKind::terminate());
    handle_signal!(SignalKind::hangup());
    handle_signal!(SignalKind::interrupt());
    let args = args::Args::parse_args();
    env_logger::Builder::new()
        .target(env_logger::Target::Stdout)
        .filter_level(if args.verbose {
            log::LevelFilter::Trace
        } else {
            log::LevelFilter::Info
        })
        .init();
    let mut quiet = false;
    let exit_code = if let Err(e) = match args.command {
        args::Command::Cloud(cloud_command) => match cloud_command {
            args::CloudCommand::Deploy(opts) => deploy::deploy_undeploy(opts, true).await,
            args::CloudCommand::Undeploy(opts) => deploy::deploy_undeploy(opts, false).await,
            args::CloudCommand::Update(opts) => cloud_update::cloud_update(opts).await,
        },
        args::Command::Node(node_command) => {
            ecm::tools::load_config().await.unwrap();
            match node_command {
                args::NodeCommand::Update(opts) => update::update(opts).await,
                args::NodeCommand::MirrorUpdate(opts) => mirror::update(opts).await,
                args::NodeCommand::MirrorSet(opts) => mirror::set(opts).await,
                args::NodeCommand::Version => match ecm::tools::svc_version_info().await {
                    Ok(v) => {
                        println!("build: {}", v.build);
                        println!("version: {}", v.version);
                        quiet = true;
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                args::NodeCommand::Dump(opts) => dump::dump(opts).await,
            }
        }
    } {
        error!("operation failed: {}", e);
        1
    } else {
        if !quiet {
            info!("operation completed");
        }
        0
    };
    FILE_CLEANER.clear();
    std::process::exit(exit_code);
}
