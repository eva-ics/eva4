use eva_common::prelude::*;
use eva_common::registry::format_config_key;
use log::debug;
use serde::Deserialize;

#[derive(Deserialize)]
struct RegistryConfig {
    auto_bak: u64,
    auto_flush: bool,
    cache_size: usize,
    skip_bak: Vec<String>,
    strict_schema: bool,
}

pub fn load(db: &mut yedb::Database) -> EResult<()> {
    let registry_config: RegistryConfig =
        serde_json::from_value(db.key_get(&format_config_key("registry"))?)?;
    debug!("registry.auto_bak = {}", registry_config.auto_bak);
    db.auto_bak = registry_config.auto_bak;
    debug!("registry.auto_flush = {}", registry_config.auto_flush);
    db.auto_flush = registry_config.auto_flush;
    debug!("registry.cache_size = {:?}", registry_config.cache_size);
    db.set_cache_size(registry_config.cache_size);
    debug!("registry.skip_bak = {}", registry_config.skip_bak.join(","));
    db.skip_bak = registry_config.skip_bak;
    debug!("registry.strict_schema = {}", registry_config.strict_schema);
    db.strict_schema = registry_config.strict_schema;
    Ok(())
}
