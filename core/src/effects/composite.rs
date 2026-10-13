//! Background replace: `out = fg·mask + bg·(1-mask)` against a user image, a
//! solid color, or (for portrait blur) a low-res blurred background.

use tracing::warn;

use super::resample::sample_bilinear;

/// A background image decoded and rescaled to the current frame size.
struct BgImage {
    path: String,
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

/// The replacement background: a `#RRGGBB` color, or an image cached at the
/// frame size.
#[derive(Default)]
pub(crate) struct Background {
    image: Option<BgImage>,
}

impl Background {
    /// Forget the cached image (path or frame size changed).
    pub(crate) fn clear(&mut self) {
        self.image = None;
    }

    /// Composite `path`'s background behind the subject in `img`.
    pub(crate) fn composite(&mut self, img: &mut [u8], lut: &[u8], path: &str, w: usize, h: usize) {
        self.ensure(path, w, h);
        if let Some(color) = path.strip_prefix('#').and_then(parse_hex_color) {
            composite_color(img, color, lut);
        } else if let Some(bg) = self.image.as_ref().filter(|b| b.w == w && b.h == h) {
            composite_full(img, &bg.rgba, lut);
        }
    }

    /// Refresh the cached image if the configured path changed or no image is
    /// cached at the current size. Solid colors (`#RRGGBB`) need no cache.
    fn ensure(&mut self, path: &str, w: usize, h: usize) {
        if path.is_empty() || path.starts_with('#') {
            return;
        }
        let fresh = self
            .image
            .as_ref()
            .map(|b| b.path == path && b.w == w && b.h == h)
            .unwrap_or(false);
        if fresh {
            return;
        }
        match load_image_rgba(path, w, h) {
            Ok(rgba) => {
                self.image = Some(BgImage {
                    path: path.to_string(),
                    w,
                    h,
                    rgba,
                })
            }
            Err(err) => {
                warn!(%err, path, "failed to load background image");
                self.image = None;
            }
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
            fg[p + c] = ((fg[p + c] as u32 * m + bg[p + c] as u32 * (255 - m) + 127) / 255) as u8;
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
pub(crate) fn composite_low(
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
