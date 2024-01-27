use eva_common::err_logger;
use eva_common::events::{RawStateEventOwned, RAW_STATE_TOPIC};
use eva_common::payload::{pack, unpack};
use eva_common::prelude::*;
use eva_sdk::hmi::XParamsOwned;
use eva_sdk::prelude::*;
use serde::Deserialize;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Sensor state manipulations";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

// BUS/RT RPC handlers
struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            // handle extra calls from HMI
            "x" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let xp: XParamsOwned = unpack(payload)?;
                    match xp.method() {
                        "set" => {
                            #[derive(Deserialize)]
                            #[serde(deny_unknown_fields)]
                            struct Params {
                                i: OID,
                                status: ItemStatus,
                                #[serde(default)]
                                value: ValueOptionOwned,
                            }
                            // deserialize params
                            let params = Params::deserialize(xp.params)?;
                            // check if the item is a sensor
                            if params.i.kind() != ItemKind::Sensor {
                                return Err(Error::access("can set states for sensors only").into());
                            }
                            // check if the caller's session is not a read-only one
                            xp.aci.check_write()?;
                            // check if the caller's ACL has write access to the provided OID
                            xp.acl.require_item_write(&params.i)?;
                            // set the sensor state
                            // in this example the service does not check does the sensor really
                            // exist in the core or not
                            let mut event = RawStateEventOwned::new0(params.status);
                            event.value = params.value;
                            let topic = format!("{}{}", RAW_STATE_TOPIC, params.i.as_path());
                            eapi_bus::publish(&topic, pack(&event)?.into()).await?;
                            Ok(None)
                        }
                        _ => Err(RpcError::method(None)),
                    }
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
}

// The service configuration must be empty
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let _config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let rpc = initial.init_rpc(Handlers { info }).await?;
    let timeout = initial.timeout();
    eapi_bus::set(rpc, timeout)?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
