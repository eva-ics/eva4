use eva_common::common_payloads::{ParamsOID, ParamsUuid};
use eva_common::prelude::*;
use eva_sdk::controller::{Action, actt::Actt, format_action_topic};
use eva_sdk::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

mod actions;
mod common;
mod updates;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Script-runner controller service";

static TIMEOUT: OnceLock<Duration> = OnceLock::new();
static EVA_DIR: OnceLock<String> = OnceLock::new();
static UPDATE_TRIGGERS: LazyLock<Mutex<HashMap<OID, async_channel::Sender<()>>>> =
    LazyLock::new(<_>::default);
static ACTION_QUEUES: OnceLock<HashMap<OID, async_channel::Sender<Action>>> = OnceLock::new();
static ACTT: OnceLock<Actt> = OnceLock::new();
static BUS_PATH: OnceLock<String> = OnceLock::new();

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
    tx: async_channel::Sender<(String, Vec<u8>)>,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "update" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsOID = unpack(payload)?;
                    updates::trigger_update(&p.i)?;
                    Ok(None)
                }
            }
            "action" => {
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let action: Action = unpack(payload)?;
                let actt = crate::ACTT.get().unwrap();
                let action_topic = format_action_topic(action.oid());
                let payload_pending = pack(&action.event_pending())?;
                let action_uuid = *action.uuid();
                let action_oid = action.oid().clone();
                if let Some(tx) = crate::ACTION_QUEUES.get().unwrap().get(action.oid()) {
                    actt.append(action.oid(), action_uuid)?;
                    if let Err(e) = tx.send(action).await {
                        actt.remove(&action_oid, &action_uuid)?;
                        Err(Error::core(format!("action queue broken: {}", e)).into())
                    } else if let Err(e) = self
                        .tx
                        .send((action_topic, payload_pending))
                        .await
                        .map_err(Error::io)
                    {
                        actt.remove(&action_oid, &action_uuid)?;
                        Err(e.into())
                    } else {
                        Ok(None)
                    }
                } else {
                    Err(Error::not_found(format!("{} has no action map", action.oid())).into())
                }
            }
            "terminate" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsUuid = unpack(payload)?;
                    crate::ACTT.get().unwrap().mark_terminated(&p.u)?;
                    Ok(None)
                }
            }
            "kill" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsOID = unpack(payload)?;
                    crate::ACTT.get().unwrap().mark_killed(&p.i)?;
                    Ok(None)
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: common::Config = common::Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    BUS_PATH
        .set(initial.bus_path().to_owned())
        .map_err(|_| Error::core("Unable to set BUS_PATH"))?;
    let eva_dir = initial.eva_dir();
    EVA_DIR
        .set(eva_dir.to_owned())
        .map_err(|_| Error::core("Unable to set EVA_DIR"))?;
    let action_oids = config.action_map.keys().collect::<Vec<&OID>>();
    ACTT.set(Actt::new(&action_oids))
        .map_err(|_| Error::core("Unable to set ACTT"))?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("update").required("i"));
    let (tx, rx) = async_channel::bounded::<(String, Vec<u8>)>(config.queue_size);
    eapi_bus::init_blocking(
        &initial,
        Handlers {
            info,
            tx: tx.clone(),
        },
    )
    .await?;
    initial.drop_privileges()?;
    tokio::spawn(async move {
        let client = eapi_bus::client();
        while let Ok((topic, payload)) = rx.recv().await {
            client
                .lock()
                .await
                .publish(&topic, payload.into(), QoS::No)
                .await
                .log_ef();
        }
    });
    eapi_bus::init_logs(&initial)?;
    svc_start_signal_handlers();
    tokio::spawn(async move {
        eapi_bus::wait_core(true).await.log_ef();
        for update in config.update {
            tokio::spawn(async move {
                updates::update_handler(update).await.log_ef();
            });
        }
        for update in config.update_pipe {
            tokio::spawn(async move {
                updates::run_update_pipe(update).await;
            });
        }
    });
    let mut action_queues: HashMap<OID, async_channel::Sender<Action>> = HashMap::new();
    for (oid, map) in config.action_map {
        let (atx, arx) = async_channel::bounded::<Action>(config.action_queue_size);
        debug!("starting action queue for {}", oid);
        actions::start_action_handler(&oid, map, arx, tx.clone());
        action_queues.insert(oid, atx);
    }
    ACTION_QUEUES
        .set(action_queues)
        .map_err(|_| Error::core("Unable to set ACTION_QUEUES"))?;
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    Ok(())
}
