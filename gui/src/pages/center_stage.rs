//! Center Stage card: the effect switch in the header, zoom and tracking-mode
//! segmented controls inside. The header summary reads back the current
//! framing ("Normal zoom · One face").

use adw::prelude::*;
use shared::dbus::{VariantMap, value_as_bool, value_as_string};

use crate::constants::{FRAMING_LEVELS, FRAMING_MODES};
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{EffectCard, ToggleCtl, toggle_row};

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    pub card: EffectCard,
    framing: ToggleCtl,
    mode: ToggleCtl,
    /// Dimmed while Center Stage is off.
    rows: [adw::ActionRow; 2],
}

pub fn build(cmd_tx: &CmdTx) -> Widgets {
    let card = {
        let cmd_tx = cmd_tx.clone();
        EffectCard::new("Center Stage", "zoom-fit-best-symbolic", move |on| {
            let _ = cmd_tx.send(GuiCommand::SetEnabled {
                id: "center_stage".into(),
                on,
            });
        })
    };

    let (zoom_row, framing) = {
        let cmd_tx = cmd_tx.clone();
        toggle_row("Zoom", "", FRAMING_LEVELS, move |value| {
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "center_stage".into(),
                key: "zoom".into(),
                value: shared::dbus::str_value(value),
            });
        })
    };
    let (mode_row, mode) = {
        let cmd_tx = cmd_tx.clone();
        toggle_row("Tracking", "", FRAMING_MODES, move |value| {
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "center_stage".into(),
                key: "mode".into(),
                value: shared::dbus::str_value(value),
            });
        })
    };
    card.add_row(&zoom_row);
    card.add_row(&mode_row);

    Widgets {
        card,
        framing,
        mode,
        rows: [zoom_row, mode_row],
    }
}

impl Widgets {
    pub fn apply(&self, params: &VariantMap) {
        if let Some(on) = params.get("enabled").and_then(value_as_bool) {
            self.card.set_on(on);
            for row in &self.rows {
                row.set_sensitive(on);
            }
        }
        if let Some(zoom) = params.get("zoom").and_then(value_as_string) {
            self.framing.set_value(&zoom);
        }
        if let Some(mode) = params.get("mode").and_then(value_as_string) {
            self.mode.set_value(&mode);
        }
        self.update_summary();
    }

    fn update_summary(&self) {
        if !self.card.is_on() {
            self.card.set_summary("Off");
            return;
        }
        let zoom = label_for(FRAMING_LEVELS, self.framing.group.active_name().as_deref());
        let mode = label_for(FRAMING_MODES, self.mode.group.active_name().as_deref());
        self.card.set_summary(&format!("{zoom} zoom · {mode}"));
    }
}

/// Display label for a stored option value (first option if unknown).
fn label_for(options: &[(&str, &'static str)], value: Option<&str>) -> &'static str {
    options
        .iter()
        .find(|(name, _)| Some(*name) == value)
        .or(options.first())
        .map(|(_, label)| *label)
        .unwrap_or_default()
}
