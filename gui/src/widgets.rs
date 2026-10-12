//! Shared building blocks for the effect cards: the expandable
//! [`EffectCard`] itself, plus segmented-control ([`ToggleCtl`]) and slider
//! ([`SliderCtl`]) rows that the per-effect modules own directly.

use std::cell::Cell;
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

/// An `AdwToggleGroup` (built by [`toggle_row`]) whose selection change is
/// forwarded as a string value.
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

/// Build a settings row with the title/subtitle on the left and a compact,
/// rounded segmented control on the right. `on_change` receives the active
/// toggle's name whenever the user changes the selection. Prefer this over a
/// full-width toggle group: it keeps related options on one line each and the
/// row shrinks gracefully at narrow window widths.
pub fn toggle_row(
    title: &str,
    subtitle: &str,
    options: &[(&'static str, &'static str)],
    on_change: impl Fn(&str) + 'static,
) -> (adw::ActionRow, ToggleCtl) {
    let group = adw::ToggleGroup::builder()
        .valign(gtk::Align::Center)
        .css_classes(["round"])
        .build();
    for (name, label) in options {
        group.add(adw::Toggle::builder().name(*name).label(*label).build());
    }
    let handler = group.connect_active_notify(move |group| {
        if let Some(name) = group.active_name() {
            on_change(name.as_str());
        }
    });
    let ctl = ToggleCtl { group, handler };

    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_suffix(&ctl.group);
    (row, ctl)
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

/// Build a slider row: title + an exact-value spin button on the top line,
/// optional subtitle, and a scale spanning the row underneath. The scale and
/// spin button share one adjustment, so dragging, scrolling, typing an exact
/// number, or stepping with the spin arrows all stay in sync. Bipolar ranges
/// get a center tick at 0 so "neutral" is easy to find.
///
/// A drag/step re-arms a trailing debounce timer, so only the last value of a
/// burst is sent as a `SetParam`; the input timestamp feeds the echo holdoff.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    to_value: impl Fn(f64) -> zvariant::OwnedValue + 'static,
) -> (gtk::ListBoxRow, SliderCtl) {
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

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    body.append(&header);
    if !subtitle.is_empty() {
        let sub = gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build();
        body.append(&sub);
    }
    body.append(&scale);

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

    (
        plain_row(&body),
        SliderCtl {
            adjustment,
            handler,
            last_input,
        },
    )
}

/// A slider row reporting an unsigned integer `SetParam`.
#[allow(clippy::too_many_arguments)]
pub fn slider_row_u32(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
) -> (gtk::ListBoxRow, SliderCtl) {
    slider_row(title, subtitle, min, max, step, id, key, cmd_tx, |v| {
        shared::dbus::u32_value(v as u32)
    })
}

/// A slider row reporting a signed integer `SetParam`.
#[allow(clippy::too_many_arguments)]
pub fn slider_row_i32(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
) -> (gtk::ListBoxRow, SliderCtl) {
    slider_row(title, subtitle, min, max, step, id, key, cmd_tx, |v| {
        shared::dbus::i32_value(v as i32)
    })
}

/// Wrap `child` in a padded, non-activatable list row, for custom content
/// (sliders, thumbnail grids) inside an effect card.
pub fn plain_row(child: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    child.set_margin_top(12);
    child.set_margin_bottom(12);
    child.set_margin_start(12);
    child.set_margin_end(12);
    gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(child)
        .build()
}

/// A small heading that splits an effect card into sections ("On you",
/// "Behind you", "Images", ...).
pub fn section_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["caption-heading", "dim-label"])
        .build()
}

/// One effect in the controls column: a rounded card whose header shows an
/// icon, the effect name, a one-line summary of its current settings and its
/// on/off switch. Clicking the header expands the card to reveal the effect's
/// settings. State sync goes through [`EffectCard::set_on`], which blocks the
/// switch handler so daemon echoes don't loop back as new commands.
pub struct EffectCard {
    /// The `boxed-list` that draws the card; add this to the column.
    pub list: gtk::ListBox,
    expander: adw::ExpanderRow,
    switch: gtk::Switch,
    handler: glib::SignalHandlerId,
    icon: gtk::Image,
}

impl EffectCard {
    /// `on_toggle` receives the new switch state whenever the user flips it.
    pub fn new(title: &str, icon_name: &str, on_toggle: impl Fn(bool) + 'static) -> Self {
        let icon = gtk::Image::builder()
            .icon_name(icon_name)
            .pixel_size(20)
            .valign(gtk::Align::Center)
            .css_classes(["oe-effect-icon"])
            .build();
        let switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .tooltip_text(title)
            .build();
        switch.update_property(&[gtk::accessible::Property::Label(title)]);
        let handler = switch.connect_active_notify(move |sw| on_toggle(sw.is_active()));

        let expander = adw::ExpanderRow::builder()
            .title(title)
            .subtitle("Off")
            .build();
        expander.add_prefix(&icon);
        expander.add_suffix(&switch);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.append(&expander);

        Self {
            list,
            expander,
            switch,
            handler,
            icon,
        }
    }

    /// Reflect the effect's enabled state without echoing it to the daemon.
    pub fn set_on(&self, on: bool) {
        self.switch.block_signal(&self.handler);
        self.switch.set_active(on);
        self.switch.unblock_signal(&self.handler);
        if on {
            self.icon.add_css_class("oe-on");
        } else {
            self.icon.remove_css_class("oe-on");
        }
    }

    pub fn is_on(&self) -> bool {
        self.switch.is_active()
    }

    /// The one-line summary under the effect name ("Normal zoom · One face").
    pub fn set_summary(&self, summary: &str) {
        self.expander.set_subtitle(summary);
    }

    pub fn set_expanded(&self, expanded: bool) {
        self.expander.set_expanded(expanded);
    }

    pub fn add_row(&self, row: &impl IsA<gtk::Widget>) {
        self.expander.add_row(row);
    }
}

pub fn pref_group(title: &str, description: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder()
        .title(title)
        .description(description)
        .build()
}
