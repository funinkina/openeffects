//! Rule-based hand-gesture classifier — pure geometry on the 21 MediaPipe hand
//! landmarks, no model. Recognizes the reaction gestures: thumbs up, thumbs
//! down, a two-hand heart, and an open-palm wave. Operating on landmarks (not
//! pixels) makes this lighting- and background-invariant and trivially
//! unit-testable. A wave is keyed off a single open-palm frame; the pipeline's
//! hold-to-confirm debounce supplies the temporal gate.
//!
//! Landmark indices (MediaPipe): 0 wrist; 1-4 thumb (CMC,MCP,IP,TIP);
//! 5-8 index; 9-12 middle; 13-16 ring; 17-20 pinky. Coordinates are normalized
//! with y growing downward.

use super::hand::HandLandmarks;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    ThumbsUp,
    ThumbsDown,
    Heart,
    Wave,
}

impl Gesture {
    /// The reaction id (`shared::dbus::REACTION_IDS`) this gesture triggers.
    pub fn reaction_id(self) -> &'static str {
        match self {
            Gesture::ThumbsUp => "thumbs_up",
            Gesture::ThumbsDown => "thumbs_down",
            Gesture::Heart => "heart",
            Gesture::Wave => "wave",
        }
    }
}

/// The four non-thumb fingers as `(mcp, pip, tip)` landmark indices.
const FINGERS: [(usize, usize, usize); 4] = [(5, 6, 8), (9, 10, 12), (13, 14, 16), (17, 18, 20)];

type P = [f32; 3];

/// Classify a set of detected hands. Two hands are first tested for a heart;
/// otherwise (or on no heart) each hand is tested for a thumb gesture.
pub fn classify(hands: &[HandLandmarks]) -> Option<Gesture> {
    if hands.len() >= 2 && is_heart(&hands[0].points, &hands[1].points) {
        return Some(Gesture::Heart);
    }
    hands
        .iter()
        .find_map(|h| thumb_gesture(&h.points).or_else(|| open_palm(&h.points)))
}

/// An open palm (all four fingers and the thumb extended) → wave. Direction is
/// irrelevant; the pipeline debounce turns a held open hand into one reaction.
fn open_palm(p: &[P; 21]) -> Option<Gesture> {
    if palm_width(p) < 1e-4 {
        return None;
    }
    if !FINGERS.iter().all(|&(m, pi, t)| !curled(p, m, pi, t)) {
        return None;
    }
    // Thumb extended: tip farther from the wrist than its IP joint.
    if dist(p[4], p[0]) <= dist(p[3], p[0]) {
        return None;
    }
    Some(Gesture::Wave)
}

fn thumb_gesture(p: &[P; 21]) -> Option<Gesture> {
    let pw = palm_width(p);
    if pw < 1e-4 {
        return None;
    }
    // All four fingers curled.
    if !FINGERS.iter().all(|&(m, pi, t)| curled(p, m, pi, t)) {
        return None;
    }
    // Thumb extended: tip farther from the wrist than the IP joint, and well
    // clear of the index MCP.
    if dist(p[4], p[0]) <= dist(p[3], p[0]) || dist(p[4], p[5]) < 0.5 * pw {
        return None;
    }
    // Thumb pointing roughly vertical (tip vs thumb MCP).
    let vx = p[4][0] - p[2][0];
    let vy = p[4][1] - p[2][1];
    if vy.abs() <= vx.abs() {
        return None;
    }
    Some(if vy < 0.0 {
        Gesture::ThumbsUp
    } else {
        Gesture::ThumbsDown
    })
}

fn is_heart(a: &[P; 21], b: &[P; 21]) -> bool {
    let scale = (palm_width(a) + palm_width(b)) / 2.0;
    if scale < 1e-4 {
        return false;
    }
    // Thumb tips and index tips of the two hands meet (forming the heart).
    if dist(a[4], b[4]) > 1.2 * scale || dist(a[8], b[8]) > 1.2 * scale {
        return false;
    }
    // Index tips above their thumb tips (heart apex at the bottom).
    if a[8][1] >= a[4][1] || b[8][1] >= b[4][1] {
        return false;
    }
    // Lower fingers mostly curled on each hand (tolerate one stray).
    lower_curled(a) >= 2 && lower_curled(b) >= 2
}

/// How many of {middle, ring, pinky} are curled on a hand.
fn lower_curled(p: &[P; 21]) -> usize {
    [(9, 10, 12), (13, 14, 16), (17, 18, 20)]
        .iter()
        .filter(|&&(m, pi, t)| curled(p, m, pi, t))
        .count()
}

/// A finger is curled when its tip is closer to the wrist than its PIP joint.
fn curled(p: &[P; 21], _mcp: usize, pip: usize, tip: usize) -> bool {
    dist(p[tip], p[0]) < dist(p[pip], p[0])
}

fn palm_width(p: &[P; 21]) -> f32 {
    dist(p[5], p[17])
}

fn dist(a: P, b: P) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(points: [[f32; 3]; 21]) -> HandLandmarks {
        HandLandmarks {
            points,
            confidence: 1.0,
        }
    }

    /// Build landmarks for a fist (all four fingers curled toward the wrist)
    /// with an explicit thumb tip. Wrist at (0.5, 0.8).
    fn curled_hand(thumb_tip: [f32; 3], thumb_mcp: [f32; 3]) -> [[f32; 3]; 21] {
        let mut p = [[0.0f32; 3]; 21];
        p[0] = [0.5, 0.8, 0.0]; // wrist
        // thumb chain (1 CMC, 2 MCP, 3 IP, 4 TIP)
        p[1] = [0.46, 0.76, 0.0];
        p[2] = thumb_mcp;
        p[3] = [
            (thumb_mcp[0] + thumb_tip[0]) / 2.0,
            (thumb_mcp[1] + thumb_tip[1]) / 2.0,
            0.0,
        ];
        p[4] = thumb_tip;
        // Four fingers: MCP high (far from wrist), PIP a bit lower, tip folded
        // back near the wrist so dist(tip,wrist) < dist(pip,wrist).
        let cols = [0.55, 0.52, 0.49, 0.46];
        for (i, &cx) in cols.iter().enumerate() {
            let base = 5 + i * 4;
            p[base] = [cx, 0.65, 0.0]; // MCP
            p[base + 1] = [cx, 0.58, 0.0]; // PIP (highest, far from wrist)
            p[base + 2] = [cx, 0.63, 0.0]; // DIP
            p[base + 3] = [cx, 0.66, 0.0]; // TIP (folded, near wrist)
        }
        p
    }

    #[test]
    fn classifies_thumbs_up() {
        let p = curled_hand([0.41, 0.50, 0.0], [0.43, 0.70, 0.0]);
        assert_eq!(classify(&[hand(p)]), Some(Gesture::ThumbsUp));
    }

    #[test]
    fn classifies_thumbs_down() {
        // Wrist near the top, thumb pointing down (tip below the thumb MCP).
        let mut p = curled_hand([0.41, 0.95, 0.0], [0.43, 0.75, 0.0]);
        // Move the curled fingers above the wrist so they still fold back.
        p[0] = [0.5, 0.55, 0.0];
        let cols = [0.55, 0.52, 0.49, 0.46];
        for (i, &cx) in cols.iter().enumerate() {
            let base = 5 + i * 4;
            p[base] = [cx, 0.70, 0.0]; // MCP
            p[base + 1] = [cx, 0.77, 0.0]; // PIP (far from wrist)
            p[base + 2] = [cx, 0.72, 0.0];
            p[base + 3] = [cx, 0.69, 0.0]; // TIP (near wrist)
        }
        assert_eq!(classify(&[hand(p)]), Some(Gesture::ThumbsDown));
    }

    #[test]
    fn classifies_wave() {
        // All fingers and the thumb extended (tips far from the wrist) → wave.
        let mut p = [[0.0f32; 3]; 21];
        p[0] = [0.5, 0.9, 0.0];
        p[1] = [0.4, 0.85, 0.0];
        p[2] = [0.36, 0.8, 0.0];
        p[3] = [0.33, 0.72, 0.0];
        p[4] = [0.3, 0.64, 0.0];
        let cols = [0.55, 0.52, 0.49, 0.46];
        for (i, &cx) in cols.iter().enumerate() {
            let base = 5 + i * 4;
            p[base] = [cx, 0.65, 0.0];
            p[base + 1] = [cx, 0.5, 0.0];
            p[base + 2] = [cx, 0.4, 0.0];
            p[base + 3] = [cx, 0.3, 0.0]; // tip far from wrist → extended
        }
        assert_eq!(classify(&[hand(p)]), Some(Gesture::Wave));
    }

    #[test]
    fn half_open_hand_is_none() {
        // Fingers extended but thumb folded across the palm → neither wave nor
        // thumb gesture.
        let mut p = [[0.0f32; 3]; 21];
        p[0] = [0.5, 0.9, 0.0];
        p[1] = [0.46, 0.86, 0.0];
        p[2] = [0.44, 0.83, 0.0];
        p[3] = [0.45, 0.82, 0.0];
        p[4] = [0.47, 0.84, 0.0]; // thumb tip near wrist → folded
        let cols = [0.55, 0.52, 0.49, 0.46];
        for (i, &cx) in cols.iter().enumerate() {
            let base = 5 + i * 4;
            p[base] = [cx, 0.65, 0.0];
            p[base + 1] = [cx, 0.5, 0.0];
            p[base + 2] = [cx, 0.4, 0.0];
            p[base + 3] = [cx, 0.3, 0.0]; // extended
        }
        assert_eq!(classify(&[hand(p)]), None);
    }

    #[test]
    fn sideways_thumb_is_none() {
        // Thumb extended horizontally → neither up nor down.
        let p = curled_hand([0.2, 0.69, 0.0], [0.43, 0.70, 0.0]);
        assert_eq!(classify(&[hand(p)]), None);
    }

    #[test]
    fn classifies_two_hand_heart() {
        // Two mirrored hands whose thumb tips and index tips meet near the
        // center, index tips above the thumb tips, lower fingers curled.
        let mk = |sign: f32| {
            let mut p = [[0.0f32; 3]; 21];
            let wrist_x = 0.5 + sign * 0.2;
            p[0] = [wrist_x, 0.8, 0.0];
            // index MCP/PIP and tip near center-top
            p[5] = [0.5 + sign * 0.12, 0.55, 0.0];
            p[6] = [0.5 + sign * 0.08, 0.5, 0.0];
            p[7] = [0.5 + sign * 0.04, 0.46, 0.0];
            p[8] = [0.5 + sign * 0.01, 0.42, 0.0]; // index tip (top, near center)
            // thumb tip just below index tip, near center
            p[2] = [0.5 + sign * 0.13, 0.6, 0.0];
            p[3] = [0.5 + sign * 0.06, 0.52, 0.0];
            p[4] = [0.5 + sign * 0.01, 0.48, 0.0]; // thumb tip below index tip
            // palm width reference (index MCP 5 to pinky MCP 17)
            p[17] = [0.5 + sign * 0.2, 0.6, 0.0];
            // middle/ring/pinky curled (tips near wrist)
            for &(m, pi, t) in &[(9usize, 10usize, 12usize), (13, 14, 16), (17, 18, 20)] {
                let cx = 0.5 + sign * 0.16;
                p[m] = [cx, 0.6, 0.0];
                p[pi] = [cx, 0.55, 0.0]; // PIP far from wrist
                p[t] = [cx, 0.66, 0.0]; // tip near wrist → curled
            }
            p
        };
        let left = hand(mk(-1.0));
        let right = hand(mk(1.0));
        assert_eq!(classify(&[left, right]), Some(Gesture::Heart));
    }

    #[test]
    fn reaction_ids_match_constants() {
        assert_eq!(Gesture::ThumbsUp.reaction_id(), "thumbs_up");
        assert_eq!(Gesture::ThumbsDown.reaction_id(), "thumbs_down");
        assert_eq!(Gesture::Heart.reaction_id(), "heart");
        assert_eq!(Gesture::Wave.reaction_id(), "wave");
    }
}
