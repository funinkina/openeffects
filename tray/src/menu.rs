use std::{cell::Cell, collections::HashMap, process::Command, rc::Rc, sync::mpsc};

use gtk::prelude::*;
use shared::dbus::{value_as_bool, VariantMap, EFFECT_IDS};

use crate::{TrayCommand, TrayUpdate};

pub struct MenuHandles {
    pub menu: gtk::Menu,
    effect_items: HashMap<String, gtk::CheckMenuItem>,
    status_item: gtk::MenuItem,
    camera_item: gtk::MenuItem,
    updating: Rc<Cell<bool>>,
}

impl MenuHandles {
    pub fn apply_update(&self, update: TrayUpdate) {
        match update {
            TrayUpdate::Status(status) => {
                self.status_item.set_label(&format!("Status: {status}"));
            }
            TrayUpdate::AllEffects(values) => self.apply_flat_effect_state(&values),
            TrayUpdate::EffectChanged { id, params } => self.apply_effect_params(&id, &params),
            TrayUpdate::Error(error) => {
                self.status_item.set_label(&format!("Status: error"));
                self.camera_item.set_label(&format!("Camera: {error}"));
            }
        }
    }

    fn apply_flat_effect_state(&self, values: &VariantMap) {
        self.updating.set(true);
        for id in EFFECT_IDS {
            if let Some(item) = self.effect_items.get(id) {
                let key = format!("{id}.enabled");
                let active = values.get(&key).and_then(value_as_bool).unwrap_or(false);
                item.set_active(active);
            }
        }
        self.updating.set(false);
    }

    fn apply_effect_params(&self, id: &str, params: &VariantMap) {
        if let Some(item) = self.effect_items.get(id) {
            self.updating.set(true);
            item.set_active(
                params
                    .get("enabled")
                    .and_then(value_as_bool)
                    .unwrap_or(false),
            );
            self.updating.set(false);
        }
    }
}

pub fn build(cmd_tx: mpsc::Sender<TrayCommand>) -> MenuHandles {
    let menu = gtk::Menu::new();
    let updating = Rc::new(Cell::new(false));
    let mut effect_items = HashMap::new();

    for (id, label) in [
        ("center_stage", "Center Stage"),
        ("portrait_blur", "Portrait Blur"),
        ("bg_replace", "Background Replace"),
        ("studio_light", "Studio Light"),
        ("reactions", "Reactions"),
    ] {
        let item = gtk::CheckMenuItem::with_label(label);
        item.set_active(false);
        let effect_id = id.to_string();
        let sender = cmd_tx.clone();
        let updating_flag = Rc::clone(&updating);
        item.connect_toggled(move |item| {
            if updating_flag.get() {
                return;
            }
            let _ = sender.send(TrayCommand::SetEnabled {
                id: effect_id.clone(),
                on: item.is_active(),
            });
        });
        menu.append(&item);
        effect_items.insert(id.to_string(), item);
    }

    menu.append(&gtk::SeparatorMenuItem::new());

    let camera_item = gtk::MenuItem::with_label("Camera: Auto");
    camera_item.set_sensitive(false);
    menu.append(&camera_item);

    let status_item = gtk::MenuItem::with_label("Status: connecting");
    status_item.set_sensitive(false);
    menu.append(&status_item);

    menu.append(&gtk::SeparatorMenuItem::new());

    let start_item = gtk::MenuItem::with_label("Start");
    {
        let sender = cmd_tx.clone();
        start_item.connect_activate(move |_| {
            let _ = sender.send(TrayCommand::Start);
        });
    }
    menu.append(&start_item);

    let stop_item = gtk::MenuItem::with_label("Stop");
    {
        let sender = cmd_tx;
        stop_item.connect_activate(move |_| {
            let _ = sender.send(TrayCommand::Stop);
        });
    }
    menu.append(&stop_item);

    let open_item = gtk::MenuItem::with_label("Open OpenEffects...");
    open_item.connect_activate(|_| {
        let _ = Command::new("openeffects").spawn();
    });
    menu.append(&open_item);

    let quit_item = gtk::MenuItem::with_label("Quit");
    quit_item.connect_activate(|_| gtk::main_quit());
    menu.append(&quit_item);

    menu.show_all();

    MenuHandles {
        menu,
        effect_items,
        status_item,
        camera_item,
        updating,
    }
}
