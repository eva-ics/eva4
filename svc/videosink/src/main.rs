use std::time::Duration;

use eva_common::err_logger;
use eva_common::multimedia::{FrameHeader, VideoFormat};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::service::{poc, set_poc};
use gst::glib::object::{Cast as _, ObjectExt as _};
use gst::prelude::{ElementExt as _, GstBinExt as _};
use serde::Deserialize;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "GStreamer Audio/Video sink service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        #[allow(clippy::single_match, clippy::match_single_binding)]
        match method {
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_frame(&self, _frame: Frame) {
        svc_need_ready!();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    pipeline: String,
    oid: OID,
}

fn handle_pipeline(pipeline: &str, tx: async_channel::Sender<Value>) -> EResult<()> {
    debug!("Using GStreamer pipeline: {}", pipeline);
    let mut pipeline = pipeline.to_owned();

    pipeline.push_str(" ! appsink name=evasvcsink");

    let pipeline = gst::parse::launch(&pipeline).map_err(Error::invalid_params)?;
    let pipeline = pipeline
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| Error::failed("Failed to cast pipeline"))?;
    let appsink = pipeline
        .by_name("evasvcsink")
        .unwrap()
        .dynamic_cast::<gst_app::AppSink>()
        .map_err(|_| Error::failed("Failed to get appsink"))?;
    appsink.set_caps(Some(&VideoFormat::all_caps()));

    appsink.set_property("emit-signals", true);

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample({
                let tx = tx.clone();
                move |sink| {
                    if eva_sdk::service::svc_is_terminating() {
                        return Err(gst::FlowError::Eos);
                    }
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().expect("Failed to get buffer");
                    let header = FrameHeader::try_from_caps_ref(sample.caps().unwrap())
                        .expect("Invalid caps for EVA ICS sink");

                    let val = Value::try_from_gstreamer_buffer_ref(&header, buffer)
                        .expect("Failed to convert GStreamer buffer to Value");

                    if let Err(e) = tx.try_send(val) {
                        if e.is_closed() {
                            return Err(gst::FlowError::Eos);
                        }
                        panic!("Failed to send value to channel: {}", e);
                    }
                    Ok(gst::FlowSuccess::Ok)
                }
            })
            .build(),
    );

    debug!("Starting GStreamer pipeline");

    pipeline
        .set_state(gst::State::Playing)
        .map_err(Error::failed)?;

    debug!("Handling GStreamer pipeline messages");

    let bus = pipeline.bus().unwrap();
    while !eva_sdk::service::svc_is_terminating() {
        if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => break,
                MessageView::Error(err) => {
                    error!("GStreamer bus: {}", err.error());
                    break;
                }
                _ => (),
            }
        }
    }

    pipeline
        .set_state(gst::State::Null)
        .map_err(Error::failed)?;

    tx.try_send(Value::Unit).ok();

    Ok(())
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let handlers = Handlers { info };
    eapi_bus::init(&initial, handlers).await?;
    set_poc(Some(Duration::from_secs(1)));
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    gst::init().map_err(Error::failed)?;
    svc_start_signal_handlers();
    let (tx, rx) = async_channel::bounded::<Value>(4096);
    let oid = config.oid;
    tokio::spawn(async move {
        while let Ok(value) = rx.recv().await {
            eapi_bus::publish_item_state(&oid, 1, Some(&value))
                .await
                .expect("Failed to publish item state");
        }
    });
    let pipe_thread = tokio::task::spawn_blocking(move || {
        if let Err(err) = handle_pipeline(&config.pipeline, tx) {
            error!("Failed to handle pipeline: {}", err);
            poc();
        }
        eva_sdk::service::svc_terminate();
    });
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    pipe_thread
        .await
        .map_err(|_| Error::failed("Failed to join pipeline thread"))?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}
