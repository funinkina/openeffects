//! Center Stage page: a master switch plus framing (zoom) and tracking-mode
//! toggle groups. Turning the switch off disables the framing/mode controls.

use adw::glib;
use adw::prelude::*;
use shared::dbus::{VariantMap, value_as_bool, value_as_string};

use crate::constants::{FRAMING_LEVELS, FRAMING_MODE_ICONS, FRAMING_MODES};
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{ToggleCtl, pref_group, toggle_group, toggle_group_stacked};

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    switch: adw::SwitchRow,
    switch_handler: glib::SignalHandlerId,
    framing: ToggleCtl,
    mode: ToggleCtl,
    /// Groups dimmed while Center Stage is off.
    controls: Vec<adw::PreferencesGroup>,
}

pub fn build(cmd_tx: &CmdTx) -> (adw::PreferencesPage, Widgets) {
    let page = adw::PreferencesPage::new();

    // master switch
    let main = pref_group(
        "Center Stage",
        "Keep the subject framed and centered as they move",
    );
    let switch = adw::SwitchRow::builder()
        .title("Center Stage")
        .subtitle("Track and crop to keep the subject in frame")
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

    // framing - zoom
    let framing_group = pref_group("Framing", "How tightly to crop around the subject");
    let framing = {
        let cmd_tx = cmd_tx.clone();
        toggle_group(FRAMING_LEVELS, move |value| {
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "center_stage".into(),
                key: "zoom".into(),
                value: shared::dbus::str_value(value),
            });
        })
    };
    framing_group.add(&framing.group);
    page.add(&framing_group);

    // tracking mode
    let mode_group = pref_group("Mode", "Track one face or frame the whole group");
    let mode = {
        let cmd_tx = cmd_tx.clone();
        toggle_group_stacked(FRAMING_MODES, FRAMING_MODE_ICONS, move |value| {
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "center_stage".into(),
                key: "mode".into(),
                value: shared::dbus::str_value(value),
            });
        })
    };
    mode_group.add(&mode.group);
    page.add(&mode_group);

    let widgets = Widgets {
        switch,
        switch_handler,
        framing,
        mode,
        controls: vec![framing_group, mode_group],
    };
    (page, widgets)
}

impl Widgets {
    pub fn apply(&self, params: &VariantMap) {
        if let Some(on) = params.get("enabled").and_then(value_as_bool) {
            self.switch.block_signal(&self.switch_handler);
            self.switch.set_active(on);
            self.switch.unblock_signal(&self.switch_handler);
            for group in &self.controls {
                group.set_sensitive(on);
            }
        }
        if let Some(zoom) = params.get("zoom").and_then(value_as_string) {
            self.framing.set_value(&zoom);
        }
        if let Some(mode) = params.get("mode").and_then(value_as_string) {
            self.mode.set_value(&mode);
        }
    }
}
