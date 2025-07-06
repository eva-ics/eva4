use gst::glib::{self, types::StaticType as _};

mod imp;

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "evavideosrvsrc",
        gst::Rank::NONE,
        EvaVideoSrvSrc::static_type(),
    )
}

glib::wrapper! {
    pub struct EvaVideoSrvSrc(ObjectSubclass<imp::EvaVideoSrvSrc>)
            @extends gst_base::PushSrc, gst_base::BaseSrc, gst::Element, gst::Object;
}
