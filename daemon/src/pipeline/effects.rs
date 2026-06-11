use anyhow::{Context, Result};
use gst::prelude::*;
use gstreamer as gst;

use shared::config::AppState;

/// Build the effects bin:
///
/// `queue → videoconvert → RGBA caps → oe_effects → videoconvert → videoscale`
///
/// All effects (studio light, portrait blur, background replace, center stage)
/// live in the single `oe_effects` CPU filter, which runs on RGBA system memory
/// — hence the `videoconvert` + RGBA capsfilter wrapping it. Studio Light used
/// to be a stock `videobalance`, but it now runs inside `oe_effects` so the
/// brightness/contrast lift can be masked to the person via selfie segmentation
/// (sharing the same mask as portrait blur / bg replace). The trailing
/// `videoconvert`/`videoscale` convert back toward the I420 output the appsink
/// and provide node expect.
pub fn build_effects_bin(app: &AppState) -> Result<gst::Bin> {
    let bin = gst::Bin::new();

    let queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 2u32)
        .property_from_str("leaky", "downstream")
        .build()
        .context("create queue")?;
    let convert_in = gst::ElementFactory::make("videoconvert")
        .build()
        .context("create videoconvert (to RGBA)")?;
    let rgba_caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .build(),
        )
        .build()
        .context("create RGBA capsfilter")?;
    let effects = gst::ElementFactory::make("oe_effects")
        .name("oe_effects")
        .build()
        .context("create oe_effects")?;
    let convert_out = gst::ElementFactory::make("videoconvert")
        .build()
        .context("create videoconvert (from RGBA)")?;
    let scale = gst::ElementFactory::make("videoscale")
        .build()
        .context("create videoscale")?;

    apply_app_state_to_elements(&effects, app);

    bin.add_many([
        &queue,
        &convert_in,
        &rgba_caps,
        &effects,
        &convert_out,
        &scale,
    ])?;
    gst::Element::link_many([
        &queue,
        &convert_in,
        &rgba_caps,
        &effects,
        &convert_out,
        &scale,
    ])?;

    let sink_pad = queue.static_pad("sink").context("queue sink pad")?;
    let src_pad = scale.static_pad("src").context("scale src pad")?;
    bin.add_pad(&gst::GhostPad::with_target(&sink_pad)?)?;
    bin.add_pad(&gst::GhostPad::with_target(&src_pad)?)?;

    Ok(bin)
}

/// Push the current config into the live `oe_effects` element. Studio Light's
/// brightness/contrast are computed here (the same mapping the old
/// `videobalance` used) and pushed as `oe_effects` properties, where the lift
/// is masked to the person.
pub fn apply_app_state_to_elements(effects: &gst::Element, app: &AppState) {
    let studio = &app.effects.studio_light;
    let intensity = if studio.enabled {
        studio.intensity as f64 / 100.0
    } else {
        0.0
    };
    let brightness = (studio.brightness as f64 / 100.0) * intensity;
    let contrast_delta = (studio.contrast as f64 - 50.0) / 50.0;
    let contrast = 1.0 + contrast_delta * intensity;
    effects.set_property("studio-light-enabled", studio.enabled);
    effects.set_property("studio-light-brightness", brightness.clamp(-1.0, 1.0) as f32);
    effects.set_property("studio-light-contrast", contrast.clamp(0.0, 2.0) as f32);

    let blur = &app.effects.portrait_blur;
    effects.set_property("portrait-blur-enabled", blur.enabled);
    effects.set_property("portrait-blur-strength", blur.strength as u32);

    let bg = &app.effects.bg_replace;
    effects.set_property("bg-replace-enabled", bg.enabled);
    effects.set_property("bg-replace-path", &bg.background);

    let center = &app.effects.center_stage;
    effects.set_property("center-stage-enabled", center.enabled);
    effects.set_property("center-stage-zoom", &center.zoom);
    effects.set_property("center-stage-mode", &center.mode);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects_available() -> bool {
        gst::init().unwrap();
        super::super::filters::register().unwrap();
        gst::ElementFactory::find("videoconvert").is_some()
    }

    #[test]
    fn effects_bin_builds_without_panic() {
        if !effects_available() {
            eprintln!("skipping: base GStreamer plugins not available");
            return;
        }
        let bin = build_effects_bin(&AppState::default()).unwrap();
        assert!(bin.static_pad("sink").is_some());
        assert!(bin.static_pad("src").is_some());
    }

    #[test]
    fn oe_effects_registers_with_default_properties() {
        gst::init().unwrap();
        super::super::filters::register().unwrap();

        let effects = gst::ElementFactory::make("oe_effects").build().unwrap();
        assert!(!effects.property::<bool>("portrait-blur-enabled"));
        assert_eq!(effects.property::<u32>("portrait-blur-strength"), 50);
        assert!(!effects.property::<bool>("center-stage-enabled"));
        assert_eq!(effects.property::<String>("center-stage-zoom"), "normal");
        assert_eq!(effects.property::<String>("center-stage-mode"), "single");
        assert!(!effects.property::<bool>("bg-replace-enabled"));
    }

    #[test]
    fn app_state_maps_to_elements() {
        gst::init().unwrap();
        super::super::filters::register().unwrap();
        let effects = gst::ElementFactory::make("oe_effects").build().unwrap();
        let mut app = AppState::default();

        apply_app_state_to_elements(&effects, &app);
        assert!(!effects.property::<bool>("studio-light-enabled"));
        assert_eq!(effects.property::<f32>("studio-light-brightness"), 0.0);
        assert_eq!(effects.property::<f32>("studio-light-contrast"), 1.0);
        assert!(!effects.property::<bool>("portrait-blur-enabled"));
        assert!(!effects.property::<bool>("center-stage-enabled"));

        app.effects.studio_light.enabled = true;
        app.effects.studio_light.intensity = 50;
        app.effects.studio_light.brightness = 80;
        app.effects.studio_light.contrast = 100;
        app.effects.center_stage.enabled = true;
        app.effects.center_stage.zoom = "tight".into();
        app.effects.portrait_blur.enabled = true;
        app.effects.portrait_blur.strength = 75;
        app.effects.bg_replace.enabled = true;
        app.effects.bg_replace.background = "#102030".into();

        apply_app_state_to_elements(&effects, &app);
        assert!(effects.property::<bool>("studio-light-enabled"));
        assert_eq!(effects.property::<f32>("studio-light-brightness"), 0.4);
        assert_eq!(effects.property::<f32>("studio-light-contrast"), 1.5);
        assert!(effects.property::<bool>("center-stage-enabled"));
        assert_eq!(effects.property::<String>("center-stage-zoom"), "tight");
        assert!(effects.property::<bool>("portrait-blur-enabled"));
        assert_eq!(effects.property::<u32>("portrait-blur-strength"), 75);
        assert!(effects.property::<bool>("bg-replace-enabled"));
        assert_eq!(effects.property::<String>("bg-replace-path"), "#102030");
    }

    /// End-to-end smoke test of the hot path: push real frames through the
    /// effects bin with portrait blur + center stage enabled, exercising
    /// `oe_effects::transform_frame_ip` (segmentation, composite, crop) on the
    /// GStreamer streaming thread. Runs to EOS without error.
    #[test]
    fn effects_bin_processes_frames() {
        if !effects_available() || gst::ElementFactory::find("videotestsrc").is_none() {
            eprintln!("skipping: GStreamer plugins not available");
            return;
        }
        let mut app = AppState::default();
        app.effects.portrait_blur.enabled = true;
        app.effects.portrait_blur.strength = 70;
        app.effects.center_stage.enabled = true;
        app.effects.center_stage.zoom = "normal".into();

        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 6i32)
            .build()
            .unwrap();
        let caps = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", 320i32)
                    .field("height", 240i32)
                    .build(),
            )
            .build()
            .unwrap();
        let bin = build_effects_bin(&app).unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        pipeline
            .add_many([&src, &caps, bin.upcast_ref(), &sink])
            .unwrap();
        gst::Element::link_many([&src, &caps, bin.upcast_ref(), &sink]).unwrap();

        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let mut saw_eos = false;
        for msg in bus.iter_timed(gst::ClockTime::from_seconds(15)) {
            match msg.view() {
                gst::MessageView::Eos(_) => {
                    saw_eos = true;
                    break;
                }
                gst::MessageView::Error(err) => {
                    pipeline.set_state(gst::State::Null).unwrap();
                    panic!("pipeline error: {} ({:?})", err.error(), err.debug());
                }
                _ => {}
            }
        }
        pipeline.set_state(gst::State::Null).unwrap();
        assert!(saw_eos, "pipeline did not reach EOS within timeout");
    }
}
