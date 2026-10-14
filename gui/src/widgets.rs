//! Shared `AdwPreferencesGroup` row builders and reusable toggle-group /
//! slider controls used by the effect pages.
//!
//! [`add_switch`] wires a row's change signal straight to a `GuiCommand` on
//! `cmd_tx` and stashes the row (plus its signal handler, so daemon state
//! updates can be applied without re-triggering the handler) in `switches`
//! for [`crate::app::apply_update`]. [`ToggleCtl`] and [`SliderCtl`] are
//! lighter primitives the bespoke pages own directly.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::glib;
use adw::prelude::*;

use crate::dbus_client::{CmdTx, GuiCommand};

/// Trailing debounce for slider drags: one `SetParam` per pause in movement
/// instead of one per tick. Every call makes the daemon save config to disk,
/// reapply the pipeline, and broadcast `EffectChanged`, so an undebounced
/// drag floods it with hundreds of round trips.
const SLIDER_DEBOUNCE: Duration = Duration::from_millis(120);
/// How long after the user last moved a slider daemon echoes are ignored.
/// An `EffectChanged` echo is stale relative to a drag in progress; applying
/// it would snap the slider back to an older value.
const SLIDER_ECHO_HOLDOFF: Duration = Duration::from_millis(1000);

/// effect id -> (enable switch, its notify::active handler)
pub type Switches = HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>;

/// A linked row of grouped `GtkToggleButton`s (segmented control) whose
/// selection change is forwarded as a string value. Each button's widget name
/// is its option name. Built from plain GTK rather than `AdwToggleGroup` so the
/// GUI runs on libadwaita 1.5 (Ubuntu 24.04). State sync goes through
/// [`ToggleCtl::set_value`], which suppresses the change callback so it
/// doesn't echo back to the daemon.
pub struct ToggleCtl {
    pub group: gtk::Box,
    suppress: Rc<Cell<bool>>,
}

impl ToggleCtl {
    /// Select the toggle whose name equals `value` (no-op if none matches).
    pub fn set_value(&self, value: &str) {
        self.suppress.set(true);
        if let Some(btn) = toggle_buttons(&self.group).find(|b| b.widget_name() == value) {
            btn.set_active(true);
        }
        self.suppress.set(false);
    }
}

fn toggle_buttons(group: &gtk::Box) -> impl Iterator<Item = gtk::ToggleButton> {
    let mut child = group.first_child();
    std::iter::from_fn(move || {
        let w = child.take()?;
        child = w.next_sibling();
        Some(w)
    })
    .filter_map(|w| w.downcast::<gtk::ToggleButton>().ok())
}

/// Name of the active toggle in a [`ToggleCtl::group`].
pub fn active_name(group: &gtk::Box) -> Option<glib::GString> {
    toggle_buttons(group)
        .find(|b| b.is_active())
        .map(|b| b.widget_name())
}

/// Build a homogeneous toggle group from `(name, label)` options. `on_change`
/// receives the active toggle's name whenever the user changes the selection.
pub fn toggle_group(
    options: &[(&'static str, &'static str)],
    on_change: impl Fn(&str) + 'static,
) -> ToggleCtl {
    let buttons = options
        .iter()
        .map(|(name, label)| (*name, gtk::ToggleButton::with_label(label)))
        .collect();
    finish_toggle_group(buttons, on_change)
}

/// Build a homogeneous toggle group whose toggles show a symbolic icon
/// stacked above a centered label, one icon per option (same order).
pub fn toggle_group_stacked(
    options: &[(&'static str, &'static str)],
    icons: &[&'static str],
    on_change: impl Fn(&str) + 'static,
) -> ToggleCtl {
    let buttons = options
        .iter()
        .zip(icons)
        .map(|((name, label), icon)| {
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(8)
                .margin_end(8)
                .build();
            let image = gtk::Image::from_icon_name(icon);
            image.set_pixel_size(32);
            content.append(&image);
            content.append(&gtk::Label::new(Some(label)));
            let btn = gtk::ToggleButton::new();
            btn.set_child(Some(&content));
            (*name, btn)
        })
        .collect();
    finish_toggle_group(buttons, on_change)
}

fn finish_toggle_group(
    buttons: Vec<(&'static str, gtk::ToggleButton)>,
    on_change: impl Fn(&str) + 'static,
) -> ToggleCtl {
    let group = gtk::Box::builder()
        .homogeneous(true)
        .halign(gtk::Align::Fill)
        .css_classes(["linked"])
        .build();
    let suppress = Rc::new(Cell::new(false));
    let on_change = Rc::new(on_change);
    let first = buttons.first().map(|(_, b)| b.clone());
    for (name, btn) in buttons {
        btn.set_widget_name(name);
        btn.set_hexpand(true);
        if let Some(first) = first.as_ref().filter(|f| *f != &btn) {
            btn.set_group(Some(first));
        }
        let (suppress, on_change) = (suppress.clone(), on_change.clone());
        btn.connect_toggled(move |b| {
            if b.is_active() && !suppress.get() {
                on_change(name);
            }
        });
        group.append(&btn);
    }
    let ctl = ToggleCtl { group, suppress };
    if let Some(first) = first {
        ctl.suppress.set(true);
        first.set_active(true);
        ctl.suppress.set(false);
    }
    ctl
}

/// A slider row paired with the signal handler for its adjustment's
/// `value-changed` signal. State sync goes through [`SliderCtl::set_value`],
/// which blocks the handler so daemon echoes don't loop back as new commands.
pub struct SliderCtl {
    adjustment: gtk::Adjustment,
    handler: glib::SignalHandlerId,
    /// When the user last moved this slider (drives the echo holdoff).
    last_input: Rc<Cell<Option<Instant>>>,
}

impl SliderCtl {
    pub fn set_value(&self, value: f64) {
        if let Some(at) = self.last_input.get()
            && at.elapsed() < SLIDER_ECHO_HOLDOFF
        {
            return;
        }
        if (self.adjustment.value() - value).abs() < 0.5 {
            return;
        }
        self.adjustment.block_signal(&self.handler);
        self.adjustment.set_value(value);
        self.adjustment.unblock_signal(&self.handler);
    }
}

/// Build a full-width slider row: title + an exact-value spin button on the top
/// line, subtitle below, and a scale spanning the row underneath. The scale and
/// spin button share one adjustment, so dragging, scrolling, typing an exact
/// number, or stepping with the spin arrows all stay in sync. Every row is the
/// same width regardless of title length (the scale hexpands), and bipolar
/// ranges get a center tick at 0 so "neutral" is easy to find.
///
/// A drag/step re-arms a trailing debounce timer, so only the last value of a
/// burst is sent as a `SetParam`; the input timestamp feeds the echo holdoff.
#[allow(clippy::too_many_arguments)]
fn add_slider(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    to_value: impl Fn(f64) -> zvariant::OwnedValue + 'static,
) -> SliderCtl {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);

    let title_label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .build();
    let spin = gtk::SpinButton::builder()
        .adjustment(&adjustment)
        .climb_rate(step)
        .digits(0)
        .numeric(true)
        .valign(gtk::Align::Center)
        .build();
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.append(&title_label);
    header.append(&spin);

    let scale = gtk::Scale::builder()
        .orientation(gtk::Orientation::Horizontal)
        .adjustment(&adjustment)
        .digits(0)
        .draw_value(false)
        .hexpand(true)
        .build();
    // Center detent on bipolar ranges (e.g. -100..100): a visual zero anchor.
    if min < 0.0 && max > 0.0 {
        scale.add_mark(0.0, gtk::PositionType::Bottom, None);
    }

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    row.append(&header);
    if !subtitle.is_empty() {
        let sub = gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build();
        row.append(&sub);
    }
    row.append(&scale);
    group.add(&row);

    let cmd_tx = cmd_tx.clone();
    let last_input: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let pending: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let handler = {
        let last_input = last_input.clone();
        adjustment.connect_value_changed(move |adj| {
            last_input.set(Some(Instant::now()));
            if let Some(source) = pending.take() {
                source.remove();
            }
            let value = to_value(adj.value());
            let cmd_tx = cmd_tx.clone();
            let pending_done = pending.clone();
            pending.set(Some(glib::timeout_add_local_once(
                SLIDER_DEBOUNCE,
                move || {
                    pending_done.set(None);
                    let _ = cmd_tx.send(GuiCommand::SetParam {
                        id: id.to_string(),
                        key: key.to_string(),
                        value,
                    });
                },
            )));
        })
    };

    SliderCtl {
        adjustment,
        handler,
        last_input,
    }
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
    add_slider(
        group,
        title,
        subtitle,
        min,
        max,
        step,
        id,
        key,
        cmd_tx,
        |v| shared::dbus::u32_value(v as u32),
    )
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
    add_slider(
        group,
        title,
        subtitle,
        min,
        max,
        step,
        id,
        key,
        cmd_tx,
        |v| shared::dbus::i32_value(v as i32),
    )
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
