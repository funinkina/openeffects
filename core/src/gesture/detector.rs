//! The reactions gesture pipeline, run on a cadence over the original frame:
//! palm-detect (only when fewer than two hands are tracked) → per-hand
//! landmarks (tracked from the previous frame's crop, the cheap path) →
//! rule-based classify, with hold-to-confirm debounce and a post-trigger
//! cooldown.

use super::tracking::HandTracker;
use super::{Gesture, classify};
use crate::inference::engine::Engine;

/// Run the gesture pipeline every Nth frame (offset from the seg/face slots).
const GESTURE_INTERVAL: u64 = 3;
/// Consecutive gesture checks the same gesture must hold before triggering.
const GESTURE_CONFIRM: u32 = 3;
/// Frames to suppress new gesture triggers after one fires (~3 s at 30 fps).
const REACTION_COOLDOWN: u64 = 90;

#[derive(Default)]
pub struct GestureDetector {
    hands: HandTracker,
    /// Gesture held across the last `held_count` checks (debounce).
    held: Option<Gesture>,
    held_count: u32,
    /// No new gesture trigger fires before this frame.
    cooldown_until: u64,
}

impl GestureDetector {
    /// Forget tracked hands and the held gesture (the cooldown survives).
    pub fn reset(&mut self) {
        self.hands.clear();
        self.held = None;
        self.held_count = 0;
    }

    /// Advance on frame `frame`; returns a gesture once it is confirmed and
    /// out of cooldown.
    pub fn step(
        &mut self,
        engine: &mut Engine,
        frame: u64,
        img: &[u8],
        w: usize,
        h: usize,
    ) -> Option<Gesture> {
        if frame % GESTURE_INTERVAL != 2 {
            return None;
        }
        let landmarks = self.hands.step(engine, frame, img, w, h, w * 4);

        match classify(&landmarks) {
            g if g == self.held => self.held_count = self.held_count.saturating_add(1),
            g => {
                self.held = g;
                self.held_count = if g.is_some() { 1 } else { 0 };
            }
        }
        let g = self.held?;
        if self.held_count >= GESTURE_CONFIRM && frame >= self.cooldown_until {
            self.cooldown_until = frame + REACTION_COOLDOWN;
            self.held = None;
            self.held_count = 0;
            return Some(g);
        }
        None
    }
}
