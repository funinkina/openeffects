//! Center Stage page: a master switch, then one "Framing" group holding the
//! zoom and tracking-mode segmented controls. Turning the switch off disables
//! the framing group.

use adw::glib;
use adw::prelude::*;
use shared::dbus::{VariantMap, value_as_bool, value_as_string};

use crate::constants::{FRAMING_LEVELS, FRAMING_MODES};
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{ToggleCtl, pref_group, toggle_row};

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    switch: adw::SwitchRow,
    switch_handler: glib::SignalHandlerId,
    framing: ToggleCtl,
    mode: ToggleCtl,
    /// Dimmed while Center Stage is off.
    controls: adw::PreferencesGroup,
}

pub fn build(cmd_tx: &CmdTx) -> (adw::PreferencesPage, Widgets) {
    let page = adw::PreferencesPage::new();

    // master switch (no group title: the page tab already says "Center Stage")
    let main = adw::PreferencesGroup::new();
    let switch = adw::SwitchRow::builder()
        .title("Center Stage")
        .subtitle("Keep you framed and centered as you move")
        .build();
    let switch_handler = {
        let cmd_tx = cmd_tx.clone();
        switch.connect_active_notify(move |row| {
            let _ = cmd_tx.send(GuiCommand::SetEnabled {
                id: "center_stage".into(),
                on: row.is_active(),
            });
        })
    };
    main.add(&switch);
    page.add(&main);

    // framing: zoom + tracking mode, one line each
    let controls = pref_group("Framing", "");
    let (zoom_row, framing) = {
        let cmd_tx = cmd_tx.clone();
        toggle_row(
            "Zoom",
            "How tightly to crop",
            FRAMING_LEVELS,
            move |value| {
                let _ = cmd_tx.send(GuiCommand::SetParam {
                    id: "center_stage".into(),
                    key: "zoom".into(),
                    value: shared::dbus::str_value(value),
                });
            },
        )
    };
    let (mode_row, mode) = {
        let cmd_tx = cmd_tx.clone();
        toggle_row(
            "Tracking",
            "Follow one face or frame everyone",
            FRAMING_MODES,
            move |value| {
                let _ = cmd_tx.send(GuiCommand::SetParam {
                    id: "center_stage".into(),
                    key: "mode".into(),
                    value: shared::dbus::str_value(value),
                });
            },
        )
    };
    controls.add(&zoom_row);
    controls.add(&mode_row);
    page.add(&controls);

    let widgets = Widgets {
        switch,
        switch_handler,
        framing,
        mode,
        controls,
    };
    (page, widgets)
}

impl Widgets {
    pub fn apply(&self, params: &VariantMap) {
        if let Some(on) = params.get("enabled").and_then(value_as_bool) {
            self.switch.block_signal(&self.switch_handler);
            self.switch.set_active(on);
            self.switch.unblock_signal(&self.switch_handler);
            self.controls.set_sensitive(on);
        }
        if let Some(zoom) = params.get("zoom").and_then(value_as_string) {
            self.framing.set_value(&zoom);
        }
        if let Some(mode) = params.get("mode").and_then(value_as_string) {
            self.mode.set_value(&mode);
        }
    }
}
