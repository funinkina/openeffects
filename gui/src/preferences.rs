//! The Preferences dialog (`AdwPreferencesDialog`): camera selection, the
//! virtual-camera node readout, and the bundled-model library. Built once and
//! re-presented from the primary menu.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::glib;
use adw::prelude::*;

use crate::config::GuiConfig;
use crate::constants::BUNDLED_MODELS;
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::pref_group;

/// Rows the state-sync layer updates when the daemon's state changes.
pub struct Widgets {
    /// Physical-camera picker.
    pub camera_combo: adw::ComboRow,
    pub camera_handler: glib::SignalHandlerId,
    /// Camera ids in the same order as `camera_combo`'s model, shared with its
    /// change handler so a selection resolves back to an id.
    pub camera_ids: Rc<RefCell<Vec<String>>>,
    pub virtual_cam_row: adw::ActionRow,
    pub model_rows: HashMap<&'static str, adw::ActionRow>,
}

pub fn build(
    cmd_tx: &CmdTx,
    gui_config: Rc<RefCell<GuiConfig>>,
) -> (adw::PreferencesDialog, Widgets) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .build();

    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    // ── Camera ───────────────────────────────────────────────────────────────
    let camera_group = pref_group(
        "Camera",
        "Choose the physical camera OpenEffects captures from",
    );
    let camera_combo = adw::ComboRow::builder()
        .title("Source")
        .model(&gtk::StringList::new(&["No cameras found"]))
        .build();
    let camera_ids = Rc::new(RefCell::new(Vec::<String>::new()));
    let camera_handler = {
        let cmd_tx = cmd_tx.clone();
        let camera_ids = Rc::clone(&camera_ids);
        camera_combo.connect_selected_notify(move |row| {
            let ids = camera_ids.borrow();
            if let Some(id) = ids.get(row.selected() as usize) {
                let _ = cmd_tx.send(GuiCommand::SelectCamera { id: id.clone() });
            }
        })
    };
    camera_group.add(&camera_combo);

    let virtual_cam_row = adw::ActionRow::builder()
        .title("Virtual Camera")
        .subtitle("—")
        .build();
    camera_group.add(&virtual_cam_row);
    page.add(&camera_group);

    // ── Bundled models ───────────────────────────────────────────────────────
    let models_group = pref_group(
        "Bundled Models",
        "Run scripts/fetch-models.sh to install missing models",
    );
    let mut model_rows = HashMap::new();
    for (id, name, purpose) in BUNDLED_MODELS {
        let row = adw::ActionRow::builder()
            .title(*name)
            .subtitle(*purpose)
            .build();
        let pill = gtk::Label::builder()
            .label("checking…")
            .css_classes(["dim-label"])
            .build();
        row.add_suffix(&pill);
        unsafe { row.set_data("pill", pill) };
        models_group.add(&row);
        model_rows.insert(*id, row);
    }
    page.add(&models_group);

    // ── Startup ──────────────────────────────────────────────────────────────
    let startup = pref_group("Startup", "Control when the OpenEffects daemon runs");

    // No systemd unit inside the sandbox, so the boot-autostart row is desktop-only.
    let in_flatpak = std::env::var_os("FLATPAK_ID").is_some();

    let keep_running_row = adw::SwitchRow::builder()
        .title("Keep running in the background")
        .subtitle("Leave the daemon running after this window is closed")
        .active(gui_config.borrow().keep_running_in_background)
        .build();

    if !in_flatpak {
        let boot_row = adw::SwitchRow::builder()
            .title("Start on system boot")
            .subtitle("Launch the daemon automatically when you log in")
            .active(shared::systemd::is_enabled())
            .build();
        // Moot when boot-autostart is on (the daemon always runs then).
        keep_running_row.set_sensitive(!boot_row.is_active());

        let keep_running_row = keep_running_row.clone();
        boot_row.connect_active_notify(move |row| {
            let on = row.is_active();
            if let Err(err) = shared::systemd::set_enabled(on) {
                tracing::warn!(%err, "failed to change daemon autostart");
            }
            keep_running_row.set_sensitive(!on);
        });
        startup.add(&boot_row);
    }

    {
        let gui_config = Rc::clone(&gui_config);
        keep_running_row.connect_active_notify(move |row| {
            gui_config.borrow_mut().keep_running_in_background = row.is_active();
            gui_config.borrow().save();
        });
    }
    startup.add(&keep_running_row);

    page.add(&startup);

    dialog.add(&page);

    (
        dialog,
        Widgets {
            camera_combo,
            camera_handler,
            camera_ids,
            virtual_cam_row,
            model_rows,
        },
    )
}
