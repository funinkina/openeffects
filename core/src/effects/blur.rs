//! Portrait blur: a background-weighted box blur of a downscaled frame,
//! composited behind the subject.

use super::composite::composite_low;

/// Scratch for the downscaled blur, kept across frames so a blur does no
/// per-frame heap work.
#[derive(Default)]
pub(crate) struct BlurScratch {
    /// The downscaled image and its ping-pong buffer.
    low: Vec<u8>,
    tmp: Vec<u8>,
    /// Prefix-sum scratch for the separable box passes, sized to
    /// `max(w,h)+1` on demand.
    prefix: Vec<i32>,
}

impl BlurScratch {
    /// Blur the background behind the subject in `img`.
    ///
    /// The blur runs on a downscaled copy of the frame, then the composite
    /// bilinearly upsamples it on the fly. A blurred background is
    /// low-frequency by construction, so the result is visually identical at
    /// a fraction of the full-resolution cost (the dominant per-frame load
    /// when blur is on).
    ///
    /// The blur is *background-weighted*: each pixel is premultiplied by its
    /// background weight `255-mask` before the box passes, and the weight is
    /// carried in alpha and blurred alongside, so the composite can normalize
    /// (`rgb·255/alpha`) to a weighted mean that ignores the subject. Blurring
    /// the raw frame instead would smear the person's colors into the
    /// background and halo the edge (#1).
    pub(crate) fn blur_background(
        &mut self,
        img: &mut [u8],
        lut: &[u8],
        w: usize,
        h: usize,
        strength: u32,
    ) {
        let radius = blur_radius(strength);
        let scale = blur_scale(radius);
        let lw = w.div_ceil(scale).max(1);
        let lh = h.div_ceil(scale).max(1);
        downsample_masked(img, lut, w, h, scale, &mut self.low, lw, lh);
        box_blur(
            &mut self.low,
            &mut self.tmp,
            &mut self.prefix,
            lw,
            lh,
            (radius / scale).max(1),
        );
        composite_low(img, &self.low, lw, lh, scale, lut, w, h);
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

#[cfg(test)]
mod tests {
    use super::*;

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
