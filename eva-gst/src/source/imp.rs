use std::{sync::LazyLock, thread};

use busrt::sync::client::SyncClient as _;
use eva_common::{
    events::{LOCAL_STATE_TOPIC, REMOTE_STATE_TOPIC},
    multimedia::{FrameHeader, VideoFormat},
    payload::unpack,
    value::Value,
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
    PadDirection, PadPresence, PadTemplate,
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
use serde::Deserialize;

type Sender = rtsc::channel::Sender<BusFrame, parking_lot::RawMutex, parking_lot::Condvar>;
type Receiver = rtsc::channel::Receiver<BusFrame, parking_lot::RawMutex, parking_lot::Condvar>;

use crate::{default_bus_client_name, DEFAULT_BUS_PATH};

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "evasrc",
        gst::DebugColorFlags::empty(),
        Some("EVA ICS Frame Source"),
    )
});

struct BusFrame {
    header: FrameHeader,
    video_format: VideoFormat,
    data: Vec<u8>,
}

fn bus_reader(tx: Sender, bus_path: &str, bus_client_name: &str, oid: OID) {
    #[derive(Deserialize)]
    struct BusPayload {
        value: Value,
    }
    let bus_config = busrt::sync::ipc::Config::new(bus_path, bus_client_name);
    let (mut client, reader) =
        busrt::sync::ipc::Client::connect(&bus_config).expect("Failed to connect to bus IPC");
    thread::spawn(move || {
        reader.run();
    });
    client
        .subscribe_bulk(
            &[
                &format!("{}{}", LOCAL_STATE_TOPIC, oid.as_path()),
                &format!("{}{}", REMOTE_STATE_TOPIC, oid.as_path()),
            ],
            busrt::QoS::No,
        )
        .expect("Failed to subscribe bus");
    let ch = client.take_event_channel().unwrap();
    while let Ok(data) = ch.recv() {
        let p: BusPayload = unpack(data.payload()).expect("Failed to unpack bus data");
        if p.value.is_unit() {
            break;
        }
        let Value::Bytes(mut bytes) = p.value else {
            break;
        };
        let data_bytes = bytes.split_off(FrameHeader::SIZE);
        let header = FrameHeader::from_slice(&bytes).expect("Failed to parse frame header");
        assert!((header.is_version_valid()), "Unsupported frame version",);
        let video_format: VideoFormat = header.format().expect("Unsupported video format");
        let frame = BusFrame {
            header,
            video_format,
            data: data_bytes,
        };
        if let Err(e) = tx.send(frame) {
            if matches!(e, rtsc::Error::ChannelClosed) {
                break;
            }
        }
    }
}

struct Session {
    format: VideoFormat,
    width: u16,
    height: u16,
    rx: Receiver,
}

struct Settings {
    oid: Option<OID>,
    bus_path: String,
    bus_client_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            oid: None,
            bus_path: DEFAULT_BUS_PATH.to_string(),
            bus_client_name: <_>::default(),
        }
    }
}

#[derive(Default)]
pub struct EvaSrc {
    session: Mutex<Option<Session>>,
    settings: Mutex<Settings>,
}

#[glib::object_subclass]
impl ObjectSubclass for EvaSrc {
    const NAME: &'static str = "GstEvaSrc";
    type Type = super::EvaSrc;
    type ParentType = gst_base::PushSrc;
}

impl ObjectImpl for EvaSrc {
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
                glib::ParamSpecString::builder("bus-path")
                .nick("Bus path/addr:port")
                .blurb("EVA ICS bus socket path or address:port to connect to")
                .mutable_ready()
                .build(),
                glib::ParamSpecString::builder("bus-client-name")
                .nick("Bus client name")
                .blurb("Bus client name to use for this sink. If empty, a default name will be generated based on the hostname and process ID.")
                .mutable_ready()
                .build()
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
            "bus-path" => {
                let settings = self.settings.lock();
                settings.bus_path.to_value()
            }
            "bus-client-name" => {
                let settings = self.settings.lock();
                settings.bus_client_name.to_value()
            }
            _ => unimplemented!(),
        }
    }
}

impl GstObjectImpl for EvaSrc {}

impl ElementImpl for EvaSrc {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "Video source",
                "Source/Video",
                "Sources video frames from EVA ICS EAPI",
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

impl BaseSrcImpl for EvaSrc {
    fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
        let info = VideoInfo::from_caps(caps)
            .map_err(|_| gst::loggable_error!(CAT, "Failed to parse video info from caps"))?;
        gst::debug!(CAT, "Setting video info: {:?}", info);
        Ok(())
    }
    fn fixate(&self, mut caps: gst::Caps) -> gst::Caps {
        let mut session = self.session.lock();
        if session.is_none() {
            let (tx, rx) =
                rtsc::channel::bounded::<BusFrame, parking_lot::RawMutex, parking_lot::Condvar>(1);
            let settings = self.settings.lock();
            thread::spawn({
                let oid = settings.oid.as_ref().expect("OID is not set").clone();
                let bus_path = settings.bus_path.clone();
                let bus_client_name = if settings.bus_client_name.is_empty() {
                    default_bus_client_name("evasrc")
                } else {
                    settings.bus_client_name.clone()
                };
                move || bus_reader(tx, &bus_path, &bus_client_name, oid)
            });
            let frame = rx.recv().unwrap();
            let width = frame.header.width();
            let height = frame.header.height();
            let format = frame.video_format;
            session.replace(Session {
                format,
                width,
                height,
                rx,
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

impl PushSrcImpl for EvaSrc {
    fn create(
        &self,
        _buffer: Option<&mut gst::BufferRef>,
    ) -> Result<CreateSuccess, gst::FlowError> {
        let mut session = self.session.lock();
        let Some(session) = session.as_mut() else {
            gst::element_imp_error!(self, gst::CoreError::Negotiation, ["Have no caps yet"]);
            return Err(gst::FlowError::NotNegotiated);
        };
        let frame = session.rx.recv().unwrap();
        if frame.header.width() != session.width
            || frame.header.height() != session.height
            || frame.video_format != session.format
        {
            gst::warning!(CAT, "Stream parameters has been changed");
            return Err(gst::FlowError::Eos);
        }
        let mut buffer =
            gst::Buffer::with_size(frame.data.len()).expect("Failed to allocate buffer");
        buffer
            .get_mut()
            .unwrap()
            .copy_from_slice(0, &frame.data)
            .expect("Failed to copy pixels into buffer");
        Ok(CreateSuccess::NewBuffer(buffer))
    }
}
