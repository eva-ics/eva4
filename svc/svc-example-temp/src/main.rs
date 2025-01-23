// the structure contains a set of EVA ICS OIDs, which are going to be monitored
use eva_common::acl::OIDMaskList;
// a log helper, which allows to quickly log std::result::Result error messages
use eva_common::err_logger;
// only local bus states are going to be monitored (states, replicated from other EVA ICS nodes are
// not processed)
use eva_common::events::LOCAL_STATE_TOPIC;
// EVA ICS bus uses MessagePack as the primary serialization format, the following methods are
// aliases for rmp_serde ones
use eva_common::payload::{pack, unpack};
// import common EVA ICS structures and methods, such as the standard Error, result (EResult), OID,
// Value (represents any valid MessagePack value) etc.
use eva_common::prelude::*;
// import common SDK methods, required for services
use eva_sdk::prelude::*;
// import basic item state structure
use eva_sdk::types::State;
// it is recommended to keep EVA ICS services simple and small and make majority of shared
// variables as static ones, so OnceCell (and/or lazy_static) is here
use once_cell::sync::Lazy;
// serde, used to process bus structures
use serde::{Deserialize, Serialize};
// and a few others
use std::collections::HashMap;
use std::sync::atomic;
use std::sync::Mutex;

// this macro alters Result with additional methods:
// log_ef() - log error and forget the result
// log_efd() - log error as debug and forget the result
// log_err() - log error and keep the result
// log_ed() - log error as debug and keep the result
//
// Why isn't a trait imported? The reason is simple: the macro defines the trait inside THE CURRENT
// module, which allows to log messages on behalf of it.
err_logger!();

// Typical service constants, used in bus EAPI etc.
const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Temperature monitor";

// Let us define a hysteresis: a sensor is considered as back-to-normal, if the temperature drops
// below threshold-hysteresis. Values between are not processed.
const HYSTERESIS: f64 = 2.0;

// Let us calculate how many email notifications are sent
//
// The value can be obtained later using the console command:
//
// eva svc call eva.svc.alarm_temp get_counter
static COUNTER: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

// It is recommended to use Jemalloc, which is found to provide the best functionality in
// industrial automation tasks. But tasks may vary, so let us keep a way to switch back to the
// standard one.
#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

// The notification map, keeps info which sensors were already processed
static NOTIFIED: Lazy<Mutex<HashMap<OID, bool>>> = Lazy::new(<_>::default);

// BUS/RT RPC handlers
struct Handlers {
    // EVA ICS service info structure
    info: ServiceInfo,
    // Let us put some variables inside the handlers object directly:
    // e-mail recipient
    rcpt: String,
    // EVA ICS mailer service id
    mailer_svc: String,
    // The temperature threshold
    threshold: f64,
}

// Let us describe BUS/RT RPC handlers methods
#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    // Handle RPC call
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        // The following macro automatically returns Error::not_ready if the service is not fully
        // initialized yet
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        match method {
            // There is only one RPC method - "get_counter", which returns the number of email
            // messages sent
            "get_counter" => {
                #[derive(Serialize)]
                struct Reply {
                    count: usize,
                }
                Ok(Some(pack(&Reply {
                    count: COUNTER.load(atomic::Ordering::SeqCst),
                })?))
            }
            // This SDK function automatically provides the following standard EVA ICS service RPC
            // methods: "test", "info" and "stop", which must be included in all service
            // implementations
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    // handle BUS/RT frames
    async fn handle_frame(&self, frame: Frame) {
        // The following macro automatically makes function return if the service is not fully
        // initialized yet
        svc_need_ready!();
        // Only frames with topic (bus publications) are processed
        //
        // When the service instance is started, it is not required to load telemetry from EVA ICS
        // core, as the core automatically sends bus notifications at startup to subscribers, after
        // all services are started
        if let Some(topic) = frame.topic() {
            // Only local state topics are processed
            if let Some(item_topic) = topic.strip_prefix(LOCAL_STATE_TOPIC) {
                self.process_state(item_topic, frame.payload())
                    .await
                    .log_ef();
            }
        }
    }
}

impl Handlers {
    // This function processes states of subscribed sensors
    async fn process_state(&self, item_topic: &str, payload: &[u8]) -> EResult<()> {
        // Letter payload, required for the mailer service "send" RPC method
        #[derive(Serialize)]
        struct Letter<'a> {
            rcp: &'a str,
            subject: String,
            text: String,
        }
        // Parse sensor OID from the topic
        let oid = OID::from_path(item_topic)?;
        // Parse sensor state
        let state: State = unpack(payload)?;
        let mut letter_c = None;
        // The next lines represent common Rust code, so no comments are provided
        if let Some(value) = state.value {
            if let Ok(temperature) = f64::try_from(value) {
                let mut notified = NOTIFIED.lock().unwrap();
                let was_sensor_notified = notified.get(&oid).copied().unwrap_or_default();
                if temperature > self.threshold && !was_sensor_notified {
                    let text = format!("{} temperature is {}", oid, temperature);
                    warn!("{}", text);
                    letter_c = Some(Letter {
                        rcp: &self.rcpt,
                        subject: format!("{} is hot", oid),
                        text,
                    });
                    notified.insert(oid, true);
                } else if temperature < self.threshold - HYSTERESIS && was_sensor_notified {
                    let text = format!("{} temperature is {}", oid, temperature);
                    info!("{}", text);
                    letter_c = Some(Letter {
                        rcp: &self.rcpt,
                        subject: format!("{} is back to normal", oid),
                        text,
                    });
                    notified.insert(oid, false);
                }
            }
        };
        if let Some(letter) = letter_c {
            COUNTER.fetch_add(1, atomic::Ordering::SeqCst);
            eapi_bus::call(&self.mailer_svc, "send", pack(&letter)?.into()).await?;
        }
        Ok(())
    }
}

// The service configuration
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    // Array (set) of sensor OID masks
    sensors: OIDMaskList,
    // E-mail recipient
    rcpt: String,
    // Mailer service ID
    mailer_svc: String,
    // The temperature threshold
    threshold: f64,
}

#[svc_main]
// The proc attribute renames the function to eva_service_main and generates the following main
// instead:
//
// fn main() -> eva_common::EResult<()> { svc_launch(eva_service_main) }
//
// The structure Initial contains the initial payload, which is generated by EVA ICS service
// launcher and pushed to STDIN of the service process. This is also automatically handled by the
// proc attribute.
async fn main(mut initial: Initial) -> EResult<()> {
    // Get config from initial and deserialize it
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    // init RPC
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    // It is not obliged to include all available service RPC methods into ServiceInfo, however
    // including methods and their parameters provide additional interface (the data is obtained by
    // calling "info" RPC method), e.g. auto-completion for eva-shell
    info.add_method(ServiceMethod::new("get_counter"));
    // The initial payload sturucture provides several methods to init the bus RPC, the primary
    // ones are: "init_rpc" and "init_rpc_blocking".
    //
    // The difference is that "init_rpc_blocking" initializes the bus RPC in blocking mode, meaning
    // that "handle_frame" calls are always processed one-by-one to keep strict event ordering.
    //
    // As in the current example it is highly unlikely that temperatures may change rapidly fast,
    // as well as fieldbus controllers usually do not pull such data more often than e.g.
    // 100-200ms, it is fine to go with "init_rpc", which is more easy to handle.
    //
    // Blocking mode requires additional code complication, such as having a secondary instance for
    // RPC to perform bus calls inside the frame handler and be sure the handler works fast enough
    // - as soon as the service bus queue is full, it is automatically disconnected by BUS/RT
    // broker.
    //
    // Starting from EVA ICS SDK 0.3.33 it is recommended to use high-level EAPI eva_sdk::eapi_bus
    // interface instead of BUS/RT directly
    let handlers = Handlers {
        info,
        rcpt: config.rcpt,
        mailer_svc: config.mailer_svc,
        threshold: config.threshold,
    };
    eapi_bus::init(&initial, handlers).await?;
    // Services are usually started under root and should drop privileges after the bus socket is
    // connected
    initial.drop_privileges()?;
    // A helper function, which subscribes BUS/RT client to all state events for OIDs maching
    // the given OIDMaskList
    eapi_bus::subscribe_oids(&config.sensors, eva_sdk::service::EventKind::Local).await?;
    // Init service logs (bus logger)
    eapi_bus::init_logs(&initial)?;
    // Start sigterm handler
    svc_start_signal_handlers();
    // Mark the instance ready and active
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    // request the core to privately annonce of sensor states to get the current state of sensors
    // as soon as possible
    eapi_bus::request_announce(&config.sensors, eva_sdk::service::EventKind::Local).await?;
    // The service is blocked until one of the following:
    // * RPC client is disconnected from the bus
    // * The service gets a termination signal
    eapi_bus::block().await;
    // Tell the core and other services that the service is terminating
    eapi_bus::mark_terminating().await?;
    Ok(())
}
