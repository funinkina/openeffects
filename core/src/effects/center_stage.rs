//! Center Stage: a YuNet face box drives an EMA-smoothed, deadzoned crop+zoom
//! that keeps the face centered without distorting the aspect ratio.

use super::resample::sample_bilinear;
use super::settings::Settings;
use crate::inference::engine::Engine;
use crate::inference::models::Face;

/// Minimum YuNet score to accept a face for framing.
const FACE_SCORE_THRESH: f32 = 0.6;
/// Run YuNet face detection every Nth frame. The comfort zone below absorbs
/// detection jitter, so framing tolerates a slower (cheaper) detect cadence.
const FACE_INTERVAL: u64 = 4;
/// EMA smoothing factor for the center-stage crop (≈12 frames to 90%).
const CS_ALPHA: f32 = 0.18;
/// Tightest allowed crop (fraction of each axis); caps zoom at 4× so a
/// small/distant face doesn't blow up into an unusably pixelated frame.
const CS_MIN_CROP: f32 = 0.25;
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

/// EMA-smoothed crop window in normalized frame coordinates. `zf` is the
/// fraction of each axis the crop spans (1.0 = full frame, smaller = zoomed
/// in); the crop is square in normalized space so the pixel rect keeps the
/// frame's aspect ratio.
#[derive(Clone, Copy)]
pub(crate) struct CropState {
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

#[derive(Default)]
pub(crate) struct CenterStage {
    /// EMA-smoothed crop actually applied this frame.
    crop: CropState,
    /// Committed framing target the crop eases toward; locked until the
    /// subject sustains a position outside the comfort zone (see
    /// `update_target`).
    target: CropState,
    /// Consecutive frames the subject's ideal crop has been outside the
    /// committed target's comfort zone.
    out_frames: u32,
    pub(crate) faces: Vec<Face>,
}

impl CenterStage {
    /// Back to the full frame (on a resolution change). Faces are kept.
    pub(crate) fn reset(&mut self) {
        self.crop = CropState::default();
        self.target = CropState::default();
        self.out_frames = 0;
    }

    /// Decide this frame's crop on the full frame: refresh face detection on
    /// its cadence, reframe if the subject has settled somewhere new, and
    /// ease the applied crop toward the committed target.
    pub(crate) fn track(
        &mut self,
        engine: &mut Engine,
        settings: &Settings,
        frame: u64,
        img: &[u8],
        w: usize,
        h: usize,
    ) -> CropState {
        if frame % FACE_INTERVAL == 1 {
            let faces = if let Some(model) = engine.yunet() {
                model
                    .detect(img, w, h, w * 4, FACE_SCORE_THRESH)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            self.faces = faces;
        }

        // Framing is driven by the face's eye landmarks. When no face is found
        // the target is simply held, so brief look-aways don't make the crop
        // drift.
        let subject = subject_box(&self.faces, &settings.cs_mode);
        if let Some(raw) = subject.map(|s| crop_target(Some(s), &settings.cs_zoom)) {
            self.update_target(raw);
        }

        let t = self.target;
        let crop = &mut self.crop;
        crop.zf += (t.zf - crop.zf) * CS_ALPHA;
        crop.cx += (t.cx - crop.cx) * CS_ALPHA;
        crop.cy += (t.cy - crop.cy) * CS_ALPHA;
        *crop
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
    fn update_target(&mut self, raw: CropState) {
        let t = self.target;
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
            self.out_frames = 0;
            return;
        }

        self.out_frames += 1;
        let drastic = ex > CS_BREACH_POS
            || ey > CS_BREACH_POS
            || !(CS_BREACH_ZOOM_LO..=CS_BREACH_ZOOM_HI).contains(&rz);
        let confirm = if drastic { CS_FAST_CONFIRM } else { CS_CONFIRM };
        if self.out_frames >= confirm {
            self.target = raw;
            self.out_frames = 0;
        }
    }
}

/// Extract the crop window into a tight `rw×rh` RGBA buffer (bilinear), so
/// segmentation/blur/composite run on ROI-sized pixels. Returns the buffer
/// and its dimensions (`(w, h)` when the crop spans the whole frame).
pub(crate) fn extract_roi(
    img: &[u8],
    w: usize,
    h: usize,
    crop: CropState,
) -> (Vec<u8>, usize, usize) {
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
pub(crate) fn upscale_roi(img: &mut [u8], w: usize, h: usize, roi: &[u8], rw: usize, rh: usize) {
    for y in 0..h {
        let sv = (y as f32 + 0.5) / h as f32 * rh as f32 - 0.5;
        for x in 0..w {
            let su = (x as f32 + 0.5) / w as f32 * rw as f32 - 0.5;
            let dp = (y * w + x) * 4;
            sample_bilinear(roi, rw, rh, su, sv, &mut img[dp..dp + 4]);
        }
    }
}

/// Resample the crop window back up to the full frame (bilinear), in place.
pub(crate) fn resample_crop(img: &mut [u8], w: usize, h: usize, crop: CropState) {
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
