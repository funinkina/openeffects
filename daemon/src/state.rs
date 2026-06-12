use shared::{
    config::AppState,
    dbus::{VariantMap, bool_value, i32_value, str_value, u32_value},
};

use crate::inference::{EpKind, Tier};
use crate::pipeline::probe::PIPEWIRE_NODE_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStatus {
    Stopped,
    Starting,
    Running,
    #[allow(dead_code)]
    Idle,
    Error,
}

impl DaemonStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Error => "error",
        }
    }

    pub fn can_start(self) -> bool {
        matches!(self, Self::Stopped | Self::Idle | Self::Error)
    }

    pub fn can_stop(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Idle | Self::Error
        )
    }
}

#[derive(Debug, Clone)]
pub struct DaemonState {
    pub status: DaemonStatus,
    pub app: AppState,
    pub active_camera: String,
    pub virtual_camera_name: String,
    pub output_sink: String,
    pub virtual_camera_verified: Option<bool>,
    pub last_error: Option<String>,
    pub ep: EpKind,
    pub tier: Tier,
    pub models_ready: bool,
}

impl DaemonState {
    pub fn new(app: AppState, ep: EpKind, tier: Tier, models_ready: bool) -> Self {
        let active_camera = app.camera.selected.clone();
        Self {
            status: DaemonStatus::Stopped,
            app,
            active_camera,
            virtual_camera_name: PIPEWIRE_NODE_NAME.into(),
            output_sink: "none".into(),
            virtual_camera_verified: None,
            last_error: None,
            ep,
            tier,
            models_ready,
        }
    }

    pub fn capabilities(&self) -> VariantMap {
        let mut values = VariantMap::new();
        values.insert("tier".into(), str_value(self.tier.as_str()));
        values.insert("ep".into(), str_value(self.ep.as_str()));
        values.insert("models_ready".into(), bool_value(self.models_ready));
        values.insert("output_sink".into(), str_value(&self.output_sink));
        values.insert(
            "virtual_camera".into(),
            str_value(&self.virtual_camera_name),
        );
        if let Some(error) = &self.last_error {
            values.insert("error".into(), str_value(error));
        }
        values
    }

    pub fn virtual_camera_info(&self) -> VariantMap {
        let mut values = VariantMap::new();
        values.insert("node_name".into(), str_value(&self.virtual_camera_name));
        values.insert(
            "description".into(),
            str_value("OpenEffects Virtual Camera"),
        );
        values.insert("sink".into(), str_value(&self.output_sink));
        if self.output_sink.starts_with("pipewire:") {
            values.insert("media.class".into(), str_value("Video/Source"));
        }
        if let Some(verified) = self.virtual_camera_verified {
            values.insert("verified".into(), bool_value(verified));
        }
        values
    }

    pub fn effect_params(&self, id: &str) -> Option<VariantMap> {
        let mut values = VariantMap::new();
        match id {
            "center_stage" => {
                values.insert(
                    "enabled".into(),
                    bool_value(self.app.effects.center_stage.enabled),
                );
                values.insert(
                    "zoom".into(),
                    str_value(&self.app.effects.center_stage.zoom),
                );
                values.insert(
                    "mode".into(),
                    str_value(&self.app.effects.center_stage.mode),
                );
                values.insert(
                    "crop_top".into(),
                    u32_value(self.app.effects.center_stage.crop.top),
                );
                values.insert(
                    "crop_bottom".into(),
                    u32_value(self.app.effects.center_stage.crop.bottom),
                );
                values.insert(
                    "crop_left".into(),
                    u32_value(self.app.effects.center_stage.crop.left),
                );
                values.insert(
                    "crop_right".into(),
                    u32_value(self.app.effects.center_stage.crop.right),
                );
            }
            "portrait_blur" => {
                values.insert(
                    "enabled".into(),
                    bool_value(self.app.effects.portrait_blur.enabled),
                );
                values.insert(
                    "strength".into(),
                    u32_value(self.app.effects.portrait_blur.strength as u32),
                );
            }
            "bg_replace" => {
                values.insert(
                    "enabled".into(),
                    bool_value(self.app.effects.bg_replace.enabled),
                );
                values.insert(
                    "background".into(),
                    str_value(&self.app.effects.bg_replace.background),
                );
            }
            "studio_light" => {
                values.insert(
                    "enabled".into(),
                    bool_value(self.app.effects.studio_light.enabled),
                );
                values.insert(
                    "intensity".into(),
                    u32_value(self.app.effects.studio_light.intensity as u32),
                );
                values.insert(
                    "brightness".into(),
                    i32_value(self.app.effects.studio_light.brightness as i32),
                );
                values.insert(
                    "contrast".into(),
                    u32_value(self.app.effects.studio_light.contrast as u32),
                );
            }
            "reactions" => {
                values.insert(
                    "enabled".into(),
                    bool_value(self.app.effects.reactions.enabled),
                );
            }
            _ => return None,
        }
        Some(values)
    }

    pub fn all_effect_state(&self) -> VariantMap {
        let mut values = VariantMap::new();
        for id in shared::dbus::EFFECT_IDS {
            if let Some(params) = self.effect_params(id) {
                for (key, value) in params {
                    values.insert(format!("{id}.{key}"), value);
                }
            }
        }
        values
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) -> bool {
        match id {
            "center_stage" => self.app.effects.center_stage.enabled = on,
            "portrait_blur" => self.app.effects.portrait_blur.enabled = on,
            "bg_replace" => self.app.effects.bg_replace.enabled = on,
            "studio_light" => self.app.effects.studio_light.enabled = on,
            "reactions" => self.app.effects.reactions.enabled = on,
            _ => return false,
        }
        true
    }

    pub fn set_param(&mut self, id: &str, key: &str, value: &zvariant::OwnedValue) -> bool {
        match (id, key) {
            ("center_stage", "zoom") => {
                if let Some(v) = shared::dbus::value_as_string(value) {
                    self.app.effects.center_stage.zoom = v;
                    return true;
                }
            }
            ("center_stage", "mode") => {
                if let Some(v) = shared::dbus::value_as_string(value) {
                    self.app.effects.center_stage.mode = v;
                    return true;
                }
            }
            ("center_stage", "crop_top") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.center_stage.crop.top = v.min(512);
                    return true;
                }
            }
            ("center_stage", "crop_bottom") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.center_stage.crop.bottom = v.min(512);
                    return true;
                }
            }
            ("center_stage", "crop_left") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.center_stage.crop.left = v.min(512);
                    return true;
                }
            }
            ("center_stage", "crop_right") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.center_stage.crop.right = v.min(512);
                    return true;
                }
            }
            ("portrait_blur", "strength") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.portrait_blur.strength = v.min(100) as u8;
                    return true;
                }
            }
            ("bg_replace", "background") => {
                if let Some(v) = shared::dbus::value_as_string(value) {
                    self.app.effects.bg_replace.background = v;
                    return true;
                }
            }
            ("studio_light", "intensity") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.studio_light.intensity = v.min(100) as u8;
                    return true;
                }
            }
            ("studio_light", "brightness") => {
                if let Some(v) = shared::dbus::value_as_i32(value) {
                    self.app.effects.studio_light.brightness = v.clamp(-100, 100) as i8;
                    return true;
                }
            }
            ("studio_light", "contrast") => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    self.app.effects.studio_light.contrast = v.min(100) as u8;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use shared::dbus::{value_as_bool, value_as_u32};

    use super::{DaemonState, DaemonStatus, EpKind, Tier};

    #[test]
    fn state_transition_guards_match_prd() {
        assert!(DaemonStatus::Stopped.can_start());
        assert!(DaemonStatus::Idle.can_start());
        assert!(DaemonStatus::Error.can_start());
        assert!(!DaemonStatus::Running.can_start());

        assert!(DaemonStatus::Running.can_stop());
        assert!(DaemonStatus::Idle.can_stop());
        assert!(!DaemonStatus::Stopped.can_stop());
    }

    #[test]
    fn center_stage_params_include_phase1_crop_controls() {
        let mut state = DaemonState::new(
            shared::config::AppState::default(),
            EpKind::Cpu,
            Tier::T4,
            false,
        );
        assert!(state.set_enabled("center_stage", true));
        assert!(state.set_param("center_stage", "crop_left", &shared::dbus::u32_value(44)));

        let params = state.effect_params("center_stage").unwrap();
        assert_eq!(params.get("enabled").and_then(value_as_bool), Some(true));
        assert_eq!(params.get("crop_left").and_then(value_as_u32), Some(44));
        assert_eq!(params.get("crop_top").and_then(value_as_u32), Some(12));
    }

    #[test]
    fn capabilities_report_inference_engine_state() {
        let state = DaemonState::new(
            shared::config::AppState::default(),
            EpKind::Cpu,
            Tier::T2,
            true,
        );
        let caps = state.capabilities();
        assert_eq!(
            caps.get("ep").and_then(shared::dbus::value_as_string),
            Some("cpu".to_string())
        );
        assert_eq!(
            caps.get("tier").and_then(shared::dbus::value_as_string),
            Some("t2".to_string())
        );
        assert_eq!(caps.get("models_ready").and_then(value_as_bool), Some(true));
    }
}
