//! Auto light: an NPU-only low-light enhancer whose gain map brightens dark
//! frames, gated off when the frame is already well lit.

use crate::inference::engine::Engine;
use crate::inference::models::Mask;

/// Run the enhancer every Nth frame (≈7.5 Hz at 30 fps) and smooth its gain
/// map and on/off gate in between.
const AUTO_LIGHT_INTERVAL: u64 = 4;
const AUTO_GAIN_ALPHA: f32 = 0.4;
const AUTO_GATE_ALPHA: f32 = 0.15;
/// Mean frame luma at/below which auto light is fully on, and at/above
/// which it is off, so a well-lit frame passes through untouched.
const AUTO_GATE_DARK: f32 = 0.25;
const AUTO_GATE_BRIGHT: f32 = 0.55;

#[derive(Default)]
pub(crate) struct AutoLightState {
    /// The smoothed low-res gain map and on/off gate (0..1).
    pub(crate) gain: Option<Mask>,
    pub(crate) gate: f32,
}

impl AutoLightState {
    /// Refresh the enhancer's gain map on its cadence, then apply the
    /// smoothed map to the frame, scaled by `strength` and the darkness gate.
    /// Silently a no-op without an NPU or the model.
    pub(crate) fn apply(
        &mut self,
        engine: &mut Engine,
        frame: u64,
        strength: f32,
        img: &mut [u8],
        w: usize,
        h: usize,
    ) {
        if frame.is_multiple_of(AUTO_LIGHT_INTERVAL) {
            let map = engine
                .auto_light()
                .and_then(|m| m.analyze(img, w, h, w * 4).ok());
            if let Some(map) = map {
                let target = auto_gate_target(map.mean_luma);
                match &mut self.gain {
                    Some(g) if g.values.len() == map.gain.values.len() => {
                        self.gate += (target - self.gate) * AUTO_GATE_ALPHA;
                        for (e, &n) in g.values.iter_mut().zip(&map.gain.values) {
                            *e += (n - *e) * AUTO_GAIN_ALPHA;
                        }
                    }
                    _ => {
                        self.gate = target;
                        self.gain = Some(map.gain);
                    }
                }
            }
        }
        let amount = self.gate * strength;
        if let Some(gain) = &self.gain
            && amount >= 0.01
        {
            apply_gain(img, w, h, gain, amount);
        }
    }
}

/// How far auto light should act for a frame of this mean luma: 1 when
/// dark, 0 when already well lit, linear between.
fn auto_gate_target(mean_luma: f32) -> f32 {
    ((AUTO_GATE_BRIGHT - mean_luma) / (AUTO_GATE_BRIGHT - AUTO_GATE_DARK)).clamp(0.0, 1.0)
}

/// Multiply RGB by `1 + (gain - 1)·amount`, with `gain` bilinearly
/// upsampled from its low-res grid to the frame.
fn apply_gain(img: &mut [u8], w: usize, h: usize, gain: &Mask, amount: f32) {
    let s = gain.size;
    let top = (s - 1) as f32;
    let cols: Vec<(usize, usize, f32)> = (0..w)
        .map(|x| {
            let fx = ((x as f32 + 0.5) / w as f32 * s as f32 - 0.5).clamp(0.0, top);
            let x0 = fx.floor() as usize;
            (x0, (x0 + 1).min(s - 1), fx - x0 as f32)
        })
        .collect();
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / h as f32 * s as f32 - 0.5).clamp(0.0, top);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(s - 1);
        let ty = fy - y0 as f32;
        let (r0, r1) = (&gain.values[y0 * s..], &gain.values[y1 * s..]);
        let row = &mut img[y * w * 4..(y + 1) * w * 4];
        for (px, &(x0, x1, tx)) in row.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
            let a = r0[x0] + (r0[x1] - r0[x0]) * tx;
            let b = r1[x0] + (r1[x1] - r1[x0]) * tx;
            let g = 1.0 + (a + (b - a) * ty - 1.0) * amount;
            for c in &mut px[..3] {
                *c = (*c as f32 * g + 0.5).min(255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_gate_is_off_when_lit_and_on_when_dark() {
        assert_eq!(auto_gate_target(0.7), 0.0);
        assert_eq!(auto_gate_target(0.1), 1.0);
        let mid = auto_gate_target(0.4);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn apply_gain_scales_and_clamps() {
        let gain = Mask {
            size: 4,
            values: vec![2.0; 16],
        };
        let mut img = vec![50, 100, 200, 255, 10, 10, 10, 255];
        apply_gain(&mut img, 2, 1, &gain, 1.0);
        assert_eq!(&img[..4], &[100, 200, 255, 255]);
        assert_eq!(&img[4..8], &[20, 20, 20, 255]);
        // Half strength is a gain of 1.5.
        let mut img = vec![100, 100, 100, 255];
        apply_gain(&mut img, 1, 1, &gain, 0.5);
        assert_eq!(img[0], 150);
    }
}
