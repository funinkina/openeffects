//! Hand tracking across frames: palm detection finds new hands, then each
//! tracked hand's landmarks are run from its previous crop (the cheap path).

use crate::inference::engine::Engine;
use crate::inference::hand::{CropBox, HandLandmarks};

/// Frames between palm re-detections while fewer than two hands are tracked.
const REDETECT_INTERVAL: u64 = 30;
/// Minimum palm score / hand-landmark confidence to accept.
const PALM_SCORE_THRESH: f32 = 0.6;
const HAND_CONF_THRESH: f32 = 0.5;
/// Most hands tracked at once (two needed for the heart gesture).
const MAX_TRACKS: usize = 2;

/// A tracked hand: the crop to sample next frame, and how many consecutive
/// frames the landmark model returned low confidence (dropped after 2).
struct HandTrack {
    crop: CropBox,
    missed: u32,
}

/// Hands tracked in one coordinate space (full frame or center-stage ROI).
#[derive(Default)]
pub struct HandTracker {
    /// Currently tracked hands (≤ `MAX_TRACKS`).
    tracks: Vec<HandTrack>,
    /// Earliest frame palm re-detection may run again.
    redetect_at: u64,
}

impl HandTracker {
    /// Forget every hand, e.g. when the coordinate space changes.
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.redetect_at = 0;
    }

    /// One tracking step on frame `frame`: palm-detect for new hands when
    /// re-detection is due and there is room, then run landmarks per tracked
    /// hand. Returns the confident landmarks.
    pub fn step(
        &mut self,
        engine: &mut Engine,
        frame: u64,
        img: &[u8],
        w: usize,
        h: usize,
        stride: usize,
    ) -> Vec<HandLandmarks> {
        let acquire = frame >= self.redetect_at;
        if self.tracks.len() < MAX_TRACKS && acquire {
            self.redetect_at = frame + REDETECT_INTERVAL;
        }
        track_hands(engine, &mut self.tracks, acquire, img, w, h, stride)
    }
}

/// One tracking step over `tracks`: palm-detect for new hands when `acquire`
/// and there is room, then run landmarks per tracked hand from its previous
/// crop (the cheap path). Confident hands advance; a track is dropped after
/// two consecutive misses. Returns the confident landmarks.
fn track_hands(
    engine: &mut Engine,
    tracks: &mut Vec<HandTrack>,
    acquire: bool,
    img: &[u8],
    w: usize,
    h: usize,
    stride: usize,
) -> Vec<HandLandmarks> {
    if tracks.len() < MAX_TRACKS && acquire {
        // Landmarks only run once palm finds a hand, so on its own the hand
        // model idles out while palm keeps running. Load and touch it with
        // palm so the first gesture never pays a model load on this thread.
        let _ = engine.hand();
        let palms = match engine.palm() {
            Some(model) => model
                .detect(img, w, h, stride, PALM_SCORE_THRESH)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        for palm in palms {
            if tracks.len() >= MAX_TRACKS {
                break;
            }
            let crop = palm.crop(w, h);
            if !tracks.iter().any(|t| crops_overlap(t.crop, crop)) {
                tracks.push(HandTrack { crop, missed: 0 });
            }
        }
    }

    let mut landmarks = Vec::with_capacity(tracks.len());
    let mut updated = Vec::with_capacity(tracks.len());
    for track in tracks.drain(..) {
        let lm = engine
            .hand()
            .and_then(|model| model.infer(img, w, h, stride, track.crop).ok());
        match lm {
            Some(lm) if lm.confidence >= HAND_CONF_THRESH => {
                updated.push(HandTrack {
                    crop: lm.tracking_crop(w, h),
                    missed: 0,
                });
                landmarks.push(lm);
            }
            _ if track.missed + 1 < 2 => updated.push(HandTrack {
                crop: track.crop,
                missed: track.missed + 1,
            }),
            _ => {}
        }
    }
    *tracks = updated;
    landmarks
}

/// Whether two hand crops are close enough to be the same hand (dedup on
/// palm re-detection).
fn crops_overlap(a: CropBox, b: CropBox) -> bool {
    let dx = a.cx - b.cx;
    let dy = a.cy - b.cy;
    (dx * dx + dy * dy).sqrt() < 0.25 * (a.side + b.side)
}
