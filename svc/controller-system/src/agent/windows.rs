use clap::{Parser, Subcommand};
use eva_common::prelude::*;
use eva_system_common::{
    common::{self, ReportConfig},
    metric::client,
};
use log::{error, info};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic;
use std::time::Duration;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

const AUTHOR: &str = "Bohemia Automation";
//const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "EVA ICS Controller System Windows agent";
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " build ",
    include_str!("../../build.number")
);

const ERR_NO_FIPS: &str = "FIPS mode not supported";

const SVC_NAME: &str = "EvaCSAgent";
const SVC_DISPLAY_NAME: &str = "EVA ICS Controller System Agent";
const SVC_ID: &str = "EVA.cs-agent";

#[cfg(debug_assertions)]
const CONFIG_PATH: Lazy<PathBuf> =
    Lazy::new(|| std::path::Path::new("./dev/agent-config.yml").to_owned());
#[cfg(not(debug_assertions))]
static CONFIG_PATH: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::current_exe()
        .expect("Unable to get working directory")
        .parent()
        .expect("Unable to get exe directory")
        .to_owned();
    path.push("config.yml");
    path
});

static ACTIVE: atomic::AtomicBool = atomic::AtomicBool::new(true);

#[derive(Parser)]
#[command(author = AUTHOR, version = LONG_VERSION, about = DESCRIPTION, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "run in the system console with logging")]
    Run,
    #[clap(about = "register Windows service")]
    Register,
    #[clap(about = "unregister Windows service")]
    Unregister,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    report: ReportConfig,
    client: client::Config,
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) -> Result<(), windows_service::Error> {
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                ACTIVE.store(false, atomic::Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status_handle = service_control_handler::register(SVC_ID, event_handler)?;
    let mut next_status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_service_status(next_status.clone())?;
    if let Err(e) = launch() {
        error!("{}", e);
        panic!();
    }
    next_status.current_state = ServiceState::Stopped;
    status_handle.set_service_status(next_status)?;
    Ok(())
}

fn main() -> EResult<()> {
    let args = Args::parse();
    if let Some(cmd) = args.command {
        match cmd {
            Command::Run => {
                env_logger::Builder::new()
                    .target(env_logger::Target::Stdout)
                    .filter_level(log::LevelFilter::Info)
                    .init();
                launch()?;
            }
            Command::Register => {
                println!(
                    "checking the configuration file {}",
                    CONFIG_PATH.to_string_lossy()
                );
                load_config()?;
                println!("configuration check passed\n");
                match register_service() {
                    Ok(()) => {
                        println!("service registered: {}", SVC_NAME);
                    }
                    Err(e) => {
                        eprintln!("service registration failed: {}", e);
                    }
                }
            }
            Command::Unregister => match unregister_service() {
                Ok(()) => {
                    println!("service unregistered: {}", SVC_NAME);
                }
                Err(e) => {
                    eprintln!("service unregistration failed: {}", e);
                }
            },
        }
    } else {
        eventlog::register(SVC_ID).unwrap();
        eventlog::init(SVC_ID, log::Level::Info).unwrap();
        service_dispatcher::start(SVC_ID, ffi_service_main)
            .map_err(|e| Error::failed(format!("Error running as a windows service: {}, to start the program with console, add \"run\" argument", e)))?;
    }
    Ok(())
}

fn load_config() -> EResult<Config> {
    let config_path: PathBuf = CONFIG_PATH.clone();
    let config: Config =
        serde_yaml::from_str(&std::fs::read_to_string(config_path).map_err(|e| {
            Error::io(format!(
                "unable to load {}: {}",
                CONFIG_PATH.to_string_lossy(),
                e
            ))
        })?)
        .map_err(Error::invalid_params)?;
    if config.client.fips {
        error!("{}", ERR_NO_FIPS);
        return Err(Error::failed(ERR_NO_FIPS));
    }
    Ok(config)
}

fn launch() -> EResult<()> {
    let config = load_config()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(launch_agent(config))?;
    Ok(())
}

async fn launch_agent(config: Config) -> EResult<()> {
    config.report.set()?;
    common::spawn_workers();
    client::spawn_worker(config.client);
    info!("{} {} started", DESCRIPTION, LONG_VERSION);
    while ACTIVE.load(atomic::Ordering::Relaxed) {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
    info!("service stopped");
    Ok(())
}

fn register_service() -> windows_service::Result<()> {
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;

    let my_service_info = ServiceInfo {
        name: OsString::from(SVC_NAME),
        display_name: OsString::from(SVC_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe().unwrap(),
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // run as System
        account_password: None,
    };
    manager.create_service(&my_service_info, ServiceAccess::QUERY_STATUS)?;
    Ok(())
}

fn unregister_service() -> windows_service::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    if let Ok(service) = manager.open_service(SVC_NAME, ServiceAccess::STOP) {
        let _ = service.stop();
    }
    let service = manager.open_service(SVC_NAME, ServiceAccess::DELETE)?;
    service.delete()?;
    Ok(())
}
