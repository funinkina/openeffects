use std::collections::HashMap;

use ksni::{menu::*, Tray};
use shared::dbus::EFFECT_IDS;
use tokio::sync::mpsc;

use crate::{TrayCommand, TrayUpdate};

pub struct OpenEffectsTray {
    pub status: String,
    pub effects: HashMap<String, bool>,
    cmd_tx: mpsc::Sender<TrayCommand>,
}

impl OpenEffectsTray {
    pub fn new(cmd_tx: mpsc::Sender<TrayCommand>) -> Self {
        Self {
            status: "connecting".into(),
            effects: EFFECT_IDS
                .iter()
                .map(|id| (id.to_string(), false))
                .collect(),
            cmd_tx,
        }
    }

    pub fn apply_update(&mut self, update: TrayUpdate) {
        match update {
            TrayUpdate::Status(s) => self.status = s,
            TrayUpdate::AllEffects(state) => {
                for id in EFFECT_IDS {
                    let key = format!("{id}.enabled");
                    if let Some(v) = state.get(&key).and_then(|v| bool::try_from(v).ok()) {
                        self.effects.insert(id.to_string(), v);
                    }
                }
            }
            TrayUpdate::EffectChanged { id, params } => {
                if let Some(v) = params
                    .get("enabled")
                    .and_then(|v| bool::try_from(v).ok())
                {
                    self.effects.insert(id, v);
                }
            }
            TrayUpdate::Error(e) => {
                tracing::warn!("daemon error: {e}");
                self.status = "error".into();
            }
        }
    }

    fn send(&self, cmd: TrayCommand) {
        let _ = self.cmd_tx.blocking_send(cmd);
    }
}

impl Tray for OpenEffectsTray {
    fn icon_name(&self) -> String {
        match self.status.as_str() {
            "running" => "openeffects-active",
            "error" => "openeffects-error",
            _ => "openeffects-idle",
        }
        .into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: format!("OpenEffects — {}", self.status),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        for (id, label) in [
            ("center_stage", "Center Stage"),
            ("portrait_blur", "Portrait Blur"),
            ("bg_replace", "Background Replace"),
            ("studio_light", "Studio Light"),
            ("reactions", "Reactions"),
        ] {
            let checked = self.effects.get(id).copied().unwrap_or(false);
            let effect_id = id.to_string();
            items.push(
                CheckmarkItem {
                    label: label.into(),
                    checked,
                    activate: Box::new(move |this: &mut Self| {
                        let new_state = !this.effects.get(&effect_id).copied().unwrap_or(false);
                        this.effects.insert(effect_id.clone(), new_state);
                        this.send(TrayCommand::SetEnabled {
                            id: effect_id.clone(),
                            on: new_state,
                        });
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        items.push(
            StandardItem {
                label: format!("Status: {}", self.status),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        items.push(
            StandardItem {
                label: "Start pipeline".into(),
                activate: Box::new(|this: &mut Self| this.send(TrayCommand::Start)),
                ..Default::default()
            }
            .into(),
        );

        items.push(
            StandardItem {
                label: "Stop pipeline".into(),
                activate: Box::new(|this: &mut Self| this.send(TrayCommand::Stop)),
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        items.push(
            StandardItem {
                label: "Open OpenEffects…".into(),
                activate: Box::new(|_: &mut Self| {
                    let _ = std::process::Command::new("openeffects").spawn();
                }),
                ..Default::default()
            }
            .into(),
        );

        items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|_: &mut Self| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}
