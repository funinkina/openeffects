use anyhow::{Context, Result};
use gst::prelude::*;
use gstreamer as gst;

use shared::config::AppState;

pub fn build_effects_bin(app: &AppState) -> Result<gst::Bin> {
    let bin = gst::Bin::new();

    let queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 2u32)
        .property_from_str("leaky", "downstream")
        .build()
        .context("create queue")?;
    let balance = gst::ElementFactory::make("videobalance")
        .name("oe_videobalance")
        .build()
        .context("create videobalance")?;
    let crop = gst::ElementFactory::make("videocrop")
        .name("oe_videocrop")
        .build()
        .context("create videocrop")?;
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .context("create videoconvert")?;
    let scale = gst::ElementFactory::make("videoscale")
        .build()
        .context("create videoscale")?;

    apply_app_state_to_elements(&balance, &crop, app);

    bin.add_many([&queue, &balance, &crop, &convert, &scale])?;
    gst::Element::link_many([&queue, &balance, &crop, &convert, &scale])?;

    let sink_pad = queue.static_pad("sink").context("queue sink pad")?;
    let src_pad = scale.static_pad("src").context("scale src pad")?;
    bin.add_pad(&gst::GhostPad::with_target(&sink_pad)?)?;
    bin.add_pad(&gst::GhostPad::with_target(&src_pad)?)?;

    Ok(bin)
}

pub fn apply_app_state_to_elements(balance: &gst::Element, crop: &gst::Element, app: &AppState) {
    let brightness = app.effects.studio_light.brightness as f64 / 100.0;
    let contrast = app.effects.studio_light.contrast as f64 / 50.0;
    balance.set_property("brightness", brightness.clamp(-1.0, 1.0));
    balance.set_property("contrast", contrast.clamp(0.0, 2.0));

    let enabled = app.effects.center_stage.enabled;
    let crop_px = if enabled { 12i32 } else { 0i32 };
    crop.set_property("top", crop_px);
    crop.set_property("bottom", crop_px);
    crop.set_property("left", crop_px);
    crop.set_property("right", crop_px);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_bin_builds_without_panic() {
        gst::init().unwrap();
        let bin = build_effects_bin(&AppState::default()).unwrap();
        assert!(bin.static_pad("sink").is_some());
        assert!(bin.static_pad("src").is_some());
    }
}
