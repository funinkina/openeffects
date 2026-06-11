//! Reverse-rain emoji reactions.
//!
//! A reaction is a one-shot *burst* of emoji sprites that float up from the
//! bottom of the frame to the top — like rain in reverse — each with a random
//! horizontal position, rise speed, size, tilt, and a gentle side-to-side sway.
//! Bursts are triggered either manually (a GUI button / the `TriggerReaction`
//! D-Bus method) or, when the reactions effect is enabled, automatically by the
//! hand-gesture pipeline (Phase B).
//!
//! The system is owned by `oe_effects`' per-instance `State`, advanced and
//! composited once per frame from `transform_frame_ip` after every other effect
//! so the emojis always sit on top of the final image. Animation timing is in
//! frames (the camera runs at ~`FPS` = 30); no PTS plumbing is needed.

use shared::dbus::REACTION_IDS;
use tracing::warn;

/// Embedded 128px Noto Emoji PNGs (Apache-2.0), indexed parallel to
/// [`REACTION_IDS`]: thumbs_up, thumbs_down, heart, joy, tada, clap.
static EMOJI_PNG: [&[u8]; REACTION_IDS.len()] = [
    include_bytes!("../../../assets/emoji/emoji_u1f44d.png"),
    include_bytes!("../../../assets/emoji/emoji_u1f44e.png"),
    include_bytes!("../../../assets/emoji/emoji_u2764.png"),
    include_bytes!("../../../assets/emoji/emoji_u1f602.png"),
    include_bytes!("../../../assets/emoji/emoji_u1f389.png"),
    include_bytes!("../../../assets/emoji/emoji_u1f44f.png"),
];

/// Particles spawned per burst.
const BURST_COUNT: u64 = 15;
/// Frames between successive particle spawns within one burst (≈1 s spread).
const SPAWN_STAGGER: u64 = 2;
/// Hard lifetime cap so a stuck particle can never leak.
const MAX_AGE: u64 = 150;
/// Frames over which a freshly spawned particle fades in.
const FADE_IN_FRAMES: f32 = 5.0;
/// Below this normalized y, particles fade out toward the top edge.
const FADE_TOP_START: f32 = 0.15;
/// Base upward speed in normalized units per frame (≈2.5 s to cross the frame).
const RISE_PER_FRAME: f32 = 0.016;
const SIZE_MIN: f32 = 0.08;
const SIZE_MAX: f32 = 0.16;
const TILT_MAX: f32 = 0.4;
const SWAY_AMP_MIN: f32 = 0.01;
const SWAY_AMP_MAX: f32 = 0.03;
const SWAY_FREQ_MIN: f32 = 0.05;
const SWAY_FREQ_MAX: f32 = 0.12;
const TAU: f32 = std::f32::consts::TAU;

/// A decoded emoji texture (straight-alpha RGBA, tightly packed).
struct Sprite {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

impl Sprite {
    /// Bilinear-sample at texel coords `(fx, fy)` into `out` (RGBA, 0..255 f32).
    fn sample(&self, fx: f32, fy: f32, out: &mut [f32; 4]) {
        let x0 = fx.floor().max(0.0) as usize;
        let y0 = fy.floor().max(0.0) as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let (i00, i10, i01, i11) = (
            (y0 * self.w + x0) * 4,
            (y0 * self.w + x1) * 4,
            (y1 * self.w + x0) * 4,
            (y1 * self.w + x1) * 4,
        );
        for (c, o) in out.iter_mut().enumerate() {
            let a = self.rgba[i00 + c] as f32;
            let b = self.rgba[i10 + c] as f32;
            let cc = self.rgba[i01 + c] as f32;
            let d = self.rgba[i11 + c] as f32;
            let top = a + (b - a) * tx;
            let bot = cc + (d - cc) * tx;
            *o = top + (bot - top) * ty;
        }
    }
}

/// One floating emoji.
struct Particle {
    /// Index into [`REACTION_IDS`] / the sprite cache.
    emoji: usize,
    /// Base horizontal position (normalized 0..1) before sway.
    x: f32,
    /// Vertical center (normalized); starts > 1.0 (below frame), decreases.
    y: f32,
    /// Vertical velocity per frame (negative = upward).
    vy: f32,
    /// Sprite height as a fraction of frame height.
    size: f32,
    /// Rotation, radians.
    tilt: f32,
    sway_amp: f32,
    sway_freq: f32,
    sway_phase: f32,
    /// Frame at which this particle becomes visible (burst stagger).
    spawn_frame: u64,
}

/// Tiny deterministic PRNG (xorshift64*) — avoids pulling in a `rand`
/// dependency and keeps tests reproducible.
struct XorShift64(u64);

impl Default for XorShift64 {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Fold a seed into the state so different triggers diverge while a given
    /// `(fresh state, frame)` pair stays reproducible.
    fn mix(&mut self, seed: u64) {
        self.0 ^= seed.wrapping_mul(0x2545_F491_4F6C_DD1D).rotate_left(17);
        if self.0 == 0 {
            self.0 = 1;
        }
    }
}

/// The reaction particle system: lazily-decoded sprites + live particles.
#[derive(Default)]
pub struct Reactions {
    sprites: [Option<Sprite>; REACTION_IDS.len()],
    particles: Vec<Particle>,
    rng: XorShift64,
}

impl Reactions {
    /// Spawn a burst of the reaction named `id` (one of [`REACTION_IDS`]).
    /// Returns `false` for an unknown id (no particles spawned).
    pub fn trigger(&mut self, id: &str, frame: u64) -> bool {
        let Some(emoji) = REACTION_IDS.iter().position(|&r| r == id) else {
            return false;
        };
        self.ensure_sprite(emoji);
        self.rng.mix(frame ^ ((emoji as u64) << 32));
        for i in 0..BURST_COUNT {
            let p = self.make_particle(emoji, frame + i * SPAWN_STAGGER);
            self.particles.push(p);
        }
        true
    }

    /// Whether any particle is alive or pending. The element must keep running
    /// (non-passthrough) while this is true so the burst finishes drawing.
    pub fn active(&self) -> bool {
        !self.particles.is_empty()
    }

    /// Advance physics by one frame and alpha-blend every live particle onto the
    /// tight RGBA buffer `img` (`w`×`h`).
    pub fn step_and_render(&mut self, img: &mut [u8], w: usize, h: usize, frame: u64) {
        self.particles.retain_mut(|p| {
            if frame < p.spawn_frame {
                return true; // pending: keep, not yet moving or drawn
            }
            p.y += p.vy;
            let age = frame - p.spawn_frame;
            p.y > -0.05 && age <= MAX_AGE
        });

        for p in &self.particles {
            if frame < p.spawn_frame {
                continue;
            }
            if let Some(Some(sprite)) = self.sprites.get(p.emoji) {
                render_particle(img, w, h, sprite, p, frame);
            }
        }
    }

    fn ensure_sprite(&mut self, idx: usize) {
        if self.sprites[idx].is_some() {
            return;
        }
        match decode_sprite(EMOJI_PNG[idx]) {
            Ok(sprite) => self.sprites[idx] = Some(sprite),
            Err(err) => warn!(%err, id = REACTION_IDS[idx], "failed to decode emoji sprite"),
        }
    }

    fn make_particle(&mut self, emoji: usize, spawn_frame: u64) -> Particle {
        let r = &mut self.rng;
        Particle {
            emoji,
            x: r.range(0.08, 0.92),
            y: 1.1 + r.range(0.0, 0.15),
            vy: -RISE_PER_FRAME * r.range(0.7, 1.3),
            size: r.range(SIZE_MIN, SIZE_MAX),
            tilt: r.range(-TILT_MAX, TILT_MAX),
            sway_amp: r.range(SWAY_AMP_MIN, SWAY_AMP_MAX),
            sway_freq: r.range(SWAY_FREQ_MIN, SWAY_FREQ_MAX),
            sway_phase: r.range(0.0, TAU),
            spawn_frame,
        }
    }
}

/// Per-particle opacity: ramp in over the first frames, fade out near the top.
fn particle_alpha(p: &Particle, age: f32) -> f32 {
    let fade_in = (age / FADE_IN_FRAMES).clamp(0.0, 1.0);
    let fade_top = (p.y / FADE_TOP_START).clamp(0.0, 1.0);
    fade_in * fade_top
}

/// Composite one (rotated, scaled) emoji sprite onto the frame with straight
/// alpha. For each destination pixel in the particle's bounding box, the screen
/// offset is inverse-rotated into sprite space and bilinearly sampled — the same
/// inverse-mapping trick `oe_effects` uses for crop resampling.
fn render_particle(img: &mut [u8], w: usize, h: usize, sprite: &Sprite, p: &Particle, frame: u64) {
    let age = (frame - p.spawn_frame) as f32;
    let alpha = particle_alpha(p, age);
    if alpha <= 0.001 {
        return;
    }
    let cx = (p.x + p.sway_amp * (p.sway_freq * age + p.sway_phase).sin()) * w as f32;
    let cy = p.y * h as f32;
    let side = (p.size * h as f32).max(1.0); // square sprite side, px
    let (sin, cos) = p.tilt.sin_cos();
    // A square rotated by any angle fits within side·√2.
    let reach = side * 0.5 * std::f32::consts::SQRT_2 + 1.0;
    let x_lo = (cx - reach).floor().clamp(0.0, w as f32) as usize;
    let x_hi = (cx + reach).ceil().clamp(0.0, w as f32) as usize;
    let y_lo = (cy - reach).floor().clamp(0.0, h as f32) as usize;
    let y_hi = (cy + reach).ceil().clamp(0.0, h as f32) as usize;
    let mut texel = [0f32; 4];
    for dy in y_lo..y_hi {
        let oy = dy as f32 + 0.5 - cy;
        for dx in x_lo..x_hi {
            let ox = dx as f32 + 0.5 - cx;
            // screen offset → sprite-space offset (inverse rotation)
            let sx = cos * ox + sin * oy;
            let sy = -sin * ox + cos * oy;
            // sprite space [-side/2, side/2] → texel coords
            let tx = (sx / side + 0.5) * sprite.w as f32 - 0.5;
            let ty = (sy / side + 0.5) * sprite.h as f32 - 0.5;
            if tx < 0.0 || ty < 0.0 || tx > sprite.w as f32 - 1.0 || ty > sprite.h as f32 - 1.0 {
                continue;
            }
            sprite.sample(tx, ty, &mut texel);
            let a = texel[3] / 255.0 * alpha;
            if a <= 0.001 {
                continue;
            }
            let i = (dy * w + dx) * 4;
            for c in 0..3 {
                let dst = img[i + c] as f32;
                img[i + c] = (texel[c] * a + dst * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
            }
            // leave frame alpha as-is
        }
    }
}

fn decode_sprite(bytes: &[u8]) -> anyhow::Result<Sprite> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    Ok(Sprite {
        w: img.width() as usize,
        h: img.height() as usize,
        rgba: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_id_spawns_nothing() {
        let mut r = Reactions::default();
        assert!(!r.trigger("not_a_reaction", 0));
        assert!(!r.active());
    }

    #[test]
    fn burst_spawns_expected_count() {
        let mut r = Reactions::default();
        assert!(r.trigger("tada", 0));
        assert_eq!(r.particles.len(), BURST_COUNT as usize);
        assert!(r.active());
    }

    #[test]
    fn particles_rise_and_eventually_despawn() {
        let mut r = Reactions::default();
        r.trigger("thumbs_up", 0);
        let mut buf = vec![0u8; 64 * 64 * 4];
        let first_y = r.particles[0].y;
        // Step one frame: the first particle (spawn_frame 0) must move upward.
        r.step_and_render(&mut buf, 64, 64, 1);
        assert!(r.particles[0].y < first_y, "particle should rise");
        // Run well past the worst-case lifetime; everything must clear out.
        for f in 2..=400 {
            r.step_and_render(&mut buf, 64, 64, f);
        }
        assert!(!r.active(), "all particles should have despawned");
    }

    #[test]
    fn render_draws_onto_black_frame() {
        let mut r = Reactions::default();
        r.trigger("heart", 0);
        let (w, h) = (96, 96);
        let mut buf = vec![0u8; w * h * 4];
        // Advance enough that a particle is fully on-screen and faded in.
        let mut touched = false;
        for f in 1..=60 {
            buf.iter_mut().for_each(|b| *b = 0);
            r.step_and_render(&mut buf, w, h, f);
            if buf.iter().any(|&b| b != 0) {
                touched = true;
                break;
            }
        }
        assert!(touched, "a visible particle should write non-zero pixels");
    }

    #[test]
    fn same_trigger_is_deterministic() {
        let mut a = Reactions::default();
        let mut b = Reactions::default();
        a.trigger("clap", 42);
        b.trigger("clap", 42);
        assert_eq!(a.particles.len(), b.particles.len());
        for (pa, pb) in a.particles.iter().zip(&b.particles) {
            assert_eq!(pa.x.to_bits(), pb.x.to_bits());
            assert_eq!(pa.vy.to_bits(), pb.vy.to_bits());
            assert_eq!(pa.tilt.to_bits(), pb.tilt.to_bits());
        }
    }
}
