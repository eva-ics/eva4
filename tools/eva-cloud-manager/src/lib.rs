use once_cell::sync::OnceCell;
use std::path::{Path, PathBuf};

pub const BUS_CLIENT_NAME: &str = "eva-cloud-manager";

pub mod tools;

lazy_static::lazy_static! {
    pub static ref EVA_DIR: PathBuf = Path::new(&eva_common::tools::get_eva_dir()).to_path_buf();
    pub static ref REPOSITORY_URL: OnceCell<String> = <_>::default();
}
