use crate::bus::EvaBroker;
use crate::core::Core;
use crate::eapi;
use crate::Mode;
use crate::{EResult, Error};
use crate::{ARCH_SFX, BUILD, PRODUCT_CODE, PRODUCT_NAME, VERSION};
use busrt::rpc::{self, RpcClient};
use eva_common::err_logger;
use eva_common::registry;
use eva_common::tools::get_eva_dir;
use eva_common::value::Value;
use log::{debug, info, trace};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{atomic, Arc};
use tokio::sync::RwLock;

err_logger!();

#[derive(Serialize)]
struct Info<'a> {
    name: &'a str,
    code: &'a str,
    build: u64,
    version: &'a str,
    arch: &'a str,
}

async fn run_regular(
    reg_db: Arc<RwLock<yedb::Database>>,
    mut broker: EvaBroker,
    mut core: Core,
) -> EResult<()> {
    core.init_inventory_db().await?;
    core.load_inventory_db().await?;
    trace!("starting log workers");
    crate::logs::start().await?;
    trace!("initializing the broker");
    broker.init(&mut core).await?;
    let queue_size = broker.queue_size();
    let mut core_client = broker.register_client("eva.core").await?;
    crate::core::init_core_client(&mut core_client).await?;
    trace!("registering the registry service");
    let reg_client = broker.register_client(registry::SERVICE_NAME).await?;
    trace!(
        "registering the default launcher service: {}",
        crate::svcmgr::DEFAULT_LAUNCHER
    );
    let launcher_client = broker
        .register_client(crate::svcmgr::DEFAULT_LAUNCHER)
        .await?;
    let launcher_client_secondary = broker.register_secondary_for(&launcher_client).await?;
    trace!("initializing the default launcher");
    crate::launcher::init(launcher_client, launcher_client_secondary, queue_size).await?;
    let _reg_rpc = RpcClient::new(reg_client, yedb::server::BusRtApi::new(reg_db.clone()));
    let core = Arc::new(core);
    trace!("registering the core RPC (EAPI)");
    let mut bus_api = eapi::BusApi::new(core.clone(), queue_size);
    bus_api.start()?;
    let core_sec_client = broker.register_secondary_for(&core_client).await?;
    let _core_rpc = RpcClient::create(core_client, bus_api, rpc::Options::new().blocking_frames());
    let rpc = RpcClient::new0(core_sec_client);
    core.set_rpc(Arc::new(rpc))?;
    core.set_components()?;
    crate::logs::start_bus_logger().await;
    trace!("writing the pid file");
    core.write_pid_file().await?;
    trace!("registering the signal handler");
    core.register_signals().await;
    trace!("starting the core");
    core.start(queue_size).await?;
    trace!("starting the service manager");
    core.service_manager()
        .start(core.system_name(), core.timeout(), false)
        .await;
    core.service_manager()
        .wait_all_started(core.timeout())
        .await
        .log_ef();
    trace!("services started, announcing local states");
    let c = core.clone();
    let (active_trig, active_listen) = triggered::trigger();
    tokio::spawn(async move {
        active_listen.await;
        c.announce_local().await;
    });
    crate::core::start_node_checker(core.clone());
    core.mark_loaded().await;
    active_trig.trigger();
    core.block(true).await;
    Ok(())
}

pub fn launch(
    mode: Mode,
    system_name: Option<&str>,
    pid_file: Option<&str>,
    connection_path: Option<&str>,
    fips: bool,
) -> EResult<()> {
    if fips {
        openssl::fips::enable(true)?;
        crate::FIPS.store(true, atomic::Ordering::SeqCst);
    }
    match mode {
        Mode::Info => {
            let info = Info {
                name: PRODUCT_NAME,
                code: PRODUCT_CODE,
                build: BUILD,
                version: VERSION,
                arch: ARCH_SFX,
            };
            println!("{}", serde_json::to_string(&info)?);
            Ok(())
        }
        Mode::Regular => {
            let dir_eva = get_eva_dir();
            let reg_db = yedb::server::create_db();
            let mut db = reg_db.try_write()?;
            let db_path = format!("{dir_eva}/runtime/registry");
            db.set_db_path(&db_path)?;
            db.open().map_err(|e| {
                Error::registry(format!("unable to open the registry {}: {}", db_path, e))
            })?;
            let mut core = Core::new_from_db(&mut db, &dir_eva, system_name, pid_file)?;
            let log_config: Vec<HashMap<String, Value>> =
                serde_json::from_value(db.key_get(&registry::format_config_key("logs"))?)?;
            crate::logs::init(
                core.system_name(),
                "eva-node",
                log_config,
                &dir_eva,
                mode == Mode::Regular,
            );
            info!("starting EVA ICS node {}", core.system_name());
            info!("mode: regular (primary node point)");
            if fips {
                info!("FIPS: enabled");
            }
            info!("dir: {}", dir_eva);
            trace!("loading registry config");
            crate::regsvc::load(&mut db)?;
            trace!("generating core boot id");
            core.set_boot_id(&mut db)?;
            core.log_summary();
            trace!("creating the broker");
            let broker = EvaBroker::new_from_db(&mut db, &dir_eva)?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(core.workers().try_into().map_err(Error::failed)?)
                .enable_all()
                .build()?;
            core.load_inventory(&mut db)?;
            core.service_manager().load(&mut db)?;
            drop(db);
            debug!("starting runtime");
            rt.block_on(async move { run_regular(reg_db, broker, core).await })
        }
        Mode::SPoint => {
            let dir_eva = get_eva_dir();
            if let Some(cpath) = connection_path {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(
                        crate::spoint::SPOINT_WORKERS
                            .try_into()
                            .map_err(Error::failed)?,
                    )
                    .enable_all()
                    .build()?;
                rt.block_on(async move {
                    crate::spoint::run(&dir_eva, system_name, pid_file, cpath).await
                })
            } else {
                Err(Error::invalid_params("connection path not specified"))
            }
        }
    }
}
