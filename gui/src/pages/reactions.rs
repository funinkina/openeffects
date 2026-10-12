//! Reactions card: the header switch turns on gesture-triggered reactions;
//! inside, an always-available emoji burst picker that wraps to fit the
//! column width.
//!
//! The emoji buttons send a one-shot `TriggerReaction` regardless of the
//! switch; the switch enables the hand-gesture pipeline that fires the matching
//! burst automatically.

use adw::prelude::*;
use shared::dbus::{VariantMap, value_as_bool};

use crate::constants::REACTION_BUTTONS;
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{EffectCard, plain_row, section_label};

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    pub card: EffectCard,
}

pub fn build(cmd_tx: &CmdTx) -> Widgets {
    let card = {
        let cmd_tx = cmd_tx.clone();
        EffectCard::new("Reactions", "face-smile-symbolic", move |on| {
            let _ = cmd_tx.send(GuiCommand::SetEnabled {
                id: "reactions".into(),
                on,
            });
        })
    };

    // Manual burst picker (always sensitive). A FlowBox wraps the buttons
    // instead of forcing a minimum window width.
    let emoji_flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(5)
        .row_spacing(8)
        .column_spacing(8)
        .build();
    for (id, glyph, tooltip) in REACTION_BUTTONS {
        let button = gtk::Button::builder()
            .label(*glyph)
            .tooltip_text(*tooltip)
            .css_classes(["oe-emoji-btn"])
            .build();
        let cmd_tx = cmd_tx.clone();
        let id = (*id).to_string();
        button.connect_clicked(move |_| {
            let _ = cmd_tx.send(GuiCommand::TriggerReaction { id: id.clone() });
        });
        emoji_flow.append(&button);
    }

    let hint = gtk::Label::builder()
        .label("Turn the switch on to trigger these with hand gestures.")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption", "dim-label"])
        .build();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    body.append(&section_label("Tap to send"));
    body.append(&emoji_flow);
    body.append(&hint);
    card.add_row(&plain_row(&body));

    let widgets = Widgets { card };
    widgets.apply_on(false);
    widgets
}

impl Widgets {
    pub fn apply(&self, params: &VariantMap) {
        if let Some(on) = params.get("enabled").and_then(value_as_bool) {
            self.apply_on(on);
        }
    }

    fn apply_on(&self, on: bool) {
        self.card.set_on(on);
        self.card.set_summary(if on {
            "Gestures on · tap to send"
        } else {
            "Gestures off · tap to send"
        });
    }
}
