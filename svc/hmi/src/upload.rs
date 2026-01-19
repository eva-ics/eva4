use eva_common::hyper_tools::{HContent, HResult};
use eva_common::prelude::*;
use hyper::Body;
use multer::Multipart;
use openssl::sha::Sha256;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;

#[derive(Serialize)]
struct UploadData {
    aci: Value,
    content_type: Option<String>,
    file_name: Option<String>,
    form: BTreeMap<String, String>,
    sha256: String,
    system_name: &'static str,
}

struct UploadFile {
    content: Vec<u8>,
    content_type: Option<String>,
    file_name: Option<String>,
    sha256: [u8; 32],
}

impl UploadFile {
    #[inline]
    fn new(content: Vec<u8>, content_type: Option<String>, file_name: Option<String>) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Self {
            content,
            content_type,
            file_name,
            sha256: hasher.finish(),
        }
    }
}

pub async fn process(
    body: Body,
    boundary: String,
    headers: &hyper::HeaderMap,
    ip: IpAddr,
) -> HResult {
    let mut multipart = Multipart::new(body, boundary);
    let mut process_macro_id: Option<OID> = None;
    let mut rdr: Option<String> = None;
    let mut form = BTreeMap::new();
    let mut params = HashMap::new();
    let mut wait: Option<f64> = None;
    let mut priority: Option<u8> = None;
    let mut ufile: Option<UploadFile> = None;
    while let Some(mut field) = multipart.next_field().await.map_err(Error::invalid_data)? {
        if let Some(name) = field.name().map(ToOwned::to_owned) {
            let mut data = Vec::new();
            while let Some(field_chunk) = field.chunk().await.map_err(Error::invalid_data)? {
                data.extend_from_slice(&field_chunk);
            }
            match name.as_str() {
                "process_macro_id" => {
                    process_macro_id = Some(String::from_utf8(data)?.parse()?);
                }
                "ufile" | "upload_file" => {
                    ufile = Some(UploadFile::new(
                        data,
                        field.content_type().map(ToString::to_string),
                        field.file_name().map(ToOwned::to_owned),
                    ));
                }
                "rdr" | "redirect" => {
                    rdr = Some(String::from_utf8(data)?);
                }
                "k" => {
                    params.insert(name, String::from_utf8(data)?);
                }
                "w" | "wait" => wait = Some(String::from_utf8(data)?.parse()?),
                "p" | "priority" => priority = Some(String::from_utf8(data)?.parse()?),
                _ => {
                    form.insert(name, String::from_utf8(data)?);
                }
            }
        }
    }
    if let Some(u) = ufile
        && let Some(macro_id) = process_macro_id
    {
        let auth = crate::aaa::parse_auth(Some(&params), headers);
        if let Some(ref k) = auth
            && let Ok(auth) = crate::aaa::authenticate(k, Some(ip)).await
        {
            let mut aci = crate::aci::ACI::new(auth, "upload", ip.to_string());
            let upload_data = UploadData {
                aci: to_value(&aci)?,
                content_type: u.content_type,
                file_name: u.file_name,
                sha256: hex::encode(u.sha256),
                form,
                system_name: crate::SYSTEM_NAME.get().unwrap(),
            };
            let mut kwargs = HashMap::new();
            kwargs.insert("content".to_owned(), Value::Bytes(u.content));
            kwargs.insert("data".to_owned(), to_value(upload_data)?);
            let p = crate::api::ParamsRun {
                i: macro_id,
                args: vec![],
                kwargs,
                priority,
                wait,
                note: None,
            };
            let result = crate::api::method_run(to_value(p)?, &mut aci).await?;
            if let Some(uri) = rdr {
                return Ok(HContent::Redirect(uri));
            }
            return Ok(HContent::Value(result));
        }
        return Err(Error::new0(ErrorKind::AccessDenied));
    }
    if let Some(uri) = rdr {
        Ok(HContent::Redirect(uri))
    } else {
        Ok(HContent::not_ok())
    }
}
