#[cfg(any(feature = "service", feature = "agent"))]
use crate::providers;
use eva_common::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientMetric {
    pub i: String,
    #[serde(alias = "s", rename(serialize = "s"))]
    pub status: ItemStatus,
    #[serde(default, alias = "v", rename(serialize = "v"))]
    pub value: ValueOptionOwned,
}

#[cfg(any(feature = "service", feature = "agent"))]
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
    #[cfg(not(target_os = "windows"))]
    launch_provider_worker!(blk);
    launch_provider_worker!(network);
    launch_provider_worker!(exe);
}

#[cfg(any(feature = "service", feature = "agent"))]
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
    #[cfg(not(target_os = "windows"))]
    #[serde(default)]
    blk: providers::blk::Config,
    #[serde(default)]
    network: providers::network::Config,
    #[serde(default)]
    exe: providers::exe::Config,
}

#[cfg(any(feature = "service", feature = "agent"))]
impl ReportConfig {
    pub fn set(self) -> EResult<()> {
        providers::cpu::set_config(self.cpu)?;
        providers::load_avg::set_config(self.load_avg)?;
        providers::memory::set_config(self.memory)?;
        providers::disks::set_config(self.disks)?;
        #[cfg(not(target_os = "windows"))]
        providers::blk::set_config(self.blk)?;
        providers::network::set_config(self.network)?;
        providers::system::set_config(self.system)?;
        providers::exe::set_config(self.exe)?;
        Ok(())
    }
}
