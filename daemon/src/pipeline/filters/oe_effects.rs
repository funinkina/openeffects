//! `oe_effects` — the single CPU video filter that hosts every ML-driven
//! effect. It receives RGBA frames in system memory, runs selfie segmentation
//! once per frame (reused for one extra frame), and composites, in order:
//!
//! 1. **Portrait blur** / **Background replace** — `out = fg·mask + bg·(1-mask)`
//!    where `bg` is either a box-blurred copy of the frame or a user image/color.
//! 2. **Center Stage** — a YuNet (or mask-derived) subject box drives an
//!    EMA-smoothed crop+zoom that keeps the subject framed without distorting
//!    the aspect ratio.
//!
//! Studio Light stays in the upstream `videobalance` element (no ML needed);
//! Reactions arrive in Phase 4.
//!
//! Consolidating all ML effects into one element means segmentation runs once
//! per frame regardless of how many effects are enabled, matching PRD §6.4's
//! single-inference, mask-reuse threading model.

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

    const DEFAULT_STRENGTH: u32 = 50;
    const DEFAULT_ZOOM: &str = "normal";
    const DEFAULT_MODE: &str = "single";
    /// Foreground-probability threshold for the mask-derived subject box.
    const MASK_FG_THRESH: f32 = 0.6;
    /// Minimum YuNet score to accept a face for framing.
    const FACE_SCORE_THRESH: f32 = 0.6;
    /// EMA smoothing factor for the center-stage crop (≈12 frames to 90%).
    const CS_ALPHA: f32 = 0.18;

    #[derive(Debug, Clone)]
    struct Settings {
        blur_enabled: bool,
        blur_strength: u32,
        bg_enabled: bool,
        bg_path: String,
        cs_enabled: bool,
        cs_zoom: String,
        cs_mode: String,
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
            }
        }
    }

    impl Settings {
        /// Whether any effect in this element does real work (otherwise the
        /// element runs in passthrough).
        fn active(&self) -> bool {
            self.blur_enabled || self.bg_enabled || (self.cs_enabled && self.cs_zoom != "off")
        }

        /// Whether selfie segmentation should run this frame: needed for the
        /// blur/bg-replace composite, and as Center Stage's subject-framing
        /// fallback when YuNet doesn't find a face.
        fn needs_mask(&self) -> bool {
            self.blur_enabled || self.bg_enabled || (self.cs_enabled && self.cs_zoom != "off")
        }

        /// Whether the blur/bg-replace composite step should run.
        fn needs_composite(&self) -> bool {
            self.blur_enabled || self.bg_enabled
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
                        warn!(%err, "yunet unavailable; center stage falls back to mask framing");
                        self.yunet_failed = true;
                    }
                }
            }
            self.yunet.as_mut()
        }
    }

    #[derive(Default)]
    struct State {
        settings: Settings,
        engine: Engine,
        crop: CropState,
        frame: u64,
        mask: Option<Mask>,
        faces: Vec<Face>,
        bg: Option<BgImage>,
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
                _ => unimplemented!(),
            }
            let active = state.settings.active();
            drop(state);
            self.obj().set_passthrough(!active);
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
            state.mask = None;
            state.bg = None;
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
            if !state.settings.active() {
                return Ok(gst::FlowSuccess::Ok);
            }
            state.frame = state.frame.wrapping_add(1);

            // Pack the (possibly strided) plane into a tight RGBA buffer.
            let mut img = pack(plane, w, h, stride);

            // ── Segmentation (reused for one extra frame to halve cost) ──────
            if state.settings.needs_mask()
                && (state.mask.is_none() || state.frame.is_multiple_of(2))
            {
                if let Some(model) = state.engine.selfie() {
                    match model.segment(&img, w, h, w * 4) {
                        Ok(mask) => state.mask = Some(mask),
                        Err(err) => warn!(%err, "segmentation failed this frame"),
                    }
                }
            }

            // ── Portrait blur / background replace ───────────────────────────
            if state.settings.needs_composite() {
                if state.settings.bg_enabled {
                    ensure_bg(&mut state, w, h);
                }
                // Borrow split: take mask out so we can also touch state.bg.
                if let Some(mask) = state.mask.take() {
                    let bg_buf = if state.settings.bg_enabled {
                        background_buffer(&state, w, h)
                    } else {
                        Some(box_blur(
                            &img,
                            w,
                            h,
                            blur_radius(state.settings.blur_strength),
                        ))
                    };
                    if let Some(bg) = bg_buf {
                        composite(&mut img, &bg, &mask, w, h);
                    }
                    state.mask = Some(mask);
                }
            }

            // ── Center stage crop + zoom ─────────────────────────────────────
            if state.settings.cs_enabled && state.settings.cs_zoom != "off" {
                // Refresh face detection every 3rd frame (PRD §6.4 budget).
                if state.frame % 3 == 1 {
                    let faces = if let Some(model) = state.engine.yunet() {
                        model
                            .detect(&img, w, h, w * 4, FACE_SCORE_THRESH)
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    state.faces = faces;
                }
                let subject =
                    subject_box(&state.faces, state.mask.as_ref(), &state.settings.cs_mode);
                let base_zf = zoom_scale(&state.settings.cs_zoom);
                let target = crop_target(subject, base_zf);
                let crop = &mut state.crop;
                crop.zf += (target.zf - crop.zf) * CS_ALPHA;
                crop.cx += (target.cx - crop.cx) * CS_ALPHA;
                crop.cy += (target.cy - crop.cy) * CS_ALPHA;
                let crop = *crop;
                resample_crop(&mut img, w, h, crop);
            }

            unpack(&img, plane, w, h, stride);
            Ok(gst::FlowSuccess::Ok)
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

    /// Normalized subject box (cx, cy, w, h) to frame on, from YuNet faces when
    /// available (expanded head→torso), else the mask's foreground extent.
    fn subject_box(
        faces: &[Face],
        mask: Option<&Mask>,
        mode: &str,
    ) -> Option<(f32, f32, f32, f32)> {
        if !faces.is_empty() {
            let person = |f: &Face| {
                let cx = f.cx;
                let cy = (f.cy + f.h * 0.9).min(1.0);
                let pw = (f.w * 2.2).min(1.0);
                let ph = (f.h * 3.2).min(1.0);
                (cx, cy, pw, ph)
            };
            if mode == "group" {
                let (mut x0, mut y0, mut x1, mut y1) = (1.0f32, 1.0f32, 0.0f32, 0.0f32);
                for f in faces {
                    let (cx, cy, pw, ph) = person(f);
                    x0 = x0.min(cx - pw / 2.0);
                    y0 = y0.min(cy - ph / 2.0);
                    x1 = x1.max(cx + pw / 2.0);
                    y1 = y1.max(cy + ph / 2.0);
                }
                return Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0, x1 - x0, y1 - y0));
            }
            let best = faces.iter().max_by(|a, b| a.score.total_cmp(&b.score))?;
            return Some(person(best));
        }
        let (x0, y0, x1, y1) = mask?.bounding_box(MASK_FG_THRESH)?;
        Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0, x1 - x0, y1 - y0))
    }

    /// Map a zoom level to the base crop fraction (smaller = tighter zoom).
    fn zoom_scale(zoom: &str) -> f32 {
        match zoom {
            "subtle" => 0.9,
            "normal" => 0.8,
            "tight" => 0.65,
            _ => 0.8,
        }
    }

    /// Target crop for a subject box and base zoom: zoom in to `base_zf`, but
    /// widen if needed so the (margin-padded) subject still fits. No subject →
    /// relax back to the full frame so the zoom resets smoothly.
    fn crop_target(subject: Option<(f32, f32, f32, f32)>, base_zf: f32) -> CropState {
        let Some((cx, cy, sw, sh)) = subject else {
            return CropState::default();
        };
        let fit = (sw.max(sh) * 1.3).clamp(0.05, 1.0);
        let zf = base_zf.max(fit).min(1.0);
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
