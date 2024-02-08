use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;

static NAME_CACHE: Lazy<Mutex<BTreeMap<String, Arc<String>>>> = Lazy::new(<_>::default);

#[allow(clippy::cast_precision_loss)]
pub fn calc_usage<N: Into<u64>>(total: N, free: N) -> f64 {
    let t: u64 = total.into();
    let f: u64 = free.into();
    if t == 0 || f >= t {
        0.0
    } else {
        let used = t - f;
        used as f64 / t as f64 * 100.0
    }
}

fn format_metric_name(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric() || eva_common::OID_ALLOWED_SYMBOLS.contains(ch) {
            result.push(ch);
        } else {
            result.push_str(crate::REPLACE_UNSUPPORTED_SYMBOLS);
        }
    }
    result
}

pub fn format_name(s: &str) -> Arc<String> {
    let mut cache = NAME_CACHE.lock();
    if let Some(name) = cache.get(s) {
        name.clone()
    } else {
        let name = Arc::new(format_metric_name(s));
        cache.insert(s.to_owned(), name.clone());
        name
    }
}
