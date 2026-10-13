use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::ConfigError;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    /// Master passthrough switch. When true, every effect is bypassed and the
    /// camera passes through untouched, regardless of per-effect `enabled`
    /// flags (which are preserved so flipping this back off restores them).
    #[serde(default)]
    pub bypass: bool,
    #[serde(default)]
    pub inference: InferenceConfig,
    #[serde(default)]
    pub effects: EffectsConfig,
    #[serde(default)]
    pub camera: CameraConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InferenceProvider {
    #[default]
    Auto,
    Cpu,
    Npu,
}

impl InferenceProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Npu => "npu",
        }
    }
}

impl std::fmt::Display for InferenceProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for InferenceProvider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "cpu" => Ok(Self::Cpu),
            "npu" => Ok(Self::Npu),
            other => {
                anyhow::bail!("unknown inference provider '{other}': expected auto, cpu, or npu")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct InferenceConfig {
    #[serde(default)]
    pub provider: InferenceProvider,
}

// effects

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EffectsConfig {
    #[serde(default)]
    pub center_stage: CenterStageState,
    #[serde(default)]
    pub portrait_blur: PortraitBlurState,
    #[serde(default)]
    pub bg_replace: BgReplaceState,
    #[serde(default)]
    pub studio_light: StudioLightState,
    #[serde(default)]
    pub reactions: EffectState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EffectState {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CenterStageState {
    pub enabled: bool,
    /// "subtle" | "normal" | "tight"
    pub zoom: String,
    /// "single" | "group"
    pub mode: String,
    #[serde(default = "phase1_default_crop")]
    pub crop: CropConfig,
}
impl Default for CenterStageState {
    fn default() -> Self {
        Self {
            enabled: false,
            zoom: "normal".into(),
            mode: "single".into(),
            crop: phase1_default_crop(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortraitBlurState {
    pub enabled: bool,
    /// 0–100
    pub strength: u8,
}
impl Default for PortraitBlurState {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BgReplaceState {
    pub enabled: bool,
    /// path to image, or "" for none
    pub background: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioLightState {
    pub enabled: bool,
    /// 0–100
    pub intensity: u8,
    /// -1.0..1.0 mapped to the masked brightness lift in `oe_effects`
    pub brightness: i8,
    /// 0–100 mapped to the masked contrast in `oe_effects`
    pub contrast: u8,
    /// -100..100 mapped to background brightness in `oe_effects`
    pub bg_brightness: i8,
    /// NPU-only low-light model. Silently does nothing without an NPU.
    #[serde(default)]
    pub auto_light: bool,
    /// 0–100, how much of the model's correction to apply.
    #[serde(default = "default_auto_strength")]
    pub auto_strength: u8,
}

fn default_auto_strength() -> u8 {
    80
}

impl Default for StudioLightState {
    fn default() -> Self {
        Self {
            enabled: false,
            // Visible out of the box: at the default intensity this lifts the
            // subject by ~15% rather than being an identity transform.
            intensity: 50,
            brightness: 30,
            contrast: 50,
            bg_brightness: 0,
            auto_light: false,
            auto_strength: default_auto_strength(),
        }
    }
}

// camera

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CameraConfig {
    /// PipeWire node ID or /dev/videoN path; empty = auto-select
    pub selected: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CropConfig {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

fn phase1_default_crop() -> CropConfig {
    CropConfig {
        top: 12,
        bottom: 12,
        left: 12,
        right: 12,
    }
}

// io helpers

impl AppState {
    pub fn config_path() -> Result<PathBuf, ConfigError> {
        Self::config_path_from(dirs::config_dir())
    }

    pub fn config_path_from(config_dir: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
        config_dir
            .map(|d| d.join("vinoopeneffects/state.toml"))
            .ok_or(ConfigError::NoConfigDir)
    }

    pub fn load_or_default() -> Result<Self, ConfigError> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let state: Self = toml::from_str(&text)?;
        Ok(state)
    }

    pub fn load_or_default_from(path: PathBuf) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let state: Self = toml::from_str(&text)?;
        Ok(state)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::config_path()?;
        self.save_to(path)
    }

    pub fn save_to(&self, path: PathBuf) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }
}

// tests

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[effects.center_stage]
enabled = true
zoom = "tight"
mode = "group"
crop = { top = 12, bottom = 12, left = 12, right = 12 }

[effects.portrait_blur]
enabled = false
strength = 75

[effects.bg_replace]
enabled = false
background = ""

[effects.studio_light]
enabled = false
intensity = 50
brightness = 30
contrast = 50
bg_brightness = 0

[effects.reactions]
enabled = false

[camera]
selected = ""
"#;

    #[test]
    fn toml_round_trip() {
        let state: AppState = toml::from_str(SAMPLE_TOML).expect("parse failed");
        assert!(state.effects.center_stage.enabled);
        assert_eq!(state.effects.center_stage.zoom, "tight");
        assert_eq!(state.effects.center_stage.crop.left, 12);
        assert_eq!(state.effects.portrait_blur.strength, 75);

        let serialized = toml::to_string_pretty(&state).expect("serialize failed");
        let reparsed: AppState = toml::from_str(&serialized).expect("re-parse failed");
        assert_eq!(reparsed.effects.center_stage.zoom, "tight");
        assert_eq!(reparsed.effects.portrait_blur.strength, 75);
    }

    #[test]
    fn missing_file_returns_default() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let state = AppState::load_or_default_from(tmp.path().join("missing/state.toml")).unwrap();
        assert!(!state.effects.reactions.enabled);
        assert_eq!(state.effects.portrait_blur.strength, 50);
        assert_eq!(state.effects.center_stage.zoom, "normal");
        assert_eq!(state.effects.center_stage.crop.top, 12);
    }

    #[test]
    fn save_creates_parent_dirs() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("vinoopeneffects/state.toml");

        let state = AppState::default();
        state.save_to(config_path.clone()).unwrap();

        assert!(config_path.exists());
        let loaded: AppState =
            toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert!(!loaded.effects.reactions.enabled);
    }

    #[test]
    fn bypass_defaults_off_and_round_trips() {
        let state = AppState::default();
        assert!(!state.bypass);

        let mut on = AppState {
            bypass: true,
            ..Default::default()
        };
        on.effects.portrait_blur.enabled = true;
        let text = toml::to_string_pretty(&on).unwrap();
        let reparsed: AppState = toml::from_str(&text).unwrap();
        assert!(reparsed.bypass);
        // Per-effect state is preserved alongside bypass.
        assert!(reparsed.effects.portrait_blur.enabled);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let partial = r#"
[effects.portrait_blur]
enabled = true
strength = 90
"#;
        let state: AppState = toml::from_str(partial).expect("parse failed");
        assert!(state.effects.portrait_blur.enabled);
        assert_eq!(state.effects.portrait_blur.strength, 90);
        // Unspecified fields should get serde defaults
        assert!(!state.effects.reactions.enabled);
        assert_eq!(state.effects.center_stage.zoom, "normal");
    }

    #[test]
    fn config_round_trip_with_and_without_inference() {
        let without_inf = r#"
bypass = false
[effects.portrait_blur]
enabled = true
strength = 50
"#;
        let loaded: AppState = toml::from_str(without_inf).expect("parse without inference table");
        assert_eq!(loaded.inference.provider, InferenceProvider::Auto);

        let with_inf = r#"
[inference]
provider = "npu"
"#;
        let loaded_npu: AppState = toml::from_str(with_inf).expect("parse with inference table");
        assert_eq!(loaded_npu.inference.provider, InferenceProvider::Npu);

        let text = toml::to_string_pretty(&loaded_npu).expect("serialize");
        let reparsed: AppState = toml::from_str(&text).expect("reparse");
        assert_eq!(reparsed.inference.provider, InferenceProvider::Npu);
    }

    #[test]
    fn provider_string_parse_and_unknown_rejected() {
        use std::str::FromStr;
        assert_eq!(
            InferenceProvider::from_str("auto").unwrap(),
            InferenceProvider::Auto
        );
        assert_eq!(
            InferenceProvider::from_str("cpu").unwrap(),
            InferenceProvider::Cpu
        );
        assert_eq!(
            InferenceProvider::from_str("npu").unwrap(),
            InferenceProvider::Npu
        );
        assert_eq!(
            InferenceProvider::from_str("  NPU  ").unwrap(),
            InferenceProvider::Npu
        );
        assert!(InferenceProvider::from_str("unknown").is_err());
        assert!(InferenceProvider::from_str("gpu").is_err());
    }
}
