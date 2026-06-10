//! Center Stage page: framing/zoom controls that keep the subject centered.

use adw::prelude::*;

use crate::constants::{FRAMING_MODES, ZOOM_LEVELS};
use crate::dbus_client::CmdTx;
use crate::widgets::{add_combo, add_switch, pref_group, Params, Switches};

pub fn build(cmd_tx: &CmdTx, switches: &mut Switches, params: &mut Params) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let group = pref_group(
        "Center Stage",
        "Keep the subject framed and centered as they move",
    );
    add_switch(
        &group,
        "Center Stage",
        "Track and crop to keep the subject in frame",
        "center_stage",
        cmd_tx,
        switches,
    );
    add_combo(
        &group,
        "Framing",
        ZOOM_LEVELS,
        "center_stage",
        "zoom",
        cmd_tx,
        params,
    );
    add_combo(
        &group,
        "Mode",
        FRAMING_MODES,
        "center_stage",
        "mode",
        cmd_tx,
        params,
    );
    page.add(&group);

    page
}
