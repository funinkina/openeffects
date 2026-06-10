//! Shared `AdwPreferencesGroup` row builders and a reusable toggle-group
//! control used by the effect pages.
//!
//! The `add_*` helpers wire a row's change signal straight to a `GuiCommand`
//! on `cmd_tx` and stash the row (plus its signal handler, so daemon state
//! updates can be applied without re-triggering the handler) in
//! `switches`/`params` for [`crate::app::apply_update`]. [`ToggleCtl`] is a
//! lighter primitive the bespoke pages own directly.

use std::collections::HashMap;

use adw::glib;
use adw::prelude::*;

use crate::dbus_client::{CmdTx, GuiCommand};

/// A spin-row parameter holding either an unsigned or signed integer.
pub enum SpinParam {
    U32 {
        row: adw::SpinRow,
        handler: glib::SignalHandlerId,
    },
    I32 {
        row: adw::SpinRow,
        handler: glib::SignalHandlerId,
    },
}

pub enum ParamWidget {
    Spin(SpinParam),
}

/// effect id -> (enable switch, its notify::active handler)
pub type Switches = HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>;
/// "{effect_id}.{param_key}" -> param widget
pub type Params = HashMap<String, ParamWidget>;

/// An `AdwToggleGroup` whose selection change is forwarded as a string value.
/// State sync goes through [`ToggleCtl::set_value`] / [`ToggleCtl::clear`],
/// which block the change handler so they don't echo back to the daemon.
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

/// Build a homogeneous toggle group from `(name, label)` options, optionally
/// with a symbolic icon per option. `on_change` receives the active toggle's
/// name whenever the user changes the selection.
pub fn toggle_group(
    options: &[(&'static str, &'static str)],
    icons: Option<&[&'static str]>,
    on_change: impl Fn(&str) + 'static,
) -> ToggleCtl {
    let group = adw::ToggleGroup::builder()
        .homogeneous(true)
        .halign(gtk::Align::Fill)
        .build();
    for (i, (name, label)) in options.iter().enumerate() {
        let mut builder = adw::Toggle::builder().name(*name).label(*label);
        if let Some(icons) = icons {
            builder = builder.icon_name(icons[i]);
        }
        group.add(builder.build());
    }
    let handler = group.connect_active_notify(move |group| {
        if let Some(name) = group.active_name() {
            on_change(name.as_str());
        }
    });
    ToggleCtl { group, handler }
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

#[allow(clippy::too_many_arguments)]
pub fn add_spin_u32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    params: &mut Params,
) {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
    let row = adw::SpinRow::builder()
        .title(title)
        .subtitle(subtitle)
        .adjustment(&adjustment)
        .digits(0)
        .build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_value_notify(move |row| {
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::u32_value(row.value() as u32),
        });
    });
    group.add(&row);
    params.insert(
        format!("{id}.{key}"),
        ParamWidget::Spin(SpinParam::U32 { row, handler }),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn add_spin_i32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    params: &mut Params,
) {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
    let row = adw::SpinRow::builder()
        .title(title)
        .subtitle(subtitle)
        .adjustment(&adjustment)
        .digits(0)
        .build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_value_notify(move |row| {
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::i32_value(row.value() as i32),
        });
    });
    group.add(&row);
    params.insert(
        format!("{id}.{key}"),
        ParamWidget::Spin(SpinParam::I32 { row, handler }),
    );
}
