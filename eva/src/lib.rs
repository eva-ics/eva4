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

#[cfg(not(target_env = "musl"))]
fn scheduler_param(priority: i32) -> libc::sched_param {
    libc::sched_param {
        sched_priority: priority,
    }
}

#[cfg(target_env = "musl")]
fn scheduler_param(priority: i32) -> libc::sched_param {
    libc::sched_param {
        sched_priority: priority,
        sched_ss_low_priority: 0,
        sched_ss_repl_period: libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        },
        sched_ss_init_budget: libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        },
        sched_ss_max_repl: 0,
    }
}

#[cfg(target_os = "linux")]
pub fn apply_current_thread_params(
    params: &eva_common::services::RealtimeConfig,
    quiet: bool,
) -> EResult<()> {
    let tid = unsafe { i32::try_from(libc::syscall(libc::SYS_gettid)).unwrap_or(-200) };
    let user_id = unsafe { libc::getuid() };
    if !params.cpu_ids.is_empty() {
        if user_id == 0 {
            unsafe {
                let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
                for cpu in &params.cpu_ids {
                    libc::CPU_SET(*cpu, &mut cpuset);
                }
                let res =
                    libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
                if res != 0 {
                    return Err(Error::failed(format!("CPU affinity set error: {}", res)));
                }
            }
        } else if !quiet {
            eprintln!("Core CPU affinity is not set, the core is not launched as root");
        }
    }
    if let Some(priority) = params.priority {
        if user_id == 0 {
            let res = unsafe {
                let sched = if priority == 0 {
                    libc::SCHED_OTHER
                } else {
                    libc::SCHED_FIFO
                };
                libc::sched_setscheduler(tid, sched, &scheduler_param(priority))
            };
            if res != 0 {
                return Err(Error::failed(format!(
                    "Real-time priority set error: {}",
                    res
                )));
            }
        } else if !quiet {
            eprintln!("Core real-time priority is not set, the core is not launched as root");
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
