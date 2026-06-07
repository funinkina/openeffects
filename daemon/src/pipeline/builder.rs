use std::sync::Arc;

use anyhow::{Context, Result};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use shared::config::AppState;
use tracing::{info, warn};
use zvariant::OwnedValue;

use super::bridge::Bridge;
use super::{cameras, effects, FPS, HEIGHT, WIDTH};

/// The GStreamer half of the virtual camera: it captures the real webcam, runs
/// the effects bin, and hands each processed frame to the [`Bridge`] via an
/// appsink. It carries no PipeWire output sink — the native provide node
/// ([`super::provider`]) publishes the result. This pipeline only exists while a
/// consumer is connected; building it opens the camera, dropping it releases it.
pub struct BuiltCapture {
    pipeline: gst::Pipeline,
}

impl BuiltCapture {
    pub fn start(&self) -> Result<()> {
        if let Err(err) = self.pipeline.set_state(gst::State::Playing) {
            let bus_err = drain_bus_error(&self.pipeline);
            let _ = self.pipeline.set_state(gst::State::Null);
            if let Some(detail) = bus_err {
                return Err(anyhow::anyhow!("set pipeline to playing: {err}: {detail}"));
            }
            return Err(anyhow::Error::new(err).context("set pipeline to playing"));
        }

        // Block until PLAYING (or failure) up to 5 s so we surface real
        // negotiation errors from elements (camera caps, etc.) instead of
        // returning success on an async transition that later fails.
        let (result, _current, _pending) =
            self.pipeline.state(Some(gst::ClockTime::from_seconds(5)));
        if result.is_err() {
            let bus_err = drain_bus_error(&self.pipeline);
            let _ = self.pipeline.set_state(gst::State::Null);
            if let Some(detail) = bus_err {
                return Err(anyhow::anyhow!(
                    "pipeline failed to reach PLAYING: {detail}"
                ));
            }
            return Err(anyhow::anyhow!("pipeline failed to reach PLAYING"));
        }
        Ok(())
    }

    pub fn stop(self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    pub fn set_enabled(&self, id: &str, on: bool) {
        if id == "center_stage" {
            if let Some(crop) = self.pipeline.by_name("oe_videocrop") {
                let crop_px = if on { 12i32 } else { 0i32 };
                crop.set_property("top", crop_px);
                crop.set_property("bottom", crop_px);
                crop.set_property("left", crop_px);
                crop.set_property("right", crop_px);
            }
        }
    }

    pub fn set_param(&self, id: &str, key: &str, value: &OwnedValue) {
        if id != "studio_light" {
            return;
        }
        if let Some(balance) = self.pipeline.by_name("oe_videobalance") {
            match key {
                "brightness" => {
                    if let Some(v) = shared::dbus::value_as_i32(value) {
                        balance.set_property("brightness", (v as f64 / 100.0).clamp(-1.0, 1.0));
                    }
                }
                "contrast" => {
                    if let Some(v) = shared::dbus::value_as_u32(value) {
                        balance.set_property("contrast", (v as f64 / 50.0).clamp(0.0, 2.0));
                    }
                }
                _ => {}
            }
        }
    }
}

/// The fixed output caps: every processed frame the appsink delivers (and thus
/// every frame the provide node serves) is I420 at this resolution/framerate.
fn output_caps() -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("width", WIDTH as i32)
        .field("height", HEIGHT as i32)
        .field("framerate", gst::Fraction::new(FPS, 1))
        .build()
}

pub fn build_capture_pipeline(app: &AppState, bridge: Arc<Bridge>) -> Result<BuiltCapture> {
    let pipeline = gst::Pipeline::new();
    let source = build_source(app)?;

    // Many cameras (including UVC webcams) only advertise compressed formats such
    // as image/jpeg over PipeWire/V4L2. decodebin transparently plugs a decoder
    // when needed and passes raw video through otherwise.
    let decoder = gst::ElementFactory::make("decodebin")
        .name("oe_in_decode")
        .build()
        .context("create input decodebin")?;

    // Normalise any camera-native format to I420 at the fixed virtual-camera
    // resolution/framerate. videoconvert handles colorspace/pixel-format,
    // videoscale handles size, the capsfilter pins format + framerate.
    let convert = gst::ElementFactory::make("videoconvert")
        .name("oe_in_convert")
        .build()
        .context("create input videoconvert")?;
    let scale = gst::ElementFactory::make("videoscale")
        .name("oe_in_scale")
        .build()
        .context("create input videoscale")?;
    let caps = gst::ElementFactory::make("capsfilter")
        .property("caps", output_caps())
        .build()
        .context("create capsfilter")?;

    let effects_bin = effects::build_effects_bin(app)?;

    // appsink delivers each processed frame to the bridge. drop + max-buffers=1
    // keeps only the newest frame on the sink side too, matching the bridge's
    // latest-frame semantics for a live camera.
    let appsink = gst_app::AppSink::builder()
        .name("oe_appsink")
        .caps(&output_caps())
        .max_buffers(1)
        .drop(true)
        .build();

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                bridge.store(map.as_slice().to_vec());
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    pipeline.add_many([
        &source,
        &decoder,
        &convert,
        &scale,
        &caps,
        effects_bin.upcast_ref(),
        appsink.upcast_ref(),
    ])?;

    // source → decoder is a static link. decoder → convert is linked from
    // decodebin's pad-added signal because decodebin only exposes its src pad
    // after caps negotiation completes.
    gst::Element::link(&source, &decoder).context("link source -> decoder")?;
    let convert_weak = convert.downgrade();
    decoder.connect_pad_added(move |_dbin, src_pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let Some(sink_pad) = convert.static_pad("sink") else {
            return;
        };
        if sink_pad.is_linked() {
            return;
        }
        if let Err(err) = src_pad.link(&sink_pad) {
            tracing::error!(?err, "failed to link decodebin -> videoconvert");
        }
    });
    gst::Element::link_many([
        &convert,
        &scale,
        &caps,
        effects_bin.upcast_ref(),
        appsink.upcast_ref(),
    ])?;

    Ok(BuiltCapture { pipeline })
}

fn drain_bus_error(pipeline: &gst::Pipeline) -> Option<String> {
    let bus = pipeline.bus()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        let Some(msg) = bus.timed_pop(Some(gst::ClockTime::from_mseconds(50))) else {
            continue;
        };
        if let gst::MessageView::Error(err) = msg.view() {
            let src = err
                .src()
                .map(|s| s.path_string().to_string())
                .unwrap_or_else(|| "<unknown>".into());
            let debug = err.debug().map(|d| format!(" [{d}]")).unwrap_or_default();
            return Some(format!("{src}: {}{debug}", err.error()));
        }
    }
    None
}

fn build_source(app: &AppState) -> Result<gst::Element> {
    let selected = app.camera.selected.trim();

    if !selected.is_empty() {
        if let Some(info) = cameras::enumerate().into_iter().find(|c| c.id == selected) {
            info!(id = %info.id, name = %info.name, api = %info.api, "using configured camera");
            return cameras::build_source_for(&info);
        }
        // Honour an explicit /dev/videoN even if DeviceMonitor missed it.
        if selected.starts_with("/dev/video") && gst::ElementFactory::find("v4l2src").is_some() {
            info!(device = %selected, "using configured v4l2 device");
            return gst::ElementFactory::make("v4l2src")
                .property("device", selected)
                .build()
                .context("create v4l2src");
        }
        warn!(
            selected,
            "configured camera not found; falling back to autodetect"
        );
    }

    if let Some(info) = cameras::autodetect() {
        info!(id = %info.id, name = %info.name, api = %info.api, "auto-selected camera");
        return cameras::build_source_for(&info);
    }

    warn!("no camera available; using videotestsrc fallback");
    gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "smpte")
        .build()
        .context("create videotestsrc fallback")
}
