//! Static lookup tables shared across pages: app id, dropdown option lists,
//! built-in presets, and the sidebar page descriptors.

pub const APP_ID: &str = "org.openeffects.OpenEffects";

/// (stored value, display label)
pub const ZOOM_LEVELS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("subtle", "Subtle"),
    ("normal", "Normal"),
    ("tight", "Tight"),
];

pub const FRAMING_MODES: &[(&str, &str)] = &[("single", "Single Face"), ("group", "Group Framing")];

/// Built-in solid-color backgrounds (label, `#RRGGBB`).
pub const BG_PRESETS: &[(&str, &str)] = &[
    ("Charcoal", "#1e1e2e"),
    ("Slate", "#2e3440"),
    ("Deep Blue", "#1b3a5b"),
    ("Forest", "#1f3d2b"),
    ("Plum", "#3b2e4a"),
    ("Warm Gray", "#3a3a3a"),
];

/// Bundled models listed in Preferences (id, display name, purpose).
pub const BUNDLED_MODELS: &[(&str, &str, &str)] = &[
    (
        "selfie_segmentation",
        "MediaPipe Selfie Segmentation",
        "Portrait blur &amp; background replace",
    ),
    ("yunet", "YuNet", "Face detection for Center Stage"),
];

/// View-stack pages, shown as titled+icon entries in the `AdwViewSwitcher`:
/// (stack child name, title, symbolic icon name).
pub const NAV_PAGES: &[(&str, &str, &str)] = &[
    ("center_stage", "Center Stage", "zoom-fit-best-symbolic"),
    ("portrait_blur", "Portrait Blur", "view-conceal-symbolic"),
    (
        "studio_light",
        "Studio Light",
        "display-brightness-symbolic",
    ),
    ("reactions", "Reactions", "face-smile-symbolic"),
    ("camera", "Camera", "camera-web-symbolic"),
];
