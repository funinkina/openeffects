//! The shared person mask: segmented on a cadence, temporally smoothed, with
//! tracked hands painted in, then refined per frame into per-pixel weights.

use tracing::warn;

use crate::gesture::tracking::HandTracker;
use crate::inference::engine::Engine;
use crate::inference::models::Mask;

/// Run selfie segmentation every Nth frame; the mask is reused in between.
/// 2 (≈15 Hz at 30 fps) keeps the ONNX call — the per-frame energy cost —
/// off every other frame. The 1-frame hold is hidden by the temporal EMA
/// (model space) and the guided-filter edge re-snap (which re-aligns the
/// mask boundary to the *current* frame's luma every frame, see
/// `refine`).
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

#[derive(Default)]
pub(crate) struct PersonMask {
    pub(crate) mask: Option<Mask>,
    /// Temporal-EMA-smoothed copy of the raw 256² model output, carried
    /// across segmentations to suppress boundary flicker. `None` until the
    /// first segmentation (or after a resolution / coordinate-space change).
    ema: Option<Vec<f32>>,
    /// Whether the cached `mask` was segmented from the center-stage ROI
    /// (true) or the full frame (false); a mismatch forces a re-segment so a
    /// normalized mask sampled in the wrong coordinate space is never reused.
    is_roi: bool,
    /// Bumped on every fresh segmentation; keys the rasterized LUT below.
    pub(crate) generation: u64,
    /// Per-pixel mask weights (0–255) rasterized at canvas resolution.
    /// Rebuilt only when a fresh mask arrives or the canvas size changes,
    /// so the per-frame effects read one byte per pixel instead of
    /// bilinearly sampling the 256² mask once per pass.
    lut: Vec<u8>,
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
    /// `generation` the low-res coefficients were computed for.
    gf_coef_gen: u64,
    /// Hands tracked for the mask, in the coordinate space of the buffer it
    /// is segmented from (full frame or center-stage ROI), so kept apart from
    /// the gesture pipeline's.
    hands: HandTracker,
}

impl PersonMask {
    /// Drop the mask and everything sized to the old frame.
    pub(crate) fn reset(&mut self) {
        self.mask = None;
        self.ema = None;
        self.lut.clear();
        self.lut_w = 0;
        self.lut_h = 0;
        self.gf_low_w = 0;
        self.gf_low_h = 0;
        // Normalized hand crops don't survive a resolution change.
        self.hands.clear();
    }

    /// The refined per-pixel weights, if they match a `w`×`h` canvas.
    pub(crate) fn lut(&self, w: usize, h: usize) -> Option<&[u8]> {
        (self.lut_w == w && self.lut_h == h).then_some(&self.lut[..])
    }

    /// (Re)segment on the segmentation cadence, or whenever no valid mask is
    /// cached for the current coordinate space (full vs ROI).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ensure(
        &mut self,
        engine: &mut Engine,
        frame: u64,
        buf: &[u8],
        w: usize,
        h: usize,
        stride: usize,
        is_roi: bool,
    ) {
        // A coordinate-space flip (full↔ROI) makes the previous mask grid
        // incomparable, so drop the EMA history to avoid blending across spaces.
        let space_changed = self.is_roi != is_roi;
        let stale = self.mask.is_none() || space_changed || frame.is_multiple_of(SEG_INTERVAL);
        if !stale {
            return;
        }
        if space_changed {
            self.ema = None;
            self.hands.clear();
        }
        if let Some(model) = engine.selfie() {
            match model.segment(buf, w, h, stride) {
                Ok(mut mask) => {
                    // Before the EMA, so hand edges are smoothed with the body's.
                    self.paint_hands(engine, frame, buf, w, h, stride, &mut mask);
                    let a = SEG_EMA_ALPHA;
                    match &mut self.ema {
                        Some(e) if e.len() == mask.values.len() => {
                            for (e, &n) in e.iter_mut().zip(&mask.values) {
                                *e = a * n + (1.0 - a) * *e;
                            }
                            mask.values.copy_from_slice(e);
                        }
                        _ => self.ema = Some(mask.values.clone()),
                    }
                    self.mask = Some(mask);
                    self.is_roi = is_roi;
                    self.generation = self.generation.wrapping_add(1);
                }
                Err(err) => warn!(%err, "segmentation failed this frame"),
            }
        }
    }

    /// Merge tracked hands into a fresh segmentation. The selfie model was
    /// trained on torso framing and drops hands held away from the body; the
    /// landmark silhouette is steadier than the mask it corrects, so the union
    /// only fills in. Runs on the same buffer the mask was segmented from, so
    /// the coordinate space (full frame or center-stage ROI) always matches.
    #[allow(clippy::too_many_arguments)]
    fn paint_hands(
        &mut self,
        engine: &mut Engine,
        frame: u64,
        buf: &[u8],
        w: usize,
        h: usize,
        stride: usize,
        mask: &mut Mask,
    ) {
        for lm in self.hands.step(engine, frame, buf, w, h, stride) {
            lm.paint_silhouette(&mut mask.values, mask.size, w, h);
        }
    }

    /// Rasterize the segmentation mask into per-pixel u8 weights at canvas
    /// resolution via a fast guided filter: the soft, image-aligned linear
    /// coefficients `a,b` are computed at 1/`GF_SUBSAMPLE`² resolution (only
    /// on a fresh mask), then `q = a·I + b` is applied at full resolution
    /// against the *current* frame's luma every frame. The full-res luma
    /// carries the sharp edge — it snaps the mask boundary onto the actual
    /// person outline and re-tracks small motion even while the coarse mask
    /// is held between segmentations.
    pub(crate) fn refine(&mut self, img: &[u8], w: usize, h: usize) {
        let Some(mask) = self.mask.as_ref() else {
            return;
        };
        let s = GF_SUBSAMPLE;
        let lw = w.div_ceil(s);
        let lh = h.div_ceil(s);
        let n = lw * lh;

        if self.gf_coef_gen != self.generation || self.gf_low_w != lw || self.gf_low_h != lh {
            self.gf_p_low.resize(n, 0.0);
            self.gf_i_low.resize(n, 0.0);
            // Low-res mask (`p`) and luma guide (`I`, box-averaged per s×s block).
            for ly in 0..lh {
                let v = (ly as f32 + 0.5) / lh as f32;
                let (y0, y1) = (ly * s, (ly * s + s).min(h));
                for lx in 0..lw {
                    let u = (lx as f32 + 0.5) / lw as f32;
                    self.gf_p_low[ly * lw + lx] = mask.sample(u, v).clamp(0.0, 1.0);
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
                    self.gf_i_low[ly * lw + lx] = acc as f32 / (cnt * 65280.0);
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
                &self.gf_i_low,
                &mut mean_i,
                &mut self.gf_tmp,
                &mut self.gf_prefix,
                lw,
                lh,
                r,
            );
            box_f32(
                &self.gf_p_low,
                &mut mean_p,
                &mut self.gf_tmp,
                &mut self.gf_prefix,
                lw,
                lh,
                r,
            );
            for (p, &a) in prod.iter_mut().zip(&self.gf_i_low).take(n) {
                *p = a * a;
            }
            box_f32(
                &prod,
                &mut corr_i,
                &mut self.gf_tmp,
                &mut self.gf_prefix,
                lw,
                lh,
                r,
            );
            for (p, (&a, &b)) in prod
                .iter_mut()
                .zip(self.gf_i_low.iter().zip(&self.gf_p_low))
                .take(n)
            {
                *p = a * b;
            }
            box_f32(
                &prod,
                &mut corr_ip,
                &mut self.gf_tmp,
                &mut self.gf_prefix,
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
                &mut self.gf_a_low,
                &mut self.gf_tmp,
                &mut self.gf_prefix,
                lw,
                lh,
                r,
            );
            box_f32(
                &b,
                &mut self.gf_b_low,
                &mut self.gf_tmp,
                &mut self.gf_prefix,
                lw,
                lh,
                r,
            );
            self.gf_low_w = lw;
            self.gf_low_h = lh;
            self.gf_coef_gen = self.generation;
        }

        // Full-res apply against the live frame luma (every frame).
        self.lut.resize(w * h, 0);
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
                let a = bilerp(&self.gf_a_low, lw, x0, x1, y0, y1, tx, ty);
                let b = bilerp(&self.gf_b_low, lw, x0, x1, y0, y1, tx, ty);
                let p = (y * w + x) * 4;
                let lum = (77 * img[p] as u32 + 150 * img[p + 1] as u32 + 29 * img[p + 2] as u32)
                    as f32
                    / 65280.0;
                let q = a * lum + b;
                // Steepen the localized edge into a thin anti-aliased outline.
                let t = ((q - (0.5 - GF_EDGE_WIDTH)) / (2.0 * GF_EDGE_WIDTH)).clamp(0.0, 1.0);
                let alpha = t * t * (3.0 - 2.0 * t);
                self.lut[y * w + x] = (alpha * 255.0) as u8;
            }
        }
        self.lut_w = w;
        self.lut_h = h;
    }
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
