//! Camera page: physical camera picker and virtual-camera node readout.

use std::cell::RefCell;
use std::rc::Rc;

use adw::glib;
use adw::prelude::*;

use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::pref_group;

/// Widgets the state-sync layer needs to keep in sync with the daemon.
pub struct CameraWidgets {
    pub combo: adw::ComboRow,
    pub handler: glib::SignalHandlerId,
    /// Camera ids in the same order as `combo`'s model, shared with the
    /// `connect_selected_notify` handler so it can resolve a selection back
    /// to an id.
    pub ids: Rc<RefCell<Vec<String>>>,
    pub virtual_cam_row: adw::ActionRow,
}

pub fn build(cmd_tx: &CmdTx) -> (adw::PreferencesPage, CameraWidgets) {
    let page = adw::PreferencesPage::new();

    let group = pref_group(
        "Camera",
        "Choose the physical camera OpenEffects captures from",
    );
    let combo = adw::ComboRow::builder()
        .title("Source")
        .model(&gtk::StringList::new(&["No cameras found"]))
        .build();
    let ids = Rc::new(RefCell::new(Vec::<String>::new()));
    let handler = {
        let cmd_tx = cmd_tx.clone();
        let ids = Rc::clone(&ids);
        combo.connect_selected_notify(move |row| {
            let ids = ids.borrow();
            if let Some(id) = ids.get(row.selected() as usize) {
                let _ = cmd_tx.send(GuiCommand::SelectCamera { id: id.clone() });
            }
        })
    };
    group.add(&combo);
    page.add(&group);

    let vgroup = pref_group(
        "Virtual Camera",
        "The node other apps select to receive the processed feed",
    );
    let virtual_cam_row = adw::ActionRow::builder()
        .title("Node")
        .subtitle("—")
        .build();
    vgroup.add(&virtual_cam_row);
    page.add(&vgroup);

    (
        page,
        CameraWidgets {
            combo,
            handler,
            ids,
            virtual_cam_row,
        },
    )
}
