use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::ConfigError;

// ── Top-level state persisted to ~/.config/openeffects/state.toml ─────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    #[serde(default)]
    pub effects: EffectsConfig,
    #[serde(default)]
    pub camera: CameraConfig,
    #[serde(default)]
    pub pipeline: PipelineConfig,
}

// ── Effects ────────────────────────────────────────────────────────────────────

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
}
impl Default for CenterStageState {
    fn default() -> Self {
        Self {
            enabled: false,
            zoom: "normal".into(),
            mode: "single".into(),
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
    /// -1.0..1.0 mapped to videobalance brightness
    pub brightness: i8,
    /// 0–100 mapped to videobalance contrast
    pub contrast: u8,
}
impl Default for StudioLightState {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 50,
            brightness: 0,
            contrast: 50,
        }
    }
}

// ── Camera ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CameraConfig {
    /// PipeWire node ID or /dev/videoN path; empty = auto-select
    pub selected: String,
}

// ── Pipeline ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PipelineConfig {
    /// "t1" | "t2" | "t3" | "t4" | "" = auto
    pub tier_override: String,
}

// ── Crop config for manual rectangular crop (Phase 1 Center Stage placeholder) ─

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CropConfig {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

// ── I/O helpers ───────────────────────────────────────────────────────────────

impl AppState {
    pub fn config_path() -> Result<PathBuf, ConfigError> {
        Self::config_path_from(dirs::config_dir())
    }

    pub fn config_path_from(config_dir: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
        config_dir
            .map(|d| d.join("openeffects/state.toml"))
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[effects.center_stage]
enabled = true
zoom = "tight"
mode = "group"

[effects.portrait_blur]
enabled = false
strength = 75

[effects.bg_replace]
enabled = false
background = ""

[effects.studio_light]
enabled = false
intensity = 50
brightness = 0
contrast = 50

[effects.reactions]
enabled = false

[camera]
selected = ""

[pipeline]
tier_override = ""
"#;

    #[test]
    fn toml_round_trip() {
        let state: AppState = toml::from_str(SAMPLE_TOML).expect("parse failed");
        assert!(state.effects.center_stage.enabled);
        assert_eq!(state.effects.center_stage.zoom, "tight");
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
    }

    #[test]
    fn save_creates_parent_dirs() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("openeffects/state.toml");

        let state = AppState::default();
        state.save_to(config_path.clone()).unwrap();

        assert!(config_path.exists());
        let loaded: AppState =
            toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert!(!loaded.effects.reactions.enabled);
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
}
