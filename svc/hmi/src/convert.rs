use eva_common::prelude::*;
use std::collections::HashMap;

pub enum Js<'a> {
    Var(&'a str),
    Func(&'a str),
}

pub enum Format<'a> {
    Json,
    Yaml,
    Js(Js<'a>),
}

pub struct Convert<'a> {
    uri: &'a str,
    format: Format<'a>,
    lang: Option<&'a str>,
}

impl<'a> Convert<'a> {
    pub fn new0(uri: &'a str) -> Self {
        Self {
            uri,
            format: Format::Json,
            lang: None,
        }
    }
}

pub async fn payload<'a>(
    mut data: Value,
    c: Convert<'a>,
) -> EResult<(Vec<u8>, Option<&'static String>)> {
    if let Some(lang) = c.lang {
        data = crate::I18N
            .get()
            .unwrap()
            .convert(c.uri, data, lang)
            .await?;
    }
    let mime_types = crate::MIME_TYPES.get().unwrap();
    match c.format {
        Format::Json => {
            let buf = serde_json::to_vec(&data)?;
            let mime = mime_types.get("json");
            Ok((buf, mime))
        }
        Format::Yaml => {
            let buf = serde_yaml::to_vec(&data).map_err(Error::invalid_data)?;
            let mime = mime_types.get("yaml");
            Ok((buf, mime))
        }
        Format::Js(js) => {
            let s = serde_json::to_string(&data)?;
            let mime = mime_types.get("js");
            let buf = match js {
                Js::Var(var) => format!("var {}={};", var, s).as_bytes().to_vec(),
                Js::Func(func) => format!("function {}(){{return {};}}", func, s)
                    .as_bytes()
                    .to_vec(),
            };
            Ok((buf, mime))
        }
    }
}

pub fn parse_convert_to<'a>(
    uri: &'a str,
    params: Option<&'a HashMap<String, String>>,
) -> EResult<Option<Convert<'a>>> {
    if let Some(q) = params {
        if let Some(a) = q.get("as") {
            let format = match a.as_str() {
                "yaml" | "yml" => Format::Yaml,
                "json" => Format::Json,
                "js" => {
                    if let Some(var) = q.get("var") {
                        Format::Js(Js::Var(var))
                    } else if let Some(func) = q.get("func") {
                        Format::Js(Js::Func(func))
                    } else {
                        return Err(Error::new0(ErrorKind::InvalidParameter));
                    }
                }
                _ => return Err(Error::new0(ErrorKind::InvalidParameter)),
            };
            return Ok(Some(Convert {
                uri,
                format,
                lang: q.get("lang").map(String::as_str),
            }));
        };
    }
    Ok(None)
}
