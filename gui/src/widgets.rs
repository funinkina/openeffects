//! Shared `AdwPreferencesGroup` row builders used by every effect page.
//!
//! Each `add_*` helper wires a row's change signal straight to a
//! `GuiCommand` on `cmd_tx` and stashes the row (plus its signal handler, so
//! state updates from the daemon can be applied without re-triggering the
//! handler) in `switches`/`params` for [`crate::app::apply_update`].

use std::collections::HashMap;

use adw::glib;
use adw::prelude::*;

use crate::dbus_client::{CmdTx, GuiCommand};

/// A combo-row parameter: maps `(stored_value, label)` pairs to a selection index.
pub struct ComboParam {
    pub row: adw::ComboRow,
    pub options: &'static [(&'static str, &'static str)],
    pub handler: glib::SignalHandlerId,
}

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
    Combo(ComboParam),
    Spin(SpinParam),
}

/// effect id -> (enable switch, its notify::active handler)
pub type Switches = HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>;
/// "{effect_id}.{param_key}" -> param widget
pub type Params = HashMap<String, ParamWidget>;

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

pub fn add_combo(
    group: &adw::PreferencesGroup,
    title: &str,
    options: &'static [(&'static str, &'static str)],
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    params: &mut Params,
) {
    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
    let model = gtk::StringList::new(&labels);
    let row = adw::ComboRow::builder().title(title).model(&model).build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_selected_notify(move |row| {
        let Some((value, _)) = options.get(row.selected() as usize) else {
            return;
        };
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::str_value(*value),
        });
    });
    group.add(&row);
    params.insert(
        format!("{id}.{key}"),
        ParamWidget::Combo(ComboParam {
            row,
            options,
            handler,
        }),
    );
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
