use eva_common::err_logger;
use eva_common::events::NodeInfo;
use eva_common::prelude::*;
use log::warn;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic;
use std::time::Duration;

err_logger!();

static FIPS: atomic::AtomicBool = atomic::AtomicBool::new(false);

const ARCH_SFX: &str = env!("ARCH_SFX");

#[macro_use]
extern crate lazy_static;

pub const LOCAL_NODE_ALIAS: &str = ".local";

pub const PRODUCT_NAME: &str = "EVA ICS node server";
pub const PRODUCT_CODE: &str = "eva4node";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[allow(clippy::unreadable_literal)]
pub const BUILD: u64 = 2024100901;
pub const AUTHOR: &str = "(c) 2022 Bohemia Automation / Altertech";

pub const SYSINFO_CHECK_INTERVAL: Duration = Duration::from_secs(5);
pub const MEMORY_WARN_DEFAULT: u64 = 134_217_728;
use sysinfo::{System, SystemExt};

pub static SYSTEM_INFO: Lazy<Mutex<System>> = Lazy::new(|| Mutex::new(System::new_all()));

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
        if e == rtsc::Error::AccessDenied {
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
            if e == rtsc::Error::AccessDenied {
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

pub fn launch_sysinfo() -> EResult<()> {
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
                let s = System::new_all();
                *SYSTEM_INFO.lock() = s;
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
