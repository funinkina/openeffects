//! Studio Light page: brightness/contrast/intensity controls (`videobalance`).

use adw::prelude::*;

use crate::dbus_client::CmdTx;
use crate::widgets::{add_spin_i32, add_spin_u32, add_switch, pref_group, Params, Switches};

pub fn build(cmd_tx: &CmdTx, switches: &mut Switches, params: &mut Params) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let group = pref_group("Studio Light", "Subtly brighten and separate the subject");
    add_switch(
        &group,
        "Studio Light",
        "Brighten and add contrast to the subject",
        "studio_light",
        cmd_tx,
        switches,
    );
    add_spin_u32(
        &group,
        "Intensity",
        "Overall effect strength, 0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "intensity",
        cmd_tx,
        params,
    );
    add_spin_i32(
        &group,
        "Brightness",
        "-100 (darker) to 100 (brighter)",
        -100.0,
        100.0,
        1.0,
        "studio_light",
        "brightness",
        cmd_tx,
        params,
    );
    add_spin_u32(
        &group,
        "Contrast",
        "0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "contrast",
        cmd_tx,
        params,
    );
    page.add(&group);

    page
}
