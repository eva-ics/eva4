use std::sync::{LazyLock, atomic};
use std::time::Duration;

use bma_ts::Monotonic;
use eva_common::err_logger;
use eva_common::multimedia::{FrameHeader, VideoFormat};
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::service::{poc, set_poc};
use gst::glib::object::{Cast as _, ObjectExt as _};
use gst::prelude::{ElementExt as _, GstBinExt as _};
use parking_lot::Mutex;
use rtsc::event_map::EventMap;
use serde::{Deserialize, Serialize};

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "GStreamer Audio/Video sink service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static STREAM_HEADER: LazyLock<Mutex<Option<FrameHeader>>> = LazyLock::new(<_>::default);
static FRAMES: LazyLock<Mutex<EventMap<Monotonic, (), Duration>>> = LazyLock::new(<_>::default);
static FRAMES_PROCESSED: atomic::AtomicU64 = atomic::AtomicU64::new(0);

#[derive(Serialize)]
struct StreamInfo {
    width: u16,
    height: u16,
    format: String,
    fps: usize,
    frames: u64,
}

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        #[allow(clippy::single_match, clippy::match_single_binding)]
        match method {
            "stream.info" => {
                if !payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let header = STREAM_HEADER.lock().clone();
                let width = header.as_ref().map_or(0, FrameHeader::width);
                let height = header.as_ref().map_or(0, FrameHeader::height);
                let format = header
                    .as_ref()
                    .map(|h| h.format().map_or("unknown".to_string(), |f| f.to_string()))
                    .unwrap_or_default();
                let fps = FRAMES.lock().data().len();
                let frames = FRAMES_PROCESSED.load(atomic::Ordering::Relaxed);
                Ok(Some(pack(&StreamInfo {
                    width,
                    height,
                    format,
                    fps,
                    frames,
                })?))
            }
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
    force_width: Option<u16>,
    force_height: Option<u16>,
    oid: OID,
}

fn handle_pipeline(
    pipeline: &str,
    tx: async_channel::Sender<Value>,
    force_width: Option<u16>,
    force_height: Option<u16>,
) -> EResult<()> {
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
                    let mut caps = sample.caps().unwrap().to_owned();
                    if let Some(w) = force_width {
                        let caps_mut = caps.make_mut();
                        let s = caps_mut.structure_mut(0).unwrap();
                        s.set("width", i32::from(w));
                    }
                    if let Some(h) = force_height {
                        let caps_mut = caps.make_mut();
                        let s = caps_mut.structure_mut(0).unwrap();
                        s.set("height", i32::from(h));
                    }
                    let buffer = sample.buffer().expect("Failed to get buffer");
                    let header = FrameHeader::try_from_caps_ref(&caps)
                        .expect("Invalid caps for EVA ICS sink");
                    STREAM_HEADER.lock().replace(header.clone());

                    let val = Value::try_from_gstreamer_buffer_ref(&header, buffer)
                        .expect("Failed to convert GStreamer buffer to Value");

                    if let Err(e) = tx.try_send(val) {
                        if e.is_closed() {
                            return Err(gst::FlowError::Eos);
                        }
                        panic!("Failed to send value to channel: {}", e);
                    }

                    let mut frames = FRAMES.lock();
                    frames.insert(Monotonic::now(), ());
                    frames.cleanup(Monotonic::now() - Duration::from_secs(1));
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
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("stream.info"));
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
            FRAMES_PROCESSED.fetch_add(1, atomic::Ordering::Relaxed);
            eapi_bus::publish_item_state(&oid, 1, Some(&value))
                .await
                .expect("Failed to publish item state");
        }
    });
    let pipe_thread = tokio::task::spawn_blocking(move || {
        if let Err(err) = handle_pipeline(
            &config.pipeline,
            tx,
            config.force_width,
            config.force_height,
        ) {
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
