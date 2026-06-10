//! Shared `AdwPreferencesGroup` row builders and reusable toggle-group /
//! slider controls used by the effect pages.
//!
//! [`add_switch`] wires a row's change signal straight to a `GuiCommand` on
//! `cmd_tx` and stashes the row (plus its signal handler, so daemon state
//! updates can be applied without re-triggering the handler) in `switches`
//! for [`crate::app::apply_update`]. [`ToggleCtl`] and [`SliderCtl`] are
//! lighter primitives the bespoke pages own directly.

use std::collections::HashMap;

use adw::glib;
use adw::prelude::*;

use crate::dbus_client::{CmdTx, GuiCommand};

/// effect id -> (enable switch, its notify::active handler)
pub type Switches = HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>;

/// An `AdwToggleGroup` whose selection change is forwarded as a string value.
/// State sync goes through [`ToggleCtl::set_value`], which blocks the change
/// handler so it doesn't echo back to the daemon.
pub struct ToggleCtl {
    pub group: adw::ToggleGroup,
    handler: glib::SignalHandlerId,
}

impl ToggleCtl {
    /// Select the toggle whose name equals `value` (no-op if none matches).
    pub fn set_value(&self, value: &str) {
        self.group.block_signal(&self.handler);
        self.group.set_active_name(Some(value));
        self.group.unblock_signal(&self.handler);
    }
}

/// Build a homogeneous toggle group from `(name, label)` options. `on_change`
/// receives the active toggle's name whenever the user changes the selection.
pub fn toggle_group(
    options: &[(&'static str, &'static str)],
    on_change: impl Fn(&str) + 'static,
) -> ToggleCtl {
    let group = adw::ToggleGroup::builder()
        .homogeneous(true)
        .halign(gtk::Align::Fill)
        .build();
    for (name, label) in options {
        group.add(adw::Toggle::builder().name(*name).label(*label).build());
    }
    finish_toggle_group(group, on_change)
}

/// Build a homogeneous toggle group whose toggles show a symbolic icon
/// stacked above a centered label, one icon per option (same order).
pub fn toggle_group_stacked(
    options: &[(&'static str, &'static str)],
    icons: &[&'static str],
    on_change: impl Fn(&str) + 'static,
) -> ToggleCtl {
    let group = adw::ToggleGroup::builder()
        .homogeneous(true)
        .halign(gtk::Align::Fill)
        .build();
    for (i, (name, label)) in options.iter().enumerate() {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(8)
            .margin_bottom(12)
            .margin_start(8)
            .margin_end(8)
            .build();
        let image = gtk::Image::from_icon_name(icons[i]);
        image.set_pixel_size(32);
        content.append(&image);
        content.append(&gtk::Label::new(Some(label)));
        let toggle = adw::Toggle::builder().name(*name).label(*label).build();
        toggle.set_child(Some(&content));
        group.add(toggle);
    }
    finish_toggle_group(group, on_change)
}

fn finish_toggle_group(group: adw::ToggleGroup, on_change: impl Fn(&str) + 'static) -> ToggleCtl {
    let handler = group.connect_active_notify(move |group| {
        if let Some(name) = group.active_name() {
            on_change(name.as_str());
        }
    });
    ToggleCtl { group, handler }
}

/// A `GtkScale` paired with the signal handler for its `value-changed`
/// signal. State sync goes through [`SliderCtl::set_value`], which blocks the
/// handler so it doesn't echo back to the daemon.
pub struct SliderCtl {
    pub scale: gtk::Scale,
    handler: glib::SignalHandlerId,
}

impl SliderCtl {
    pub fn set_value(&self, value: f64) {
        self.scale.block_signal(&self.handler);
        self.scale.set_value(value);
        self.scale.unblock_signal(&self.handler);
    }
}

fn slider_row(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
) -> gtk::Scale {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
    let scale = gtk::Scale::builder()
        .orientation(gtk::Orientation::Horizontal)
        .adjustment(&adjustment)
        .digits(0)
        .draw_value(true)
        .value_pos(gtk::PositionType::Right)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .width_request(160)
        .build();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_suffix(&scale);
    group.add(&row);
    scale
}

/// Add a slider row reporting an unsigned integer `SetParam`.
#[allow(clippy::too_many_arguments)]
pub fn add_slider_u32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
) -> SliderCtl {
    let scale = slider_row(group, title, subtitle, min, max, step);
    let cmd_tx = cmd_tx.clone();
    let handler = scale.connect_value_changed(move |s| {
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::u32_value(s.value() as u32),
        });
    });
    SliderCtl { scale, handler }
}

/// Add a slider row reporting a signed integer `SetParam`.
#[allow(clippy::too_many_arguments)]
pub fn add_slider_i32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
) -> SliderCtl {
    let scale = slider_row(group, title, subtitle, min, max, step);
    let cmd_tx = cmd_tx.clone();
    let handler = scale.connect_value_changed(move |s| {
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::i32_value(s.value() as i32),
        });
    });
    SliderCtl { scale, handler }
}

pub fn pref_group(title: &str, description: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder()
        .title(title)
        .description(description)
        .build()
}

pub fn add_switch(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    id: &'static str,
    cmd_tx: &CmdTx,
    switches: &mut Switches,
) {
    let row = adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_active_notify(move |row| {
        let _ = cmd_tx.send(GuiCommand::SetEnabled {
            id: id.to_string(),
            on: row.is_active(),
        });
    });
    group.add(&row);
    switches.insert(id, (row, handler));
}
