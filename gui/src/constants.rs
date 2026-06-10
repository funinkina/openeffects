//! Static lookup tables shared across pages: app id, toggle-group option
//! lists, built-in presets, and the view-stack page descriptors.

pub const APP_ID: &str = "org.openeffects.OpenEffects";

/// Center Stage framing levels (stored value, display label). "Off" is the
/// page's master switch, so it is not a framing choice here.
pub const FRAMING_LEVELS: &[(&str, &str)] = &[
    ("subtle", "Subtle"),
    ("normal", "Normal"),
    ("tight", "Tight"),
];

/// Center Stage tracking mode (stored value, display label).
pub const FRAMING_MODES: &[(&str, &str)] = &[("single", "Single Face"), ("group", "Group Framing")];

/// Symbolic icons for [`FRAMING_MODES`], same order.
pub const FRAMING_MODE_ICONS: &[&str] = &["avatar-default-symbolic", "system-users-symbolic"];

/// Background page modes (stored value, display label). These are virtual:
/// they map onto the `portrait_blur` / `bg_replace` enable flags. The page's
/// master switch covers "off", so only Blur/Replace are toggle choices.
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
    ("background", "Background", "view-conceal-symbolic"),
    (
        "studio_light",
        "Studio Light",
        "display-brightness-symbolic",
    ),
    ("reactions", "Reactions", "face-smile-symbolic"),
];
