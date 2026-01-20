use std::{
    path::{Path, PathBuf},
    sync::{LazyLock, OnceLock},
};

pub const BUS_CLIENT_NAME: &str = "eva-cloud-manager";

pub mod tools;

pub static EVA_DIR: LazyLock<PathBuf> =
    LazyLock::new(|| Path::new(&eva_common::tools::get_eva_dir()).to_path_buf());
pub static REPOSITORY_URL: OnceLock<String> = OnceLock::new();
