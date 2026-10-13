/// Bilinearly sample the RGBA pixel at fractional `fx`,`fy` (clamped to the
/// image) into `out`.
pub(crate) fn sample_bilinear(src: &[u8], w: usize, h: usize, fx: f32, fy: f32, out: &mut [u8]) {
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
