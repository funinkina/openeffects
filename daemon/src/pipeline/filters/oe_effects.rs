//! `oe_effects` — the single CPU video filter that hosts every ML-driven
//! effect. It receives RGBA frames in system memory and applies:
//!
//! 1. **Studio Light** — a `videobalance`-style brightness/contrast lift
//!    blended by the person mask, so only the subject is lit (the background
//!    keeps its original exposure).
//! 2. **Portrait blur** / **Background replace** — `out = fg·mask + bg·(1-mask)`
//!    where `bg` is a user image/color, or — for blur — a box blur of the
//!    frame *weighted by the background mask* so the subject's own pixels are
//!    excluded from it (otherwise the person's colors bleed outward under the
//!    blur and leave a halo ring around the composited subject).
//! 3. **Center Stage** — a YuNet face box drives an EMA-smoothed, deadzoned
//!    crop+zoom that keeps the face centered without distorting the aspect ratio.
//!
//! Studio Light, Portrait blur and Background replace all need the selfie
//! segmentation mask; it is segmented **once per frame** and shared by all
//! three, so enabling several together costs no extra ONNX work.
//!
//! Two paths, chosen per frame:
//!
//! - **Center stage on**: framing is computed on the full frame, then the crop
//!   ROI is extracted and the masked effects run on *just that ROI* (the only
//!   pixels the zoom keeps) before it's upscaled to the output. Compositing the
//!   discarded border would be wasted work, so center stage is the source of
//!   truth for the mask canvas.
//! - **Center stage off**: the masked effects run over the full frame.
//!
//! Energy: selfie segmentation (the per-frame ONNX cost) runs on a cadence
//! (`SEG_INTERVAL`) with the mask reused in between, and YuNet detection on a
//! coarser one (`FACE_INTERVAL`) — the reframe deadzone absorbs the staleness.
//!
//! 4. **Reactions** — reverse-rain emoji bursts (see [`reactions`]), composited
//!    last over the final canvas. Triggered manually (the `trigger-reaction`
//!    property) or, when enabled, automatically by the hand-gesture pipeline.
//!
//! [`reactions`]: oe_core::effects::reactions

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
    use std::time::{Duration, Instant};

    use gst::glib;
    use gst_base::subclass::BaseTransformMode;
    use gstreamer as gst;
    use gstreamer_base as gst_base;
    use gstreamer_video as gst_video;
    use gstreamer_video::prelude::*;
    use gstreamer_video::subclass::prelude::*;
    use tracing::warn;

    use oe_core::effects::reactions::Reactions;
    use oe_core::gesture::{self, Gesture};
    use oe_core::inference::hand::{CropBox, HandLandmarker, HandLandmarks, PalmDetector};
    use oe_core::inference::models::{AutoLight, Face, Mask, SelfieSeg, YuNet};

    const DEFAULT_STRENGTH: u32 = 50;
    const DEFAULT_ZOOM: &str = "normal";
    const DEFAULT_MODE: &str = "single";
    /// Minimum YuNet score to accept a face for framing.
    const FACE_SCORE_THRESH: f32 = 0.6;
    /// EMA smoothing factor for the center-stage crop (≈12 frames to 90%).
    const CS_ALPHA: f32 = 0.18;
    /// Tightest allowed crop (fraction of each axis); caps zoom at 4× so a
    /// small/distant face doesn't blow up into an unusably pixelated frame.
    const CS_MIN_CROP: f32 = 0.25;
    /// Run selfie segmentation every Nth frame; the mask is reused in between.
    /// 2 (≈15 Hz at 30 fps) keeps the ONNX call — the per-frame energy cost —
    /// off every other frame. The 1-frame hold is hidden by the temporal EMA
    /// (model space) and the guided-filter edge re-snap (which re-aligns the
    /// mask boundary to the *current* frame's luma every frame, see
    /// `refine_mask_lut`).
    const SEG_INTERVAL: u64 = 2;
    /// Temporal EMA factor for the raw 256² model output: `e = α·new + (1-α)·e`.
    /// Runs once per fresh segmentation (~15 Hz), killing boundary flicker
    /// without smearing the image-aligned edge (which lives downstream). Lower
    /// = steadier but laggier.
    const SEG_EMA_ALPHA: f32 = 0.5;
    /// Guided-filter subsample factor: coefficients are computed at 1/s² the
    /// pixels, then applied against full-res luma (which carries the sharp edge).
    const GF_SUBSAMPLE: usize = 4;
    /// Guided-filter regularization. Lower = snappier to image edges (and
    /// noisier). Kept small so the matte hugs real luma boundaries.
    const GF_EPS: f32 = 1e-4;
    /// Half-width of the final alpha transition band, in mask units (0..1).
    /// After the guided filter localizes the edge onto the real outline, the
    /// alpha is steepened with a smoothstep across `0.5 ± GF_EDGE_WIDTH`, so the
    /// subject/background boundary is a thin (~1px) anti-aliased line instead of
    /// a soft halo. Smaller = sharper/harder; larger = softer.
    const GF_EDGE_WIDTH: f32 = 0.06;
    /// Run YuNet face detection every Nth frame. The comfort zone below absorbs
    /// detection jitter, so framing tolerates a slower (cheaper) detect cadence.
    const FACE_INTERVAL: u64 = 4;
    /// Comfort zone (macOS-Center-Stage-style lock): while the subject's ideal
    /// crop center stays within this fraction of the *committed crop's span*
    /// from the committed center, the framing is locked — rotating, tilting,
    /// grinning, leaning, small back-and-forth never move the crop.
    const CS_COMFORT_POS: f32 = 0.20;
    /// Zoom side of the comfort zone: locked while `ideal zf / committed zf`
    /// stays within this ratio band (expression/pose noise lands well inside).
    const CS_COMFORT_ZOOM_LO: f32 = 0.78;
    const CS_COMFORT_ZOOM_HI: f32 = 1.30;
    /// A breach this large (fraction of committed span / zoom ratio) is
    /// "drastic": the subject stood up, walked across, or the face count
    /// changed (which jolts the union box). Confirmed on the fast path.
    const CS_BREACH_POS: f32 = 0.45;
    const CS_BREACH_ZOOM_LO: f32 = 0.55;
    const CS_BREACH_ZOOM_HI: f32 = 1.85;
    /// Consecutive out-of-comfort frames required before reframing: ≈1 s for a
    /// moderate drift (lean that stays), ≈0.3 s for a drastic one. A move that
    /// returns to the comfort zone before confirming never reframes at all.
    const CS_CONFIRM: u32 = 30;
    const CS_FAST_CONFIRM: u32 = 9;

    /// Studio Light contrast-pivot smoothing (≈10 frames to 65%) and the pixel
    /// stride the subject's mean luma is sampled at.
    const STUDIO_PIVOT_ALPHA: f32 = 0.1;
    const STUDIO_PIVOT_STRIDE: usize = 7;
    /// Auto light: run the enhancer every Nth frame (≈7.5 Hz at 30 fps) and
    /// smooth its gain map and on/off gate in between.
    const AUTO_LIGHT_INTERVAL: u64 = 4;
    const AUTO_GAIN_ALPHA: f32 = 0.4;
    const AUTO_GATE_ALPHA: f32 = 0.15;
    /// Mean frame luma at/below which auto light is fully on, and at/above
    /// which it is off, so a well-lit frame passes through untouched.
    const AUTO_GATE_DARK: f32 = 0.25;
    const AUTO_GATE_BRIGHT: f32 = 0.55;
    /// Run the gesture pipeline every Nth frame (offset from the seg/face slots).
    const GESTURE_INTERVAL: u64 = 3;
    /// Consecutive gesture checks the same gesture must hold before triggering.
    const GESTURE_CONFIRM: u32 = 3;
    /// Frames to suppress new gesture triggers after one fires (~3 s at 30 fps).
    const REACTION_COOLDOWN: u64 = 90;
    /// Frames between palm re-detections while fewer than two hands are tracked.
    const REDETECT_INTERVAL: u64 = 30;
    /// Minimum palm score / hand-landmark confidence to accept.
    const PALM_SCORE_THRESH: f32 = 0.6;
    const HAND_CONF_THRESH: f32 = 0.5;
    /// Most hands tracked at once (two needed for the heart gesture).
    const MAX_TRACKS: usize = 2;

    #[derive(Debug, Clone)]
    struct Settings {
        blur_enabled: bool,
        blur_strength: u32,
        bg_enabled: bool,
        bg_path: String,
        cs_enabled: bool,
        cs_zoom: String,
        cs_mode: String,
        studio_enabled: bool,
        /// Additive brightness, -1.0..1.0 (videobalance semantics).
        studio_brightness: f32,
        /// Contrast multiplier around mid-gray, 0.0..2.0.
        studio_contrast: f32,
        /// Background additive brightness, -1.0..1.0. Applied to the inverted
        /// mask so only the background (not the subject) is affected.
        studio_bg_brightness: f32,
        /// Reactions effect: when on, the gesture pipeline runs and may
        /// auto-trigger bursts (manual bursts work regardless, via a one-shot
        /// property that doesn't touch this flag).
        reactions_enabled: bool,
        /// Auto light (NPU-only low-light model), and how much of its
        /// correction to apply, 0.0..1.0.
        auto_light_enabled: bool,
        auto_light_strength: f32,
    }

    impl Default for Settings {
        fn default() -> Self {
            Self {
                blur_enabled: false,
                blur_strength: DEFAULT_STRENGTH,
                bg_enabled: false,
                bg_path: String::new(),
                cs_enabled: false,
                cs_zoom: DEFAULT_ZOOM.into(),
                cs_mode: DEFAULT_MODE.into(),
                studio_enabled: false,
                studio_brightness: 0.0,
                studio_contrast: 1.0,
                studio_bg_brightness: 0.0,
                reactions_enabled: false,
                auto_light_enabled: false,
                auto_light_strength: 0.8,
            }
        }
    }

    impl Settings {
        /// Whether any effect in this element does real work (otherwise the
        /// element runs in passthrough).
        fn active(&self) -> bool {
            self.blur_enabled
                || self.bg_enabled
                || self.studio_enabled
                || (self.cs_enabled && self.cs_zoom != "off")
                || self.reactions_enabled
                || self.auto_light_enabled
        }

        /// Whether the blur/bg-replace composite step should run.
        fn needs_composite(&self) -> bool {
            self.blur_enabled || self.bg_enabled
        }

        /// Whether any effect needs the selfie segmentation mask (studio light,
        /// portrait blur, bg replace). Drives the single shared segmentation.
        fn needs_mask(&self) -> bool {
            self.studio_enabled || self.blur_enabled || self.bg_enabled
        }
    }

    /// EMA-smoothed crop window in normalized frame coordinates. `zf` is the
    /// fraction of each axis the crop spans (1.0 = full frame, smaller = zoomed
    /// in); the crop is square in normalized space so the pixel rect keeps the
    /// frame's aspect ratio.
    #[derive(Clone, Copy)]
    struct CropState {
        cx: f32,
        cy: f32,
        zf: f32,
    }

    impl Default for CropState {
        fn default() -> Self {
            Self {
                cx: 0.5,
                cy: 0.5,
                zf: 1.0,
            }
        }
    }

    /// A background image decoded and rescaled to the current frame size.
    struct BgImage {
        path: String,
        w: usize,
        h: usize,
        rgba: Vec<u8>,
    }

    /// A lazily-loaded model, with a one-shot failure memo so a missing model
    /// is logged once rather than retried every frame, and a last-use stamp so
    /// an idle model can be dropped and reloaded on demand.
    struct Slot<T> {
        model: Option<T>,
        active_ep: Option<&'static str>,
        failed: bool,
        used: Instant,
    }

    impl<T> Default for Slot<T> {
        fn default() -> Self {
            Self {
                model: None,
                active_ep: None,
                failed: false,
                used: Instant::now(),
            }
        }
    }

    impl<T> Slot<T> {
        fn ensure_loaded(
            &mut self,
            provider: &str,
            load: impl FnOnce(&str) -> anyhow::Result<(T, &'static str, Option<String>)>,
            unavailable: &str,
            fallback_out: &mut Option<String>,
        ) {
            if self.model.is_none() && !self.failed {
                match load(provider) {
                    Ok((m, ep, fallback)) => {
                        self.model = Some(m);
                        self.active_ep = Some(ep);
                        if let Some(fb) = fallback {
                            *fallback_out = Some(fb);
                        }
                    }
                    Err(err) => {
                        warn!(%err, "{unavailable}");
                        self.failed = true;
                    }
                }
            }
        }

        fn as_mut(&mut self) -> Option<&mut T> {
            self.used = Instant::now();
            self.model.as_mut()
        }

        #[cfg(test)]
        fn get_simple(
            &mut self,
            load: impl FnOnce() -> anyhow::Result<T>,
            unavailable: &str,
        ) -> Option<&mut T> {
            let mut fb = None;
            self.ensure_loaded(
                "cpu",
                |_| load().map(|m| (m, "cpu", None)),
                unavailable,
                &mut fb,
            );
            self.as_mut()
        }

        fn is_loaded(&self) -> bool {
            self.model.is_some()
        }

        /// Swap in a model loaded for a new provider, or drop the current one so
        /// it reloads lazily on the new provider if it is still needed.
        fn install(&mut self, loaded: Option<(T, &'static str)>) {
            (self.model, self.active_ep) = match loaded {
                Some((model, ep)) => (Some(model), Some(ep)),
                None => (None, None),
            };
            self.failed = false;
            self.used = Instant::now();
        }

        fn reset_failed(&mut self) {
            self.failed = false;
        }

        fn unload_idle(&mut self, max_idle: Duration) -> bool {
            if self.model.is_some() && self.used.elapsed() >= max_idle {
                self.model = None;
                self.active_ep = None;
                return true;
            }
            false
        }
    }

    pub struct Engine {
        pub generation: u64,
        pub provider: String,
        pub active: &'static str,
        pub detail: Option<String>,
        selfie: Slot<SelfieSeg>,
        yunet: Slot<YuNet>,
        palm: Slot<PalmDetector>,
        hand: Slot<HandLandmarker>,
        auto_light: Slot<AutoLight>,
    }

    impl Default for Engine {
        fn default() -> Self {
            let requested = oe_core::inference::requested_ep();
            let mut eng = Self {
                generation: 0,
                provider: requested,
                active: "cpu",
                detail: None,
                selfie: Slot::default(),
                yunet: Slot::default(),
                palm: Slot::default(),
                hand: Slot::default(),
                auto_light: Slot::default(),
            };
            eng.update_active_status();
            eng
        }
    }

    impl Engine {
        pub fn update_active_status(&mut self) {
            let eps: Vec<&'static str> = [
                self.selfie.active_ep,
                self.yunet.active_ep,
                self.palm.active_ep,
                self.hand.active_ep,
                self.auto_light.active_ep,
            ]
            .into_iter()
            .flatten()
            .collect();
            self.active = match eps.split_first() {
                None => {
                    let (active, detail) = oe_core::inference::predicted_active(&self.provider);
                    self.detail = detail;
                    active
                }
                Some((first, rest)) if rest.iter().all(|ep| ep == first) => first,
                Some(_) => "mixed",
            };
        }

        pub fn reset_all_failed(&mut self) {
            self.selfie.reset_failed();
            self.yunet.reset_failed();
            self.palm.reset_failed();
            self.hand.reset_failed();
            self.auto_light.reset_failed();
            oe_core::inference::reset_accel_failed();
        }

        fn selfie(&mut self) -> Option<&mut SelfieSeg> {
            if self.selfie.model.is_none() && !self.selfie.failed {
                let mut fb = None;
                self.selfie.ensure_loaded(
                    &self.provider,
                    SelfieSeg::load_with_provider,
                    "selfie segmentation unavailable; portrait blur / bg replace disabled",
                    &mut fb,
                );
                if let Some(f) = fb {
                    self.detail = Some(f);
                }
                self.update_active_status();
            }
            self.selfie.as_mut()
        }

        fn yunet(&mut self) -> Option<&mut YuNet> {
            if self.yunet.model.is_none() && !self.yunet.failed {
                let mut fb = None;
                self.yunet.ensure_loaded(
                    &self.provider,
                    YuNet::load_with_provider,
                    "yunet unavailable; center stage cannot track a subject",
                    &mut fb,
                );
                if let Some(f) = fb {
                    self.detail = Some(f);
                }
                self.update_active_status();
            }
            self.yunet.as_mut()
        }

        fn palm(&mut self) -> Option<&mut PalmDetector> {
            if self.palm.model.is_none() && !self.palm.failed {
                let mut fb = None;
                self.palm.ensure_loaded(
                    &self.provider,
                    PalmDetector::load_with_provider,
                    "palm detection unavailable; gesture reactions disabled",
                    &mut fb,
                );
                if let Some(f) = fb {
                    self.detail = Some(f);
                }
                self.update_active_status();
            }
            self.palm.as_mut()
        }

        fn hand(&mut self) -> Option<&mut HandLandmarker> {
            if self.hand.model.is_none() && !self.hand.failed {
                let mut fb = None;
                self.hand.ensure_loaded(
                    &self.provider,
                    HandLandmarker::load_with_provider,
                    "hand landmark model unavailable; gesture reactions disabled",
                    &mut fb,
                );
                if let Some(f) = fb {
                    self.detail = Some(f);
                }
                self.update_active_status();
            }
            self.hand.as_mut()
        }

        fn auto_light(&mut self) -> Option<&mut AutoLight> {
            if self.auto_light.model.is_none() && !self.auto_light.failed {
                let mut fb = None;
                self.auto_light.ensure_loaded(
                    &self.provider,
                    AutoLight::load_with_provider,
                    "auto light unavailable (needs an NPU and the iat_lowlight model)",
                    &mut fb,
                );
                if let Some(f) = fb {
                    self.detail = Some(f);
                }
                self.update_active_status();
            }
            self.auto_light.as_mut()
        }

        /// Drop every model unused for `max_idle`; true if any was freed.
        fn unload_idle(&mut self, max_idle: Duration) -> bool {
            let selfie = self.selfie.unload_idle(max_idle);
            let yunet = self.yunet.unload_idle(max_idle);
            let palm = self.palm.unload_idle(max_idle);
            let hand = self.hand.unload_idle(max_idle);
            let auto = self.auto_light.unload_idle(max_idle);
            let freed = selfie || yunet || palm || hand || auto;
            if freed {
                self.update_active_status();
            }
            freed
        }
    }

    /// A tracked hand: the crop to sample next frame, and how many consecutive
    /// frames the landmark model returned low confidence (dropped after 2).
    struct HandTrack {
        crop: CropBox,
        missed: u32,
    }

    #[derive(Default)]
    struct State {
        settings: Settings,
        engine: Engine,
        /// EMA-smoothed crop actually applied this frame.
        crop: CropState,
        /// Committed framing target the crop eases toward; locked until the
        /// subject sustains a position outside the comfort zone (see
        /// `update_target`).
        target: CropState,
        /// Consecutive frames the subject's ideal crop has been outside the
        /// committed target's comfort zone.
        cs_out_frames: u32,
        frame: u64,
        mask: Option<Mask>,
        /// Temporal-EMA-smoothed copy of the raw 256² model output, carried
        /// across segmentations to suppress boundary flicker. `None` until the
        /// first segmentation (or after a resolution / coordinate-space change).
        mask_ema: Option<Vec<f32>>,
        /// Whether the cached `mask` was segmented from the center-stage ROI
        /// (true) or the full frame (false); a mismatch forces a re-segment so a
        /// normalized mask sampled in the wrong coordinate space is never reused.
        mask_is_roi: bool,
        /// Bumped on every fresh segmentation; keys the rasterized LUT below.
        mask_gen: u64,
        /// Per-pixel mask weights (0–255) rasterized at canvas resolution.
        /// Rebuilt only when a fresh mask arrives or the canvas size changes,
        /// so the per-frame effects read one byte per pixel instead of
        /// bilinearly sampling the 256² mask once per pass.
        mask_lut: Vec<u8>,
        lut_gen: u64,
        lut_w: usize,
        lut_h: usize,
        /// Fast-guided-filter linear coefficients at low-res (`gf_low_w`×
        /// `gf_low_h`). Recomputed only on a fresh mask; the full-res apply that
        /// reads them (`q = a·I + b` against the live frame luma) runs every
        /// frame so the edge tracks motion even while the coarse mask is held.
        gf_a_low: Vec<f32>,
        gf_b_low: Vec<f32>,
        /// Reusable low-res scratch planes for the guided-filter box passes.
        gf_p_low: Vec<f32>,
        gf_i_low: Vec<f32>,
        gf_tmp: Vec<f32>,
        gf_prefix: Vec<f32>,
        gf_low_w: usize,
        gf_low_h: usize,
        /// `mask_gen` the low-res coefficients were computed for.
        gf_coef_gen: u64,
        /// Reusable packed-frame buffer (a multi-MB alloc otherwise repeated
        /// every frame).
        canvas: Vec<u8>,
        /// Scratch for the downscaled blur (image + ping-pong buffer).
        blur_low: Vec<u8>,
        blur_tmp: Vec<u8>,
        /// Reusable prefix-sum scratch shared by the separable box blurs (frame
        /// blur + studio mask blur), hoisted out of the passes so each blur does
        /// no per-frame heap work. Sized to `max(w,h)+1` on demand.
        blur_prefix: Vec<i32>,
        faces: Vec<Face>,
        bg: Option<BgImage>,
        /// Reverse-rain emoji particle system (manual + gesture triggers).
        reactions: Reactions,
        /// Hands currently tracked by the gesture pipeline (≤ `MAX_TRACKS`).
        tracks: Vec<HandTrack>,
        /// Earliest frame palm re-detection may run again.
        redetect_at: u64,
        /// Auto light's smoothed low-res gain map and on/off gate (0..1).
        auto_gain: Option<Mask>,
        auto_gate: f32,
        /// EMA of the subject's mean luma (0..1): the pivot Studio Light's
        /// contrast scales around, so contrast never changes how bright the
        /// face is overall. `None` until the first masked frame.
        studio_pivot: Option<f32>,
        /// Hands tracked for the segmentation mask, in the coordinate space of
        /// the buffer it is segmented from (full frame or center-stage ROI), so
        /// kept apart from the gesture `tracks`.
        mask_tracks: Vec<HandTrack>,
        mask_redetect_at: u64,
        /// Gesture held across the last `held_count` checks (debounce).
        held: Option<Gesture>,
        held_count: u32,
        /// No new gesture trigger fires before this frame.
        cooldown_until: u64,
        width: usize,
        height: usize,
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
            self.state.lock().unwrap().engine.generation
        }

        /// Reload every loaded model on `target_provider` without holding the
        /// state lock, so frames keep flowing on the old sessions while the new
        /// ones compile, then swap them in at once. On error the models stay put.
        pub fn swap_provider(
            &self,
            target_provider: &str,
        ) -> anyhow::Result<(String, Option<String>)> {
            let target = target_provider.trim().to_ascii_lowercase();
            let (want_selfie, want_yunet, want_palm, want_hand, want_auto) = {
                let state = self.state.lock().unwrap();
                (
                    state.engine.selfie.is_loaded(),
                    state.engine.yunet.is_loaded(),
                    state.engine.palm.is_loaded(),
                    state.engine.hand.is_loaded(),
                    state.engine.auto_light.is_loaded(),
                )
            };

            // Before loading: a stale "accelerator failed" memo from the old
            // provider would otherwise pin `auto` to CPU.
            oe_core::inference::reset_accel_failed();
            let mut detail = None;
            let selfie = reload(
                want_selfie,
                SelfieSeg::load_with_provider,
                &target,
                &mut detail,
            )?;
            let yunet = reload(want_yunet, YuNet::load_with_provider, &target, &mut detail)?;
            let palm = reload(
                want_palm,
                PalmDetector::load_with_provider,
                &target,
                &mut detail,
            )?;
            let hand = reload(
                want_hand,
                HandLandmarker::load_with_provider,
                &target,
                &mut detail,
            )?;
            // Auto light only runs on the NPU, so a provider it can't use turns it
            // off (the lazy reload logs why) instead of failing the whole swap.
            let auto_light = reload(
                want_auto,
                AutoLight::load_with_provider,
                &target,
                &mut detail,
            )
            .unwrap_or_default();

            let mut state = self.state.lock().unwrap();
            let engine = &mut state.engine;
            engine.generation += 1;
            engine.provider = target;
            engine.detail = detail;
            engine.reset_all_failed();
            // A slot the streaming thread loaded since the snapshot is on the old
            // provider; installing `None` drops it so it reloads on the new one.
            engine.selfie.install(selfie);
            engine.yunet.install(yunet);
            engine.palm.install(palm);
            engine.hand.install(hand);
            engine.auto_light.install(auto_light);
            engine.update_active_status();
            Ok((engine.active.to_string(), engine.detail.clone()))
        }
    }

    /// Load a model on `provider` if `wanted`, recording any CPU fallback reason.
    fn reload<T>(
        wanted: bool,
        load: impl FnOnce(&str) -> anyhow::Result<(T, &'static str, Option<String>)>,
        provider: &str,
        detail: &mut Option<String>,
    ) -> anyhow::Result<Option<(T, &'static str)>> {
        if !wanted {
            return Ok(None);
        }
        let (model, ep, fallback) = load(provider)?;
        if fallback.is_some() {
            *detail = fallback;
        }
        Ok(Some((model, ep)))
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
            match pspec.name() {
                "portrait-blur-enabled" => state.settings.blur_enabled = value.get().unwrap(),
                "portrait-blur-strength" => state.settings.blur_strength = value.get().unwrap(),
                "bg-replace-enabled" => state.settings.bg_enabled = value.get().unwrap(),
                "bg-replace-path" => {
                    state.settings.bg_path =
                        value.get::<Option<String>>().unwrap().unwrap_or_default();
                    state.bg = None; // force reload on next frame
                }
                "center-stage-enabled" => state.settings.cs_enabled = value.get().unwrap(),
                "center-stage-zoom" => {
                    state.settings.cs_zoom = value
                        .get::<Option<String>>()
                        .unwrap()
                        .unwrap_or_else(|| DEFAULT_ZOOM.into());
                }
                "center-stage-mode" => {
                    state.settings.cs_mode = value
                        .get::<Option<String>>()
                        .unwrap()
                        .unwrap_or_else(|| DEFAULT_MODE.into());
                }
                "studio-light-enabled" => state.settings.studio_enabled = value.get().unwrap(),
                "auto-light-enabled" => state.settings.auto_light_enabled = value.get().unwrap(),
                "auto-light-strength" => state.settings.auto_light_strength = value.get().unwrap(),
                "studio-light-brightness" => {
                    state.settings.studio_brightness = value.get().unwrap()
                }
                "studio-light-contrast" => state.settings.studio_contrast = value.get().unwrap(),
                "studio-light-bg-brightness" => {
                    state.settings.studio_bg_brightness = value.get().unwrap()
                }
                "reactions-enabled" => state.settings.reactions_enabled = value.get().unwrap(),
                "trigger-reaction" => {
                    let id = value.get::<Option<String>>().unwrap().unwrap_or_default();
                    let frame = state.frame;
                    state.reactions.trigger(&id, frame);
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
            let active = state.settings.active();
            // A burst must keep the filter live even with every effect off, so a
            // trigger always disables passthrough; it is restored lazily in
            // transform once the burst drains.
            let triggered = pspec.name() == "trigger-reaction";
            drop(state);
            self.obj().set_passthrough(!active && !triggered);
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let state = self.state.lock().unwrap();
            let s = &state.settings;
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
                "inference-provider" => state.engine.provider.to_value(),
                "inference-generation" => state.engine.generation.to_value(),
                "inference-active" => state.engine.active.to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let active = self.state.lock().unwrap().settings.active();
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
            let mut state = self.state.lock().unwrap();
            state.width = in_info.width() as usize;
            state.height = in_info.height() as usize;
            // Resolution change invalidates the smoothed crop, cached mask, and
            // pre-scaled background.
            state.crop = CropState::default();
            state.target = CropState::default();
            state.cs_out_frames = 0;
            state.mask = None;
            state.mask_ema = None;
            state.mask_lut.clear();
            state.lut_w = 0;
            state.lut_h = 0;
            state.gf_low_w = 0;
            state.gf_low_h = 0;
            state.bg = None;
            // Normalized hand crops don't survive a resolution change.
            state.tracks.clear();
            state.redetect_at = 0;
            state.mask_tracks.clear();
            state.mask_redetect_at = 0;
            state.auto_gain = None;
            state.held = None;
            state.held_count = 0;
            drop(state);
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
            if !state.settings.active() && !state.reactions.active() {
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
            process_frame(&mut state, &mut img, w, h);
            unpack(&img, plane, w, h, stride);
            state.canvas = img;
            Ok(gst::FlowSuccess::Ok)
        }
    }

    /// Run every enabled effect over one tight RGBA frame.
    fn process_frame(state: &mut State, img: &mut [u8], w: usize, h: usize) {
        state.frame = state.frame.wrapping_add(1);

        // Gesture detection runs on the original frame, before center stage
        // crops it in place, so hands outside the crop are still seen. It may
        // enqueue a reaction burst, drawn by the overlay step below.
        if state.settings.reactions_enabled {
            run_gestures(state, img, w, h);
        }

        // Auto light brightens the frame the segmenter and effects see, but
        // runs after gestures, which keep the original.
        if state.settings.auto_light_enabled {
            auto_light(state, img, w, h);
        }

        let cs_active = state.settings.cs_enabled && state.settings.cs_zoom != "off";
        if cs_active {
            // Center stage owns the canvas: framing is decided on the full
            // frame, but the masked effects then run only on the cropped ROI
            // — the only pixels that survive the zoom — instead of segmenting
            // and compositing the whole frame and discarding the border.
            center_stage(state, img, w, h);
        } else if state.settings.needs_mask() {
            // No crop: studio light / blur / bg-replace over the full frame.
            masked_effects(state, img, w, h, false);
        }

        // Reactions overlay runs last, on the final canvas, so emojis are
        // never cropped/blurred by the effects above.
        if state.settings.reactions_enabled || state.reactions.active() {
            let frame = state.frame;
            state.reactions.step_and_render(img, w, h, frame);
        }
    }

    /// The reactions gesture pipeline, run on a cadence over the original frame:
    /// palm-detect (only when fewer than two hands are tracked) → per-hand
    /// landmarks (tracked from the previous frame's crop, the cheap path) →
    /// rule-based classify, with hold-to-confirm debounce and a post-trigger
    /// cooldown. A confirmed gesture enqueues the matching emoji burst.
    fn run_gestures(state: &mut State, img: &[u8], w: usize, h: usize) {
        if state.frame % GESTURE_INTERVAL != 2 {
            return;
        }
        let stride = w * 4;

        // 1. Acquire (palm detection when there's room) and track landmarks.
        let acquire = state.frame >= state.redetect_at;
        if state.tracks.len() < MAX_TRACKS && acquire {
            state.redetect_at = state.frame + REDETECT_INTERVAL;
        }
        let State { engine, tracks, .. } = state;
        let landmarks = track_hands(engine, tracks, acquire, img, w, h, stride);

        // 2. Classify with hold-to-confirm debounce, gated by the cooldown.
        match gesture::classify(&landmarks) {
            g if g == state.held => state.held_count = state.held_count.saturating_add(1),
            g => {
                state.held = g;
                state.held_count = if g.is_some() { 1 } else { 0 };
            }
        }
        if let Some(g) = state.held
            && state.held_count >= GESTURE_CONFIRM
            && state.frame >= state.cooldown_until
        {
            let frame = state.frame;
            state.reactions.trigger(g.reaction_id(), frame);
            state.cooldown_until = frame + REACTION_COOLDOWN;
            state.held = None;
            state.held_count = 0;
        }
    }

    /// One tracking step over `tracks`: palm-detect for new hands when `acquire`
    /// and there is room, then run landmarks per tracked hand from its previous
    /// crop (the cheap path). Confident hands advance; a track is dropped after
    /// two consecutive misses. Returns the confident landmarks.
    fn track_hands(
        engine: &mut Engine,
        tracks: &mut Vec<HandTrack>,
        acquire: bool,
        img: &[u8],
        w: usize,
        h: usize,
        stride: usize,
    ) -> Vec<HandLandmarks> {
        if tracks.len() < MAX_TRACKS && acquire {
            // Landmarks only run once palm finds a hand, so on its own the hand
            // model idles out while palm keeps running. Load and touch it with
            // palm so the first gesture never pays a model load on this thread.
            let _ = engine.hand();
            let palms = match engine.palm() {
                Some(model) => model
                    .detect(img, w, h, stride, PALM_SCORE_THRESH)
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            for palm in palms {
                if tracks.len() >= MAX_TRACKS {
                    break;
                }
                let crop = palm.crop(w, h);
                if !tracks.iter().any(|t| crops_overlap(t.crop, crop)) {
                    tracks.push(HandTrack { crop, missed: 0 });
                }
            }
        }

        let mut landmarks = Vec::with_capacity(tracks.len());
        let mut updated = Vec::with_capacity(tracks.len());
        for track in tracks.drain(..) {
            let lm = engine
                .hand()
                .and_then(|model| model.infer(img, w, h, stride, track.crop).ok());
            match lm {
                Some(lm) if lm.confidence >= HAND_CONF_THRESH => {
                    updated.push(HandTrack {
                        crop: lm.tracking_crop(w, h),
                        missed: 0,
                    });
                    landmarks.push(lm);
                }
                _ if track.missed + 1 < 2 => updated.push(HandTrack {
                    crop: track.crop,
                    missed: track.missed + 1,
                }),
                _ => {}
            }
        }
        *tracks = updated;
        landmarks
    }

    /// Merge tracked hands into a fresh segmentation. The selfie model was
    /// trained on torso framing and drops hands held away from the body; the
    /// landmark silhouette is steadier than the mask it corrects, so the union
    /// only fills in. Runs on the same buffer the mask was segmented from, so
    /// the coordinate space (full frame or center-stage ROI) always matches.
    fn paint_hands(
        state: &mut State,
        buf: &[u8],
        w: usize,
        h: usize,
        stride: usize,
        mask: &mut Mask,
    ) {
        let acquire = state.frame >= state.mask_redetect_at;
        if state.mask_tracks.len() < MAX_TRACKS && acquire {
            state.mask_redetect_at = state.frame + REDETECT_INTERVAL;
        }
        let State {
            engine,
            mask_tracks,
            ..
        } = state;
        for lm in track_hands(engine, mask_tracks, acquire, buf, w, h, stride) {
            lm.paint_silhouette(&mut mask.values, mask.size, w, h);
        }
    }

    /// Whether two hand crops are close enough to be the same hand (dedup on
    /// palm re-detection).
    fn crops_overlap(a: CropBox, b: CropBox) -> bool {
        let dx = a.cx - b.cx;
        let dy = a.cy - b.cy;
        (dx * dx + dy * dy).sqrt() < 0.25 * (a.side + b.side)
    }

    /// Run every mask-gated effect over a tight RGBA buffer: segment **once**,
    /// then (in order) studio-light the person, and blur / bg-replace the
    /// background. `is_roi` selects the mask coordinate space (full vs crop ROI).
    fn masked_effects(state: &mut State, img: &mut [u8], w: usize, h: usize, is_roi: bool) {
        ensure_mask(state, img, w, h, w * 4, is_roi);
        refine_mask_lut(state, img, w, h);
        if state.lut_w != w || state.lut_h != h {
            // No usable mask (e.g. the model is missing or the first
            // segmentation failed): skip the masked effects entirely.
            return;
        }

        // Studio light first. The guided-filter mask already has a soft,
        // image-aligned edge, so it's used directly (no separate blur pass).
        if state.settings.studio_enabled {
            let pivot = studio_pivot(state, img);
            apply_studio(
                img,
                &state.mask_lut,
                pivot,
                state.settings.studio_brightness,
                state.settings.studio_contrast,
                state.settings.studio_bg_brightness,
            );
        }
        if !state.settings.needs_composite() {
            return;
        }
        if state.settings.bg_enabled {
            ensure_bg(state, w, h);
            let path = &state.settings.bg_path;
            if let Some(color) = path.strip_prefix('#').and_then(parse_hex_color) {
                composite_color(img, color, &state.mask_lut);
            } else if let Some(bg) = state.bg.as_ref().filter(|b| b.w == w && b.h == h) {
                composite_full(img, &bg.rgba, &state.mask_lut);
            }
        } else {
            // Portrait blur runs on a downscaled copy of the frame, then the
            // composite bilinearly upsamples it on the fly. A blurred
            // background is low-frequency by construction, so the result is
            // visually identical at a fraction of the full-resolution cost
            // (the dominant per-frame load when blur is on).
            //
            // The blur is *background-weighted*: each pixel is premultiplied by
            // its background weight `255-mask` before the box passes, and the
            // weight is carried in alpha and blurred alongside, so the composite
            // can normalize (`rgb·255/alpha`) to a weighted mean that ignores
            // the subject. Blurring the raw frame instead would smear the
            // person's colors into the background and halo the edge (#1).
            let radius = blur_radius(state.settings.blur_strength);
            let scale = blur_scale(radius);
            let lw = w.div_ceil(scale).max(1);
            let lh = h.div_ceil(scale).max(1);
            downsample_masked(
                img,
                &state.mask_lut,
                w,
                h,
                scale,
                &mut state.blur_low,
                lw,
                lh,
            );
            box_blur(
                &mut state.blur_low,
                &mut state.blur_tmp,
                &mut state.blur_prefix,
                lw,
                lh,
                (radius / scale).max(1),
            );
            composite_low(img, &state.blur_low, lw, lh, scale, &state.mask_lut, w, h);
        }
    }

    /// Rasterize the segmentation mask into per-pixel u8 weights at canvas
    /// resolution via a fast guided filterthe soft, image-
    /// aligned linear coefficients `a,b` are computed at 1/`GF_SUBSAMPLE`²
    /// resolution (only on a fresh mask), then `q = a·I + b` is applied at full
    /// resolution against the *current* frame's luma every frame. The full-res
    /// luma carries the sharp edge — it snaps the mask boundary onto the actual
    /// person outline and re-tracks small motion even while the coarse mask is
    /// held between segmentations.
    fn refine_mask_lut(state: &mut State, img: &[u8], w: usize, h: usize) {
        if state.mask.is_none() {
            return;
        }
        let s = GF_SUBSAMPLE;
        let lw = w.div_ceil(s);
        let lh = h.div_ceil(s);
        let n = lw * lh;

        if state.gf_coef_gen != state.mask_gen || state.gf_low_w != lw || state.gf_low_h != lh {
            state.gf_p_low.resize(n, 0.0);
            state.gf_i_low.resize(n, 0.0);
            // Low-res mask (`p`) and luma guide (`I`, box-averaged per s×s block).
            {
                let mask = state.mask.as_ref().unwrap();
                for ly in 0..lh {
                    let v = (ly as f32 + 0.5) / lh as f32;
                    let (y0, y1) = (ly * s, (ly * s + s).min(h));
                    for lx in 0..lw {
                        let u = (lx as f32 + 0.5) / lw as f32;
                        state.gf_p_low[ly * lw + lx] = mask.sample(u, v).clamp(0.0, 1.0);
                        let (x0, x1) = (lx * s, (lx * s + s).min(w));
                        let mut acc = 0u32;
                        for yy in y0..y1 {
                            let row = yy * w;
                            for xx in x0..x1 {
                                let p = (row + xx) * 4;
                                acc += 77 * img[p] as u32
                                    + 150 * img[p + 1] as u32
                                    + 29 * img[p + 2] as u32;
                            }
                        }
                        let cnt = ((y1 - y0) * (x1 - x0)) as f32;
                        state.gf_i_low[ly * lw + lx] = acc as f32 / (cnt * 65280.0);
                    }
                }
            }

            // Tight window: a small radius keeps the linear fit local so the
            // matte stays crisp (a wide box smears the edge into a halo).
            let r = (lw.min(lh) / 96).max(1);
            let mut mean_i = vec![0f32; n];
            let mut mean_p = vec![0f32; n];
            let mut corr_i = vec![0f32; n];
            let mut corr_ip = vec![0f32; n];
            let mut prod = vec![0f32; n];
            box_f32(
                &state.gf_i_low,
                &mut mean_i,
                &mut state.gf_tmp,
                &mut state.gf_prefix,
                lw,
                lh,
                r,
            );
            box_f32(
                &state.gf_p_low,
                &mut mean_p,
                &mut state.gf_tmp,
                &mut state.gf_prefix,
                lw,
                lh,
                r,
            );
            for (p, &a) in prod.iter_mut().zip(&state.gf_i_low).take(n) {
                *p = a * a;
            }
            box_f32(
                &prod,
                &mut corr_i,
                &mut state.gf_tmp,
                &mut state.gf_prefix,
                lw,
                lh,
                r,
            );
            for (p, (&a, &b)) in prod
                .iter_mut()
                .zip(state.gf_i_low.iter().zip(&state.gf_p_low))
                .take(n)
            {
                *p = a * b;
            }
            box_f32(
                &prod,
                &mut corr_ip,
                &mut state.gf_tmp,
                &mut state.gf_prefix,
                lw,
                lh,
                r,
            );

            let mut a = vec![0f32; n];
            let mut b = vec![0f32; n];
            for i in 0..n {
                let var_i = corr_i[i] - mean_i[i] * mean_i[i];
                let cov = corr_ip[i] - mean_i[i] * mean_p[i];
                a[i] = cov / (var_i + GF_EPS);
                b[i] = mean_p[i] - a[i] * mean_i[i];
            }
            box_f32(
                &a,
                &mut state.gf_a_low,
                &mut state.gf_tmp,
                &mut state.gf_prefix,
                lw,
                lh,
                r,
            );
            box_f32(
                &b,
                &mut state.gf_b_low,
                &mut state.gf_tmp,
                &mut state.gf_prefix,
                lw,
                lh,
                r,
            );
            state.gf_low_w = lw;
            state.gf_low_h = lh;
            state.gf_coef_gen = state.mask_gen;
        }

        // Full-res apply against the live frame luma (every frame).
        state.mask_lut.resize(w * h, 0);
        for y in 0..h {
            let fy = ((y as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, lh as f32 - 1.0);
            let y0 = fy.floor() as usize;
            let y1 = (y0 + 1).min(lh - 1);
            let ty = fy - y0 as f32;
            for x in 0..w {
                let fx = ((x as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, lw as f32 - 1.0);
                let x0 = fx.floor() as usize;
                let x1 = (x0 + 1).min(lw - 1);
                let tx = fx - x0 as f32;
                let a = bilerp(&state.gf_a_low, lw, x0, x1, y0, y1, tx, ty);
                let b = bilerp(&state.gf_b_low, lw, x0, x1, y0, y1, tx, ty);
                let p = (y * w + x) * 4;
                let lum = (77 * img[p] as u32 + 150 * img[p + 1] as u32 + 29 * img[p + 2] as u32)
                    as f32
                    / 65280.0;
                let q = a * lum + b;
                // Steepen the localized edge into a thin anti-aliased outline.
                let t = ((q - (0.5 - GF_EDGE_WIDTH)) / (2.0 * GF_EDGE_WIDTH)).clamp(0.0, 1.0);
                let alpha = t * t * (3.0 - 2.0 * t);
                state.mask_lut[y * w + x] = (alpha * 255.0) as u8;
            }
        }
        state.lut_gen = state.mask_gen;
        state.lut_w = w;
        state.lut_h = h;
    }

    /// Single-pass separable box mean over an f32 plane (`src` → `dst`), using
    /// row/column prefix sums (cost independent of radius). `tmp` holds the
    /// horizontal pass; `prefix[0]` stays 0 (never written), which the window
    /// subtraction at `lo == 0` relies on.
    fn box_f32(
        src: &[f32],
        dst: &mut Vec<f32>,
        tmp: &mut Vec<f32>,
        prefix: &mut Vec<f32>,
        w: usize,
        h: usize,
        r: usize,
    ) {
        let n = w * h;
        dst.resize(n, 0.0);
        tmp.resize(n, 0.0);
        prefix.resize(w.max(h) + 1, 0.0);
        for y in 0..h {
            let base = y * w;
            for x in 0..w {
                prefix[x + 1] = prefix[x] + src[base + x];
            }
            for x in 0..w {
                let lo = x.saturating_sub(r);
                let hi = (x + r + 1).min(w);
                tmp[base + x] = (prefix[hi] - prefix[lo]) / (hi - lo) as f32;
            }
        }
        for x in 0..w {
            for y in 0..h {
                prefix[y + 1] = prefix[y] + tmp[y * w + x];
            }
            for y in 0..h {
                let lo = y.saturating_sub(r);
                let hi = (y + r + 1).min(h);
                dst[y * w + x] = (prefix[hi] - prefix[lo]) / (hi - lo) as f32;
            }
        }
    }

    /// Bilinear lookup into an `lw`-stride f32 plane at the four pre-resolved
    /// corner indices with fractional weights `tx`,`ty`.
    #[allow(clippy::too_many_arguments)] // per-pixel hot path; a params struct would only add copies
    fn bilerp(
        buf: &[f32],
        lw: usize,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
        tx: f32,
        ty: f32,
    ) -> f32 {
        let top = buf[y0 * lw + x0] + (buf[y0 * lw + x1] - buf[y0 * lw + x0]) * tx;
        let bot = buf[y1 * lw + x0] + (buf[y1 * lw + x1] - buf[y1 * lw + x0]) * tx;
        top + (bot - top) * ty
    }

    /// Center stage: decide framing on the full frame, then crop to the ROI and
    /// run blur / bg-replace on just that ROI before upscaling it to the output.
    fn center_stage(state: &mut State, img: &mut [u8], w: usize, h: usize) {
        // Refresh face detection on the full frame on the detect cadence.
        if state.frame % FACE_INTERVAL == 1 {
            let faces = if let Some(model) = state.engine.yunet() {
                model
                    .detect(img, w, h, w * 4, FACE_SCORE_THRESH)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            state.faces = faces;
        }

        // Framing is driven by the face's eye landmarks. When no face is found
        // the target is simply held, so brief look-aways don't make the crop
        // drift.
        let subject = subject_box(&state.faces, &state.settings.cs_mode);
        if let Some(raw) = subject.map(|s| crop_target(Some(s), &state.settings.cs_zoom)) {
            update_target(state, raw);
        }

        // Ease the applied crop toward the committed target.
        let t = state.target;
        let crop = &mut state.crop;
        crop.zf += (t.zf - crop.zf) * CS_ALPHA;
        crop.cx += (t.cx - crop.cx) * CS_ALPHA;
        crop.cy += (t.cy - crop.cy) * CS_ALPHA;
        let crop = *crop;

        if !state.settings.needs_mask() {
            // Crop + zoom only: a single full-frame resample.
            resample_crop(img, w, h, crop);
            return;
        }

        // Extract the ROI, run the masked effects on it (smaller area than the
        // full frame), then upscale it back to the output resolution.
        let (mut roi, rw, rh) = extract_roi(img, w, h, crop);
        if rw == w && rh == h {
            // No zoom (full-frame ROI): work in place, no upscale needed.
            masked_effects(state, img, w, h, false);
            return;
        }
        masked_effects(state, &mut roi, rw, rh, true);
        upscale_roi(img, w, h, &roi, rw, rh);
    }

    /// (Re)segment into `state.mask` on the segmentation cadence, or whenever no
    /// valid mask is cached for the current coordinate space (full vs ROI).
    fn ensure_mask(state: &mut State, buf: &[u8], w: usize, h: usize, stride: usize, is_roi: bool) {
        // A coordinate-space flip (full↔ROI) makes the previous mask grid
        // incomparable, so drop the EMA history to avoid blending across spaces.
        let space_changed = state.mask_is_roi != is_roi;
        let stale =
            state.mask.is_none() || space_changed || state.frame.is_multiple_of(SEG_INTERVAL);
        if !stale {
            return;
        }
        if space_changed {
            state.mask_ema = None;
            state.mask_tracks.clear();
            state.mask_redetect_at = 0;
        }
        if let Some(model) = state.engine.selfie() {
            match model.segment(buf, w, h, stride) {
                Ok(mut mask) => {
                    // Before the EMA, so hand edges are smoothed with the body's.
                    paint_hands(state, buf, w, h, stride, &mut mask);
                    let a = SEG_EMA_ALPHA;
                    match &mut state.mask_ema {
                        Some(e) if e.len() == mask.values.len() => {
                            for (e, &n) in e.iter_mut().zip(&mask.values) {
                                *e = a * n + (1.0 - a) * *e;
                            }
                            mask.values.copy_from_slice(e);
                        }
                        _ => state.mask_ema = Some(mask.values.clone()),
                    }
                    state.mask = Some(mask);
                    state.mask_is_roi = is_roi;
                    state.mask_gen = state.mask_gen.wrapping_add(1);
                }
                Err(err) => warn!(%err, "segmentation failed this frame"),
            }
        }
    }

    /// Lock-and-confirm reframing (the macOS Center Stage feel). The committed
    /// target is *locked* while the subject's ideal crop stays inside the
    /// comfort zone around it — natural movement (rotate, tilt, grin, lean,
    /// small back-and-forth) never reframes, no matter how long it lasts. Only
    /// a displacement that *stays* outside the comfort zone for the
    /// confirmation window commits a new target: ≈1 s for a moderate drift,
    /// ≈0.3 s for a drastic one (stood up, walked across, face count changed).
    /// A transient excursion that returns in time resets the counter and never
    /// moves the crop.
    fn update_target(state: &mut State, raw: CropState) {
        let t = state.target;
        // Errors are relative to the committed crop's span: what matters
        // visually is how far the subject wandered within the current framing,
        // so a zoomed-in crop is proportionally more sensitive.
        let span = t.zf.max(CS_MIN_CROP);
        let ex = (raw.cx - t.cx).abs() / span;
        let ey = (raw.cy - t.cy).abs() / span;
        let rz = raw.zf / t.zf;

        let comfortable = ex < CS_COMFORT_POS
            && ey < CS_COMFORT_POS
            && (CS_COMFORT_ZOOM_LO..CS_COMFORT_ZOOM_HI).contains(&rz);
        if comfortable {
            state.cs_out_frames = 0;
            return;
        }

        state.cs_out_frames += 1;
        let drastic = ex > CS_BREACH_POS
            || ey > CS_BREACH_POS
            || !(CS_BREACH_ZOOM_LO..=CS_BREACH_ZOOM_HI).contains(&rz);
        let confirm = if drastic { CS_FAST_CONFIRM } else { CS_CONFIRM };
        if state.cs_out_frames >= confirm {
            state.target = raw;
            state.cs_out_frames = 0;
        }
    }

    /// Extract the crop window into a tight `rw×rh` RGBA buffer (bilinear), so
    /// segmentation/blur/composite run on ROI-sized pixels. Returns the buffer
    /// and its dimensions (`(w, h)` when the crop spans the whole frame).
    fn extract_roi(img: &[u8], w: usize, h: usize, crop: CropState) -> (Vec<u8>, usize, usize) {
        let zf = crop.zf.clamp(CS_MIN_CROP, 1.0);
        if zf >= 0.999 {
            return (img.to_vec(), w, h);
        }
        let span_x = zf * w as f32;
        let span_y = zf * h as f32;
        let x0 = (crop.cx - zf / 2.0) * w as f32;
        let y0 = (crop.cy - zf / 2.0) * h as f32;
        let rw = (span_x.round() as usize).max(1);
        let rh = (span_y.round() as usize).max(1);
        let mut roi = vec![0u8; rw * rh * 4];
        for y in 0..rh {
            let sv = y0 + (y as f32 + 0.5) / rh as f32 * span_y - 0.5;
            for x in 0..rw {
                let su = x0 + (x as f32 + 0.5) / rw as f32 * span_x - 0.5;
                let dp = (y * rw + x) * 4;
                sample_bilinear(img, w, h, su, sv, &mut roi[dp..dp + 4]);
            }
        }
        (roi, rw, rh)
    }

    /// Upscale the composited ROI back into the full-frame output (bilinear).
    fn upscale_roi(img: &mut [u8], w: usize, h: usize, roi: &[u8], rw: usize, rh: usize) {
        for y in 0..h {
            let sv = (y as f32 + 0.5) / h as f32 * rh as f32 - 0.5;
            for x in 0..w {
                let su = (x as f32 + 0.5) / w as f32 * rw as f32 - 0.5;
                let dp = (y * w + x) * 4;
                sample_bilinear(roi, rw, rh, su, sv, &mut img[dp..dp + 4]);
            }
        }
    }

    /// Refresh `state.bg` if the configured image path changed or no image is
    /// cached at the current size. Solid colors (`#RRGGBB`) need no cache.
    fn ensure_bg(state: &mut State, w: usize, h: usize) {
        let path = state.settings.bg_path.clone();
        if path.is_empty() || path.starts_with('#') {
            return;
        }
        let fresh = state
            .bg
            .as_ref()
            .map(|b| b.path == path && b.w == w && b.h == h)
            .unwrap_or(false);
        if fresh {
            return;
        }
        match load_image_rgba(&path, w, h) {
            Ok(rgba) => state.bg = Some(BgImage { path, w, h, rgba }),
            Err(err) => {
                warn!(%err, path, "failed to load background image");
                state.bg = None;
            }
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

    /// How far auto light should act for a frame of this mean luma: 1 when
    /// dark, 0 when already well lit, linear between.
    fn auto_gate_target(mean_luma: f32) -> f32 {
        ((AUTO_GATE_BRIGHT - mean_luma) / (AUTO_GATE_BRIGHT - AUTO_GATE_DARK)).clamp(0.0, 1.0)
    }

    /// Auto light: refresh the enhancer's gain map on its cadence, then apply
    /// the smoothed map to the frame, scaled by the strength setting and the
    /// darkness gate. Silently a no-op without an NPU or the model.
    fn auto_light(state: &mut State, img: &mut [u8], w: usize, h: usize) {
        if state.frame.is_multiple_of(AUTO_LIGHT_INTERVAL) {
            let map = state
                .engine
                .auto_light()
                .and_then(|m| m.analyze(img, w, h, w * 4).ok());
            if let Some(map) = map {
                let target = auto_gate_target(map.mean_luma);
                match &mut state.auto_gain {
                    Some(g) if g.values.len() == map.gain.values.len() => {
                        state.auto_gate += (target - state.auto_gate) * AUTO_GATE_ALPHA;
                        for (e, &n) in g.values.iter_mut().zip(&map.gain.values) {
                            *e += (n - *e) * AUTO_GAIN_ALPHA;
                        }
                    }
                    _ => {
                        state.auto_gate = target;
                        state.auto_gain = Some(map.gain);
                    }
                }
            }
        }
        let amount = state.auto_gate * state.settings.auto_light_strength;
        if let Some(gain) = &state.auto_gain
            && amount >= 0.01
        {
            apply_gain(img, w, h, gain, amount);
        }
    }

    /// Multiply RGB by `1 + (gain - 1)·amount`, with `gain` bilinearly
    /// upsampled from its low-res grid to the frame.
    fn apply_gain(img: &mut [u8], w: usize, h: usize, gain: &Mask, amount: f32) {
        let s = gain.size;
        let top = (s - 1) as f32;
        let cols: Vec<(usize, usize, f32)> = (0..w)
            .map(|x| {
                let fx = ((x as f32 + 0.5) / w as f32 * s as f32 - 0.5).clamp(0.0, top);
                let x0 = fx.floor() as usize;
                (x0, (x0 + 1).min(s - 1), fx - x0 as f32)
            })
            .collect();
        for y in 0..h {
            let fy = ((y as f32 + 0.5) / h as f32 * s as f32 - 0.5).clamp(0.0, top);
            let y0 = fy.floor() as usize;
            let y1 = (y0 + 1).min(s - 1);
            let ty = fy - y0 as f32;
            let (r0, r1) = (&gain.values[y0 * s..], &gain.values[y1 * s..]);
            let row = &mut img[y * w * 4..(y + 1) * w * 4];
            for (px, &(x0, x1, tx)) in row.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
                let a = r0[x0] + (r0[x1] - r0[x0]) * tx;
                let b = r1[x0] + (r1[x1] - r1[x0]) * tx;
                let g = 1.0 + (a + (b - a) * ty - 1.0) * amount;
                for c in &mut px[..3] {
                    *c = (*c as f32 * g + 0.5).min(255.0) as u8;
                }
            }
        }
    }

    /// Studio Light's per-frame contrast pivot: the subject's mean luma,
    /// sampled on a coarse grid weighted by the mask and EMA-smoothed so it
    /// can't flicker. Pivoting on the subject (not mid-grey) means contrast
    /// spreads tones around how bright you actually are, instead of darkening
    /// a dim face. Falls back to the last pivot (or 0.5) with no subject.
    fn studio_pivot(state: &mut State, img: &[u8]) -> f32 {
        match subject_mean_luma(img, &state.mask_lut) {
            Some(mean) => {
                let next = match state.studio_pivot {
                    Some(prev) => prev + (mean - prev) * STUDIO_PIVOT_ALPHA,
                    None => mean,
                };
                state.studio_pivot = Some(next);
                next
            }
            None => state.studio_pivot.unwrap_or(0.5),
        }
    }

    /// Mask-weighted mean luma (0..1) over every `STUDIO_PIVOT_STRIDE`th
    /// pixel; `None` when (almost) nothing is foreground.
    fn subject_mean_luma(img: &[u8], lut: &[u8]) -> Option<f32> {
        let (mut acc, mut weight) = (0u64, 0u64);
        for (px, &m) in img
            .as_chunks::<4>()
            .0
            .iter()
            .zip(lut)
            .step_by(STUDIO_PIVOT_STRIDE)
        {
            let m = m as u64;
            let luma = (77 * px[0] as u64 + 150 * px[1] as u64 + 29 * px[2] as u64) >> 8;
            acc += luma * m;
            weight += m;
        }
        // Under ~1% coverage the mean is noise (a stray hand, a mask blip).
        let sampled = img.len() / 4 / STUDIO_PIVOT_STRIDE;
        if weight < 255 * sampled as u64 / 100 {
            return None;
        }
        Some(acc as f32 / (weight as f32 * 255.0))
    }

    /// Studio Light: a `videobalance`-style brightness/contrast lift applied
    /// to the person (masked by the foreground mask), with an independent
    /// background brightness applied to the complementary area (inverted mask).
    /// Per channel, on normalized values:
    /// `fg_adjusted = (p − pivot)·contrast + pivot + brightness` (foreground),
    /// `bg_adjusted = p + bg_brightness` (background), then
    /// `out = blend(lerp(p, fg_adj, m), lerp(p, bg_adj, 1-m), m)`.
    /// The transfer depends only on the input value, so it uses 256-entry
    /// lookup tables and pure integer math.
    fn apply_studio(
        img: &mut [u8],
        lut: &[u8],
        pivot: f32,
        brightness: f32,
        contrast: f32,
        bg_brightness: f32,
    ) {
        let has_bg = bg_brightness != 0.0;
        let mut adj_fg = [0u8; 256];
        let mut adj_bg = [0u8; 256];
        for p in 0..=255 {
            let v = p as f32 / 255.0;
            adj_fg[p] = (((v - pivot) * contrast + pivot + brightness).clamp(0.0, 1.0) * 255.0)
                .round() as u8;
            adj_bg[p] = ((v + bg_brightness).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        for (px, &m) in img.as_chunks_mut::<4>().0.iter_mut().zip(lut) {
            let m = m as u32;
            let bg_m = 255 - m;
            // Skip pixels entirely unaffected by either pass (m == 0 and no bg
            // adjustment) — this is the vast majority when bg_brightness == 0.
            if m == 0 && !has_bg {
                continue;
            }
            for c in 0..3 {
                let p = px[c] as u32;
                // Foreground blend: lerp(p, adj_fg, m/255)
                let fg = (p * bg_m + adj_fg[px[c] as usize] as u32 * m + 127) / 255;
                let out = if has_bg && bg_m > 0 {
                    // Background blend: lerp(result, adj_bg, bg_m/255)
                    let bg = (p * m + adj_bg[px[c] as usize] as u32 * bg_m + 127) / 255;
                    // Weighted sum of the two blends by the mask
                    (fg * m + bg * bg_m + 127) / 255
                } else {
                    fg
                };
                px[c] = out as u8;
            }
        }
    }

    /// `out = fg·m + bg·(1-m)` against a full-resolution background buffer
    /// (user image, already cached at frame size).
    fn composite_full(fg: &mut [u8], bg: &[u8], lut: &[u8]) {
        for (i, &m) in lut.iter().enumerate() {
            if m == 255 {
                continue; // fully foreground: nothing to blend
            }
            let p = i * 4;
            let m = m as u32;
            for c in 0..3 {
                fg[p + c] =
                    ((fg[p + c] as u32 * m + bg[p + c] as u32 * (255 - m) + 127) / 255) as u8;
            }
        }
    }

    /// Composite a solid color behind the subject — no background buffer is
    /// ever materialized.
    fn composite_color(fg: &mut [u8], color: [u8; 4], lut: &[u8]) {
        for (px, &m) in fg.as_chunks_mut::<4>().0.iter_mut().zip(lut) {
            if m == 255 {
                continue;
            }
            let m = m as u32;
            for c in 0..3 {
                px[c] = ((px[c] as u32 * m + color[c] as u32 * (255 - m) + 127) / 255) as u8;
            }
        }
    }

    /// Composite the low-res, background-weighted blurred background behind the
    /// subject, bilinearly upsampling it on the fly (no full-resolution
    /// intermediate buffer). The `low` buffer holds premultiplied color in RGB
    /// and the summed background weight in alpha (see `downsample_masked`); the
    /// true background color is recovered as `rgb·255/alpha` — a weighted mean
    /// that excludes the subject, so no person color halos the edge. Where the
    /// blur neighborhood was entirely subject (alpha ≈ 0) there is no
    /// background to show, but such pixels are deep inside the person (mask ≈
    /// 255) and contribute almost nothing to the blend.
    #[allow(clippy::too_many_arguments)]
    fn composite_low(
        fg: &mut [u8],
        low: &[u8],
        lw: usize,
        lh: usize,
        scale: usize,
        lut: &[u8],
        w: usize,
        h: usize,
    ) {
        let inv = 1.0 / scale as f32;
        for y in 0..h {
            let sv = (y as f32 + 0.5) * inv - 0.5;
            for x in 0..w {
                let m = lut[y * w + x];
                if m == 255 {
                    continue;
                }
                let su = (x as f32 + 0.5) * inv - 0.5;
                let mut s = [0u8; 4];
                sample_bilinear(low, lw, lh, su, sv, &mut s);
                let wbg = s[3] as u32;
                let p = (y * w + x) * 4;
                let m = m as u32;
                for c in 0..3 {
                    // Un-premultiply: bg = premul·255 / weight. Fall back to the
                    // foreground when the neighborhood had no background weight.
                    let bg = (s[c] as u32 * 255)
                        .checked_div(wbg)
                        .map_or(fg[p + c] as u32, |v| v.min(255));
                    fg[p + c] = ((fg[p + c] as u32 * m + bg * (255 - m) + 127) / 255) as u8;
                }
            }
        }
    }

    /// Map a 0–100 strength to a box-blur radius (full-resolution units).
    fn blur_radius(strength: u32) -> usize {
        1 + (strength as usize * 24) / 100
    }

    /// Downscale factor the blur runs at. Heavier blurs tolerate (and benefit
    /// from) a smaller working image; light blurs stay near full resolution so
    /// the minimum strength still looks subtle.
    fn blur_scale(radius: usize) -> usize {
        match radius {
            0..=1 => 1,
            2..=5 => 2,
            _ => 4,
        }
    }

    /// Box-average the frame down by `scale` into `dst`, weighting every pixel
    /// by its background weight `wbg = 255 - mask` so the subject's pixels are
    /// kept out of the blurred background (the fix for the edge halo). RGB holds
    /// the premultiplied average `mean(color · wbg/255)`; alpha holds the mean
    /// weight `mean(wbg)`. Downstream the background color is recovered as
    /// `rgb·255/alpha`, i.e. the weighted mean of the background pixels alone —
    /// person pixels (wbg ≈ 0) drop out. `box_blur` then blurs all four channels
    /// so premul and weight stay consistent under the blur.
    #[allow(clippy::too_many_arguments)] // plain geometry args shared by every caller
    fn downsample_masked(
        src: &[u8],
        lut: &[u8],
        w: usize,
        h: usize,
        scale: usize,
        dst: &mut Vec<u8>,
        lw: usize,
        lh: usize,
    ) {
        dst.resize(lw * lh * 4, 0);
        for y in 0..lh {
            let y0 = y * scale;
            let y1 = (y0 + scale).min(h);
            for x in 0..lw {
                let x0 = x * scale;
                let x1 = (x0 + scale).min(w);
                let mut acc = [0u32; 3];
                let mut wsum = 0u32;
                for sy in y0..y1 {
                    let row = sy * w;
                    for sx in x0..x1 {
                        let idx = row + sx;
                        let wbg = 255 - lut[idx] as u32;
                        let p = idx * 4;
                        acc[0] += src[p] as u32 * wbg;
                        acc[1] += src[p + 1] as u32 * wbg;
                        acc[2] += src[p + 2] as u32 * wbg;
                        wsum += wbg;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                let d = (y * lw + x) * 4;
                // Premultiplied color in 0..255: mean over the block of
                // color·(wbg/255). Alpha carries the mean background weight.
                dst[d] = (acc[0] / n / 255) as u8;
                dst[d + 1] = (acc[1] / n / 255) as u8;
                dst[d + 2] = (acc[2] / n / 255) as u8;
                dst[d + 3] = (wsum / n) as u8;
            }
        }
    }

    /// Separable box blur (two passes ≈ triangle kernel) in place, ping-ponging
    /// through `tmp`. Per-row/column prefix sums keep cost independent of
    /// radius. All four channels are blurred: RGB carries premultiplied color
    /// and alpha carries the background weight, and both must see the same blur
    /// so the composite's un-premultiply stays correct (see `downsample_masked`).
    fn box_blur(
        buf: &mut [u8],
        tmp: &mut Vec<u8>,
        prefix: &mut Vec<i32>,
        w: usize,
        h: usize,
        radius: usize,
    ) {
        if radius == 0 {
            return;
        }
        tmp.resize(buf.len(), 0);
        // One scratch sized for both passes (h needs w+1, v needs h+1), reused
        // across both blur iterations. Index 0 is never written, so it stays 0 —
        // which the window subtraction at `lo == 0` relies on.
        prefix.resize(w.max(h) + 1, 0);
        for _ in 0..2 {
            box_blur_h(buf, tmp, prefix, w, h, radius);
            box_blur_v(tmp, buf, prefix, w, h, radius);
        }
    }

    fn box_blur_h(src: &[u8], dst: &mut [u8], prefix: &mut [i32], w: usize, h: usize, r: usize) {
        // prefix[i] = running sum of one channel over pixels 0..i; reused across
        // every (row, channel) to avoid per-iteration allocation on the hot path.
        for y in 0..h {
            let base = y * w * 4;
            for c in 0..4 {
                for x in 0..w {
                    prefix[x + 1] = prefix[x] + src[base + x * 4 + c] as i32;
                }
                for x in 0..w {
                    let lo = x.saturating_sub(r);
                    let hi = (x + r + 1).min(w);
                    let sum = prefix[hi] - prefix[lo];
                    dst[base + x * 4 + c] = (sum / (hi - lo) as i32) as u8;
                }
            }
        }
    }

    fn box_blur_v(src: &[u8], dst: &mut [u8], prefix: &mut [i32], w: usize, h: usize, r: usize) {
        let row = w * 4;
        for x in 0..w {
            for c in 0..4 {
                for y in 0..h {
                    prefix[y + 1] = prefix[y] + src[y * row + x * 4 + c] as i32;
                }
                for y in 0..h {
                    let lo = y.saturating_sub(r);
                    let hi = (y + r + 1).min(h);
                    let sum = prefix[hi] - prefix[lo];
                    dst[y * row + x * 4 + c] = (sum / (hi - lo) as i32) as u8;
                }
            }
        }
    }

    /// `#RRGGBB` → RGBA bytes.
    fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some([r, g, b, 255])
    }

    /// Decode an image file and nearest-neighbor rescale it to `w`×`h` RGBA.
    fn load_image_rgba(path: &str, w: usize, h: usize) -> anyhow::Result<Vec<u8>> {
        let src = image::ImageReader::open(path)?.decode()?.to_rgba8();
        let (sw, sh) = (src.width() as usize, src.height() as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            let sy = (y * sh / h).min(sh - 1);
            for x in 0..w {
                let sx = (x * sw / w).min(sw - 1);
                let sp = (sy * sw + sx) * 4;
                let dp = (y * w + x) * 4;
                out[dp..dp + 4].copy_from_slice(&src.as_raw()[sp..sp + 4]);
            }
        }
        Ok(out)
    }

    /// What the framing `span` was derived from, which sets its margin scale.
    #[derive(Clone, Copy)]
    enum SubjectKind {
        /// A single face's bbox height (forehead→chin).
        Face,
        /// Union of head boxes (group mode).
        Group,
    }

    /// Normalized subject to frame on: `(cx, cy, span, kind)`.
    ///
    /// To keep the face fully in frame while staying steady through expressions:
    /// the framing **center** uses the eye midpoint (expression-invariant — a
    /// smile doesn't move the eyes), but the framing **span** uses the bbox
    /// *height* (forehead→chin), which sets the zoom so the whole face fits with
    /// headroom. Height is far more expression-stable than width (smiling widens
    /// the mouth, not the face height), so the zoom barely moves either. Falls
    /// back to the bbox center when landmarks are missing.
    fn subject_box(faces: &[Face], mode: &str) -> Option<(f32, f32, f32, SubjectKind)> {
        if faces.is_empty() {
            return None;
        }
        if mode == "group" {
            // Union of per-face head boxes; expression-stable since it's the
            // outer extent of all faces, padded by the average face height.
            let (mut x0, mut y0, mut x1, mut y1) = (1.0f32, 1.0f32, 0.0f32, 0.0f32);
            let mut height_sum = 0.0f32;
            for f in faces {
                x0 = x0.min(f.cx - f.w / 2.0);
                y0 = y0.min(f.cy - f.h / 2.0);
                x1 = x1.max(f.cx + f.w / 2.0);
                y1 = y1.max(f.cy + f.h / 2.0);
                height_sum += f.h;
            }
            let pad = height_sum / faces.len() as f32;
            return Some((
                (x0 + x1) / 2.0,
                (y0 + y1) / 2.0,
                (x1 - x0).max(y1 - y0) + pad,
                SubjectKind::Group,
            ));
        }
        let best = faces.iter().max_by(|a, b| a.score.total_cmp(&b.score))?;
        let (cx, cy) = if best.eye_dist > 0.0 {
            // Eye midpoint, nudged down toward the face center (eyes sit above
            // it) so the crop has balanced forehead/chin headroom.
            (best.eye_cx, best.eye_cy + best.h * 0.25)
        } else {
            (best.cx, best.cy)
        };
        Some((cx, cy, best.h, SubjectKind::Face))
    }

    /// Crop span = `face height × margin`; a tighter zoom leaves less headroom.
    /// All margins are > 1 so the crop always encloses the full face (the bbox
    /// height) plus forehead/chin/hair room — even "tight" never cuts the face.
    fn zoom_margin(zoom: &str, kind: SubjectKind) -> f32 {
        match kind {
            SubjectKind::Face => match zoom {
                "subtle" => 3.4,
                "tight" => 2.1,
                _ => 2.7,
            },
            SubjectKind::Group => match zoom {
                "subtle" => 1.6,
                "tight" => 1.15,
                _ => 1.4,
            },
        }
    }

    /// Target crop: centered on the subject (clamped so the crop stays inside
    /// the frame) and sized relative to the subject, so the face is recentered
    /// wherever it moves and the zoom level only changes the spacing around it.
    /// No subject → relax back to the full frame so the zoom resets smoothly.
    fn crop_target(subject: Option<(f32, f32, f32, SubjectKind)>, zoom: &str) -> CropState {
        let Some((cx, cy, span, kind)) = subject else {
            return CropState::default();
        };
        let zf = (span * zoom_margin(zoom, kind)).clamp(CS_MIN_CROP, 1.0);
        let half = zf / 2.0;
        CropState {
            cx: cx.clamp(half, 1.0 - half),
            cy: cy.clamp(half, 1.0 - half),
            zf,
        }
    }

    /// Resample the crop window back up to the full frame (bilinear), in place.
    fn resample_crop(img: &mut [u8], w: usize, h: usize, crop: CropState) {
        if crop.zf >= 0.999 {
            return;
        }
        let src = img.to_vec();
        let half = crop.zf / 2.0;
        let x0 = (crop.cx - half) * w as f32;
        let y0 = (crop.cy - half) * h as f32;
        let span_x = crop.zf * w as f32;
        let span_y = crop.zf * h as f32;
        for y in 0..h {
            let sv = y0 + (y as f32 + 0.5) / h as f32 * span_y - 0.5;
            for x in 0..w {
                let su = x0 + (x as f32 + 0.5) / w as f32 * span_x - 0.5;
                let dp = (y * w + x) * 4;
                sample_bilinear(&src, w, h, su, sv, &mut img[dp..dp + 4]);
            }
        }
    }

    fn sample_bilinear(src: &[u8], w: usize, h: usize, fx: f32, fy: f32, out: &mut [u8]) {
        let fx = fx.clamp(0.0, w as f32 - 1.0);
        let fy = fy.clamp(0.0, h as f32 - 1.0);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(w - 1);
        let y1 = (y0 + 1).min(h - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        for c in 0..4 {
            let a = src[(y0 * w + x0) * 4 + c] as f32;
            let b = src[(y0 * w + x1) * 4 + c] as f32;
            let cc = src[(y1 * w + x0) * 4 + c] as f32;
            let d = src[(y1 * w + x1) * 4 + c] as f32;
            let top = a + (b - a) * tx;
            let bot = cc + (d - cc) * tx;
            out[c] = (top + (bot - top) * ty).round() as u8;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn auto_gate_is_off_when_lit_and_on_when_dark() {
            assert_eq!(auto_gate_target(0.7), 0.0);
            assert_eq!(auto_gate_target(0.1), 1.0);
            let mid = auto_gate_target(0.4);
            assert!(mid > 0.0 && mid < 1.0);
        }

        #[test]
        fn apply_gain_scales_and_clamps() {
            let gain = Mask {
                size: 4,
                values: vec![2.0; 16],
            };
            let mut img = vec![50, 100, 200, 255, 10, 10, 10, 255];
            apply_gain(&mut img, 2, 1, &gain, 1.0);
            assert_eq!(&img[..4], &[100, 200, 255, 255]);
            assert_eq!(&img[4..8], &[20, 20, 20, 255]);
            // Half strength is a gain of 1.5.
            let mut img = vec![100, 100, 100, 255];
            apply_gain(&mut img, 1, 1, &gain, 0.5);
            assert_eq!(img[0], 150);
        }

        #[test]
        fn studio_contrast_keeps_subject_brightness() {
            // A dim subject (luma 80) with a little spread, repeated so the
            // coarse sampling grid sees every tone; contrast 1.8 about the
            // subject's own mean must not move the average.
            let tones = [60u8, 70, 80, 90, 100];
            let mut img = Vec::new();
            for i in 0..500 {
                let v = tones[i % tones.len()];
                img.extend_from_slice(&[v, v, v, 255]);
            }
            let lut = vec![255u8; 500];
            let mean = |b: &[u8]| b.chunks(4).map(|p| p[0] as f32).sum::<f32>() / 500.0;
            let before = mean(&img);
            let pivot = subject_mean_luma(&img, &lut).unwrap();
            apply_studio(&mut img, &lut, pivot, 0.0, 1.8, 0.0);
            assert!(
                (mean(&img) - before).abs() < 2.0,
                "{} vs {before}",
                mean(&img)
            );
            assert!(img[0] < 60 && img[16] > 100, "spread should widen");
        }

        #[test]
        fn studio_brightness_lifts_subject_only() {
            let mut img = vec![100, 100, 100, 255, 100, 100, 100, 255];
            apply_studio(&mut img, &[255, 0], 0.4, 0.15, 1.0, 0.0);
            assert!(img[0] > 130);
            assert_eq!(img[4], 100);
        }

        #[test]
        fn subject_mean_luma_ignores_background_and_tiny_masks() {
            let mut img = vec![0u8; STUDIO_PIVOT_STRIDE * 400 * 4];
            let n = img.len() / 4;
            for px in img.as_chunks_mut::<4>().0.iter_mut().take(n / 2) {
                px.copy_from_slice(&[200, 200, 200, 255]);
            }
            let mut lut = vec![0u8; n];
            lut[..n / 2].fill(255);
            let m = subject_mean_luma(&img, &lut).unwrap();
            assert!((m - 200.0 / 255.0).abs() < 0.02, "{m}");
            assert!(subject_mean_luma(&img, &vec![0u8; n]).is_none());
        }

        #[test]
        fn idle_slot_unloads_then_reloads_on_demand() {
            let mut slot: Slot<u32> = Slot::default();
            assert_eq!(slot.get_simple(|| Ok(7), "x"), Some(&mut 7));
            assert!(!slot.unload_idle(Duration::from_secs(60)));
            assert!(slot.unload_idle(Duration::ZERO));
            assert!(!slot.unload_idle(Duration::ZERO));
            assert_eq!(slot.get_simple(|| Ok(8), "x"), Some(&mut 8));
        }

        #[test]
        fn failed_slot_is_not_retried() {
            let mut slot: Slot<u32> = Slot::default();
            assert!(slot.get_simple(|| anyhow::bail!("missing"), "x").is_none());
            assert!(slot.get_simple(|| Ok(1), "x").is_none());
        }

        #[test]
        fn failed_slot_can_be_reset() {
            let mut slot: Slot<u32> = Slot::default();
            assert!(slot.get_simple(|| anyhow::bail!("missing"), "x").is_none());
            assert!(slot.failed);
            slot.reset_failed();
            assert!(!slot.failed);
            assert_eq!(slot.get_simple(|| Ok(1), "x"), Some(&mut 1));
        }

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
            assert_eq!(element.state.lock().unwrap().engine.provider, "npu");

            // Switching away clears the stale fallback reason.
            let (active, detail) = element.swap_provider("cpu").expect("swap to cpu");
            assert_eq!((active.as_str(), detail), ("cpu", None));
        }

        const GOLDEN_W: usize = 96;
        const GOLDEN_H: usize = 64;
        const GOLDEN_FRAMES: usize = 24;

        /// A drifting gradient with a brighter oval "subject" that slides right.
        fn golden_frame(i: usize) -> Vec<u8> {
            let (w, h) = (GOLDEN_W, GOLDEN_H);
            let cx = 0.35 + i as f32 * 0.01;
            let mut img = vec![0u8; w * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                    let d = ((u - cx) / 0.18).powi(2) + ((v - 0.55) / 0.35).powi(2);
                    let p = (y * w + x) * 4;
                    if d < 1.0 {
                        img[p..p + 4].copy_from_slice(&[200, (120 + x) as u8, 90, 255]);
                    } else {
                        img[p..p + 4].copy_from_slice(&[
                            (x * 2 + i) as u8,
                            (y * 3) as u8,
                            (255 - x - y + i) as u8,
                            255,
                        ]);
                    }
                }
            }
            img
        }

        /// A soft oval matching `golden_frame`'s subject, as the segmenter
        /// would return it.
        fn golden_mask(i: usize) -> Mask {
            let size = 32;
            let cx = 0.35 + i as f32 * 0.01;
            let mut values = vec![0f32; size * size];
            for y in 0..size {
                for x in 0..size {
                    let (u, v) = (
                        (x as f32 + 0.5) / size as f32,
                        (y as f32 + 0.5) / size as f32,
                    );
                    let d = ((u - cx) / 0.18).powi(2) + ((v - 0.55) / 0.35).powi(2);
                    values[y * size + x] = (1.5 - d).clamp(0.0, 1.0);
                }
            }
            Mask { size, values }
        }

        fn golden_face(i: usize) -> Face {
            let cx = 0.35 + i as f32 * 0.01;
            Face {
                cx,
                cy: 0.4,
                w: 0.2,
                h: 0.25,
                score: 0.9,
                eye_cx: cx,
                eye_cy: 0.35,
                eye_dist: 0.08,
            }
        }

        /// Every model slot pre-failed, so nothing loads and the injected mask,
        /// faces and gain map are what the effects see, models installed or not.
        fn offline_state(settings: Settings) -> State {
            let mut state = State {
                settings,
                ..State::default()
            };
            state.engine.selfie.failed = true;
            state.engine.yunet.failed = true;
            state.engine.palm.failed = true;
            state.engine.hand.failed = true;
            state.engine.auto_light.failed = true;
            state
        }

        /// SHA-256 of every output frame of the golden clip under `settings`.
        fn golden_digest(settings: Settings) -> String {
            use sha2::{Digest, Sha256};
            let mut state = offline_state(settings);
            if state.settings.auto_light_enabled {
                let values = (0..64).map(|i| 1.0 + (i % 8) as f32 * 0.15).collect();
                state.auto_gain = Some(Mask { size: 8, values });
                state.auto_gate = 0.7;
            }
            if state.settings.reactions_enabled {
                state.reactions.trigger("heart", 0);
            }
            let mut hasher = Sha256::new();
            let mut changed = false;
            for i in 0..GOLDEN_FRAMES {
                let mut img = golden_frame(i);
                state.mask = Some(golden_mask(i));
                state.mask_gen = state.mask_gen.wrapping_add(1);
                state.faces = vec![golden_face(i)];
                process_frame(&mut state, &mut img, GOLDEN_W, GOLDEN_H);
                changed |= img != golden_frame(i);
                hasher.update(&img);
            }
            assert!(changed, "no frame was touched, so the digest pins nothing");
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }

        fn golden_bg_image() -> tempfile::NamedTempFile {
            let file = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
            let img = image::RgbaImage::from_fn(40, 30, |x, y| {
                image::Rgba([(x * 6) as u8, (y * 8) as u8, 128, 255])
            });
            img.save(file.path()).unwrap();
            file
        }

        /// Characterization of the whole effect chain on synthetic frames. The
        /// digests pin today's output byte for byte; a pure move or refactor
        /// must leave them unchanged.
        #[test]
        fn golden_frames_are_unchanged() {
            let bg = golden_bg_image();
            let cases = [
                (
                    "studio + blur",
                    Settings {
                        studio_enabled: true,
                        studio_brightness: 0.2,
                        studio_contrast: 1.4,
                        studio_bg_brightness: -0.2,
                        blur_enabled: true,
                        blur_strength: 70,
                        ..Settings::default()
                    },
                    "0502e625f2d44a633f3d3521bbcf76f36ff678f82f057e020530b5fdb4735d9a",
                ),
                (
                    "light blur",
                    Settings {
                        blur_enabled: true,
                        blur_strength: 10,
                        ..Settings::default()
                    },
                    "6a00c6ec2826a04bc7775507b5a218aac1c757004856be162ea7979a19889978",
                ),
                (
                    "bg color + studio",
                    Settings {
                        bg_enabled: true,
                        bg_path: "#204060".into(),
                        studio_enabled: true,
                        studio_brightness: 0.1,
                        ..Settings::default()
                    },
                    "fcf136772a179ea5bfaf033dddee85727dd5fed4e7367115c09d94cfe18c0a77",
                ),
                (
                    "bg image",
                    Settings {
                        bg_enabled: true,
                        bg_path: bg.path().to_str().unwrap().into(),
                        ..Settings::default()
                    },
                    "79291583db6f4209a3d0ac440ecbde24906cde9b1056ab7926f7313437aef8b3",
                ),
                (
                    "center stage only",
                    Settings {
                        cs_enabled: true,
                        cs_zoom: "tight".into(),
                        ..Settings::default()
                    },
                    "9602119145eaf540b0245d68c5d0c2ec652611474ec00d6200177dfc7b8b5f7e",
                ),
                (
                    "center stage + blur",
                    Settings {
                        cs_enabled: true,
                        blur_enabled: true,
                        ..Settings::default()
                    },
                    "bbc8e81bacf68111de09e4e805b00e2a533775650c59a5690fd4270820b99c64",
                ),
                (
                    "center stage group + bg color",
                    Settings {
                        cs_enabled: true,
                        cs_mode: "group".into(),
                        cs_zoom: "subtle".into(),
                        bg_enabled: true,
                        bg_path: "#102030".into(),
                        ..Settings::default()
                    },
                    "f171b59f3876419af500a1ac6a879b1af8bb549836dcee356c76f651d8eb1139",
                ),
                (
                    "auto light + reactions",
                    Settings {
                        auto_light_enabled: true,
                        reactions_enabled: true,
                        ..Settings::default()
                    },
                    "f4d93c718370072a9ec17bd82c9e5abad4ed06322a1d2a642f19eb4f284c2fd8",
                ),
            ];
            let mut failures = Vec::new();
            for (name, settings, want) in cases {
                let got = golden_digest(settings);
                if got != want {
                    failures.push(format!("{name}: {got}"));
                }
            }
            assert!(
                failures.is_empty(),
                "digests changed:\n{}",
                failures.join("\n")
            );
        }

        /// Regression for #1: portrait blur must not bleed the subject's color
        /// into the blurred background (the halo). A red subject on a blue
        /// background, blurred with the background-weighted path, leaves the
        /// background pure blue right up to the subject edge — a plain blur of
        /// the whole frame would smear red into that band.
        #[test]
        fn blurred_background_excludes_subject() {
            let (w, h) = (16usize, 16usize);
            let mut img = vec![0u8; w * h * 4];
            let mut lut = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    let person = (5..11).contains(&x) && (5..11).contains(&y);
                    let p = (y * w + x) * 4;
                    if person {
                        img[p] = 255; // red subject
                        lut[y * w + x] = 255;
                    } else {
                        img[p + 2] = 255; // blue background
                    }
                    img[p + 3] = 255;
                }
            }

            let mut low = Vec::new();
            let mut tmp = Vec::new();
            let mut prefix = Vec::new();
            downsample_masked(&img, &lut, w, h, 1, &mut low, w, h);
            box_blur(&mut low, &mut tmp, &mut prefix, w, h, 4);

            let mut out = img.clone();
            composite_low(&mut out, &low, w, h, 1, &lut, w, h);

            // Background pixel hugging the subject's left edge: no red leak.
            let edge = (8 * w + 4) * 4;
            assert_eq!(
                out[edge], 0,
                "subject color bled into the background (halo)"
            );
            assert!(out[edge + 2] >= 250, "background color was lost");

            // Subject interior is untouched (fully foreground).
            let inside = (8 * w + 8) * 4;
            assert_eq!(&out[inside..inside + 3], &[255, 0, 0]);
        }
    }
}
