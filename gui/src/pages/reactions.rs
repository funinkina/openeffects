//! Reactions page: a always-available emoji burst picker plus the
//! gesture-triggered overlay toggle.
//!
//! The emoji buttons send a one-shot `TriggerReaction` regardless of the
//! switch; the switch enables the hand-gesture pipeline that fires the matching
//! burst automatically.

use adw::prelude::*;

use crate::constants::REACTION_BUTTONS;
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{add_switch, pref_group, Switches};

/// One-time CSS so the emoji buttons render the glyphs large.
fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(".oe-emoji-btn { font-size: 24px; padding: 6px 12px; }");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

pub fn build(cmd_tx: &CmdTx, switches: &mut Switches) -> adw::PreferencesPage {
    install_css();
    let page = adw::PreferencesPage::new();

    // ── Manual burst picker (always visible, always sensitive) ────────────────
    let send_group = pref_group(
        "Send a Reaction",
        "Play a one-shot emoji burst in the video",
    );
    let emoji_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    for (id, glyph, tooltip) in REACTION_BUTTONS {
        let button = gtk::Button::builder()
            .label(*glyph)
            .tooltip_text(*tooltip)
            .css_classes(["oe-emoji-btn", "flat"])
            .build();
        let cmd_tx = cmd_tx.clone();
        let id = (*id).to_string();
        button.connect_clicked(move |_| {
            let _ = cmd_tx.send(GuiCommand::TriggerReaction { id: id.clone() });
        });
        emoji_box.append(&button);
    }
    send_group.add(&emoji_box);
    page.add(&send_group);

    // ── Gesture auto-trigger toggle ───────────────────────────────────────────
    let group = pref_group("Gesture Reactions", "Trigger reactions with hand gestures");
    add_switch(
        &group,
        "Reactions",
        "Off by default — enable to react to hand gestures",
        "reactions",
        cmd_tx,
        switches,
    );
    page.add(&group);

    page
}
