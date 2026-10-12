//! Static lookup tables shared across the effect cards: app id, toggle-group
//! option lists and built-in presets.

pub const APP_ID: &str = "io.github.karanshukla.vinoopeneffects";

/// PipeWire node name the daemon advertises the virtual camera under. The
/// preview pipeline targets this to show the processed output. Must match the
/// daemon's `PIPEWIRE_NODE_NAME`.
pub const VCAM_NODE_NAME: &str = "vinoopeneffects";

/// Center Stage framing levels (stored value, display label). "Off" is the
/// card's switch, so it is not a framing choice here.
pub const FRAMING_LEVELS: &[(&str, &str)] = &[
    ("subtle", "Subtle"),
    ("normal", "Normal"),
    ("tight", "Tight"),
];

/// Center Stage tracking mode (stored value, display label).
pub const FRAMING_MODES: &[(&str, &str)] = &[("single", "One face"), ("group", "Whole group")];

/// Background card modes (stored value, display label). These are virtual:
/// they map onto the `portrait_blur` / `bg_replace` enable flags. The card's
/// switch covers "off", so only Blur/Replace are toggle choices.
pub const BG_BLUR: &str = "blur";
pub const BG_REPLACE: &str = "replace";
pub const BG_MODES: &[(&str, &str)] = &[(BG_BLUR, "Blur"), (BG_REPLACE, "Replace")];

/// Blur strength buckets: toggle name (the stored `portrait_blur.strength`
/// value as a string) and display label.
pub const BLUR_LEVELS: &[(&str, &str)] = &[("33", "Low"), ("66", "Medium"), ("100", "High")];

/// Built-in solid-color backgrounds (label, `#RRGGBB`).
pub const BG_PRESETS: &[(&str, &str)] = &[
    ("Charcoal", "#1e1e2e"),
    ("Slate", "#2e3440"),
    ("Deep Blue", "#1b3a5b"),
    ("Forest", "#1f3d2b"),
    ("Plum", "#3b2e4a"),
    ("Warm Gray", "#3a3a3a"),
];

/// Built-in image backgrounds (label, cache filename, embedded PNG bytes).
/// `bg_replace.background` stores a filesystem path for image backgrounds, so
/// these are written to the user's data dir on first use (see
/// `pages::background::ensure_preset_images`).
pub const BG_IMAGE_PRESETS: &[(&str, &str, &[u8])] = &[
    (
        "Modern Living Room",
        "modern-living-room.jpg",
        include_bytes!("../data/backgrounds/modern-living-room.jpg"),
    ),
    (
        "Serene Nature",
        "serene-nature.jpg",
        include_bytes!("../data/backgrounds/serene-nature.jpg"),
    ),
    (
        "Sunset Room",
        "sunset-room.jpg",
        include_bytes!("../data/backgrounds/sunset-room.jpg"),
    ),
    (
        "Wall of Books",
        "wall-of-books.jpg",
        include_bytes!("../data/backgrounds/wall-of-books.jpg"),
    ),
];

/// Emoji reaction buttons shown on the Reactions card (reaction id, glyph,
/// tooltip), in the same order as `shared::dbus::REACTION_IDS`. Clicking one
/// fires a one-shot reverse-rain burst, independent of the gesture toggle.
pub const REACTION_BUTTONS: &[(&str, &str, &str)] = &[
    ("heart", "\u{1F496}", "Heart"),
    ("thumbs_up", "\u{1F44D}", "Thumbs up"),
    ("thumbs_down", "\u{1F44E}", "Thumbs down"),
    ("tada", "\u{1F389}", "Celebrate"),
    ("clap", "\u{1F44F}", "Applause"),
    ("wave", "\u{1F44B}", "Wave"),
    ("joy", "\u{1F602}", "Laughing"),
    ("open_mouth", "\u{1F62E}", "Wow"),
    ("cry", "\u{1F622}", "Crying"),
    ("thinking", "\u{1F914}", "Thinking"),
];

/// Bundled models listed in Preferences (id, display name, purpose).
pub const BUNDLED_MODELS: &[(&str, &str, &str)] = &[
    (
        "selfie_segmentation",
        "MediaPipe Selfie Segmentation",
        "Portrait blur &amp; background replace",
    ),
    ("yunet", "YuNet", "Face detection for Center Stage"),
    (
        "palm_detection",
        "MediaPipe Palm Detection",
        "Hand detection for gesture reactions",
    ),
    (
        "hand_landmark",
        "MediaPipe Hand Landmark",
        "Hand keypoints for gesture reactions",
    ),
];
