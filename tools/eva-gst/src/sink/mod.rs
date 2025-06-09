use gst::glib::{self, types::StaticType as _};

mod imp;

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "evasink",
        gst::Rank::NONE,
        EvaSink::static_type(),
    )
}

glib::wrapper! {
    pub struct EvaSink(ObjectSubclass<imp::EvaSink>)
            @extends gst_base::BaseSink, gst::Element, gst::Object;
}
