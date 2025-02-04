use eva_common::err_logger;
use eva_common::events::NodeInfo;
use eva_common::prelude::*;
use log::warn;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;
use std::{path::Path, sync::atomic};

err_logger!();

static FIPS: atomic::AtomicBool = atomic::AtomicBool::new(false);

const ARCH_SFX: &str = env!("ARCH_SFX");

#[macro_use]
extern crate lazy_static;

pub const LOCAL_NODE_ALIAS: &str = ".local";
pub const REMOTE_ANY_NODE_ALIAS: &str = ".remote-any";

pub const PRODUCT_NAME: &str = "EVA ICS node server";
pub const PRODUCT_CODE: &str = "eva4node";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[allow(clippy::unreadable_literal)]
pub const BUILD: u64 = 2025020401;
pub const AUTHOR: &str = "(c) 2022 Bohemia Automation / Altertech";

pub const SYSINFO_CHECK_INTERVAL: Duration = Duration::from_secs(5);
pub const MEMORY_WARN_DEFAULT: u64 = 134_217_728;
use sysinfo::{DiskExt as _, System, SystemExt};

pub static SYSTEM_INFO: Lazy<Mutex<System>> = Lazy::new(|| Mutex::new(System::new()));

pub fn apply_current_thread_params(
    params: &eva_common::services::RealtimeConfig,
    quiet: bool,
) -> EResult<()> {
    let mut rt_params = rtsc::thread_rt::Params::new().with_cpu_ids(&params.cpu_ids);
    if let Some(priority) = params.priority {
        rt_params = rt_params.with_priority(Some(priority));
        if priority > 0 {
            rt_params = rt_params.with_scheduling(rtsc::thread_rt::Scheduling::FIFO);
        }
    }
    if let Err(e) = rtsc::thread_rt::apply_for_current(&rt_params) {
        if matches!(e, rtsc::Error::AccessDenied) {
            if !quiet {
                eprintln!("Real-time parameters are not set, the service is not launched as root");
            }
        } else {
            return Err(Error::failed(format!(
                "Real-time priority set error: {}",
                e
            )));
        }
    }
    if let Some(prealloc_heap) = params.prealloc_heap {
        #[cfg(target_env = "gnu")]
        if let Err(e) = rtsc::thread_rt::preallocate_heap(prealloc_heap) {
            if matches!(e, rtsc::Error::AccessDenied) {
                if !quiet {
                    eprintln!("Heap preallocation failed, the service is not launched as root");
                }
            } else {
                return Err(Error::failed(format!("Heap preallocation error: {}", e)));
            }
        }
        #[cfg(not(target_env = "gnu"))]
        if prealloc_heap > 0 && !quiet {
            eprintln!("Heap preallocation is supported in native builds only");
        }
    }
    Ok(())
}

pub fn launch_sysinfo(full: bool) -> EResult<()> {
    SYSTEM_INFO.lock().refresh_disks_list();
    std::thread::Builder::new()
        .name("EVAsysinfo".to_owned())
        .spawn(move || {
            let mut int = rtsc::time::interval(SYSINFO_CHECK_INTERVAL);
            let rt_config = eva_common::services::RealtimeConfig {
                priority: Some(0),
                cpu_ids: vec![0],
                prealloc_heap: None,
            };
            apply_current_thread_params(&rt_config, true)
                .log_ef_with("sysinfo real-time params apply failed");
            loop {
                int.tick();
                let d = eva_common::tools::get_eva_dir();
                let eva_dir = Path::new(&d);
                let mut s = SYSTEM_INFO.lock();
                s.refresh_memory();
                for disk in s.disks_mut() {
                    if eva_dir.starts_with(disk.mount_point()) {
                        disk.refresh();
                    }
                }
                if full {
                    s.refresh_processes();
                }
            }
        })?;
    Ok(())
}

#[derive(Eq, PartialEq, Copy, Clone, Debug, bmart::tools::EnumStr, Serialize, Default)]
#[enumstr(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Regular,
    SPoint,
    Info,
}

#[inline]
pub fn get_eva_dir() -> String {
    std::env::var("EVA_DIR").unwrap_or_else(|_| "/opt/eva4".to_owned())
}

#[inline]
pub fn get_version() -> &'static str {
    VERSION
}

#[inline]
pub fn get_version_owned() -> String {
    VERSION.to_owned()
}

#[inline]
pub fn get_build() -> u64 {
    BUILD
}

#[inline]
fn get_product_code() -> String {
    crate::PRODUCT_CODE.to_owned()
}

#[inline]
fn get_product_name() -> String {
    crate::PRODUCT_NAME.to_owned()
}

#[inline]
fn local_node_info() -> NodeInfo {
    NodeInfo {
        build: BUILD,
        version: VERSION.to_owned(),
    }
}

#[allow(clippy::cast_precision_loss)]
pub fn check_memory_usage(source: &str, current: u64, warn_limit: u64) {
    if current >= warn_limit {
        warn!(
            "{} memory usage: {} bytes ({:.3} GiB)",
            source,
            current,
            current as f64 / 1_073_741_824.0
        );
    }
}

#[derive(Default)]
pub struct SystemConfig {
    prev_values: BTreeMap<String, String>,
}

impl SystemConfig {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// # Panics
    ///
    /// Should not panic
    pub fn apply(mut self) -> EResult<SystemConfigGuard> {
        let mut map_file = Path::new(&get_eva_dir()).to_owned();
        map_file.push("etc");
        map_file.push("system-config.map");
        if !map_file.exists() {
            return Ok(SystemConfigGuard { config: self });
        }
        let user_id = unsafe { libc::getuid() };
        if user_id != 0 {
            eprintln!("System config map is not applied, the core is not launched as root");
            return Ok(SystemConfigGuard { config: self });
        }
        let map_s = std::fs::read_to_string(map_file)?;
        let mut values: BTreeMap<&str, &str> = BTreeMap::new();
        for line in map_s.split('\n') {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let mut sp2 = line.split('=');
            let key = sp2.next().unwrap().trim();
            if !key.starts_with("/proc") && !key.starts_with("/sys") {
                return Err(Error::invalid_data(format!(
                    "invalid system config map file, key: {}",
                    key
                )));
            }
            let value = sp2.next().ok_or_else(|| {
                Error::invalid_data(format!("invalid system config map file, key: {}", key))
            })?;
            values.insert(key, value);
        }
        for (key, value) in values {
            let prev_value = std::fs::read_to_string(key)?;
            self.prev_values.insert(key.to_owned(), prev_value);
            println!("{} = {}", key, value);
            std::fs::write(key, value)?;
        }
        Ok(SystemConfigGuard { config: self })
    }
}

pub struct SystemConfigGuard {
    config: SystemConfig,
}

impl Drop for SystemConfigGuard {
    fn drop(&mut self) {
        for (key, value) in &self.config.prev_values {
            if let Err(error) = std::fs::write(key, value) {
                warn!("Failed to restore system config {key} = {value}: {error}");
            }
        }
    }
}

pub mod actmgr;
pub mod bus;
pub mod core;
pub mod eapi;
pub mod inventory_db;
pub mod items;
pub mod launcher;
pub mod logs;
pub mod node;
pub mod regsvc;
pub mod seq;
pub mod spoint;
pub mod svc;
pub mod svcmgr;

// BD: 13.08.2021
