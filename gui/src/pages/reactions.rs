//! Reactions page: gesture-triggered overlay toggle.

use adw::prelude::*;

use crate::dbus_client::CmdTx;
use crate::widgets::{add_switch, pref_group, Switches};

pub fn build(cmd_tx: &CmdTx, switches: &mut Switches) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let group = pref_group("Reactions", "Trigger animated overlays with hand gestures");
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
