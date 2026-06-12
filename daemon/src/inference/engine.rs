//! Concrete model wrappers built on top of the ONNX Runtime session API.
//!
//! Each wrapper owns one `ort::Session` and exposes a single high-level call
//! (segment / detect) that takes an RGBA frame buffer and returns the decoded
//! result. Sessions run on the GStreamer streaming thread, so the wrappers are
//! `Send` (ORT's `Session` is `Send + Sync`) and are held behind a `Mutex` in
//! the effects element.
//!
//! Both models run on the CPU execution provider — the only EP shipped by the
//! `download-binaries` feature — which is more than fast enough for the bundled
//! 256×256 selfie-segmentation (≈0.5 MB) and 640×640 YuNet (≈0.2 MB) models.

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;

use super::registry;

/// MediaPipe Selfie Segmentation: produces a per-pixel foreground probability
/// mask. Input `pixel_values` is NCHW `[1,3,256,256]`, RGB scaled to `0..1`;
/// output `alphas` is `[1,1,256,256]` already in `0..1`.
pub struct SelfieSeg {
    session: Session,
    /// Square inference resolution (256).
    size: usize,
}

impl SelfieSeg {
    pub const INPUT: &'static str = "pixel_values";
    pub const OUTPUT: &'static str = "alphas";

    pub fn load() -> Result<Self> {
        let resolved = registry::find_model("selfie_segmentation")
            .context("selfie_segmentation model not found; run scripts/fetch-models.sh")?;
        let session = Session::builder()?
            .commit_from_file(&resolved.model_path)
            .with_context(|| format!("load {}", resolved.model_path.display()))?;
        Ok(Self { session, size: 256 })
    }

    /// Segment an RGBA frame, returning a `size`×`size` row-major foreground
    /// mask (`0.0` = background, `1.0` = subject).
    pub fn segment(&mut self, rgba: &[u8], w: usize, h: usize, stride: usize) -> Result<Mask> {
        let s = self.size;
        let mut input = vec![0f32; 3 * s * s];
        let (plane, area) = (s * s, s * s);
        for y in 0..s {
            let sy = (y * h / s).min(h - 1);
            for x in 0..s {
                let sx = (x * w / s).min(w - 1);
                let p = sy * stride + sx * 4;
                input[y * s + x] = rgba[p] as f32 / 255.0;
                input[plane + y * s + x] = rgba[p + 1] as f32 / 255.0;
                input[2 * plane + y * s + x] = rgba[p + 2] as f32 / 255.0;
            }
        }
        let tensor = Tensor::from_array(([1i64, 3, s as i64, s as i64], input))?;
        let outputs = self.session.run(ort::inputs![Self::INPUT => tensor])?;
        let (_shape, data) = outputs[Self::OUTPUT].try_extract_tensor::<f32>()?;
        let mut values = vec![0f32; area];
        for (dst, &v) in values.iter_mut().zip(data.iter()) {
            // MediaPipe selfie-seg already applies the final sigmoid, so values
            // are in 0..1; clamp defensively in case a re-export emits logits.
            *dst = if !(0.0..=1.0).contains(&v) {
                1.0 / (1.0 + (-v).exp())
            } else {
                v
            };
        }
        Ok(Mask { size: s, values })
    }
}

/// A square, row-major foreground-probability mask.
pub struct Mask {
    pub size: usize,
    pub values: Vec<f32>,
}

impl Mask {
    /// Bilinearly sample the mask at normalized coords (`u`,`v` in `0..1`).
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let s = self.size as f32;
        let fx = (u * s - 0.5).clamp(0.0, s - 1.0);
        let fy = (v * s - 0.5).clamp(0.0, s - 1.0);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(self.size - 1);
        let y1 = (y0 + 1).min(self.size - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let m = self.size;
        let a = self.values[y0 * m + x0];
        let b = self.values[y0 * m + x1];
        let c = self.values[y1 * m + x0];
        let d = self.values[y1 * m + x1];
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        top + (bot - top) * ty
    }
}

/// A detected face, in normalized frame coordinates (`0..1`).
///
/// `eye_*` come from YuNet's 5-point landmarks: the eye midpoint and the
/// interocular distance. These are expression-invariant (smiling widens the
/// bbox but leaves the eyes put), so center-stage framing uses them instead of
/// the bbox. `eye_dist == 0.0` means landmarks weren't available for this face.
#[derive(Debug, Clone, Copy)]
pub struct Face {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    pub eye_cx: f32,
    pub eye_cy: f32,
    pub eye_dist: f32,
}

/// YuNet face detector (2026may). Input `input` is NCHW `[1,3,640,640]` BGR in
/// `0..255` (no normalization); outputs are anchor-free `cls`/`obj`/`bbox`/`kps`
/// tensors at strides 8/16/32, decoded the same way as OpenCV's `FaceDetectorYN`.
pub struct YuNet {
    session: Session,
    /// Square inference resolution (640).
    size: usize,
}

impl YuNet {
    pub const INPUT: &'static str = "input";
    const STRIDES: [usize; 3] = [8, 16, 32];

    pub fn load() -> Result<Self> {
        let resolved = registry::find_model("yunet")
            .context("yunet model not found; run scripts/fetch-models.sh")?;
        let session = Session::builder()?
            .commit_from_file(&resolved.model_path)
            .with_context(|| format!("load {}", resolved.model_path.display()))?;
        Ok(Self { session, size: 640 })
    }

    /// Detect faces in an RGBA frame, returning boxes in normalized coords whose
    /// score exceeds `score_thresh`, after greedy NMS.
    pub fn detect(
        &mut self,
        rgba: &[u8],
        w: usize,
        h: usize,
        stride: usize,
        score_thresh: f32,
    ) -> Result<Vec<Face>> {
        let s = self.size;
        let plane = s * s;
        let mut input = vec![0f32; 3 * plane];
        // BGR channel order to match the model's training (OpenCV blobFromImage).
        for y in 0..s {
            let sy = (y * h / s).min(h - 1);
            for x in 0..s {
                let sx = (x * w / s).min(w - 1);
                let p = sy * stride + sx * 4;
                input[y * s + x] = rgba[p + 2] as f32; // B
                input[plane + y * s + x] = rgba[p + 1] as f32; // G
                input[2 * plane + y * s + x] = rgba[p] as f32; // R
            }
        }
        let tensor = Tensor::from_array(([1i64, 3, s as i64, s as i64], input))?;
        let outputs = self.session.run(ort::inputs![Self::INPUT => tensor])?;

        let mut faces = Vec::new();
        for stride_px in Self::STRIDES {
            let cls = outputs[format!("cls_{stride_px}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;
            let obj = outputs[format!("obj_{stride_px}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;
            let bbox = outputs[format!("bbox_{stride_px}").as_str()]
                .try_extract_tensor::<f32>()?
                .1;
            // Landmarks are optional: fall back to bbox framing if absent.
            let kps = outputs
                .get(format!("kps_{stride_px}").as_str())
                .and_then(|v| v.try_extract_tensor::<f32>().ok())
                .map(|t| t.1);
            let cols = s / stride_px;
            let rows = s / stride_px;
            for row in 0..rows {
                for col in 0..cols {
                    let idx = row * cols + col;
                    let cls_p = as_prob(cls[idx]);
                    let obj_p = as_prob(obj[idx]);
                    let score = (cls_p * obj_p).sqrt();
                    if score < score_thresh {
                        continue;
                    }
                    let b = &bbox[idx * 4..idx * 4 + 4];
                    let cx = (col as f32 + b[0]) * stride_px as f32 / s as f32;
                    let cy = (row as f32 + b[1]) * stride_px as f32 / s as f32;
                    let fw = b[2].exp() * stride_px as f32 / s as f32;
                    let fh = b[3].exp() * stride_px as f32 / s as f32;
                    // YuNet kps layout: [r_eye, l_eye, nose, r_mouth, l_mouth],
                    // each (x,y), decoded like the bbox center off the anchor.
                    let (eye_cx, eye_cy, eye_dist) = kps
                        .map(|k| {
                            let kp = |i: usize| {
                                let kx = (col as f32 + k[idx * 10 + i * 2]) * stride_px as f32
                                    / s as f32;
                                let ky = (row as f32 + k[idx * 10 + i * 2 + 1]) * stride_px as f32
                                    / s as f32;
                                (kx, ky)
                            };
                            let (rx, ry) = kp(0);
                            let (lx, ly) = kp(1);
                            (
                                (rx + lx) / 2.0,
                                (ry + ly) / 2.0,
                                ((rx - lx).powi(2) + (ry - ly).powi(2)).sqrt(),
                            )
                        })
                        .unwrap_or((0.0, 0.0, 0.0));
                    faces.push(Face {
                        cx,
                        cy,
                        w: fw,
                        h: fh,
                        score,
                        eye_cx,
                        eye_cy,
                        eye_dist,
                    });
                }
            }
        }
        Ok(nms(faces, 0.3))
    }
}

/// Interpret a raw model output as a probability: pass through if already in
/// `0..1`, else squash with a sigmoid (handles models that emit logits).
fn as_prob(x: f32) -> f32 {
    if (0.0..=1.0).contains(&x) {
        x
    } else {
        1.0 / (1.0 + (-x).exp())
    }
}

/// Greedy non-max suppression by IoU, highest score first.
fn nms(mut faces: Vec<Face>, iou_thresh: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    'outer: for f in faces {
        for k in &kept {
            if iou(&f, k) > iou_thresh {
                continue 'outer;
            }
        }
        kept.push(f);
        if kept.len() >= 8 {
            break;
        }
    }
    kept
}

fn iou(a: &Face, b: &Face) -> f32 {
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
    let ix0 = ax0.max(bx0);
    let iy0 = ay0.max(by0);
    let ix1 = ax1.min(bx1);
    let iy1 = ay1.min(by1);
    let iw = (ix1 - ix0).max(0.0);
    let ih = (iy1 - iy0).max(0.0);
    let inter = iw * ih;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic 64×48 RGBA frame: a brighter blob in the center over a dark
    /// background, packed (stride = w*4).
    fn synthetic_frame(w: usize, h: usize) -> Vec<u8> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let cx = w as f32 / 2.0;
                let cy = h as f32 / 2.0;
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let v = if d < w.min(h) as f32 / 4.0 { 200 } else { 20 };
                let p = (y * w + x) * 4;
                buf[p] = v;
                buf[p + 1] = v;
                buf[p + 2] = v;
                buf[p + 3] = 255;
            }
        }
        buf
    }

    /// These exercise real ONNX Runtime inference against the bundled models, so
    /// they no-op when the models aren't installed (CI without fetch-models.sh).
    #[test]
    fn selfie_seg_runs_and_returns_valid_mask() {
        if super::registry::find_model("selfie_segmentation").is_none() {
            eprintln!("skipping: selfie_segmentation model not installed");
            return;
        }
        let (w, h) = (64usize, 48usize);
        let frame = synthetic_frame(w, h);
        let mut model = SelfieSeg::load().expect("load selfie seg");
        let mask = model.segment(&frame, w, h, w * 4).expect("segment");
        assert_eq!(mask.size, 256);
        assert_eq!(mask.values.len(), 256 * 256);
        assert!(mask.values.iter().all(|v| (0.0..=1.0).contains(v)));
        // A center-sampled probability should be a finite number.
        assert!(mask.sample(0.5, 0.5).is_finite());
    }

    #[test]
    fn yunet_runs_without_panic() {
        if super::registry::find_model("yunet").is_none() {
            eprintln!("skipping: yunet model not installed");
            return;
        }
        let (w, h) = (64usize, 48usize);
        let frame = synthetic_frame(w, h);
        let mut model = YuNet::load().expect("load yunet");
        // The synthetic blob isn't a face, so we only assert the decode path
        // runs and returns well-formed (possibly empty) detections.
        let faces = model.detect(&frame, w, h, w * 4, 0.6).expect("detect");
        for f in &faces {
            assert!(f.score >= 0.6 && f.score <= 1.0);
        }
    }
}
