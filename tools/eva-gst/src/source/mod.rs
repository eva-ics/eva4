use gst::glib::{self, types::StaticType as _};

mod imp;

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "evasrc",
        gst::Rank::NONE,
        EvaSrc::static_type(),
    )
}

glib::wrapper! {
    pub struct EvaSrc(ObjectSubclass<imp::EvaSrc>)
            @extends gst_base::PushSrc, gst_base::BaseSrc, gst::Element, gst::Object;
}
