use parking_lot::Mutex;
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::LazyLock;

const NAME_CACHE_SIZE: usize = 16384;

static NAME_CACHE: LazyLock<Mutex<FIFOCache<String, Arc<String>>>> =
    LazyLock::new(|| Mutex::new(FIFOCache::bounded(NAME_CACHE_SIZE)));

struct FIFOCache<K, V>
where
    K: Ord,
{
    limit: usize,
    keys: VecDeque<K>,
    map: BTreeMap<K, V>,
}

impl<K, V> FIFOCache<K, V>
where
    K: Ord,
{
    fn bounded(limit: usize) -> Self {
        Self {
            limit,
            keys: <_>::default(),
            map: <_>::default(),
        }
    }
    fn insert(&mut self, key: K, value: V)
    where
        K: Clone,
    {
        if self.keys.len() >= self.limit
            && let Some(k) = self.keys.pop_front()
        {
            self.map.remove(&k);
        }
        self.keys.push_back(key.clone());
        self.map.insert(key, value);
    }
    fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.map.get(key)
    }
}

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

fn format_metric_name(s: &str, allow_slash: bool) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric()
            || (allow_slash && ch == '/')
            || eva_common::OID_ALLOWED_SYMBOLS.contains(ch)
        {
            result.push(ch);
        } else {
            result.push_str(crate::REPLACE_UNSUPPORTED_SYMBOLS);
        }
    }
    result
}

pub fn format_name(s: &str, allow_slash: bool) -> Arc<String> {
    let mut cache = NAME_CACHE.lock();
    if let Some(name) = cache.get(s) {
        name.clone()
    } else {
        let name = Arc::new(format_metric_name(s, allow_slash));
        cache.insert(s.to_owned(), name.clone());
        name
    }
}
