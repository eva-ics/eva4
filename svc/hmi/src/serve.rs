use crate::convert::{self, parse_convert_to, Convert};
use eva_common::common_payloads::ParamsId;
use eva_common::hyper_tools::{HContent, HResult};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use hyper::header::HeaderMap;
use hyper_static::serve::static_file;
use log::warn;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use tokio::io::AsyncReadExt;

const DEFAULT_UI_INDEX: &[u8] = include_bytes!("../default/index.html");
const DEFAULT_UI_FAVICON: &[u8] = include_bytes!("../default/favicon.ico");

const MIME_HTML: &str = "text/html";
const MIME_ICO: &str = "image/x-icon";

struct FileData {
    content: &'static [u8],
    mime: &'static str,
}

static DEFAULT_UI_FILES: Lazy<BTreeMap<&'static str, FileData>> = Lazy::new(|| {
    let mut map = BTreeMap::new();
    map.insert(
        "index.html",
        FileData {
            content: DEFAULT_UI_INDEX,
            mime: MIME_HTML,
        },
    );
    map.insert(
        "favicon.ico",
        FileData {
            content: DEFAULT_UI_FAVICON,
            mime: MIME_ICO,
        },
    );
    map
});

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum TplDirKind {
    Ui,
    Pvt,
    No,
}

impl TplDirKind {
    fn allowed(self) -> bool {
        self != TplDirKind::No
    }
}

fn serve_ui_default(allow: bool, path: &str, file_path: &str) -> HResult {
    if allow {
        if let Some(f) = DEFAULT_UI_FILES.get(if path.is_empty() { "index.html" } else { path }) {
            return Ok(HContent::Data(f.content.to_vec(), Some(f.mime), None));
        }
    }
    Err(Error::not_found(file_path))
}

#[allow(clippy::too_many_arguments)]
async fn read_file<'a>(
    uri: &'a str,
    base: &str,
    path: &str,
    allow_default: bool,
    convert_to: Option<Convert<'a>>,
    headers: &HeaderMap,
    ip: IpAddr,
    tpl_dir_kind: TplDirKind,
    allow_ui_default: bool,
) -> HResult {
    let file_path = format!("{}/{}", base, path);
    let attr = match tokio::fs::metadata(&file_path).await {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return serve_ui_default(allow_ui_default, path, &file_path);
        }
        Err(e) => return Err(e.into()),
    };
    if attr.is_dir() && !file_path.ends_with('/') {
        return Ok(HContent::Redirect(format!("{}/", uri)));
    }
    let (p_o, target_is_tera_tpl) = if attr.is_dir() && allow_default {
        let fname = format!("{file_path}index.j2");
        if Path::new(&fname).exists() {
            (Some(fname), tpl_dir_kind.allowed())
        } else {
            (Some(format!("{file_path}index.html")), false)
        }
    } else {
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        (None, tpl_dir_kind.allowed() && file_path.ends_with(".j2"))
    };
    let target = p_o.as_ref().map_or(&file_path, |v| v);
    let target_path = Path::new(target);
    let ext: &str = target_path
        .extension()
        .map_or("", |v| v.to_str().unwrap_or_default());
    let mime_types = crate::MIME_TYPES.get().unwrap();
    let mut mime = mime_types.get(ext);
    let mut buf = if target_is_tera_tpl {
        let tera = match tpl_dir_kind {
            TplDirKind::Ui => crate::tpl::TERA_UI.read(),
            TplDirKind::Pvt => crate::tpl::TERA_PVT.read(),
            TplDirKind::No => return Err(Error::core("tpl error")),
        };
        let context = crate::tpl::request_context(headers, ip)?;
        let tpl_name = &p_o.as_ref().map_or(&file_path, |v| v)[base.len() + 1..];
        let t = tera.render(tpl_name, &context).map_err(Error::failed)?;
        t.as_bytes().to_vec()
    } else if convert_to.is_none() {
        if !target_path.exists() {
            return serve_ui_default(allow_ui_default, path, &file_path);
        }
        return match static_file(
            target_path,
            mime.map(String::as_str),
            headers,
            crate::buf_size(),
        )
        .await
        {
            Ok(v) => Ok(HContent::HyperResult(v)),
            Err(e) => Err(e.into()),
        };
    } else {
        let mut f = match tokio::fs::File::open(target).await {
            Ok(v) => v,
            Err(e) => return Err(e.into()),
        };
        let mut t = Vec::new();
        f.read_to_end(&mut t).await?;
        t
    };
    if let Some(c) = convert_to {
        if let Some(mt) = mime {
            let data: Value = match mt.as_str() {
                "application/json" => {
                    let val: serde_json::Value = serde_json::from_slice(&buf)?;
                    Value::deserialize(val)?
                }
                "application/x-yaml" => {
                    serde_yaml::from_slice(&buf).map_err(Error::invalid_data)?
                }
                _ => {
                    return Err(Error::invalid_params("unable to convert: unsupported type"));
                }
            };
            let (b, m) = convert::payload(data, c).await?;
            buf = b;
            mime = m;
        } else {
            return Err(Error::invalid_params("unable to convert: type unknown"));
        }
    }
    Ok(HContent::Data(buf, mime.map(String::as_str), None))
}

#[allow(clippy::too_many_arguments)]
pub async fn file<'a>(
    uri: &'a str,
    base: &str,
    path: &str,
    params: Option<&'a HashMap<String, String>>,
    allow_default: bool,
    headers: &HeaderMap,
    ip: IpAddr,
    tpl_dir_kind: TplDirKind,
    allow_ui_default: bool,
) -> HResult {
    if path.contains("/../") || path.contains("/.") {
        warn!("invalid entries in uri path: {}", path);
        return Err(Error::not_found(path));
    }
    match parse_convert_to(uri, params) {
        Ok(convert_to) => {
            read_file(
                uri,
                base,
                path,
                allow_default,
                convert_to,
                headers,
                ip,
                tpl_dir_kind,
                allow_ui_default,
            )
            .await
        }
        Err(e) => Err(Error::invalid_params(e)),
    }
}

pub async fn pvt<'a>(
    uri: &'a str,
    pvt_file: &str,
    params: Option<&'a HashMap<String, String>>,
    headers: &hyper::HeaderMap,
    ip: IpAddr,
) -> HResult {
    if pvt_file.contains("/../") || pvt_file.contains("/.") || pvt_file.starts_with("../") {
        warn!("invalid entries in uri path: {}", pvt_file);
        return Err(Error::access(pvt_file));
    }
    if pvt_file.is_empty() {
        return Err(Error::access(pvt_file));
    }
    if let Some(pvt_path) = crate::PVT_PATH.get().unwrap() {
        let auth = crate::aaa::parse_auth(params, headers);
        if let Some(ref k) = auth {
            if let Ok(a) = crate::aaa::authenticate(k, Some(ip)).await {
                if a.acl().check_pvt_read(pvt_file) {
                    return file(
                        uri,
                        pvt_path,
                        pvt_file,
                        params,
                        false,
                        headers,
                        ip,
                        TplDirKind::Pvt,
                        false,
                    )
                    .await;
                }
            }
        }
    }
    Err(Error::access(pvt_file))
}

pub async fn remote_pvt<'a>(
    _uri: &'a str,
    rpvt_file: &str,
    params: Option<&'a HashMap<String, String>>,
    headers: &hyper::HeaderMap,
    ip: IpAddr,
) -> HResult {
    #[derive(Deserialize)]
    struct NodeGetResponse {
        svc: Option<String>,
    }
    #[derive(Serialize)]
    struct ParamsReplRpvt<'a> {
        i: &'a str,
        node: &'a str,
    }
    let target = urlencoding::decode(rpvt_file).map_err(Error::invalid_data)?;
    if target.is_empty() {
        return Err(Error::access(target));
    }
    let auth = crate::aaa::parse_auth(params, headers);
    macro_rules! serve_local {
        ($client: expr, $uri: expr) => {
            return Ok(HContent::HyperResult(Ok($client
                .get_response($uri)
                .await?
                .try_into()?)));
        };
    }
    if let Some(ref k) = auth {
        if let Ok(a) = crate::aaa::authenticate(k, Some(ip)).await {
            if a.acl().check_rpvt_read(target.as_ref()) {
                let mut sp = target.splitn(2, '/');
                let node = sp.next().unwrap();
                let uri = sp
                    .next()
                    .ok_or_else(|| Error::invalid_data("target not specified"))?;
                let client = crate::HTTP_CLIENT.get().unwrap();
                if node == ".local" || node == crate::SYSTEM_NAME.get().unwrap() {
                    serve_local!(client, uri);
                }
                let res =
                    eapi_bus::call("eva.core", "node.get", pack(&ParamsId { i: node })?.into())
                        .await?;
                let p: NodeGetResponse = unpack(res.payload())?;
                if let Some(ref svc) = p.svc {
                    let res: eva_sdk::http::Response = unpack(
                        eapi_bus::call(
                            svc,
                            "rpvt",
                            pack(&ParamsReplRpvt {
                                i: &format!(".local/{uri}"),
                                node,
                            })?
                            .into(),
                        )
                        .await?
                        .payload(),
                    )?;
                    return Ok(HContent::HyperResult(Ok(res.try_into()?)));
                }
                serve_local!(client, uri);
            }
        }
    }
    Err(Error::access(rpvt_file))
}

pub async fn pub_key<'a>(
    uri: &'a str,
    pub_key: &str,
    params: Option<&'a HashMap<String, String>>,
) -> HResult {
    let registry = crate::REG.get().unwrap();
    let data = registry.key_userdata_get(&format!("pub/{pub_key}")).await?;
    let convert_to = parse_convert_to(uri, params)?;
    let (buf, mime) =
        convert::payload(data, convert_to.unwrap_or_else(|| Convert::new0(uri))).await?;
    Ok(HContent::Data(buf, mime.map(String::as_str), None))
}

pub async fn pvt_key<'a>(
    uri: &'a str,
    pvt_key: &str,
    params: Option<&'a HashMap<String, String>>,
    headers: &hyper::HeaderMap,
    ip: IpAddr,
) -> HResult {
    let auth = crate::aaa::parse_auth(params, headers);
    if let Some(ref k) = auth {
        match crate::aaa::authenticate(k, Some(ip)).await {
            Ok(a) => {
                if a.acl().require_pvt_read(&format!("%/{pvt_key}")).is_err() {
                    return Err(Error::access(pvt_key));
                }
            }
            Err(_) => {
                return Err(Error::access(pvt_key));
            }
        }
    } else {
        return Err(Error::access(pvt_key));
    }
    let registry = crate::REG.get().unwrap();
    let data = registry.key_userdata_get(&format!("pvt/{pvt_key}")).await?;
    let convert_to = parse_convert_to(uri, params)?;
    let (buf, mime) =
        convert::payload(data, convert_to.unwrap_or_else(|| Convert::new0(uri))).await?;
    Ok(HContent::Data(buf, mime.map(String::as_str), None))
}
