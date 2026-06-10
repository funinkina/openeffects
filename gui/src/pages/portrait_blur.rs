//! Portrait Blur page: blur strength plus the Background Replace controls
//! (current background, solid-color presets, and a custom image picker).

use adw::prelude::*;
use gtk::gio;

use crate::constants::BG_PRESETS;
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{add_spin_u32, add_switch, pref_group, Params, Switches};

pub fn build(
    cmd_tx: &CmdTx,
    switches: &mut Switches,
    params: &mut Params,
) -> (adw::PreferencesPage, adw::ActionRow) {
    let page = adw::PreferencesPage::new();

    let pb = pref_group(
        "Portrait Blur",
        "Blur the background while keeping the subject sharp",
    );
    add_switch(
        &pb,
        "Portrait Blur",
        "Blur everything behind the subject",
        "portrait_blur",
        cmd_tx,
        switches,
    );
    add_spin_u32(
        &pb,
        "Strength",
        "Blur intensity, 0–100",
        0.0,
        100.0,
        1.0,
        "portrait_blur",
        "strength",
        cmd_tx,
        params,
    );
    page.add(&pb);

    let bg = pref_group(
        "Background Replace",
        "Replace the background with an image or solid color",
    );
    add_switch(
        &bg,
        "Background Replace",
        "Choose a background below",
        "bg_replace",
        cmd_tx,
        switches,
    );
    page.add(&bg);

    let current_group = pref_group(
        "Current Background",
        "Selecting a background also enables Background Replace",
    );
    let bg_current = adw::ActionRow::builder()
        .title("Selected")
        .subtitle("None")
        .build();
    current_group.add(&bg_current);
    page.add(&current_group);

    let presets = pref_group("Solid Colors", "Built-in neutral backgrounds");
    // "None" clears the background and disables the effect.
    let none_row = adw::ActionRow::builder()
        .title("None")
        .subtitle("Disable background replace")
        .activatable(true)
        .build();
    {
        let cmd_tx = cmd_tx.clone();
        none_row.connect_activated(move |_| {
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "bg_replace".into(),
                key: "background".into(),
                value: shared::dbus::str_value(""),
            });
            let _ = cmd_tx.send(GuiCommand::SetEnabled {
                id: "bg_replace".into(),
                on: false,
            });
        });
    }
    presets.add(&none_row);
    for (label, hex) in BG_PRESETS {
        let row = adw::ActionRow::builder()
            .title(*label)
            .subtitle(*hex)
            .activatable(true)
            .build();
        let cmd_tx = cmd_tx.clone();
        let hex = hex.to_string();
        row.connect_activated(move |_| set_background(&cmd_tx, &hex));
        presets.add(&row);
    }
    page.add(&presets);

    let image_group = pref_group("Custom Image", "Use any image file as your background");
    let browse_row = adw::ActionRow::builder()
        .title("Browse…")
        .subtitle("Pick a JPEG or PNG")
        .activatable(true)
        .build();
    {
        let cmd_tx = cmd_tx.clone();
        browse_row.connect_activated(move |row| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose Background Image")
                .build();
            let filter = gtk::FileFilter::new();
            filter.add_mime_type("image/png");
            filter.add_mime_type("image/jpeg");
            filter.set_name(Some("Images"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let cmd_tx = cmd_tx.clone();
            let parent = row.root().and_downcast::<gtk::Window>();
            dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        set_background(&cmd_tx, &path.to_string_lossy());
                    }
                }
            });
        });
    }
    image_group.add(&browse_row);
    page.add(&image_group);

    (page, bg_current)
}

/// Set the background path/color and enable background replace.
fn set_background(cmd_tx: &CmdTx, value: &str) {
    let _ = cmd_tx.send(GuiCommand::SetParam {
        id: "bg_replace".into(),
        key: "background".into(),
        value: shared::dbus::str_value(value),
    });
    let _ = cmd_tx.send(GuiCommand::SetEnabled {
        id: "bg_replace".into(),
        on: true,
    });
}

/// Human-readable label for the current `bg_replace.background` value.
pub fn bg_label(bg: &str) -> String {
    if bg.is_empty() {
        "None".into()
    } else if let Some((label, _)) = BG_PRESETS.iter().find(|(_, hex)| *hex == bg) {
        format!("{label} ({bg})")
    } else {
        bg.to_string()
    }
}
