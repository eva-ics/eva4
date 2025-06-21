use binrw::prelude::*;
use gst::glib;

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
