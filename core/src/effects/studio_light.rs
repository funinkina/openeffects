//! Studio Light: a `videobalance`-style brightness/contrast lift blended by the
//! person mask, so only the subject is lit (the background keeps its original
//! exposure, or gets its own brightness offset).

/// Contrast-pivot smoothing (≈10 frames to 65%) and the pixel stride the
/// subject's mean luma is sampled at.
const STUDIO_PIVOT_ALPHA: f32 = 0.1;
const STUDIO_PIVOT_STRIDE: usize = 7;

/// EMA of the subject's mean luma (0..1): the pivot contrast scales around,
/// so contrast never changes how bright the face is overall. `None` until
/// the first masked frame.
#[derive(Default)]
pub(crate) struct StudioPivot(Option<f32>);

impl StudioPivot {
    /// This frame's contrast pivot: the subject's mean luma, sampled on a
    /// coarse grid weighted by the mask and EMA-smoothed so it can't flicker.
    /// Pivoting on the subject (not mid-grey) means contrast spreads tones
    /// around how bright you actually are, instead of darkening a dim face.
    /// Falls back to the last pivot (or 0.5) with no subject.
    pub(crate) fn update(&mut self, img: &[u8], lut: &[u8]) -> f32 {
        match subject_mean_luma(img, lut) {
            Some(mean) => {
                let next = match self.0 {
                    Some(prev) => prev + (mean - prev) * STUDIO_PIVOT_ALPHA,
                    None => mean,
                };
                self.0 = Some(next);
                next
            }
            None => self.0.unwrap_or(0.5),
        }
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

/// A `videobalance`-style brightness/contrast lift applied to the person
/// (masked by the foreground mask), with an independent background
/// brightness applied to the complementary area (inverted mask). Per
/// channel, on normalized values:
/// `fg_adjusted = (p − pivot)·contrast + pivot + brightness` (foreground),
/// `bg_adjusted = p + bg_brightness` (background), then
/// `out = blend(lerp(p, fg_adj, m), lerp(p, bg_adj, 1-m), m)`.
/// The transfer depends only on the input value, so it uses 256-entry
/// lookup tables and pure integer math.
pub(crate) fn apply_studio(
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
        adj_fg[p] =
            (((v - pivot) * contrast + pivot + brightness).clamp(0.0, 1.0) * 255.0).round() as u8;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
