use binrw::prelude::*;
use gst::{glib, Caps};
use strum::{Display, EnumIter, EnumString};

mod sink;
mod source;

const DEFAULT_BUS_PATH: &str = "/opt/eva4/var/bus.ipc";

fn default_bus_client_name(mid_name: &str) -> String {
    format!(
        "{}.{}.{}",
        hostname::get()
            .unwrap_or("unknown_host".into())
            .to_string_lossy(),
        mid_name,
        std::process::id()
    )
}

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    register(plugin)?;
    Ok(())
}

gst::plugin_define!(
    eva,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION"), "-", env!("COMMIT_ID")),
    "Apache 2.0",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    env!("BUILD_REL_DATE")
);

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    source::register(plugin)?;
    sink::register(plugin)?;
    Ok(())
}

#[binrw]
#[brw(little, magic = b"EVS")]
#[derive(Clone)]
struct FrameHeader {
    version: u8,
    format: u8,
    width: u16,
    height: u16,
    // bits
    // 0 - key frame
    // 1-7 - reserved
    metadata: u8,
}

impl FrameHeader {
    const SIZE: usize = 7 + 3;
    fn from_slice(slice: &[u8]) -> Result<Self, binrw::Error> {
        let mut cursor = std::io::Cursor::new(slice);
        FrameHeader::read(&mut cursor)
    }
    fn into_vec(self, vec_size: usize) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::with_capacity(vec_size));
        self.write(&mut buf).expect("Failed to write frame header");
        buf.into_inner()
    }
}

const VERSION: u8 = 1;

#[derive(EnumString, EnumIter, Display, Debug, PartialEq, Copy, Clone)]
#[repr(u8)]
enum VideoFormat {
    #[strum(serialize = "video/x-h264")]
    H264 = 10,
    #[strum(serialize = "video/x-h265")]
    H265 = 11,
    #[strum(serialize = "video/x-vp8")]
    VP8 = 12,
    #[strum(serialize = "video/x-vp9")]
    VP9 = 13,
    #[strum(serialize = "video/x-av1")]
    AV1 = 14,
}

impl TryFrom<u8> for VideoFormat {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            10 => Ok(VideoFormat::H264),
            11 => Ok(VideoFormat::H265),
            12 => Ok(VideoFormat::VP8),
            13 => Ok(VideoFormat::VP9),
            14 => Ok(VideoFormat::AV1),
            _ => Err(format!("Unknown video format code: {}", value)),
        }
    }
}

impl VideoFormat {
    fn into_caps(self) -> gst::Caps {
        Caps::builder(self.to_string()).build()
    }
}
