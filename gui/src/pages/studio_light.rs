//! Studio Light page: a master switch plus brightness/contrast/intensity
//! sliders. The lift is applied to the person only (masked in `oe_effects`).
//! Turning the switch off disables the sliders.

use adw::glib;
use adw::prelude::*;
use shared::dbus::{value_as_bool, value_as_i32, value_as_u32, VariantMap};

use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{add_slider_i32, add_slider_u32, pref_group, SliderCtl};

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    switch: adw::SwitchRow,
    switch_handler: glib::SignalHandlerId,
    intensity: SliderCtl,
    brightness: SliderCtl,
    contrast: SliderCtl,
    /// Group dimmed while Studio Light is off.
    controls: adw::PreferencesGroup,
}

pub fn build(cmd_tx: &CmdTx) -> (adw::PreferencesPage, Widgets) {
    let page = adw::PreferencesPage::new();

    // ── Master switch ────────────────────────────────────────────────────────
    let main = pref_group("Studio Light", "Subtly brighten and separate the subject");
    let switch = adw::SwitchRow::builder()
        .title("Studio Light")
        .subtitle("Brighten and add contrast to the subject")
        .build();
    let switch_handler = {
        let cmd_tx = cmd_tx.clone();
        switch.connect_active_notify(move |row| {
            let _ = cmd_tx.send(GuiCommand::SetEnabled {
                id: "studio_light".into(),
                on: row.is_active(),
            });
        })
    };
    main.add(&switch);
    page.add(&main);

    // ── Adjustments ──────────────────────────────────────────────────────────
    let controls = pref_group("Adjustments", "");
    let intensity = add_slider_u32(
        &controls,
        "Intensity",
        "Overall effect strength, 0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "intensity",
        cmd_tx,
    );
    let brightness = add_slider_i32(
        &controls,
        "Brightness",
        "-100 (darker) to 100 (brighter)",
        -100.0,
        100.0,
        1.0,
        "studio_light",
        "brightness",
        cmd_tx,
    );
    let contrast = add_slider_u32(
        &controls,
        "Contrast",
        "0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "contrast",
        cmd_tx,
    );
    page.add(&controls);

    let widgets = Widgets {
        switch,
        switch_handler,
        intensity,
        brightness,
        contrast,
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
        if let Some(v) = params.get("intensity").and_then(value_as_u32) {
            self.intensity.set_value(v as f64);
        }
        if let Some(v) = params.get("brightness").and_then(value_as_i32) {
            self.brightness.set_value(v as f64);
        }
        if let Some(v) = params.get("contrast").and_then(value_as_u32) {
            self.contrast.set_value(v as f64);
        }
    }
}
