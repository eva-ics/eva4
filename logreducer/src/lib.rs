use log::Level;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use ttl_cache::TtlCache;

const CAPACITY: usize = 65536;
const TTL: std::time::Duration = std::time::Duration::from_secs(300);

pub static REDUCER: Lazy<LogReducer> = Lazy::new(<_>::default);

pub struct LogReducer {
    cache: Mutex<TtlCache<(Level, String), String>>,
}

impl LogReducer {
    pub fn log(&self, target: &str, level: Level, tag: impl AsRef<str>, message: String) {
        let cache = &mut *self.cache.lock();
        match cache.entry((level, tag.as_ref().to_string())) {
            ttl_cache::Entry::Occupied(mut entry) => {
                if entry.get().as_str() != message {
                    log::log!(target: target, level, "{}", message);
                    entry.insert(message.clone(), TTL);
                }
            }
            ttl_cache::Entry::Vacant(entry) => {
                log::log!(target: target, level, "{}", message);
                entry.insert(message.clone(), TTL);
            }
        }
    }
    pub fn clear_entry(&self, level: Level, tag: impl AsRef<str>) {
        let cache = &mut *self.cache.lock();
        cache.remove(&(level, tag.as_ref().to_string()));
    }
}

impl Default for LogReducer {
    fn default() -> Self {
        LogReducer {
            cache: Mutex::new(TtlCache::new(CAPACITY)),
        }
    }
}

#[macro_export]
macro_rules! log {
    ($level:expr, $tag:expr, $($arg:tt)*) => {
        $crate::REDUCER.log(module_path!(), $level, $tag, format!($($arg)*));
    };
}

#[macro_export]
macro_rules! clear_entry {
    ($level:expr, $tag:expr) => {
        $crate::REDUCER.clear_entry($level, $tag);
    };
}

#[macro_export]
macro_rules! trace {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log!(log::Level::Trace, $tag, $($arg)*);
    };
}

#[macro_export]
macro_rules! debug {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log!(log::Level::Debug, $tag, $($arg)*);
    };
}

#[macro_export]
macro_rules! info {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log!(log::Level::Info, $tag, $($arg)*);
    };
}

#[macro_export]
macro_rules! warn {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log!(log::Level::Warn, $tag, $($arg)*);
    };
}

#[macro_export]
macro_rules! error {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log!(log::Level::Error, $tag, $($arg)*);
    };
}

#[macro_export]
macro_rules! clear_trace {
    ($tag:expr) => {
        $crate::clear_entry!(log::Level::Trace, $tag);
    };
}

#[macro_export]
macro_rules! clear_debug {
    ($tag:expr) => {
        $crate::clear_entry!(log::Level::Debug, $tag);
    };
}

#[macro_export]
macro_rules! clear_info {
    ($tag:expr) => {
        $crate::clear_entry!(log::Level::Info, $tag);
    };
}

#[macro_export]
macro_rules! clear_warn {
    ($tag:expr) => {
        $crate::clear_entry!(log::Level::Warn, $tag);
    };
}

#[macro_export]
macro_rules! clear_error {
    ($tag:expr) => {
        $crate::clear_entry!(log::Level::Error, $tag);
    };
}
