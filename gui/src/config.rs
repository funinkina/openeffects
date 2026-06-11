//! GUI-local settings, persisted to `~/.config/openeffects/gui.toml`.
//!
//! Separate from the daemon's `AppState` (state.toml): these are client-side
//! behaviour preferences, not pipeline state. Kept tiny and best-effort — a
//! failed load falls back to defaults, a failed save is logged and ignored.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    /// When true, closing the window leaves the daemon running. When false
    /// (and boot-autostart is off), closing the window stops the daemon.
    pub keep_running_in_background: bool,
}

impl GuiConfig {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("openeffects/gui.toml"))
    }

    pub fn load_or_default() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(%err, "failed to parse gui.toml, using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            tracing::warn!("no config dir; cannot save gui.toml");
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&path, text) {
                    tracing::warn!(%err, "failed to write gui.toml");
                }
            }
            Err(err) => tracing::warn!(%err, "failed to serialize gui config"),
        }
    }
}
