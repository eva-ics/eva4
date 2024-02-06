use std::time::Duration;

pub const DESCRIPTION: &str = "System service";
pub const SLEEP_STEP_ERR: Duration = Duration::from_secs(1);
pub const REPLACE_UNSUPPORTED_SYMBOLS: &str = "___";

#[cfg(feature = "service")]
pub mod api;
pub mod common;
#[cfg(any(feature = "service", feature = "agent"))]
pub mod metric;
#[cfg(any(feature = "service", feature = "agent"))]
pub mod providers;
pub mod tools;

pub const VAR_SYSTEM_NAME: &str = "${system_name}";
pub const VAR_HOST: &str = "${host}";

pub const HEADER_API_SYSTEM_NAME: &str = "X-System-Name";
pub const HEADER_API_AUTH_KEY: &str = "X-Auth-Key";

pub fn abort() {
    bmart::process::suicide(Duration::from_secs(0), false);
    std::process::exit(1);
}
