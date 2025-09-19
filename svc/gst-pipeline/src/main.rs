use std::str::FromStr;
use std::time::Duration;
use std::time::Instant;

use async_channel::Receiver;
use async_channel::Sender;
use eva_common::acl::OIDMask;
use eva_common::err_logger;
use eva_common::events::RawStateEventOwned;
use eva_common::events::RAW_STATE_TOPIC;
use eva_common::multimedia::FrameHeader;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::service::poc;
use eva_sdk::types::State;
use gstreamer::glib::object::Cast as _;
use gstreamer::glib::object::ObjectExt as _;
use gstreamer::prelude::ElementExt as _;
use gstreamer::prelude::GstBinExt as _;
use gstreamer::Caps;
use serde::Deserialize;

err_logger!();

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "Service";

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Handlers {
    info: ServiceInfo,
    pipeline_tx: Sender<Option<Vec<u8>>>,
    publisher_tx: Sender<Option<(ItemStatus, Vec<u8>)>>,
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
    async fn handle_frame(&self, frame: Frame) {
        if frame.topic().is_none() {
            return;
        }
        let Ok(state) = unpack::<State>(frame.payload()).log_err() else {
            return;
        };
        if state.status != 1 {
            self.publisher_tx
                .send(Some((state.status, vec![])))
                .await
                .log_ef();
            return;
        }
        let Some(value) = state.value else {
            self.publisher_tx.send(Some((1, vec![]))).await.log_ef();
            return;
        };
        let Value::Bytes(bytes) = value else {
            self.publisher_tx.send(Some((1, vec![]))).await.log_ef();
            return;
        };
        self.pipeline_tx.send(Some(bytes)).await.ok();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    oid_src: OID,
    oid_dst: OID,
    pipeline: String,
    caps_src: Option<String>,
    caps_dst: String,
}

fn pipeline_loop(
    pipeline: &str,
    caps_src_str: Option<&str>,
    caps_dst_str: &str,
    rx: Receiver<Option<Vec<u8>>>,
    tx: Sender<Option<(ItemStatus, Vec<u8>)>>,
    timeout: Duration,
) -> EResult<()> {
    let caps_dst = Caps::from_str(caps_dst_str)
        .log_err_with("invalid destination caps")
        .map_err(Error::invalid_params)?;
    let dst_header = FrameHeader::try_from_caps(&caps_dst)
        .log_err_with("unable to create header from destination caps")
        .map_err(Error::invalid_params)?;
    let pipeline = gstreamer::parse::launch(&format!(
        "appsrc name=evasvcsrc ! {} ! appsink name=evasvcsink",
        pipeline
    ))
    .log_err_with("invalid pipeline")
    .map_err(Error::failed)?
    .dynamic_cast::<gstreamer::Pipeline>()
    .unwrap();
    let appsrc = pipeline
        .by_name("evasvcsrc")
        .unwrap()
        .dynamic_cast::<gstreamer_app::AppSrc>()
        .unwrap();
    appsrc.set_property("is-live", true);
    appsrc.set_property("do-timestamp", true);
    appsrc.set_property("format", gstreamer::Format::Time);

    let (src_header, first_frame_data_bytes) = loop {
        let bytes = rx.recv_blocking().map_err(Error::failed)?;
        let Some(mut bytes) = bytes else {
            return Ok(());
        };
        if bytes.len() > FrameHeader::SIZE {
            let data_bytes = bytes.split_off(FrameHeader::SIZE);
            let Ok(header) = FrameHeader::from_slice(&bytes).log_err_with("invalid frame header")
            else {
                continue;
            };
            break (header, data_bytes);
        }
    };

    if let Some(caps_src_str) = caps_src_str {
        if caps_src_str.is_empty() {
            info!("Pipeline appsrc caps auto-negotiation");
        } else {
            let caps_src = Caps::from_str(caps_src_str)
                .log_err_with("invalid source caps")
                .map_err(Error::invalid_params)?;
            info!("Pipeline appsrc caps: {}", caps_src);
            appsrc.set_caps(Some(&caps_src));
        }
    } else {
        let caps_src = src_header
            .try_to_caps()
            .log_err_with("unable to create caps from source header")
            .map_err(Error::invalid_data)?;
        info!("Pipeline appsrc caps (auto): {}", caps_src);
        appsrc.set_caps(Some(&caps_src));
    }
    info!("Pipeline appsink caps: {}", caps_dst);

    let appsink = pipeline
        .by_name("evasvcsink")
        .unwrap()
        .dynamic_cast::<gstreamer_app::AppSink>()
        .unwrap();
    appsink.set_property("max-buffers", 1u32);
    appsink.set_property("emit-signals", false);
    appsink.set_property("drop", true);
    appsink.set_caps(Some(&caps_dst));

    pipeline
        .set_state(gstreamer::State::Playing)
        .log_err_with("unable to set pipeline to Playing")
        .map_err(Error::failed)?;

    let buffer = gstreamer::Buffer::from_slice(first_frame_data_bytes);
    appsrc
        .push_buffer(buffer)
        .log_err_with("unable to push buffer to appsrc")
        .map_err(Error::failed)?;

    let start = Instant::now();

    let mut prev_pipeline_state = gstreamer::State::Null;

    loop {
        let pipeline_state = pipeline.state(None).1;
        if pipeline_state != prev_pipeline_state {
            info!("Pipeline state changed: {prev_pipeline_state:?} -> {pipeline_state:?}");
            prev_pipeline_state = pipeline_state;
        }
        if start.elapsed() > timeout && pipeline_state != gstreamer::State::Playing {
            error!("Pipeline timeout");
            tx.send_blocking(Some((-1, vec![])))
                .map_err(Error::failed)?; // mark error
            return Err(Error::timeout());
        }
        while let Some(sample) =
            appsink.try_pull_sample(Some(gstreamer::ClockTime::from_mseconds(1)))
        {
            let buf = sample
                .buffer()
                .ok_or_else(|| Error::invalid_data("unable to get buffer from sample"))?;
            let data = buf
                .map_readable()
                .log_err_with("unable to map buffer readable")
                .map_err(Error::failed)?;
            let flags = buf.flags();

            let mut header = dst_header.clone();

            if !flags.contains(gstreamer::BufferFlags::DELTA_UNIT) {
                header.set_key_frame();
            }
            let mut out = header.into_vec(FrameHeader::SIZE + data.as_slice().len());
            out.extend(data.as_slice());
            if tx.send_blocking(Some((1, out))).is_err() {
                break;
            }
        }
        if svc_is_terminating() {
            break;
        }
        let Ok(bytes) = rx.recv_blocking() else {
            break;
        };
        let Some(mut bytes) = bytes else {
            break;
        };
        if bytes.len() <= FrameHeader::SIZE {
            tx.send_blocking(Some((1, vec![]))).map_err(Error::failed)?; // mark EOF
            continue;
        }
        let header = FrameHeader::from_slice(&bytes)
            .log_err_with("invalid frame header")
            .map_err(Error::invalid_data)?;
        let data_bytes = bytes.split_off(FrameHeader::SIZE);
        if header.width() != src_header.width()
            || header.height() != src_header.height()
            || header.format() != src_header.format()
        {
            tx.send_blocking(Some((1, vec![]))).map_err(Error::failed)?; // mark EOF
            return Err(Error::invalid_data("Source caps changed"));
        }
        let buffer = gstreamer::Buffer::from_slice(data_bytes);
        appsrc
            .push_buffer(buffer)
            .log_err_with("unable to push buffer to appsrc")
            .map_err(Error::failed)?;
    }
    pipeline
        .set_state(gstreamer::State::Null)
        .log_err_with("unable to set pipeline to Null")
        .map_err(Error::failed)?;
    Ok(())
}

async fn publisher_loop(oid: OID, rx: Receiver<Option<(ItemStatus, Vec<u8>)>>) -> EResult<()> {
    let topic = format!("{}{}", RAW_STATE_TOPIC, oid.as_path());
    while let Ok(Some((status, bytes))) = rx.recv().await {
        if svc_is_terminating() {
            eapi_bus::publish_confirmed(
                &topic,
                pack(&RawStateEventOwned::new(status, Value::Unit))?.into(),
            )
            .await
            .log_ef();
            return Ok(());
        }
        let ev = if status == 1 {
            if bytes.is_empty() {
                RawStateEventOwned::new(1, Value::Unit)
            } else {
                RawStateEventOwned::new(1, Value::Bytes(bytes))
            }
        } else {
            RawStateEventOwned::new(status, Value::Unit)
        };
        eapi_bus::publish(&topic, pack(&ev)?.into()).await?;
    }
    Ok(())
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    gstreamer::init().map_err(Error::failed)?;
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let timeout = initial.timeout();
    let info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    let (pipeline_tx, pipeline_rx) = async_channel::bounded(128);
    let (publisher_tx, publisher_rx) = async_channel::bounded(128);
    let handlers = Handlers {
        info,
        pipeline_tx: pipeline_tx.clone(),
        publisher_tx: publisher_tx.clone(),
    };
    eva_sdk::service::set_poc(Some(Duration::from_secs(1)));
    eapi_bus::init(&initial, handlers).await?;
    initial.drop_privileges()?;
    eapi_bus::init_logs(&initial)?;
    let pipeline_fut = tokio::task::spawn_blocking({
        let publisher_tx = publisher_tx.clone();
        move || {
            if let Err(e) = pipeline_loop(
                &config.pipeline,
                config.caps_src.as_deref(),
                &config.caps_dst,
                pipeline_rx,
                publisher_tx.clone(),
                timeout,
            ) {
                error!("Pipeline error: {}", e);
                publisher_tx.send_blocking(Some((-1, vec![]))).ok();
                poc();
            }
        }
    });
    let publisher_fut = tokio::spawn(publisher_loop(config.oid_dst, publisher_rx));
    let oid_src_mask: OIDMask = config.oid_src.into();
    eapi_bus::subscribe_oids(&[oid_src_mask], eva_sdk::service::EventKind::Actual).await?;
    svc_start_signal_handlers();
    eapi_bus::mark_ready().await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    eapi_bus::block().await;
    eapi_bus::mark_terminating().await?;
    if pipeline_fut.is_finished() {
        publisher_tx.send(None).await.ok();
    } else {
        publisher_tx.send(Some((1, vec![]))).await.ok();
    }
    pipeline_tx.send(None).await.ok();
    publisher_fut.await.ok();
    pipeline_fut.await.ok();
    Ok(())
}
