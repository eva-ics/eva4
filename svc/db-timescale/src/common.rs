use eva_common::acl::OIDMaskList;
use serde::Deserialize;
use std::time::Duration;

#[inline]
fn default_queue_size() -> usize {
    8192
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct Config {
    pub db: String,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub buf_ttl_sec: Option<Duration>,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub interval: Option<Duration>,
    #[serde(default)]
    pub skip_disconnected: bool,
    pub pool_size: Option<u32>,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default)]
    pub ignore_events: bool,
    pub keep: Option<f64>,
    #[serde(default)]
    pub cleanup_oids: bool,
    #[serde(default)]
    pub oids: OIDMaskList,
    #[serde(default)]
    pub oids_exclude: OIDMaskList,
    #[serde(default)]
    pub oids_exclude_null: OIDMaskList,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub panic_in: Option<Duration>,
    #[serde(default)]
    pub eva_pg: bool,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    pub slow_statement_threshold: Option<Duration>,
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use eva_common::prelude::Value;
    use serde::Deserialize;

    use super::Config;

    fn config_value(extra: &[(&str, Value)]) -> Value {
        let mut m = BTreeMap::new();
        m.insert(
            Value::String("db".to_owned()),
            Value::String("postgres://u:p@localhost/d".to_owned()),
        );
        for (k, v) in extra {
            m.insert(Value::String((*k).to_owned()), v.clone());
        }
        Value::Map(m)
    }

    #[test]
    fn slow_statement_threshold_absent_is_none() {
        let config = Config::deserialize(config_value(&[])).unwrap();

        assert_eq!(config.slow_statement_threshold, None);
    }

    #[test]
    fn slow_statement_threshold_parses_float_seconds() {
        let config = Config::deserialize(config_value(&[(
            "slow_statement_threshold",
            Value::F64(0.5),
        )]))
        .unwrap();

        assert_eq!(
            config.slow_statement_threshold,
            Some(Duration::from_millis(500)),
        );
    }
}
