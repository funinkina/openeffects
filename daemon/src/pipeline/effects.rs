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
    let studio = &app.effects.studio_light;
    let intensity = if studio.enabled {
        studio.intensity as f64 / 100.0
    } else {
        0.0
    };
    let brightness = (studio.brightness as f64 / 100.0) * intensity;
    let contrast_delta = (studio.contrast as f64 - 50.0) / 50.0;
    let contrast = 1.0 + contrast_delta * intensity;
    balance.set_property("brightness", brightness.clamp(-1.0, 1.0));
    balance.set_property("contrast", contrast.clamp(0.0, 2.0));

    let center = &app.effects.center_stage;
    let crop_value = |value: u32| -> i32 {
        if center.enabled {
            value.min(512) as i32
        } else {
            0
        }
    };
    crop.set_property("top", crop_value(center.crop.top));
    crop.set_property("bottom", crop_value(center.crop.bottom));
    crop.set_property("left", crop_value(center.crop.left));
    crop.set_property("right", crop_value(center.crop.right));
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

    #[test]
    fn phase1_effects_map_state_to_gstreamer_elements() {
        gst::init().unwrap();
        let balance = gst::ElementFactory::make("videobalance").build().unwrap();
        let crop = gst::ElementFactory::make("videocrop").build().unwrap();
        let mut app = AppState::default();

        apply_app_state_to_elements(&balance, &crop, &app);
        assert_eq!(balance.property::<f64>("brightness"), 0.0);
        assert_eq!(balance.property::<f64>("contrast"), 1.0);
        assert_eq!(crop.property::<i32>("left"), 0);

        app.effects.studio_light.enabled = true;
        app.effects.studio_light.intensity = 50;
        app.effects.studio_light.brightness = 80;
        app.effects.studio_light.contrast = 100;
        app.effects.center_stage.enabled = true;
        app.effects.center_stage.crop.left = 24;

        apply_app_state_to_elements(&balance, &crop, &app);
        assert_eq!(balance.property::<f64>("brightness"), 0.4);
        assert_eq!(balance.property::<f64>("contrast"), 1.5);
        assert_eq!(crop.property::<i32>("left"), 24);
    }
}
