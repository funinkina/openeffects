//! Studio Light card: the effect switch in the header, then an "On you"
//! section (intensity, brightness, contrast) and a "Behind you" section
//! (background brightness). The lift is applied to the person only
//! (foreground) and the background brightness to the complementary area
//! (inverted mask). An "Auto light" switch and strength slider drive the
//! NPU-only low-light model. Turning the switch off disables the sliders.

use adw::prelude::*;
use shared::dbus::{VariantMap, bool_value, value_as_bool, value_as_i32, value_as_u32};

use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{
    EffectCard, SliderCtl, plain_row, section_label, slider_row_i32, slider_row_u32,
};

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    pub card: EffectCard,
    intensity: SliderCtl,
    brightness: SliderCtl,
    contrast: SliderCtl,
    bg_brightness: SliderCtl,
    auto_light: adw::SwitchRow,
    auto_light_handler: gtk::glib::SignalHandlerId,
    auto_strength: SliderCtl,
    /// Dimmed while Studio Light is off.
    rows: Vec<gtk::ListBoxRow>,
}

pub fn build(cmd_tx: &CmdTx) -> Widgets {
    let card = {
        let cmd_tx = cmd_tx.clone();
        EffectCard::new("Studio Light", "display-brightness-symbolic", move |on| {
            let _ = cmd_tx.send(GuiCommand::SetEnabled {
                id: "studio_light".into(),
                on,
            });
        })
    };

    let on_you = plain_row(&section_label("On you"));
    let (intensity_row, intensity) = slider_row_u32(
        "Intensity",
        "",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "intensity",
        cmd_tx,
    );
    let (brightness_row, brightness) = slider_row_i32(
        "Brightness",
        "",
        -100.0,
        100.0,
        1.0,
        "studio_light",
        "brightness",
        cmd_tx,
    );
    let (contrast_row, contrast) = slider_row_u32(
        "Contrast",
        "",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "contrast",
        cmd_tx,
    );
    let auto_section = plain_row(&section_label("Auto light"));
    let auto_light = adw::SwitchRow::builder()
        .title("Brighten dark frames")
        .subtitle("A low-light model; needs an NPU and does nothing without one")
        .build();
    let auto_light_handler = {
        let cmd_tx = cmd_tx.clone();
        auto_light.connect_active_notify(move |row| {
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "studio_light".into(),
                key: "auto_light".into(),
                value: bool_value(row.is_active()),
            });
        })
    };
    let (auto_strength_row, auto_strength) = slider_row_u32(
        "Strength",
        "",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "auto_strength",
        cmd_tx,
    );
    let behind_you = plain_row(&section_label("Behind you"));
    let (bg_row, bg_brightness) = slider_row_i32(
        "Brightness",
        "Darken or brighten the room so you stand out",
        -100.0,
        100.0,
        1.0,
        "studio_light",
        "bg_brightness",
        cmd_tx,
    );

    let rows = vec![
        on_you,
        intensity_row,
        brightness_row,
        contrast_row,
        auto_section,
        auto_light.clone().upcast::<gtk::ListBoxRow>(),
        auto_strength_row,
        behind_you,
        bg_row,
    ];
    for row in &rows {
        card.add_row(row);
    }

    Widgets {
        card,
        intensity,
        brightness,
        contrast,
        bg_brightness,
        auto_light,
        auto_light_handler,
        auto_strength,
        rows,
    }
}

impl Widgets {
    pub fn apply(&self, params: &VariantMap) {
        if let Some(on) = params.get("enabled").and_then(value_as_bool) {
            self.card.set_on(on);
            self.card.set_summary(if on {
                "Lifts you, settles the room"
            } else {
                "Off"
            });
            for row in &self.rows {
                row.set_sensitive(on);
            }
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
        if let Some(v) = params.get("bg_brightness").and_then(value_as_i32) {
            self.bg_brightness.set_value(v as f64);
        }
        if let Some(on) = params.get("auto_light").and_then(value_as_bool)
            && self.auto_light.is_active() != on
        {
            self.auto_light.block_signal(&self.auto_light_handler);
            self.auto_light.set_active(on);
            self.auto_light.unblock_signal(&self.auto_light_handler);
        }
        if let Some(v) = params.get("auto_strength").and_then(value_as_u32) {
            self.auto_strength.set_value(v as f64);
        }
    }
}
