use std::{sync::LazyLock, thread};

use busrt::{sync::rpc::SyncRpc as _, QoS};
use eva_common::{
    multimedia::VideoFormat,
    payload::{pack, unpack},
    OID,
};
use gst::{
    glib::{
        self,
        subclass::{
            object::{ObjectImpl, ObjectImplExt as _},
            types::{ObjectSubclass, ObjectSubclassExt as _},
        },
        ParamSpecBuilderExt as _,
    },
    prelude::{GstParamSpecBuilderExt as _, ToValue as _},
    subclass::prelude::{ElementImpl, GstObjectImpl},
    Fraction, PadDirection, PadPresence, PadTemplate,
};
use gst_base::{
    prelude::BaseSrcExt as _,
    subclass::{
        base_src::CreateSuccess,
        prelude::{BaseSrcImpl, BaseSrcImplExt as _, PushSrcImpl},
    },
};
use gst_video::VideoInfo;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{default_bus_client_name, DEFAULT_BUS_PATH};

const DEFAULT_VIDEOSRV_SVC: &str = "eva.videosrv.default";

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "evavideosrvsrc",
        gst::DebugColorFlags::empty(),
        Some("EVA ICS Video Server Frame Source"),
    )
});

#[derive(Deserialize)]
struct EvaVideoInfo {
    width: u16,
    height: u16,
    format: u8,
    fps: u16,
}

#[derive(Deserialize)]
struct VideoFrame {
    t: f64,
    data: Vec<u8>,
    key_unit: bool,
}

struct Session {
    format: VideoFormat,
    width: u16,
    height: u16,
    fps: u16,
    rpc: busrt::sync::rpc::RpcClient,
    videosrv_svc: String,
    packed_cursor: Vec<u8>,
}

impl Session {
    fn next_frame(&self) -> Option<VideoFrame> {
        let payload = busrt::borrow::Cow::Borrowed(&self.packed_cursor);
        let result = self
            .rpc
            .call(&self.videosrv_svc, "Nrec.pull", payload, QoS::Processed)
            .expect("Unable to get next frame");
        let data = result.payload();
        if data.is_empty() {
            return None; // EOS
        }
        let frame: VideoFrame = unpack(data).expect("Failed to unpack video frame");
        Some(frame)
    }
}

struct Settings {
    oid: Option<OID>,
    bus_path: String,
    bus_client_name: String,
    videosrv_svc: String,
    t_start: Option<f64>,
    limit: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            oid: None,
            bus_path: DEFAULT_BUS_PATH.to_string(),
            bus_client_name: <_>::default(),
            videosrv_svc: DEFAULT_VIDEOSRV_SVC.to_string(),
            t_start: None,
            limit: 0,
        }
    }
}

#[derive(Default)]
pub struct EvaVideoSrvSrc {
    session: Mutex<Option<Session>>,
    settings: Mutex<Settings>,
}

#[glib::object_subclass]
impl ObjectSubclass for EvaVideoSrvSrc {
    const NAME: &'static str = "GstEvaVideoSrvSrc";
    type Type = super::EvaVideoSrvSrc;
    type ParentType = gst_base::PushSrc;
}

impl ObjectImpl for EvaVideoSrvSrc {
    fn constructed(&self) {
        self.parent_constructed();
        let obj = self.obj();
        obj.set_live(true);
        obj.set_do_timestamp(true);
        obj.set_format(gst::Format::Time);
    }
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            vec!
            [
                glib::ParamSpecString::builder("oid")
                .nick("Target OID")
                .blurb("OID (usually lvar) to send frames to")
                .mutable_ready()
                .build(),
                glib::ParamSpecString::builder("t-start")
                .nick("Start time")
                .blurb("Start date/time for the video stream")
                .mutable_ready()
                .build(),
                glib::ParamSpecUInt::builder("limit")
                .nick("Frame limit")
                .blurb("Maximum number of frames to process. 0 means no limit.")
                .default_value(0)
                .mutable_ready()
                .build(),
                glib::ParamSpecString::builder("bus-path")
                .nick("Bus path/addr:port")
                .blurb("EVA ICS bus socket path or address:port to connect to")
                .mutable_ready()
                .build(),
                glib::ParamSpecString::builder("bus-client-name")
                .nick("Bus client name")
                .blurb("Bus client name to use for this sink. If empty, a default name will be generated based on the hostname and process ID.")
                .mutable_ready()
                .build(),
                glib::ParamSpecString::builder("videosrv-svc")
                .nick("Video server service name")
                .blurb("EVA ICS video server service name to use for this sink. Default is 'eva.videosrv.default'.")
                .mutable_ready()
                .build(),
            ]
        });

        PROPERTIES.as_ref()
    }
    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "oid" => {
                let mut settings = self.settings.lock();
                let oid: OID = value
                    .get::<String>()
                    .expect("type checked upstream")
                    .parse()
                    .expect("Invalid OID format");
                gst::info!(CAT, "Changing OID from {:?} to {:?}", settings.oid, oid);
                settings.oid = Some(oid);
            }
            "t-start" => {
                let mut settings = self.settings.lock();
                let t_start_s: String = value.get().expect("type checked upstream");
                let t_start = t_start_s.parse::<f64>().expect("Invalid t_start format");
                gst::info!(
                    CAT,
                    "Changing t_start from {:?} to {:?}",
                    settings.t_start,
                    t_start
                );
                settings.t_start = Some(t_start);
            }
            "limit" => {
                let mut settings = self.settings.lock();
                let limit: u32 = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    "Changing frame limit from {} to {}",
                    settings.limit,
                    limit
                );
                settings.limit = limit.try_into().unwrap();
            }
            "bus-path" => {
                let mut settings = self.settings.lock();
                let bus_path: String = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    "Changing bus path from {} to {}",
                    settings.bus_path,
                    bus_path
                );
                settings.bus_path = bus_path;
            }
            "bus-client-name" => {
                let mut settings = self.settings.lock();
                let bus_client_name: String = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    "Changing bus client name from {} to {}",
                    settings.bus_client_name,
                    bus_client_name
                );
                settings.bus_client_name = bus_client_name;
            }
            "videosrv-svc" => {
                let mut settings = self.settings.lock();
                let videosrv_svc: String = value.get().expect("type checked upstream");
                gst::info!(
                    CAT,
                    "Changing video server service from {} to {}",
                    settings.videosrv_svc,
                    videosrv_svc
                );
                settings.videosrv_svc = videosrv_svc;
            }
            _ => unimplemented!(),
        }
    }
    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "oid" => {
                let settings = self.settings.lock();
                settings
                    .oid
                    .as_ref()
                    .map_or_else(String::new, ToString::to_string)
                    .to_value()
            }
            "t-start" => {
                let settings = self.settings.lock();
                settings
                    .t_start
                    .map_or_else(String::new, |t| t.to_string())
                    .to_value()
            }
            "limit" => {
                let settings = self.settings.lock();
                u32::try_from(settings.limit)
                    .expect("Limit should fit into u32")
                    .to_value()
            }
            "bus-path" => {
                let settings = self.settings.lock();
                settings.bus_path.to_value()
            }
            "bus-client-name" => {
                let settings = self.settings.lock();
                settings.bus_client_name.to_value()
            }
            "videosrv-svc" => {
                let settings = self.settings.lock();
                settings.videosrv_svc.to_value()
            }
            _ => unimplemented!(),
        }
    }
}

impl GstObjectImpl for EvaVideoSrvSrc {}

impl ElementImpl for EvaVideoSrvSrc {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "Video source",
                "Source/Video",
                "Sources video frames from EVA ICS Video Server",
                "Bohemia Automation",
            )
        });

        Some(&*ELEMENT_METADATA)
    }
    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let src_pad_template = PadTemplate::new(
                "src",
                PadDirection::Src,
                PadPresence::Always,
                &VideoFormat::all_caps(),
            )
            .unwrap();

            vec![src_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }
}

impl BaseSrcImpl for EvaVideoSrvSrc {
    fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
        let info = VideoInfo::from_caps(caps)
            .map_err(|_| gst::loggable_error!(CAT, "Failed to parse video info from caps"))?;
        gst::debug!(CAT, "Setting video info: {:?}", info);
        Ok(())
    }
    fn fixate(&self, mut caps: gst::Caps) -> gst::Caps {
        #[derive(Serialize)]
        struct RecInfoPayload<'a> {
            i: &'a OID,
            t_start: f64,
            limit: Option<usize>,
        }
        #[derive(Serialize)]
        struct RecPullParams {
            i: OID,
            t_start: Option<f64>,
            t_end: Option<f64>,
            limit: Option<usize>,
        }

        let mut session = self.session.lock();
        if session.is_none() {
            let settings = self.settings.lock();
            let bus_client_name = if settings.bus_client_name.is_empty() {
                default_bus_client_name("evavideosrvsrc")
            } else {
                settings.bus_client_name.clone()
            };
            let bus_config = busrt::sync::ipc::Config::new(&settings.bus_path, &bus_client_name);
            let (client, reader) = busrt::sync::ipc::Client::connect(&bus_config)
                .expect("Failed to connect to bus IPC");
            thread::spawn(move || {
                reader.run();
            });
            let (rpc, rpc_processor) =
                busrt::sync::rpc::RpcClient::new(client, busrt::sync::rpc::DummyHandlers {});
            thread::spawn(move || {
                rpc_processor.run();
            });
            let oid = settings.oid.as_ref().expect("OID is required");
            let payload = pack(&RecInfoPayload {
                i: oid,
                t_start: settings.t_start.expect("t-start is required"),
                limit: if settings.limit > 0 {
                    Some(settings.limit)
                } else {
                    None
                },
            })
            .unwrap();
            let res = rpc
                .call("eva.videosrv.default", "rec.info", payload.into(), QoS::No)
                .unwrap();
            let info: EvaVideoInfo = unpack(res.payload()).expect("Failed to unpack video info");
            let format = VideoFormat::try_from(info.format).expect("Failed to get video format");
            println!(
                "EVA ICS recording for: {} {} {}x{} {}fps",
                oid, format, info.width, info.height, info.fps
            );
            let payload = pack(&RecPullParams {
                i: oid.clone(),
                t_start: settings.t_start,
                t_end: None,
                limit: if settings.limit > 0 {
                    Some(settings.limit)
                } else {
                    None
                },
            })
            .unwrap();
            let cursor: busrt::cursors::Payload = unpack(
                rpc.call(
                    &settings.videosrv_svc,
                    "Crec.pull",
                    payload.into(),
                    QoS::Processed,
                )
                .unwrap()
                .payload(),
            )
            .unwrap();
            let packed_cursor = pack(&cursor).unwrap();
            session.replace(Session {
                rpc,
                format,
                width: info.width,
                height: info.height,
                fps: info.fps,
                videosrv_svc: settings.videosrv_svc.clone(),
                packed_cursor,
            });
        }
        let session = session.as_ref().unwrap();
        caps.truncate();
        {
            let caps = caps.make_mut();
            let s = caps.structure_mut(0).unwrap();
            s.set_name(session.format.to_string());
            s.fixate_field_nearest_int("width", session.width.into());
            s.fixate_field_nearest_int("height", session.height.into());
            s.fixate_field_nearest_fraction("framerate", Fraction::new(session.fps.into(), 1));
        }
        self.parent_fixate(caps)
    }
    fn start(&self) -> Result<(), gst::ErrorMessage> {
        self.unlock_stop()?;
        gst::info!(CAT, "Started");
        Ok(())
    }
    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        self.unlock_stop()?;
        self.session.lock().take();
        gst::info!(CAT, "Stopped");
        Ok(())
    }
    fn is_seekable(&self) -> bool {
        false
    }
}

impl PushSrcImpl for EvaVideoSrvSrc {
    fn create(
        &self,
        _buffer: Option<&mut gst::BufferRef>,
    ) -> Result<CreateSuccess, gst::FlowError> {
        let mut session = self.session.lock();
        let Some(session) = session.as_mut() else {
            gst::element_imp_error!(self, gst::CoreError::Negotiation, ["Have no caps yet"]);
            return Err(gst::FlowError::NotNegotiated);
        };
        let Some(video_frame) = session.next_frame() else {
            return Err(gst::FlowError::Eos);
        };
        let header = eva_common::multimedia::FrameHeader::from_slice(&video_frame.data)
            .expect("Unable to parse frame header");
        if header.width() != session.width
            || header.height() != session.height
            || header.format().unwrap() != session.format
        {
            gst::warning!(CAT, "Stream parameters has been changed");
            return Err(gst::FlowError::Eos);
        }
        let mut buffer =
            gst::Buffer::with_size(video_frame.data.len()).expect("Failed to allocate buffer");
        buffer
            .get_mut()
            .unwrap()
            .copy_from_slice(0, &video_frame.data)
            .expect("Failed to copy pixels into buffer");
        Ok(CreateSuccess::NewBuffer(buffer))
    }
}
