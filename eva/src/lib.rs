use eva_common::events::NodeInfo;
use eva_common::prelude::*;
use log::warn;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::sync::atomic;
use std::time::Duration;
use tokio::sync::RwLock;

static FIPS: atomic::AtomicBool = atomic::AtomicBool::new(false);

const ARCH_SFX: &str = env!("ARCH_SFX");

#[macro_use]
extern crate lazy_static;

pub const LOCAL_NODE_ALIAS: &str = ".local";

pub const PRODUCT_NAME: &str = "EVA ICS node server";
pub const PRODUCT_CODE: &str = "eva4node";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[allow(clippy::unreadable_literal)]
pub const BUILD: u64 = 2024013001;
pub const AUTHOR: &str = "(c) 2022 Bohemia Automation / Altertech";

pub const SYSINFO_CHECK_INTERVAL: Duration = Duration::from_secs(5);
pub const MEMORY_WARN_DEFAULT: u64 = 134_217_728;
use sysinfo::{System, SystemExt};

pub static SYSTEM_INFO: Lazy<RwLock<System>> = Lazy::new(|| RwLock::new(System::new_all()));

fn launch_sysinfo() {
    tokio::spawn(async move {
        let mut int = tokio::time::interval(SYSINFO_CHECK_INTERVAL);
        loop {
            int.tick().await;
            {
                let mut s = SYSTEM_INFO.write().await;
                s.refresh_processes();
                s.refresh_disks_list();
            }
        }
    });
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
