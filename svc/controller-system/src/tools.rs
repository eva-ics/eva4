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

pub fn format_name(s: &str) -> String {
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
