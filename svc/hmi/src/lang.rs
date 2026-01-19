use eva_common::prelude::*;
use gettext::Catalog;
use log::{info, trace};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub struct Converter {
    locale_dirs: Vec<PathBuf>,
    catalogs: Mutex<BTreeMap<PathBuf, Arc<Catalog>>>,
}

impl Converter {
    pub fn append_dir(&mut self, dir: &str) {
        self.locale_dirs.push(PathBuf::from(dir));
    }
    fn find_mo(d: &Path, u: &Path, lang: &str) -> Option<PathBuf> {
        let mut path = d.to_path_buf();
        let mut uri_path = u.to_path_buf();
        path.extend(&PathBuf::from(format!("{}/LC_MESSAGES", lang)));
        path.extend(&uri_path);
        let mut mo_path = path.with_extension("mo");
        trace!("i18n trying {}", mo_path.display());
        if mo_path.exists() {
            return Some(mo_path);
        }
        uri_path.pop();
        loop {
            mo_path.pop();
            mo_path.push("messages.mo");
            trace!("i18n trying {}", mo_path.display());
            if mo_path.exists() {
                return Some(mo_path);
            } else if !uri_path.pop() {
                break;
            }
            mo_path.pop();
        }
        trace!("i18n giving up");
        None
    }
    pub async fn convert(&self, uri: &str, value: Value, lang: &str) -> EResult<Value> {
        trace!("i18n {} {}", uri, lang);
        let u_path = Path::new(uri);
        let u: PathBuf = u_path.iter().skip(2).collect();
        for d in &self.locale_dirs {
            if let Some(mo_path) = Self::find_mo(d, &u, lang) {
                let cat = self.get_cat(&mo_path).await?;
                let value = Self::convert_value(value, &cat);
                return Ok(value);
            }
        }
        Ok(value)
    }
    async fn get_cat(&self, mo_path: &Path) -> EResult<Arc<Catalog>> {
        if let Some(cat) = self.catalogs.lock().get(mo_path) {
            return Ok(cat.clone());
        }
        let data = tokio::fs::read(mo_path).await?;
        let buf = std::io::Cursor::new(data);
        let cat = Arc::new(Catalog::parse(buf).map_err(Error::invalid_data)?);
        self.catalogs
            .lock()
            .insert(mo_path.to_path_buf(), cat.clone());
        Ok(cat)
    }
    pub fn cache_purge(&self) {
        info!("cleaning i18n cache");
        self.catalogs.lock().clear();
    }
    fn convert_value(value: Value, cat: &Catalog) -> Value {
        if let Value::String(s) = value {
            Value::String(Self::convert_str(s, cat))
        } else if let Value::Seq(s) = value {
            Value::Seq(
                s.into_iter()
                    .map(|value| Self::convert_value(value, cat))
                    .collect(),
            )
        } else if let Value::Map(m) = value {
            let mut result = BTreeMap::new();
            for (k, v) in m {
                result.insert(k, Self::convert_value(v, cat));
            }
            Value::Map(result)
        } else {
            value
        }
    }
    fn convert_str(mut s: String, cat: &Catalog) -> String {
        s.retain(|c| c != '\r');
        let mut result = Vec::new();
        for line in s.split('\n') {
            if line.is_empty() {
                result.push(String::new());
            } else {
                let ls = line.trim_start();
                let rs = line.trim_end();
                let left_side = if ls.len() == line.len() {
                    ""
                } else {
                    &line[..line.len() - ls.len()]
                };
                let right_side = if rs.len() == line.len() {
                    ""
                } else {
                    &line[line.len() - rs.len()..]
                };
                result.push(format!(
                    "{}{}{}",
                    left_side,
                    cat.gettext(line.trim()),
                    right_side
                ));
            }
        }
        result.join("\n")
    }
}
