//! Two-stage MediaPipe hand pipeline used by the reactions gesture trigger:
//! [`PalmDetector`] (SSD over the whole frame) finds palms, then
//! [`HandLandmarker`] regresses 21 keypoints from a square crop. Both are
//! OpenCV-Zoo ONNX exports (`*_mediapipe_2023feb.onnx`), loaded the same way as
//! [`super::engine`]'s `SelfieSeg`/`YuNet`.
//!
//! v1 keeps the crop axis-aligned (no rotation normalization) — fine for the
//! upright thumbs-up / thumbs-down / heart gestures we classify; extreme angles
//! degrade gracefully. Decode math mirrors the Zoo reference `mp_palmdet.py` /
//! `mp_handpose.py`, reduced to what the gesture classifier needs.

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;

use super::{build_session, registry};

/// Both exports take a single NHWC input named `input_1`.
const INPUT: &str = "input_1";

/// A detected palm in normalized frame coords (`0..1`).
#[derive(Debug, Clone, Copy)]
pub struct Palm {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
}

/// A square crop region: center normalized to the frame, `side` as a fraction
/// of frame width (the crop is square in pixels).
#[derive(Debug, Clone, Copy)]
pub struct CropBox {
    pub cx: f32,
    pub cy: f32,
    pub side: f32,
}

impl CropBox {
    /// Top-left and side length in pixels for a `w`×`h` frame.
    fn to_px(self, w: usize, h: usize) -> (f32, f32, f32) {
        let side_px = self.side * w as f32;
        let x0 = self.cx * w as f32 - side_px / 2.0;
        let y0 = self.cy * h as f32 - side_px / 2.0;
        (x0, y0, side_px)
    }
}

/// 21 hand landmarks in normalized frame coords plus a relative depth `z`.
#[derive(Debug, Clone)]
pub struct HandLandmarks {
    pub points: [[f32; 3]; 21],
    pub confidence: f32,
}

impl HandLandmarks {
    /// Axis-aligned bbox of the landmarks (normalized): `(x0, y0, x1, y1)`.
    pub fn bbox(&self) -> (f32, f32, f32, f32) {
        let mut x0 = 1.0f32;
        let mut y0 = 1.0f32;
        let mut x1 = 0.0f32;
        let mut y1 = 0.0f32;
        for p in &self.points {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        (x0, y0, x1, y1)
    }

    /// A tracking crop for the next frame, derived from these landmarks
    /// (MediaPipe's hand-box enlarge ≈1.8, shifted up toward the fingers).
    pub fn tracking_crop(&self, w: usize, h: usize) -> CropBox {
        let (x0, y0, x1, y1) = self.bbox();
        let wpx = (x1 - x0) * w as f32;
        let hpx = (y1 - y0) * h as f32;
        let side_px = wpx.max(hpx) * HAND_CROP_ENLARGE;
        CropBox {
            cx: (x0 + x1) / 2.0,
            cy: (y0 + y1) / 2.0 - 0.1 * (y1 - y0),
            side: side_px / w as f32,
        }
    }
}

const PALM_SIZE: usize = 192;
const HAND_SIZE: usize = 224;
/// Palm-box → hand-crop enlarge factor (Zoo uses 3 with rotation; a bit tighter
/// is enough without rotation and keeps more resolution on the hand).
const PALM_CROP_ENLARGE: f32 = 2.7;
const HAND_CROP_ENLARGE: f32 = 1.8;

/// MediaPipe palm detector (192² SSD, 2016 anchors).
pub struct PalmDetector {
    session: Session,
    anchors: Vec<(f32, f32)>,
}

impl PalmDetector {
    pub fn load() -> Result<Self> {
        let resolved = registry::find_model("palm_detection")
            .context("palm_detection model not found; run scripts/fetch-models.sh")?;
        let session = build_session()?
            .commit_from_file(&resolved.model_path)
            .with_context(|| format!("load {}", resolved.model_path.display()))?;
        Ok(Self {
            session,
            anchors: gen_anchors(),
        })
    }

    /// Detect palms with score ≥ `score_thresh`, returning at most 2 (after NMS)
    /// in normalized frame coords.
    pub fn detect(
        &mut self,
        rgba: &[u8],
        w: usize,
        h: usize,
        stride: usize,
        score_thresh: f32,
    ) -> Result<Vec<Palm>> {
        let input = letterbox_192(rgba, w, h, stride);
        let tensor = Tensor::from_array(([1i64, PALM_SIZE as i64, PALM_SIZE as i64, 3], input))?;
        let outputs = self.session.run(ort::inputs![INPUT => tensor])?;

        // tf2onnx names the outputs Identity/Identity_1; bind by length
        // (regressors = 2016×18, scores = 2016×1) rather than trusting order.
        let mut regressors: Option<Vec<f32>> = None;
        let mut scores: Option<Vec<f32>> = None;
        for name in ["Identity", "Identity_1"] {
            let Some(value) = outputs.get(name) else {
                continue;
            };
            let data = value.try_extract_tensor::<f32>()?.1;
            match data.len() {
                n if n == self.anchors.len() => scores = Some(data.to_vec()),
                n if n == self.anchors.len() * 18 => regressors = Some(data.to_vec()),
                _ => {}
            }
        }
        let (Some(reg), Some(score)) = (regressors, scores) else {
            anyhow::bail!("palm_detection: unexpected output shapes");
        };

        // Letterbox mapping back to normalized frame coords.
        let l = w.max(h) as f32;
        let padx = (l - w as f32) / 2.0;
        let pady = (l - h as f32) / 2.0;
        let inv = PALM_SIZE as f32;

        let mut palms = Vec::new();
        for (i, &(ax, ay)) in self.anchors.iter().enumerate() {
            let s = sigmoid(score[i]);
            if s < score_thresh {
                continue;
            }
            let r = &reg[i * 18..i * 18 + 4];
            // Box center/size are deltas in 192px, normalized then anchor-shifted.
            let cxn = r[0] / inv + ax;
            let cyn = r[1] / inv + ay;
            let wn = r[2] / inv;
            let hn = r[3] / inv;
            // padded-square normalized → frame normalized
            let cx = (cxn * l - padx) / w as f32;
            let cy = (cyn * l - pady) / h as f32;
            let pw = wn * l / w as f32;
            let ph = hn * l / h as f32;
            palms.push(Palm {
                cx,
                cy,
                w: pw,
                h: ph,
                score: s,
            });
        }
        Ok(nms_palms(palms, 0.3, 2))
    }
}

impl Palm {
    /// A square crop around this palm for the landmark stage (MediaPipe's
    /// palm-box enlarge, shifted up toward the fingers).
    pub fn crop(&self, w: usize, h: usize) -> CropBox {
        let wpx = self.w * w as f32;
        let hpx = self.h * h as f32;
        let side_px = wpx.max(hpx) * PALM_CROP_ENLARGE;
        CropBox {
            cx: self.cx,
            cy: self.cy - 0.4 * self.h,
            side: side_px / w as f32,
        }
    }
}

/// MediaPipe hand landmark model (224² crop → 21 keypoints).
pub struct HandLandmarker {
    session: Session,
}

impl HandLandmarker {
    pub fn load() -> Result<Self> {
        let resolved = registry::find_model("hand_landmark")
            .context("hand_landmark model not found; run scripts/fetch-models.sh")?;
        let session = build_session()?
            .commit_from_file(&resolved.model_path)
            .with_context(|| format!("load {}", resolved.model_path.display()))?;
        Ok(Self { session })
    }

    /// Regress landmarks from the square `crop` of the frame.
    pub fn infer(
        &mut self,
        rgba: &[u8],
        w: usize,
        h: usize,
        stride: usize,
        crop: CropBox,
    ) -> Result<HandLandmarks> {
        let (x0, y0, side_px) = crop.to_px(w, h);
        let input = sample_crop_224(rgba, w, h, stride, x0, y0, side_px);
        let tensor = Tensor::from_array(([1i64, HAND_SIZE as i64, HAND_SIZE as i64, 3], input))?;
        let outputs = self.session.run(ort::inputs![INPUT => tensor])?;

        // Four outputs (Identity..Identity_3): screen landmarks (63, values in
        // 0..224), confidence (1), handedness (1), world landmarks (63, metric).
        // Pick the screen-landmark output by magnitude and confidence by order.
        let mut screen: Option<Vec<f32>> = None;
        let mut confidence: Option<f32> = None;
        for name in ["Identity", "Identity_1", "Identity_2", "Identity_3"] {
            let Some(value) = outputs.get(name) else {
                continue;
            };
            let data = value.try_extract_tensor::<f32>()?.1;
            match data.len() {
                63 => {
                    let max_abs = data.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
                    if max_abs > 5.0 {
                        screen = Some(data.to_vec());
                    }
                }
                1 if confidence.is_none() => confidence = Some(data[0]),
                _ => {}
            }
        }
        let Some(lm) = screen else {
            anyhow::bail!("hand_landmark: no screen-landmark output");
        };

        let scale = side_px / HAND_SIZE as f32;
        let mut points = [[0.0f32; 3]; 21];
        for (k, p) in points.iter_mut().enumerate() {
            let lx = lm[k * 3];
            let ly = lm[k * 3 + 1];
            let lz = lm[k * 3 + 2];
            p[0] = (x0 + lx * scale) / w as f32;
            p[1] = (y0 + ly * scale) / h as f32;
            p[2] = lz / HAND_SIZE as f32;
        }
        Ok(HandLandmarks {
            points,
            confidence: as_prob(confidence.unwrap_or(0.0)),
        })
    }
}

/// Build the 2016 SSD anchors for the 192² palm model: a 24×24 grid (stride 8,
/// 2 anchors/cell) followed by a 12×12 grid (stride 16, 6 anchors/cell), each
/// anchor at its cell center in normalized coords.
fn gen_anchors() -> Vec<(f32, f32)> {
    let mut anchors = Vec::with_capacity(2016);
    for (grid, count) in [(24usize, 2usize), (12, 6)] {
        for row in 0..grid {
            for col in 0..grid {
                let cx = (col as f32 + 0.5) / grid as f32;
                let cy = (row as f32 + 0.5) / grid as f32;
                for _ in 0..count {
                    anchors.push((cx, cy));
                }
            }
        }
    }
    anchors
}

/// Letterbox an RGBA frame into a 192×192×3 NHWC RGB `0..1` buffer (aspect
/// preserved, centered, black padding) — matching the palm decode's anchors.
fn letterbox_192(rgba: &[u8], w: usize, h: usize, stride: usize) -> Vec<f32> {
    let s = PALM_SIZE;
    let l = w.max(h) as f32;
    let ratio = s as f32 / l;
    let cw = w as f32 * ratio;
    let ch = h as f32 * ratio;
    let left = (s as f32 - cw) / 2.0;
    let top = (s as f32 - ch) / 2.0;
    let mut out = vec![0f32; s * s * 3];
    for yy in 0..s {
        let fy = (yy as f32 - top) / ratio;
        for xx in 0..s {
            let fx = (xx as f32 - left) / ratio;
            if fx < 0.0 || fy < 0.0 || fx >= w as f32 || fy >= h as f32 {
                continue;
            }
            let sx = (fx as usize).min(w - 1);
            let sy = (fy as usize).min(h - 1);
            let p = sy * stride + sx * 4;
            let d = (yy * s + xx) * 3;
            out[d] = rgba[p] as f32 / 255.0;
            out[d + 1] = rgba[p + 1] as f32 / 255.0;
            out[d + 2] = rgba[p + 2] as f32 / 255.0;
        }
    }
    out
}

/// Sample a square crop (px-space `x0,y0,side`) into a 224×224×3 NHWC RGB
/// `0..1` buffer; out-of-frame pixels are black.
fn sample_crop_224(
    rgba: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    x0: f32,
    y0: f32,
    side: f32,
) -> Vec<f32> {
    let s = HAND_SIZE;
    let mut out = vec![0f32; s * s * 3];
    for yy in 0..s {
        let fy = y0 + (yy as f32 + 0.5) * side / s as f32 - 0.5;
        for xx in 0..s {
            let fx = x0 + (xx as f32 + 0.5) * side / s as f32 - 0.5;
            if fx < 0.0 || fy < 0.0 || fx >= w as f32 || fy >= h as f32 {
                continue;
            }
            let sx = (fx as usize).min(w - 1);
            let sy = (fy as usize).min(h - 1);
            let p = sy * stride + sx * 4;
            let d = (yy * s + xx) * 3;
            out[d] = rgba[p] as f32 / 255.0;
            out[d + 1] = rgba[p + 1] as f32 / 255.0;
            out[d + 2] = rgba[p + 2] as f32 / 255.0;
        }
    }
    out
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-80.0, 80.0)).exp())
}

/// Pass through if already a probability, else squash logits.
fn as_prob(x: f32) -> f32 {
    if (0.0..=1.0).contains(&x) {
        x
    } else {
        sigmoid(x)
    }
}

/// Greedy NMS over palms by IoU, highest score first, keeping at most `max`.
fn nms_palms(mut palms: Vec<Palm>, iou_thresh: f32, max: usize) -> Vec<Palm> {
    palms.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Palm> = Vec::new();
    'outer: for p in palms {
        for k in &kept {
            if palm_iou(&p, k) > iou_thresh {
                continue 'outer;
            }
        }
        kept.push(p);
        if kept.len() >= max {
            break;
        }
    }
    kept
}

fn palm_iou(a: &Palm, b: &Palm) -> f32 {
    let (ax0, ay0, ax1, ay1) = (
        a.cx - a.w / 2.0,
        a.cy - a.h / 2.0,
        a.cx + a.w / 2.0,
        a.cy + a.h / 2.0,
    );
    let (bx0, by0, bx1, by1) = (
        b.cx - b.w / 2.0,
        b.cy - b.h / 2.0,
        b.cx + b.w / 2.0,
        b.cy + b.h / 2.0,
    );
    let iw = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let ih = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let inter = iw * ih;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_frame(w: usize, h: usize) -> Vec<u8> {
        let mut buf = vec![0u8; w * h * 4];
        for (i, px) in buf.chunks_exact_mut(4).enumerate() {
            let v = ((i % 97) * 2) as u8;
            px.copy_from_slice(&[v, v / 2, 200 - v / 2, 255]);
        }
        buf
    }

    #[test]
    fn anchors_count_is_2016() {
        assert_eq!(gen_anchors().len(), 2016);
    }

    #[test]
    fn anchor_first_cell_center() {
        let a = gen_anchors();
        assert!((a[0].0 - 0.5 / 24.0).abs() < 1e-6);
        assert!((a[0].1 - 0.5 / 24.0).abs() < 1e-6);
    }

    #[test]
    fn crop_round_trips_to_pixels() {
        let c = CropBox {
            cx: 0.5,
            cy: 0.5,
            side: 0.25,
        };
        let (x0, y0, side) = c.to_px(640, 480);
        assert!((side - 160.0).abs() < 1e-3);
        assert!((x0 - 240.0).abs() < 1e-3);
        assert!((y0 - 160.0).abs() < 1e-3);
    }

    /// Exercises real ONNX inference; no-ops when the models aren't installed.
    #[test]
    fn palm_detector_runs_without_panic() {
        if registry::find_model("palm_detection").is_none() {
            eprintln!("skipping: palm_detection model not installed");
            return;
        }
        let (w, h) = (160usize, 120usize);
        let frame = synthetic_frame(w, h);
        let mut model = PalmDetector::load().expect("load palm");
        let palms = model.detect(&frame, w, h, w * 4, 0.6).expect("detect");
        assert!(palms.len() <= 2);
        for p in &palms {
            assert!(p.score >= 0.6 && p.score <= 1.0);
        }
    }

    #[test]
    fn hand_landmarker_runs_without_panic() {
        if registry::find_model("hand_landmark").is_none() {
            eprintln!("skipping: hand_landmark model not installed");
            return;
        }
        let (w, h) = (160usize, 120usize);
        let frame = synthetic_frame(w, h);
        let mut model = HandLandmarker::load().expect("load hand");
        let crop = CropBox {
            cx: 0.5,
            cy: 0.5,
            side: 0.6,
        };
        let lm = model.infer(&frame, w, h, w * 4, crop).expect("infer");
        assert!(lm.confidence >= 0.0 && lm.confidence <= 1.0);
        for p in &lm.points {
            assert!(p[0].is_finite() && p[1].is_finite());
        }
    }
}
