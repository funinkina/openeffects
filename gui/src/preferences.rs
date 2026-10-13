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
    /// Node name of the virtual camera, shown on the "Virtual Camera" row.
    pub virtual_cam_label: gtk::Label,
    pub model_rows: HashMap<&'static str, adw::ActionRow>,
    pub inference_row: adw::ActionRow,
    pub inference_group: adw::ToggleGroup,
    #[allow(dead_code)]
    pub inference_toggle_auto: adw::Toggle,
    #[allow(dead_code)]
    pub inference_toggle_cpu: adw::Toggle,
    pub inference_toggle_npu: adw::Toggle,
    pub inference_handler: glib::SignalHandlerId,
    pub inference_badge: gtk::Label,
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

    // Camera
    let camera_group = pref_group(
        "Camera",
        "Choose the physical camera vinoOpenEffects captures from",
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
        .subtitle("Pick this one in Zoom, OBS or your browser")
        .build();
    let virtual_cam_label = gtk::Label::builder()
        .label("—")
        .selectable(true)
        .css_classes(["monospace"])
        .build();
    virtual_cam_row.add_suffix(&virtual_cam_label);
    camera_group.add(&virtual_cam_row);
    page.add(&camera_group);

    // Inference
    let inference_group_box = pref_group(
        "Inference",
        "Machine learning execution provider for effects (hot-swappable)",
    );

    let inference_row = adw::ActionRow::builder()
        .title("Execution Provider")
        .subtitle("Auto selects NPU when available, falls back to CPU")
        .build();

    let inference_toggle_auto = adw::Toggle::builder().name("auto").label("Auto").build();
    let inference_toggle_cpu = adw::Toggle::builder().name("cpu").label("CPU").build();
    let inference_toggle_npu = adw::Toggle::builder().name("npu").label("NPU").build();

    let inference_group = adw::ToggleGroup::builder()
        .valign(gtk::Align::Center)
        .css_classes(["round"])
        .build();
    inference_group.add(inference_toggle_auto.clone());
    inference_group.add(inference_toggle_cpu.clone());
    inference_group.add(inference_toggle_npu.clone());
    inference_group.set_active_name(Some("auto"));

    let inference_badge = gtk::Label::builder()
        .label("Active: CPU")
        .valign(gtk::Align::Center)
        .css_classes(["dim-label"])
        .build();

    let inference_handler = {
        let cmd_tx = cmd_tx.clone();
        inference_group.connect_active_notify(move |group| {
            if let Some(name) = group.active_name() {
                let _ = cmd_tx.send(GuiCommand::SetInferenceProvider(name.to_string()));
            }
        })
    };

    let suffix_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .valign(gtk::Align::Center)
        .build();
    suffix_box.append(&inference_badge);
    suffix_box.append(&inference_group);

    inference_row.add_suffix(&suffix_box);
    inference_group_box.add(&inference_row);
    page.add(&inference_group_box);

    // Bundled models
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

    // Startup
    let startup = pref_group("Startup", "Control when the vinoOpenEffects daemon runs");

    let keep_running_row = adw::SwitchRow::builder()
        .title("Keep running in the background")
        .subtitle("Leave the daemon on after this window closes")
        .active(gui_config.borrow().keep_running_in_background)
        .build();

    let boot_row = adw::SwitchRow::builder()
        .title("Start when I log in")
        .subtitle("Launch the daemon automatically")
        .active(shared::systemd::is_enabled())
        .build();
    // Moot when boot-autostart is on (the daemon always runs then).
    keep_running_row.set_sensitive(!boot_row.is_active());

    let keep_running_for_boot = keep_running_row.clone();
    boot_row.connect_active_notify(move |row| {
        let on = row.is_active();
        if let Err(err) = shared::systemd::set_enabled(on) {
            tracing::warn!(%err, "failed to change daemon autostart");
        }
        keep_running_for_boot.set_sensitive(!on);
    });
    startup.add(&boot_row);

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
            virtual_cam_label,
            model_rows,
            inference_row,
            inference_group,
            inference_toggle_auto,
            inference_toggle_cpu,
            inference_toggle_npu,
            inference_handler,
            inference_badge,
        },
    )
}
