use crate::providers;
use eva_common::prelude::*;
use serde::Deserialize;

pub const AUTHOR: &str = "Bohemia Automation";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn spawn_workers() {
    macro_rules! launch_provider_worker {
        ($mod: ident) => {
            tokio::spawn(providers::$mod::report_worker());
        };
    }
    launch_provider_worker!(system);
    launch_provider_worker!(cpu);
    launch_provider_worker!(load_avg);
    launch_provider_worker!(memory);
    launch_provider_worker!(disks);
    launch_provider_worker!(network);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportConfig {
    #[cfg(feature = "service")]
    pub oid_prefix: String,
    #[serde(default)]
    system: providers::system::Config,
    #[serde(default)]
    cpu: providers::cpu::Config,
    #[serde(default)]
    load_avg: providers::load_avg::Config,
    #[serde(default)]
    memory: providers::memory::Config,
    #[serde(default)]
    disks: providers::disks::Config,
    #[serde(default)]
    network: providers::network::Config,
}

impl ReportConfig {
    pub fn set(self) -> EResult<()> {
        providers::cpu::set_config(self.cpu)?;
        providers::load_avg::set_config(self.load_avg)?;
        providers::memory::set_config(self.memory)?;
        providers::disks::set_config(self.disks)?;
        providers::network::set_config(self.network)?;
        providers::system::set_config(self.system)?;
        Ok(())
    }
}