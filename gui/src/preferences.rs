//! The Preferences dialog (`AdwPreferencesDialog`): engine/runtime info and
//! the bundled-model library. Built once and re-presented from the primary
//! menu.

use std::collections::HashMap;

use adw::prelude::*;

use crate::constants::BUNDLED_MODELS;
use crate::widgets::pref_group;

/// Rows the state-sync layer updates when `Capabilities` changes.
pub struct Widgets {
    pub tier: adw::ActionRow,
    pub ep: adw::ActionRow,
    pub model_rows: HashMap<&'static str, adw::ActionRow>,
}

pub fn build() -> (adw::PreferencesDialog, Widgets) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .build();

    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let engine = pref_group(
        "Engine",
        "ONNX runtime platform, detected at daemon startup",
    );
    let tier = adw::ActionRow::builder()
        .title("Hardware tier")
        .subtitle("—")
        .build();
    let ep = adw::ActionRow::builder()
        .title("Running on")
        .subtitle("—")
        .build();
    engine.add(&tier);
    engine.add(&ep);
    page.add(&engine);

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

    dialog.add(&page);

    (
        dialog,
        Widgets {
            tier,
            ep,
            model_rows,
        },
    )
}
