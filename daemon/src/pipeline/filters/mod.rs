pub mod oe_effects;
pub mod reactions;

pub use oe_effects::OeEffects;

use gst::glib;
use gst::prelude::*;
use gstreamer as gst;

/// Register the `oe_effects` CPU video filter so the effects bin can create it
/// by name. Idempotent: skips registration if the element already exists (e.g.
/// when called more than once in the same process, such as across tests).
pub fn register() -> Result<(), glib::BoolError> {
    if gst::ElementFactory::find("oe_effects").is_none() {
        gst::Element::register(
            None,
            "oe_effects",
            gst::Rank::NONE,
            OeEffects::static_type(),
        )?;
    }
    Ok(())
}
