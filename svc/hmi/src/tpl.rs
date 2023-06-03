use eva_sdk::prelude::*;
use hyper::header::HeaderMap;
use lazy_static::lazy_static;
use log::info;
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use tera::{Context, Tera};

lazy_static! {
    pub static ref TERA_UI: RwLock<Tera> = <_>::default();
    pub static ref TERA_PVT: RwLock<Tera> = <_>::default();
}

#[derive(Serialize)]
struct RequestInfo<'a> {
    ip: IpAddr,
    headers: HashMap<&'a str, &'a str>,
}

pub fn request_context(headers: &HeaderMap, ip: IpAddr) -> EResult<Context> {
    let mut context = Context::new();
    let mut hmap: HashMap<&str, &str> = HashMap::new();
    for (k, v) in headers {
        hmap.insert(k.as_str(), v.to_str().map_err(Error::failed)?);
    }
    let request_info = RequestInfo { ip, headers: hmap };
    context
        .try_insert("request", &request_info)
        .map_err(Error::failed)?;
    Ok(context)
}

fn reload_tera_dir(path: &str) -> EResult<Tera> {
    info!("reindexing tera dir {}", path);
    let tera = Tera::new(&format!("{path}/**/*.j2")).map_err(Error::failed)?;
    for t in tera.get_template_names() {
        trace!("tera template: {}", t);
    }
    Ok(tera)
}

pub fn reload_ui(ui_path: &str) -> EResult<()> {
    *TERA_UI.write() = reload_tera_dir(ui_path)?;
    Ok(())
}

pub fn reload_pvt(pvt_path: &str) -> EResult<()> {
    *TERA_PVT.write() = reload_tera_dir(pvt_path)?;
    Ok(())
}
