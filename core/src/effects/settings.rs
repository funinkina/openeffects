pub const DEFAULT_STRENGTH: u32 = 50;
pub const DEFAULT_ZOOM: &str = "normal";
pub const DEFAULT_MODE: &str = "single";

#[derive(Debug, Clone)]
pub struct Settings {
    pub blur_enabled: bool,
    pub blur_strength: u32,
    pub bg_enabled: bool,
    pub bg_path: String,
    pub cs_enabled: bool,
    pub cs_zoom: String,
    pub cs_mode: String,
    pub studio_enabled: bool,
    /// Additive brightness, -1.0..1.0 (videobalance semantics).
    pub studio_brightness: f32,
    /// Contrast multiplier around mid-gray, 0.0..2.0.
    pub studio_contrast: f32,
    /// Background additive brightness, -1.0..1.0. Applied to the inverted
    /// mask so only the background (not the subject) is affected.
    pub studio_bg_brightness: f32,
    /// Reactions effect: when on, the gesture pipeline runs and may
    /// auto-trigger bursts (manual bursts work regardless, via a one-shot
    /// property that doesn't touch this flag).
    pub reactions_enabled: bool,
    /// Auto light (NPU-only low-light model), and how much of its
    /// correction to apply, 0.0..1.0.
    pub auto_light_enabled: bool,
    pub auto_light_strength: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            blur_enabled: false,
            blur_strength: DEFAULT_STRENGTH,
            bg_enabled: false,
            bg_path: String::new(),
            cs_enabled: false,
            cs_zoom: DEFAULT_ZOOM.into(),
            cs_mode: DEFAULT_MODE.into(),
            studio_enabled: false,
            studio_brightness: 0.0,
            studio_contrast: 1.0,
            studio_bg_brightness: 0.0,
            reactions_enabled: false,
            auto_light_enabled: false,
            auto_light_strength: 0.8,
        }
    }
}

impl Settings {
    /// Whether any effect does real work (otherwise the element runs in
    /// passthrough).
    pub fn active(&self) -> bool {
        self.blur_enabled
            || self.bg_enabled
            || self.studio_enabled
            || self.center_stage_active()
            || self.reactions_enabled
            || self.auto_light_enabled
    }

    pub fn center_stage_active(&self) -> bool {
        self.cs_enabled && self.cs_zoom != "off"
    }

    /// Whether the blur/bg-replace composite step should run.
    pub fn needs_composite(&self) -> bool {
        self.blur_enabled || self.bg_enabled
    }

    /// Whether any effect needs the selfie segmentation mask (studio light,
    /// portrait blur, bg replace). Drives the single shared segmentation.
    pub fn needs_mask(&self) -> bool {
        self.studio_enabled || self.blur_enabled || self.bg_enabled
    }
}
