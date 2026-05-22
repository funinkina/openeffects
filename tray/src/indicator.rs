use std::{rc::Rc, sync::mpsc};

use crate::{menu, TrayCommand, TrayUpdate};
use appindicator3::{prelude::*, Indicator, IndicatorCategory, IndicatorStatus};

pub struct TrayIndicator {
    _indicator: Indicator,
    _menu: Rc<menu::MenuHandles>,
}

pub fn build_and_show(
    state_rx: glib::Receiver<TrayUpdate>,
    cmd_tx: mpsc::Sender<TrayCommand>,
) -> TrayIndicator {
    let menu = Rc::new(menu::build(cmd_tx));
    let indicator = Indicator::builder("openeffects")
        .category(IndicatorCategory::ApplicationStatus)
        .menu(&menu.menu)
        .icon("openeffects-idle", "OpenEffects")
        .status(IndicatorStatus::Active)
        .build();

    let menu_for_updates = Rc::clone(&menu);
    let indicator_for_updates = indicator.clone();
    state_rx.attach(None, move |update| {
        match &update {
            TrayUpdate::Status(status) if status == "running" => {
                indicator_for_updates.set_icon("openeffects-active");
            }
            TrayUpdate::Status(status) if status == "error" => {
                indicator_for_updates.set_icon("openeffects-error");
            }
            TrayUpdate::Error(_) => {
                indicator_for_updates.set_icon("openeffects-error");
            }
            _ => {
                indicator_for_updates.set_icon("openeffects-idle");
            }
        }
        menu_for_updates.apply_update(update);
        glib::ControlFlow::Continue
    });

    TrayIndicator {
        _indicator: indicator,
        _menu: menu,
    }
}
