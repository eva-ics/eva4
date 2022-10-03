use crate::logs::LogLevel;

#[inline]
pub fn emit(lvl: LogLevel, service: &str, msg: &str) {
    log::log!(lvl.into(), "{} {}", service, msg);
}
