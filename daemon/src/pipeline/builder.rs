use std::sync::Arc;

use anyhow::{Context, Result};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use shared::config::AppState;
use tracing::{info, warn};

use super::bridge::Bridge;
use super::{cameras, effects, PipelineFormat};

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

    pub fn apply_app_state(&self, app: &AppState) {
        if let Some(effects_el) = self.pipeline.by_name("oe_effects") {
            effects::apply_app_state_to_elements(&effects_el, app);
        }
    }

    /// Fire a one-shot reaction burst into the live effects element via its
    /// write-only `trigger-reaction` property.
    pub fn trigger_reaction(&self, id: &str) {
        if let Some(effects_el) = self.pipeline.by_name("oe_effects") {
            effects_el.set_property("trigger-reaction", id);
        }
    }
}

/// The output caps: every processed frame the appsink delivers (and thus every
/// frame the provide node serves) is I420 at the camera's native mode.
fn output_caps(fmt: PipelineFormat) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("width", fmt.width as i32)
        .field("height", fmt.height as i32)
        .field("framerate", gst::Fraction::new(fmt.fps, 1))
        .build()
}

/// Caps pinned directly on the camera source: same dimensions/rate as the
/// output, in either raw or MJPEG (decodebin decodes the latter). This forces
/// the camera to capture *at* the virtual-camera mode instead of letting
/// negotiation pick an arbitrary default (e.g. raw 640x480 on cameras that only
/// do 720p in MJPEG), which videoscale would then stretch to the output size
/// and distort the aspect ratio.
fn source_caps(fmt: PipelineFormat) -> gst::Caps {
    let dims = |name: &str| {
        gst::Structure::builder(name)
            .field("width", fmt.width as i32)
            .field("height", fmt.height as i32)
            .field("framerate", gst::Fraction::new(fmt.fps, 1))
            .build()
    };
    gst::Caps::builder_full()
        .structure(dims("video/x-raw"))
        .structure(dims("image/jpeg"))
        .build()
}

/// The format the virtual camera should advertise for the currently selected
/// camera: its preferred native mode, or the default if nothing can be probed
/// (no camera -> `videotestsrc`, which renders any mode).
pub fn probe_format(app: &AppState) -> PipelineFormat {
    resolve_camera(app)
        .and_then(|info| cameras::preferred_format(&info))
        .unwrap_or(PipelineFormat::DEFAULT)
}

pub fn build_capture_pipeline(
    app: &AppState,
    fmt: PipelineFormat,
    bridge: Arc<Bridge>,
) -> Result<BuiltCapture> {
    let pipeline = gst::Pipeline::new();
    let source = build_source(app)?;
    let src_filter = gst::ElementFactory::make("capsfilter")
        .name("oe_src_caps")
        .property("caps", source_caps(fmt))
        .build()
        .context("create source capsfilter")?;

    // Many cameras (including UVC webcams) only advertise compressed formats such
    // as image/jpeg over PipeWire/V4L2. decodebin transparently plugs a decoder
    // when needed and passes raw video through otherwise.
    let decoder = gst::ElementFactory::make("decodebin")
        .name("oe_in_decode")
        .build()
        .context("create input decodebin")?;

    // Convert the camera-native pixel format to I420. The source capsfilter
    // already pinned the dimensions/rate to the output mode, so videoconvert
    // only changes colorspace/pixel-format and videoscale is a no-op kept as a
    // safety net — it never has to change the aspect ratio.
    let convert = gst::ElementFactory::make("videoconvert")
        .name("oe_in_convert")
        .build()
        .context("create input videoconvert")?;
    let scale = gst::ElementFactory::make("videoscale")
        .name("oe_in_scale")
        .build()
        .context("create input videoscale")?;
    let caps = gst::ElementFactory::make("capsfilter")
        .property("caps", output_caps(fmt))
        .build()
        .context("create capsfilter")?;

    let effects_bin = effects::build_effects_bin(app)?;

    // appsink delivers each processed frame to the bridge. drop + max-buffers=1
    // keeps only the newest frame on the sink side too, matching the bridge's
    // latest-frame semantics for a live camera.
    let appsink = gst_app::AppSink::builder()
        .name("oe_appsink")
        .caps(&output_caps(fmt))
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
        &src_filter,
        &decoder,
        &convert,
        &scale,
        &caps,
        effects_bin.upcast_ref(),
        appsink.upcast_ref(),
    ])?;

    // source → src_filter → decoder is a static link. decoder → convert is
    // linked from decodebin's pad-added signal because decodebin only exposes
    // its src pad after caps negotiation completes.
    gst::Element::link_many([&source, &src_filter, &decoder])
        .context("link source -> src caps -> decoder")?;
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

/// Resolve which physical camera the pipeline should use: the configured one
/// when it exists, otherwise autodetect. `None` means no camera at all
/// (`videotestsrc` fallback). Shared by `probe_format` and `build_source` so
/// the probed mode always belongs to the camera that is actually opened.
pub(crate) fn resolve_camera(app: &AppState) -> Option<cameras::CameraInfo> {
    let selected = app.camera.selected.trim();

    if !selected.is_empty() {
        if let Some(info) = cameras::enumerate().into_iter().find(|c| c.id == selected) {
            return Some(info);
        }
        // Honour an explicit /dev/videoN even if DeviceMonitor missed it.
        if selected.starts_with("/dev/video") {
            return Some(cameras::CameraInfo {
                id: selected.to_string(),
                name: selected.to_string(),
                path: selected.to_string(),
                api: String::from("v4l2"),
            });
        }
        warn!(
            selected,
            "configured camera not found; falling back to autodetect"
        );
    }

    cameras::autodetect()
}

fn build_source(app: &AppState) -> Result<gst::Element> {
    if let Some(info) = resolve_camera(app) {
        info!(id = %info.id, name = %info.name, api = %info.api, "using camera");
        return cameras::build_source_for(&info);
    }

    warn!("no camera available; using videotestsrc fallback");
    gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "smpte")
        .build()
        .context("create videotestsrc fallback")
}
