//! `oe_effects` — the single CPU video filter that hosts every ML-driven
//! effect. It receives RGBA frames in system memory and applies:
//!
//! 1. **Studio Light** — a `videobalance`-style brightness/contrast lift
//!    blended by the person mask, so only the subject is lit (the background
//!    keeps its original exposure).
//! 2. **Portrait blur** / **Background replace** — `out = fg·mask + bg·(1-mask)`
//!    where `bg` is either a box-blurred copy of the frame or a user image/color.
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
//! [`reactions`]: crate::pipeline::filters::reactions

use gst::glib;
use gstreamer as gst;
use gstreamer_video as gst_video;

glib::wrapper! {
    pub struct OeEffects(ObjectSubclass<imp::OeEffects>)
        @extends gst_video::VideoFilter, gstreamer_base::BaseTransform, gst::Element, gst::Object;
}

mod imp {
    use std::sync::{LazyLock, Mutex};

    use gst::glib;
    use gst_base::subclass::BaseTransformMode;
    use gstreamer as gst;
    use gstreamer_base as gst_base;
    use gstreamer_video as gst_video;
    use gstreamer_video::prelude::*;
    use gstreamer_video::subclass::prelude::*;
    use tracing::warn;

    use crate::inference::engine::{Face, Mask, SelfieSeg, YuNet};
    use crate::inference::gesture::{self, Gesture};
    use crate::inference::hand::{CropBox, HandLandmarker, HandLandmarks, PalmDetector};
    use crate::pipeline::filters::reactions::Reactions;

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
    /// 3 (≈10 Hz at 30 fps) keeps the ONNX call — the per-frame energy cost —
    /// off two of every three frames while the subject's silhouette barely
    /// moves frame-to-frame.
    const SEG_INTERVAL: u64 = 3;
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
        /// Reactions effect: when on, the gesture pipeline runs and may
        /// auto-trigger bursts (manual bursts work regardless, via a one-shot
        /// property that doesn't touch this flag).
        reactions_enabled: bool,
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
                reactions_enabled: false,
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

    /// Lazily-loaded models, with a one-shot failure memo so a missing model is
    /// logged once rather than retried every frame.
    #[derive(Default)]
    struct Engine {
        selfie: Option<SelfieSeg>,
        selfie_failed: bool,
        yunet: Option<YuNet>,
        yunet_failed: bool,
        palm: Option<PalmDetector>,
        palm_failed: bool,
        hand: Option<HandLandmarker>,
        hand_failed: bool,
    }

    impl Engine {
        fn selfie(&mut self) -> Option<&mut SelfieSeg> {
            if self.selfie.is_none() && !self.selfie_failed {
                match SelfieSeg::load() {
                    Ok(m) => self.selfie = Some(m),
                    Err(err) => {
                        warn!(%err, "selfie segmentation unavailable; portrait blur / bg replace disabled");
                        self.selfie_failed = true;
                    }
                }
            }
            self.selfie.as_mut()
        }

        fn yunet(&mut self) -> Option<&mut YuNet> {
            if self.yunet.is_none() && !self.yunet_failed {
                match YuNet::load() {
                    Ok(m) => self.yunet = Some(m),
                    Err(err) => {
                        warn!(%err, "yunet unavailable; center stage cannot track a subject");
                        self.yunet_failed = true;
                    }
                }
            }
            self.yunet.as_mut()
        }

        fn palm(&mut self) -> Option<&mut PalmDetector> {
            if self.palm.is_none() && !self.palm_failed {
                match PalmDetector::load() {
                    Ok(m) => self.palm = Some(m),
                    Err(err) => {
                        warn!(%err, "palm detection unavailable; gesture reactions disabled");
                        self.palm_failed = true;
                    }
                }
            }
            self.palm.as_mut()
        }

        fn hand(&mut self) -> Option<&mut HandLandmarker> {
            if self.hand.is_none() && !self.hand_failed {
                match HandLandmarker::load() {
                    Ok(m) => self.hand = Some(m),
                    Err(err) => {
                        warn!(%err, "hand landmark model unavailable; gesture reactions disabled");
                        self.hand_failed = true;
                    }
                }
            }
            self.hand.as_mut()
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
        /// Whether the cached `mask` was segmented from the center-stage ROI
        /// (true) or the full frame (false); a mismatch forces a re-segment so a
        /// normalized mask sampled in the wrong coordinate space is never reused.
        mask_is_roi: bool,
        faces: Vec<Face>,
        bg: Option<BgImage>,
        /// Reverse-rain emoji particle system (manual + gesture triggers).
        reactions: Reactions,
        /// Hands currently tracked by the gesture pipeline (≤ `MAX_TRACKS`).
        tracks: Vec<HandTrack>,
        /// Earliest frame palm re-detection may run again.
        redetect_at: u64,
        /// Gesture held across the last `held_count` checks (debounce).
        held: Option<Gesture>,
        held_count: u32,
        /// No new gesture trigger fires before this frame.
        cooldown_until: u64,
        width: usize,
        height: usize,
    }

    #[derive(Default)]
    pub struct OeEffects {
        state: Mutex<State>,
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
                    glib::ParamSpecBoolean::builder("reactions-enabled").build(),
                    // Write-only one-shot: setting it (even to a repeated value)
                    // fires a burst of that reaction id.
                    glib::ParamSpecString::builder("trigger-reaction")
                        .flags(glib::ParamFlags::WRITABLE)
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
                "studio-light-brightness" => {
                    state.settings.studio_brightness = value.get().unwrap()
                }
                "studio-light-contrast" => state.settings.studio_contrast = value.get().unwrap(),
                "reactions-enabled" => state.settings.reactions_enabled = value.get().unwrap(),
                "trigger-reaction" => {
                    let id = value.get::<Option<String>>().unwrap().unwrap_or_default();
                    let frame = state.frame;
                    state.reactions.trigger(&id, frame);
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
                "studio-light-brightness" => s.studio_brightness.to_value(),
                "studio-light-contrast" => s.studio_contrast.to_value(),
                "reactions-enabled" => s.reactions_enabled.to_value(),
                "trigger-reaction" => "".to_value(), // write-only; defensive
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let active = self.state.lock().unwrap().settings.active();
            self.obj().set_passthrough(!active);
        }
    }

    impl GstObjectImpl for OeEffects {}

    impl ElementImpl for OeEffects {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static META: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
                gst::subclass::ElementMetadata::new(
                    "OpenEffects ML Effects",
                    "Filter/Effect/Video",
                    "Portrait blur, background replace, and center stage via ONNX inference",
                    "Aryan <aryankushwaha3101@gmail.com>",
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
            state.bg = None;
            // Normalized hand crops don't survive a resolution change.
            state.tracks.clear();
            state.redetect_at = 0;
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
            state.frame = state.frame.wrapping_add(1);

            // Pack the (possibly strided) plane into a tight RGBA buffer.
            let mut img = pack(plane, w, h, stride);

            // Gesture detection runs on the original frame, before center stage
            // crops it in place, so hands outside the crop are still seen. It may
            // enqueue a reaction burst, drawn by the overlay step below.
            if state.settings.reactions_enabled {
                run_gestures(&mut state, &img, w, h);
            }

            let cs_active = state.settings.cs_enabled && state.settings.cs_zoom != "off";
            if cs_active {
                // Center stage owns the canvas: framing is decided on the full
                // frame, but the masked effects then run only on the cropped ROI
                // — the only pixels that survive the zoom — instead of segmenting
                // and compositing the whole frame and discarding the border.
                center_stage(&mut state, &mut img, w, h);
            } else if state.settings.needs_mask() {
                // No crop: studio light / blur / bg-replace over the full frame.
                masked_effects(&mut state, &mut img, w, h, false);
            }

            // Reactions overlay runs last, on the final canvas, so emojis are
            // never cropped/blurred by the effects above.
            if state.settings.reactions_enabled || state.reactions.active() {
                let frame = state.frame;
                state.reactions.step_and_render(&mut img, w, h, frame);
            }

            unpack(&img, plane, w, h, stride);
            Ok(gst::FlowSuccess::Ok)
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

        // 1. (Re)acquire hands via palm detection when there's room for more.
        if state.tracks.len() < MAX_TRACKS && state.frame >= state.redetect_at {
            let palms = match state.engine.palm() {
                Some(model) => model
                    .detect(img, w, h, stride, PALM_SCORE_THRESH)
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            state.redetect_at = state.frame + REDETECT_INTERVAL;
            for palm in palms {
                if state.tracks.len() >= MAX_TRACKS {
                    break;
                }
                let crop = palm.crop(w, h);
                if !state.tracks.iter().any(|t| crops_overlap(t.crop, crop)) {
                    state.tracks.push(HandTrack { crop, missed: 0 });
                }
            }
        }

        if state.tracks.is_empty() {
            return;
        }

        // 2. Landmarks per tracked hand. Crops are copied out so the engine can
        //    be borrowed while we read them.
        let crops: Vec<CropBox> = state.tracks.iter().map(|t| t.crop).collect();
        let mut landmarks: Vec<HandLandmarks> = Vec::with_capacity(crops.len());
        // Per track: the refreshed crop on success, or None to count a miss.
        let mut next: Vec<Option<CropBox>> = Vec::with_capacity(crops.len());
        for crop in crops {
            let lm = state
                .engine
                .hand()
                .and_then(|model| model.infer(img, w, h, stride, crop).ok());
            match lm {
                Some(lm) if lm.confidence >= HAND_CONF_THRESH => {
                    next.push(Some(lm.tracking_crop(w, h)));
                    landmarks.push(lm);
                }
                _ => next.push(None),
            }
        }

        // Advance confident tracks; drop a track after two consecutive misses.
        let mut updated = Vec::with_capacity(state.tracks.len());
        for (i, track) in state.tracks.drain(..).enumerate() {
            match next.get(i).and_then(|o| *o) {
                Some(crop) => updated.push(HandTrack { crop, missed: 0 }),
                None if track.missed + 1 < 2 => updated.push(HandTrack {
                    crop: track.crop,
                    missed: track.missed + 1,
                }),
                None => {}
            }
        }
        state.tracks = updated;

        // 3. Classify with hold-to-confirm debounce, gated by the cooldown.
        match gesture::classify(&landmarks) {
            g if g == state.held => state.held_count = state.held_count.saturating_add(1),
            g => {
                state.held = g;
                state.held_count = if g.is_some() { 1 } else { 0 };
            }
        }
        if let Some(g) = state.held {
            if state.held_count >= GESTURE_CONFIRM && state.frame >= state.cooldown_until {
                let frame = state.frame;
                state.reactions.trigger(g.reaction_id(), frame);
                state.cooldown_until = frame + REACTION_COOLDOWN;
                state.held = None;
                state.held_count = 0;
            }
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
        if state.settings.bg_enabled {
            ensure_bg(state, w, h);
        }
        // Borrow split: take mask out so we can also touch state.bg.
        if let Some(mask) = state.mask.take() {
            // Studio light first, so the lit subject is what blur/bg-replace
            // then composites over the (untouched-exposure) background.
            if state.settings.studio_enabled {
                apply_studio(
                    img,
                    &mask,
                    w,
                    h,
                    state.settings.studio_brightness,
                    state.settings.studio_contrast,
                );
            }
            if state.settings.needs_composite() {
                let bg_buf = if state.settings.bg_enabled {
                    background_buffer(state, w, h)
                } else {
                    Some(box_blur(
                        img,
                        w,
                        h,
                        blur_radius(state.settings.blur_strength),
                    ))
                };
                if let Some(bg) = bg_buf {
                    composite(img, &bg, &mask, w, h);
                }
            }
            state.mask = Some(mask);
        }
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
        let stale = state.mask.is_none()
            || state.mask_is_roi != is_roi
            || state.frame.is_multiple_of(SEG_INTERVAL);
        if !stale {
            return;
        }
        if let Some(model) = state.engine.selfie() {
            match model.segment(buf, w, h, stride) {
                Ok(mask) => {
                    state.mask = Some(mask);
                    state.mask_is_roi = is_roi;
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

    /// The background pixels to composite behind the subject: either the cached
    /// image, or a solid color filling the frame.
    fn background_buffer(state: &State, w: usize, h: usize) -> Option<Vec<u8>> {
        let path = &state.settings.bg_path;
        if let Some(color) = path.strip_prefix('#').and_then(parse_hex_color) {
            let mut buf = vec![0u8; w * h * 4];
            for px in buf.chunks_exact_mut(4) {
                px.copy_from_slice(&color);
            }
            return Some(buf);
        }
        state
            .bg
            .as_ref()
            .filter(|b| b.w == w && b.h == h)
            .map(|b| b.rgba.clone())
    }

    /// Pack a strided RGBA plane into a tight `w*h*4` buffer.
    fn pack(plane: &[u8], w: usize, h: usize, stride: usize) -> Vec<u8> {
        let row = w * 4;
        let mut out = vec![0u8; row * h];
        for y in 0..h {
            out[y * row..y * row + row].copy_from_slice(&plane[y * stride..y * stride + row]);
        }
        out
    }

    /// Write a tight RGBA buffer back into the strided plane.
    fn unpack(img: &[u8], plane: &mut [u8], w: usize, h: usize, stride: usize) {
        let row = w * 4;
        for y in 0..h {
            plane[y * stride..y * stride + row].copy_from_slice(&img[y * row..y * row + row]);
        }
    }

    /// Studio Light: a `videobalance`-style brightness/contrast lift applied
    /// only to the person, blended by the mask so the background keeps its
    /// original exposure. Per channel, on normalized values:
    /// `adjusted = (p − 0.5)·contrast + 0.5 + brightness`, then
    /// `out = lerp(p, adjusted, mask)`.
    fn apply_studio(
        img: &mut [u8],
        mask: &Mask,
        w: usize,
        h: usize,
        brightness: f32,
        contrast: f32,
    ) {
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                let m = mask.sample(u, v).clamp(0.0, 1.0);
                if m <= 0.0 {
                    continue;
                }
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    let p = img[i + c] as f32 / 255.0;
                    let adj = ((p - 0.5) * contrast + 0.5 + brightness).clamp(0.0, 1.0);
                    img[i + c] = ((p + (adj - p) * m) * 255.0).round() as u8;
                }
                // leave alpha as-is
            }
        }
    }

    /// `out = fg·mask + bg·(1-mask)` per pixel, with the mask bilinearly sampled.
    fn composite(fg: &mut [u8], bg: &[u8], mask: &Mask, w: usize, h: usize) {
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                let m = mask.sample(u, v).clamp(0.0, 1.0);
                let inv = 1.0 - m;
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    fg[i + c] = (fg[i + c] as f32 * m + bg[i + c] as f32 * inv).round() as u8;
                }
                // leave alpha as-is
            }
        }
    }

    /// Map a 0–100 strength to a box-blur radius.
    fn blur_radius(strength: u32) -> usize {
        1 + (strength as usize * 24) / 100
    }

    /// Separable box blur (two passes ≈ triangle kernel) over a tight RGBA
    /// buffer, using per-row/column prefix sums so cost is independent of radius.
    fn box_blur(src: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
        if radius == 0 {
            return src.to_vec();
        }
        let mut a = src.to_vec();
        let mut b = vec![0u8; src.len()];
        for _ in 0..2 {
            box_blur_h(&a, &mut b, w, h, radius);
            box_blur_v(&b, &mut a, w, h, radius);
        }
        a
    }

    fn box_blur_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
        // prefix[i] = running sum of one channel over pixels 0..i; reused across
        // every (row, channel) to avoid per-iteration allocation on the hot path.
        let mut prefix = vec![0i32; w + 1];
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

    fn box_blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
        let row = w * 4;
        let mut prefix = vec![0i32; h + 1];
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
}
