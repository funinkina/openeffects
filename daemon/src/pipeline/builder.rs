use anyhow::{Context, Result};
use gst::prelude::*;
use gstreamer as gst;
use shared::config::AppState;
use zvariant::OwnedValue;

use super::{
    effects,
    probe::{probe_output_sink, OutputSink},
};

pub struct BuiltPipeline {
    pipeline: gst::Pipeline,
    output_sink: String,
    sink_type: OutputSink,
}

impl BuiltPipeline {
    pub fn start(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .context("set pipeline to playing")?;
        Ok(())
    }

    pub fn stop(self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    pub fn pause(&self) {
        let _ = self.pipeline.set_state(gst::State::Paused);
    }

    pub fn output_sink(&self) -> &str {
        &self.output_sink
    }

    pub fn sink_type(&self) -> &OutputSink {
        &self.sink_type
    }

    pub fn consumer_connected(&self) -> Option<bool> {
        match &self.sink_type {
            // fakesink: dev/test mode, treat as always connected so auto-pause never fires
            OutputSink::None => Some(true),
            // v4l2: no reliable consumer count; never auto-pause
            OutputSink::V4l2Loopback { .. } => None,
            // pipewire: no cheap consumer probe; never auto-pause until a
            // PipeWire registry-based count is wired up.
            OutputSink::PipeWire { .. } => None,
        }
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

pub fn build_pipeline(app: &AppState) -> Result<BuiltPipeline> {
    let pipeline = gst::Pipeline::new();
    let source = build_source(app)?;
    // Input caps: constrain source to a concrete format so downstream elements
    // and v4l2sink can advertise a definite format.
    let caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "I420")
                .field("width", 1280i32)
                .field("height", 720i32)
                .field("framerate", gst::Fraction::new(30, 1))
                .build(),
        )
        .build()
        .context("create capsfilter")?;

    let effects_bin = effects::build_effects_bin(app)?;
    let (sink, output_sink, sink_type) = build_sink()?;

    pipeline.add_many([&source, &caps, effects_bin.upcast_ref(), &sink])?;
    gst::Element::link_many([&source, &caps, effects_bin.upcast_ref(), &sink])?;

    Ok(BuiltPipeline {
        pipeline,
        output_sink,
        sink_type,
    })
}

fn build_source(app: &AppState) -> Result<gst::Element> {
    if app.camera.selected.starts_with("/dev/video")
        && gst::ElementFactory::find("v4l2src").is_some()
    {
        return gst::ElementFactory::make("v4l2src")
            .property("device", &app.camera.selected)
            .build()
            .context("create v4l2src");
    }

    // Non-empty, non-path selection = PipeWire node name/ID.
    // Empty = no camera configured yet; pipewiresrc without a target-object
    // fails caps negotiation, so fall through to videotestsrc until Phase 2
    // camera enumeration is implemented.
    if !app.camera.selected.is_empty() && gst::ElementFactory::find("pipewiresrc").is_some() {
        return gst::ElementFactory::make("pipewiresrc")
            .property("target-object", &app.camera.selected)
            .build()
            .context("create pipewiresrc");
    }

    gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "smpte")
        .build()
        .context("create videotestsrc fallback")
}

fn build_sink() -> Result<(gst::Element, String, OutputSink)> {
    let sink = probe_output_sink();
    match sink {
        OutputSink::PipeWire { ref node_name } => {
            // pipewiresink in mode=provide registers a new PipeWire node that
            // other clients can consume. The media.class=Video/Source tag is
            // what makes PipeWire-aware apps (browsers via xdg-portal, OBS,
            // etc.) discover the node as a camera.
            let props = gst::Structure::builder("props")
                .field("media.class", "Video/Source")
                .field("media.type", "Video")
                .field("media.role", "Camera")
                .field("node.name", node_name.as_str())
                .field("node.description", "OpenEffects Virtual Camera")
                .field("object.register", "true")
                .field("node.export", "true")
                .build();
            let element = gst::ElementFactory::make("pipewiresink")
                .name("oe_output_sink")
                .property_from_str("mode", "provide")
                .property("stream-properties", &props)
                .build()
                .context("create pipewiresink")?;
            let label = format!("pipewire:{node_name}");
            Ok((element, label, sink))
        }
        OutputSink::V4l2Loopback { ref device } => {
            let element = gst::ElementFactory::make("v4l2sink")
                .name("oe_output_sink")
                .property("device", device.as_str())
                .build()
                .context("create v4l2sink")?;
            let label = format!("v4l2:{device}");
            Ok((element, label, sink))
        }
        OutputSink::None => {
            let element = gst::ElementFactory::make("fakesink")
                .name("oe_output_sink")
                .property("sync", false)
                .build()
                .context("create fakesink")?;
            Ok((element, "none".into(), sink))
        }
    }
}
