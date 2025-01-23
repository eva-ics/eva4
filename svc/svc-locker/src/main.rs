use bmart::sync::SharedLockFactory;
use eva_common::common_payloads::ParamsIdOwned;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Shared locker service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
    default_timeout: Duration,
    factory: SharedLockFactory,
}

#[derive(Serialize, bmart::tools::Sorting)]
struct LockInfo<'a> {
    id: &'a str,
    locked: bool,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "lock" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct LockParams {
                        i: String,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
                        expires: Duration,
                    }
                    let p: LockParams = unpack(payload)?;
                    tokio::time::timeout(
                        p.timeout.unwrap_or(self.default_timeout),
                        self.factory.acquire(&p.i, p.expires),
                    )
                    .await
                    .map_err(Into::<Error>::into)?
                    .map_err(Error::failed)?;
                    debug!(
                        "lock {} acquired for {:?} by {}",
                        p.i,
                        p.expires,
                        event.primary_sender()
                    );
                    Ok(None)
                }
            }
            "unlock" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdOwned = unpack(payload)?;
                    self.factory
                        .release(&p.i, None)
                        .await
                        .map_err(Error::core)?;
                    debug!("lock {} released by {}", p.i, event.primary_sender());
                    Ok(None)
                }
            }
            "status" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdOwned = unpack(payload)?;
                    let info = LockInfo {
                        id: &p.i,
                        locked: self.factory.status(&p.i).map_err(Error::failed)?,
                    };
                    Ok(Some(pack(&info)?))
                }
            }
            "list" => {
                if payload.is_empty() {
                    let mut result: Vec<LockInfo> = self
                        .factory
                        .list()
                        .iter()
                        .map(|v| LockInfo {
                            id: v.0,
                            locked: v.1,
                        })
                        .collect();
                    result.sort();
                    Ok(Some(pack(&result)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    locks: HashSet<String>,
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    // get config from initial and deserialize it
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    // init rpc
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("lock")
            .required("i")
            .required("expires")
            .optional("timeout"),
    );
    info.add_method(ServiceMethod::new("unlock").required("i"));
    info.add_method(ServiceMethod::new("status").required("i"));
    info.add_method(ServiceMethod::new("list"));
    let mut factory = SharedLockFactory::new();
    for lock in &config.locks {
        factory.create(lock).map_err(Error::failed)?;
    }
    let handlers = Handlers {
        info,
        default_timeout: initial.timeout(),
        factory,
    };
    let rpc = initial.init_rpc(handlers).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
