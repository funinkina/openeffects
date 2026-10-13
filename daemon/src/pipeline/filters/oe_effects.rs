//! `oe_effects` — the single CPU video filter that hosts every ML-driven
//! effect. It receives RGBA frames in system memory, packs them into a tight
//! buffer and hands them to [`oe_core::effects::Effects`], which holds the
//! effects themselves. This element only maps GObject properties onto the
//! effect settings, swaps inference providers, and frees idle models.

use gst::glib;
use gst::glib::subclass::prelude::*;
use gstreamer as gst;
use gstreamer_video as gst_video;

glib::wrapper! {
    pub struct OeEffects(ObjectSubclass<imp::OeEffects>)
        @extends gst_video::VideoFilter, gstreamer_base::BaseTransform, gst::Element, gst::Object;
}

impl OeEffects {
    pub fn swap_provider(&self, target_provider: &str) -> anyhow::Result<(String, Option<String>)> {
        self.imp().swap_provider(target_provider)
    }

    pub fn generation(&self) -> u64 {
        self.imp().generation()
    }
}

mod imp {
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    use gst::glib;
    use gst_base::subclass::BaseTransformMode;
    use gstreamer as gst;
    use gstreamer_base as gst_base;
    use gstreamer_video as gst_video;
    use gstreamer_video::prelude::*;
    use gstreamer_video::subclass::prelude::*;
    use oe_core::effects::Effects;
    use oe_core::effects::settings::{DEFAULT_MODE, DEFAULT_STRENGTH, DEFAULT_ZOOM};
    use oe_core::inference::engine::Engine;

    #[derive(Default)]
    struct State {
        effects: Effects,
        /// Reusable packed-frame buffer (a multi-MB alloc otherwise repeated
        /// every frame).
        canvas: Vec<u8>,
    }

    /// How long a model may sit unused before its session is freed. Passthrough
    /// stops frames reaching the filter, so this runs on a timer, not per frame.
    const MODEL_IDLE_UNLOAD: Duration = Duration::from_secs(60);
    const REAP_INTERVAL: Duration = Duration::from_secs(5);

    fn spawn_model_reaper(weak: glib::WeakRef<super::OeEffects>) {
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(REAP_INTERVAL);
                let Some(obj) = weak.upgrade() else { return };
                let freed = obj
                    .imp()
                    .state
                    .lock()
                    .unwrap()
                    .effects
                    .engine
                    .unload_idle(MODEL_IDLE_UNLOAD);
                if freed {
                    tracing::info!("unloaded idle models");
                    unsafe { libc::malloc_trim(0) };
                }
            }
        });
    }

    #[derive(Default)]
    pub struct OeEffects {
        state: Mutex<State>,
    }

    impl OeEffects {
        pub fn generation(&self) -> u64 {
            self.state.lock().unwrap().effects.engine.generation
        }

        /// Reload every loaded model on `target_provider` without holding the
        /// state lock, so frames keep flowing on the old sessions while the new
        /// ones compile, then swap them in at once. On error the models stay put.
        pub fn swap_provider(
            &self,
            target_provider: &str,
        ) -> anyhow::Result<(String, Option<String>)> {
            let target = target_provider.trim().to_ascii_lowercase();
            let wanted = self.state.lock().unwrap().effects.engine.loaded();
            let fresh = Engine::load(&target, wanted)?;
            let mut state = self.state.lock().unwrap();
            let engine = &mut state.effects.engine;
            engine.replace(fresh);
            Ok((engine.active.to_string(), engine.detail.clone()))
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OeEffects {
        const NAME: &'static str = "OeEffects";
        type Type = super::OeEffects;
        type ParentType = gst_video::VideoFilter;
    }

    impl ObjectImpl for OeEffects {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
                vec![
                    glib::ParamSpecBoolean::builder("portrait-blur-enabled").build(),
                    glib::ParamSpecUInt::builder("portrait-blur-strength")
                        .minimum(0)
                        .maximum(100)
                        .default_value(DEFAULT_STRENGTH)
                        .build(),
                    glib::ParamSpecBoolean::builder("bg-replace-enabled").build(),
                    glib::ParamSpecString::builder("bg-replace-path").build(),
                    glib::ParamSpecBoolean::builder("center-stage-enabled").build(),
                    glib::ParamSpecString::builder("center-stage-zoom")
                        .default_value(Some(DEFAULT_ZOOM))
                        .build(),
                    glib::ParamSpecString::builder("center-stage-mode")
                        .default_value(Some(DEFAULT_MODE))
                        .build(),
                    glib::ParamSpecBoolean::builder("studio-light-enabled").build(),
                    glib::ParamSpecBoolean::builder("auto-light-enabled").build(),
                    glib::ParamSpecFloat::builder("auto-light-strength")
                        .minimum(0.0)
                        .maximum(1.0)
                        .default_value(0.8)
                        .build(),
                    glib::ParamSpecFloat::builder("studio-light-brightness")
                        .minimum(-1.0)
                        .maximum(1.0)
                        .default_value(0.0)
                        .build(),
                    glib::ParamSpecFloat::builder("studio-light-contrast")
                        .minimum(0.0)
                        .maximum(2.0)
                        .default_value(1.0)
                        .build(),
                    glib::ParamSpecFloat::builder("studio-light-bg-brightness")
                        .minimum(-1.0)
                        .maximum(1.0)
                        .default_value(0.0)
                        .build(),
                    glib::ParamSpecBoolean::builder("reactions-enabled").build(),
                    // Write-only one-shot: setting it (even to a repeated value)
                    // fires a burst of that reaction id.
                    glib::ParamSpecString::builder("trigger-reaction")
                        .flags(glib::ParamFlags::WRITABLE)
                        .build(),
                    glib::ParamSpecString::builder("inference-provider")
                        .default_value(Some("auto"))
                        .build(),
                    glib::ParamSpecUInt64::builder("inference-generation")
                        .default_value(0)
                        .read_only()
                        .build(),
                    glib::ParamSpecString::builder("inference-active")
                        .default_value(Some("cpu"))
                        .read_only()
                        .build(),
                ]
            });
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let mut state = self.state.lock().unwrap();
            let fx = &mut state.effects;
            let s = &mut fx.settings;
            match pspec.name() {
                "portrait-blur-enabled" => s.blur_enabled = value.get().unwrap(),
                "portrait-blur-strength" => s.blur_strength = value.get().unwrap(),
                "bg-replace-enabled" => s.bg_enabled = value.get().unwrap(),
                "bg-replace-path" => {
                    fx.set_bg_path(value.get::<Option<String>>().unwrap().unwrap_or_default())
                }
                "center-stage-enabled" => s.cs_enabled = value.get().unwrap(),
                "center-stage-zoom" => {
                    s.cs_zoom = value
                        .get::<Option<String>>()
                        .unwrap()
                        .unwrap_or_else(|| DEFAULT_ZOOM.into());
                }
                "center-stage-mode" => {
                    s.cs_mode = value
                        .get::<Option<String>>()
                        .unwrap()
                        .unwrap_or_else(|| DEFAULT_MODE.into());
                }
                "studio-light-enabled" => s.studio_enabled = value.get().unwrap(),
                "auto-light-enabled" => s.auto_light_enabled = value.get().unwrap(),
                "auto-light-strength" => s.auto_light_strength = value.get().unwrap(),
                "studio-light-brightness" => s.studio_brightness = value.get().unwrap(),
                "studio-light-contrast" => s.studio_contrast = value.get().unwrap(),
                "studio-light-bg-brightness" => s.studio_bg_brightness = value.get().unwrap(),
                "reactions-enabled" => s.reactions_enabled = value.get().unwrap(),
                "trigger-reaction" => {
                    let id = value.get::<Option<String>>().unwrap().unwrap_or_default();
                    fx.trigger_reaction(&id);
                }
                "inference-provider" => {
                    let p = value
                        .get::<Option<String>>()
                        .unwrap()
                        .unwrap_or_else(|| "auto".into());
                    drop(state);
                    if let Err(err) = self.swap_provider(&p) {
                        tracing::error!(%err, "failed to swap inference provider");
                    }
                    return;
                }
                _ => unimplemented!(),
            }
            let active = state.effects.settings.active();
            // A burst must keep the filter live even with every effect off, so a
            // trigger always disables passthrough; it is restored lazily in
            // transform once the burst drains.
            let triggered = pspec.name() == "trigger-reaction";
            drop(state);
            self.obj().set_passthrough(!active && !triggered);
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let state = self.state.lock().unwrap();
            let s = &state.effects.settings;
            let engine = &state.effects.engine;
            match pspec.name() {
                "portrait-blur-enabled" => s.blur_enabled.to_value(),
                "portrait-blur-strength" => s.blur_strength.to_value(),
                "bg-replace-enabled" => s.bg_enabled.to_value(),
                "bg-replace-path" => s.bg_path.to_value(),
                "center-stage-enabled" => s.cs_enabled.to_value(),
                "center-stage-zoom" => s.cs_zoom.to_value(),
                "center-stage-mode" => s.cs_mode.to_value(),
                "studio-light-enabled" => s.studio_enabled.to_value(),
                "auto-light-enabled" => s.auto_light_enabled.to_value(),
                "auto-light-strength" => s.auto_light_strength.to_value(),
                "studio-light-brightness" => s.studio_brightness.to_value(),
                "studio-light-contrast" => s.studio_contrast.to_value(),
                "studio-light-bg-brightness" => s.studio_bg_brightness.to_value(),
                "reactions-enabled" => s.reactions_enabled.to_value(),
                "trigger-reaction" => "".to_value(), // write-only; defensive
                "inference-provider" => engine.provider.to_value(),
                "inference-generation" => engine.generation.to_value(),
                "inference-active" => engine.active.to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let active = self.state.lock().unwrap().effects.settings.active();
            self.obj().set_passthrough(!active);
            spawn_model_reaper(self.obj().downgrade());
        }
    }

    impl GstObjectImpl for OeEffects {}

    impl ElementImpl for OeEffects {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
                gst::subclass::ElementMetadata::new(
                    "vinoOpenEffects ML Effects",
                    "Filter/Effect/Video",
                    "Portrait blur, background replace, and center stage via ONNX inference",
                    "Aryan <hello@funinkina.co.in>",
                )
            });
            Some(&*META)
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
                let caps = gst_video::VideoCapsBuilder::new()
                    .format(gst_video::VideoFormat::Rgba)
                    .build();
                let src = gst::PadTemplate::new(
                    "src",
                    gst::PadDirection::Src,
                    gst::PadPresence::Always,
                    &caps,
                )
                .unwrap();
                let sink = gst::PadTemplate::new(
                    "sink",
                    gst::PadDirection::Sink,
                    gst::PadPresence::Always,
                    &caps,
                )
                .unwrap();
                vec![src, sink]
            });
            TEMPLATES.as_ref()
        }
    }

    impl BaseTransformImpl for OeEffects {
        const MODE: BaseTransformMode = BaseTransformMode::AlwaysInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;
    }

    impl VideoFilterImpl for OeEffects {
        fn set_info(
            &self,
            incaps: &gst::Caps,
            in_info: &gst_video::VideoInfo,
            outcaps: &gst::Caps,
            out_info: &gst_video::VideoInfo,
        ) -> Result<(), gst::LoggableError> {
            self.state.lock().unwrap().effects.reset_for_new_size();
            self.parent_set_info(incaps, in_info, outcaps, out_info)
        }

        fn transform_frame_ip(
            &self,
            frame: &mut gst_video::VideoFrameRef<&mut gst::BufferRef>,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let w = frame.width() as usize;
            let h = frame.height() as usize;
            if w == 0 || h == 0 {
                return Ok(gst::FlowSuccess::Ok);
            }
            let stride = frame.plane_stride()[0] as usize;
            let plane = frame.plane_data_mut(0).map_err(|_| gst::FlowError::Error)?;

            let mut state = self.state.lock().unwrap();
            if !state.effects.busy() {
                // No effect is on and no manual burst is in flight: restore
                // passthrough so we stop being handed frames. (Fallback if this
                // ever misbehaves: drop the set_passthrough call and just return
                // — that costs one buffer map per idle frame.)
                drop(state);
                self.obj().set_passthrough(true);
                return Ok(gst::FlowSuccess::Ok);
            }

            // Pack the (possibly strided) plane into a tight RGBA buffer,
            // reusing the canvas allocation across frames.
            let mut img = std::mem::take(&mut state.canvas);
            pack_into(plane, &mut img, w, h, stride);
            state.effects.process(&mut img, w, h);
            unpack(&img, plane, w, h, stride);
            state.canvas = img;
            Ok(gst::FlowSuccess::Ok)
        }
    }

    /// Pack a strided RGBA plane into the reusable tight `w*h*4` buffer.
    fn pack_into(plane: &[u8], out: &mut Vec<u8>, w: usize, h: usize, stride: usize) {
        let row = w * 4;
        out.resize(row * h, 0);
        for y in 0..h {
            out[y * row..y * row + row].copy_from_slice(&plane[y * stride..y * stride + row]);
        }
    }

    /// Write a tight RGBA buffer back into the strided plane.
    fn unpack(img: &[u8], plane: &mut [u8], w: usize, h: usize, stride: usize) {
        let row = w * 4;
        for y in 0..h {
            plane[y * stride..y * stride + row].copy_from_slice(&img[y * row..y * row + row]);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn element_swap_provider_bumps_generation_and_updates_active() {
            let element = OeEffects::default();
            assert_eq!(element.generation(), 0);

            let (active, _) = element.swap_provider("cpu").expect("swap to cpu");
            assert_eq!(element.generation(), 1);
            assert_eq!(active, "cpu");

            let (_active2, _) = element.swap_provider("auto").expect("swap to auto");
            assert_eq!(element.generation(), 2);
        }

        #[test]
        fn npu_request_without_npu_reports_cpu_and_why() {
            if oe_core::inference::probe_npu_cached().available {
                eprintln!("skipping: an NPU is present");
                return;
            }
            let element = OeEffects::default();
            let (active, detail) = element.swap_provider(" NPU ").expect("swap to npu");
            assert_eq!(active, "cpu");
            assert!(detail.is_some(), "the CPU fallback must say why");
            assert_eq!(element.state.lock().unwrap().effects.engine.provider, "npu");

            // Switching away clears the stale fallback reason.
            let (active, detail) = element.swap_provider("cpu").expect("swap to cpu");
            assert_eq!((active.as_str(), detail), ("cpu", None));
        }
    }
}
