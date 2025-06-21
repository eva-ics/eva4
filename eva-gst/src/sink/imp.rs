use std::{
    sync::{Arc, LazyLock},
    thread,
    time::Duration,
};

use crate::{default_bus_client_name, DEFAULT_BUS_PATH};
use atomic_timer::AtomicTimer;
use busrt::{sync::client::SyncClient, QoS};
use eva_common::{
    events::{RawStateEventOwned, RAW_STATE_TOPIC},
    multimedia::{FrameHeader, VideoFormat},
    payload::pack,
    value::Value,
    OID,
};
use gst::{
    glib::{
        self,
        subclass::{object::ObjectImpl, types::ObjectSubclass},
        value::ToValue as _,
        ParamSpecBuilderExt,
    },
    prelude::GstParamSpecBuilderExt as _,
    subclass::prelude::{ElementImpl, GstObjectImpl, ObjectSubclassExt},
    Buffer, ErrorMessage, FlowError, FlowSuccess, LoggableError, PadDirection, PadPresence,
    PadTemplate,
};
use gst_base::subclass::prelude::BaseSinkImpl;
use parking_lot::Mutex;

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "evasink",
        gst::DebugColorFlags::empty(),
        Some("EVA ICS Frame Sink"),
    )
});

#[derive(Default)]
struct Session {
    frame_header: Option<FrameHeader>,
    bus_client: Option<busrt::sync::ipc::Client>,
    bus_client_config: Option<busrt::sync::ipc::Config>,
    bus_topic: Option<Arc<String>>,
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

pub struct EvaSink {
    session: Mutex<Session>,
    settings: Mutex<Settings>,
    bus_reconnect: AtomicTimer,
}

impl EvaSink {
    fn clear_session(&self) -> Option<busrt::sync::ipc::Client> {
        let mut session = self.session.lock();
        self.bus_reconnect.expire_now();
        session.bus_client.take()
    }
}

impl Default for EvaSink {
    fn default() -> Self {
        Self {
            session: Mutex::new(Session::default()),
            settings: Mutex::new(Settings::default()),
            bus_reconnect: AtomicTimer::new(Duration::from_secs(1)),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for EvaSink {
    const NAME: &'static str = "GstEvaSink";
    type Type = super::EvaSink;
    type ParentType = gst_base::BaseSink;
}

impl ObjectImpl for EvaSink {
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

impl GstObjectImpl for EvaSink {}

impl ElementImpl for EvaSink {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "EVA ICS Video sink",
                "Sink/Video",
                "Sinks video frames via EVA ICS EAPI",
                "Bohemia Automation",
            )
        });

        Some(&*ELEMENT_METADATA)
    }
    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let sink_pad_template = PadTemplate::new(
                "sink",
                PadDirection::Sink,
                PadPresence::Always,
                &VideoFormat::all_caps(),
            )
            .unwrap();

            vec![sink_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }
}

impl BaseSinkImpl for EvaSink {
    fn set_caps(&self, caps: &gst::Caps) -> Result<(), LoggableError> {
        let settings = self.settings.lock();
        let oid = settings.oid.as_ref().expect("OID is not set");
        let header = FrameHeader::try_from_caps(caps).expect("Invalid caps for EVA ICS sink");
        println!(
            "EVA ICS sink stream {} {}x{} -> {}",
            header.format().expect("Unsupported video format"),
            header.width(),
            header.height(),
            oid
        );
        let mut session = self.session.lock();
        session.frame_header = Some(header);
        session.bus_topic = Some(format!("{}{}", RAW_STATE_TOPIC, oid.as_path()).into());
        let bus_client_name = if settings.bus_client_name.is_empty() {
            default_bus_client_name("evasink")
        } else {
            settings.bus_client_name.clone()
        };
        session.bus_client_config = Some(busrt::sync::ipc::Config::new(
            &settings.bus_path,
            &bus_client_name,
        ));
        Ok(())
    }
    fn start(&self) -> Result<(), ErrorMessage> {
        self.clear_session();
        self.unlock_stop()?;
        gst::info!(CAT, "Started");
        Ok(())
    }
    fn stop(&self) -> Result<(), ErrorMessage> {
        let bus_client = self.clear_session();
        self.unlock_stop()?;
        gst::info!(CAT, "Stopped");
        let session = self.session.lock();
        let Some(mut bus_client) = bus_client else {
            return Ok(());
        };
        let Some(bus_topic) = session.bus_topic.clone() else {
            return Ok(());
        };
        let payload = pack(&RawStateEventOwned::new(1, eva_common::value::Value::Unit)).unwrap();
        if let Err(e) = bus_client.publish(&bus_topic, payload.into(), QoS::No) {
            gst::warning!(
                CAT,
                "Failed to publish cleanup message to bus topic {}: {}",
                bus_topic,
                e
            );
        }
        Ok(())
    }
    fn render(&self, buffer: &Buffer) -> Result<FlowSuccess, FlowError> {
        let mut session = self.session.lock();
        let header = match session.frame_header.as_ref() {
            None => {
                gst::element_imp_error!(self, gst::CoreError::Negotiation, ["Have no caps yet"]);
                return Err(gst::FlowError::NotNegotiated);
            }
            Some(h) => Clone::clone(h),
        };
        let bus_topic = match session.bus_topic.as_ref() {
            None => {
                gst::element_imp_error!(
                    self,
                    gst::CoreError::Negotiation,
                    ["Have no bus topic yet"]
                );
                return Err(gst::FlowError::NotNegotiated);
            }
            Some(t) => Arc::clone(t),
        };
        if session.bus_client.is_none() {
            if !self.bus_reconnect.reset_if_expired() {
                return Ok(FlowSuccess::Ok);
            }
            let client_config = match session.bus_client_config.as_ref() {
                None => {
                    gst::element_imp_error!(
                        self,
                        gst::CoreError::Negotiation,
                        ["Have no bus client config yet"]
                    );
                    return Err(gst::FlowError::NotNegotiated);
                }
                Some(config) => Clone::clone(config),
            };
            let (client, reader) = match busrt::sync::ipc::Client::connect(&client_config) {
                Ok(c) => c,
                Err(e) => {
                    gst::warning!(CAT, "Failed to connect to bus: {}. Retrying in 1 second", e);
                    return Ok(FlowSuccess::Ok);
                }
            };
            session.bus_client.replace(client);
            thread::spawn(move || {
                reader.run();
            });
        }
        let value =
            Value::try_from_gstreamer_buffer(&header, buffer).expect("Failed to parse buffer");
        let client = session.bus_client.as_mut().unwrap();
        let payload = pack(&RawStateEventOwned::new(1, value)).unwrap();
        if let Err(e) = client.publish(&bus_topic, payload.into(), QoS::No) {
            gst::warning!(
                CAT,
                "Failed to publish to bus topic {}: {}. Reconnecting",
                bus_topic,
                e
            );
            self.clear_session();
        }
        Ok(FlowSuccess::Ok)
    }
}
