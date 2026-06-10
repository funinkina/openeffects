//! Top-level window. Follows the GNOME HIG multi-view pattern: an
//! `AdwViewStack` of `AdwPreferencesPage`s driven by an `AdwViewSwitcher` in
//! the header (wide) and an `AdwViewSwitcherBar` at the bottom (narrow,
//! revealed by a breakpoint). The primary menu (Preferences / About) lives on
//! the right of the header bar; daemon connection trouble surfaces in an
//! `AdwBanner`.

use std::collections::HashMap;
use std::rc::Rc;

use adw::glib;
use adw::prelude::*;
use adw::ApplicationWindow;
use gtk::gio;
use shared::dbus::{
    value_as_bool, value_as_i32, value_as_string, value_as_u32, VariantMap, EFFECT_IDS,
};

use crate::about;
use crate::constants::NAV_PAGES;
use crate::dbus_client::{CameraInfo, CmdTx, UiUpdate};
use crate::pages::camera::CameraWidgets;
use crate::pages::{camera, center_stage, portrait_blur, reactions, studio_light};
use crate::preferences;
use crate::widgets::{ParamWidget, Params, SpinParam, Switches};

struct Widgets {
    banner: adw::Banner,
    switches: Switches,
    params: Params,
    camera: CameraWidgets,
    bg_current: adw::ActionRow,
    prefs: preferences::Widgets,
}

pub fn build_window(
    app: &adw::Application,
    cmd_tx: CmdTx,
    update_rx: async_channel::Receiver<UiUpdate>,
) {
    // ── Pages (each an AdwPreferencesPage) added to the view stack ───────────
    let stack = adw::ViewStack::new();

    let mut switches = HashMap::new();
    let mut params = HashMap::new();

    let cs_page = center_stage::build(&cmd_tx, &mut switches, &mut params);
    let (pb_page, bg_current) = portrait_blur::build(&cmd_tx, &mut switches, &mut params);
    let sl_page = studio_light::build(&cmd_tx, &mut switches, &mut params);
    let rx_page = reactions::build(&cmd_tx, &mut switches);
    let (cam_page, camera_widgets) = camera::build(&cmd_tx);

    let page_widgets: [&gtk::Widget; 5] = [
        cs_page.upcast_ref(),
        pb_page.upcast_ref(),
        sl_page.upcast_ref(),
        rx_page.upcast_ref(),
        cam_page.upcast_ref(),
    ];
    for ((name, title, icon), widget) in NAV_PAGES.iter().zip(page_widgets) {
        stack.add_titled_with_icon(widget, Some(name), title, icon);
    }

    // ── View switchers: header (wide) + bottom bar (narrow) ──────────────────
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();

    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();

    // ── Preferences dialog (engine info + model library) ─────────────────────
    let (prefs_dialog, prefs_widgets) = preferences::build();

    // ── Primary menu (Preferences / About), right side of the header ─────────
    let menu = gio::Menu::new();
    menu.append(Some("Preferences"), Some("win.preferences"));
    menu.append(Some("About OpenEffects"), Some("win.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();

    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    header.pack_end(&menu_button);

    // Connection-trouble banner, sits just under the header.
    let banner = adw::Banner::builder().revealed(false).build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(&stack));
    toolbar.add_bottom_bar(&switcher_bar);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("OpenEffects")
        .default_width(720)
        .default_height(640)
        .content(&toolbar)
        .build();

    // Adaptive: hide the header switcher and reveal the bottom bar when narrow.
    if let Ok(condition) = adw::BreakpointCondition::parse("max-width: 850sp") {
        let breakpoint = adw::Breakpoint::new(condition);
        breakpoint.add_setter(&switcher, "visible", Some(&false.to_value()));
        breakpoint.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
        window.add_breakpoint(breakpoint);
    }

    // ── Primary menu actions ──────────────────────────────────────────────────
    let action_preferences = gio::SimpleAction::new("preferences", None);
    {
        let window = window.clone();
        action_preferences.connect_activate(move |_, _| prefs_dialog.present(Some(&window)));
    }
    window.add_action(&action_preferences);

    let action_about = gio::SimpleAction::new("about", None);
    {
        let window = window.clone();
        action_about.connect_activate(move |_, _| about::present(&window));
    }
    window.add_action(&action_about);

    let widgets = Rc::new(Widgets {
        banner,
        switches,
        params,
        camera: camera_widgets,
        bg_current,
        prefs: prefs_widgets,
    });

    glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = update_rx.recv().await {
            apply_update(&widgets, update);
        }
    });

    window.present();
}

// ── Apply daemon state updates ───────────────────────────────────────────────

fn apply_update(w: &Widgets, update: UiUpdate) {
    match update {
        UiUpdate::AllState(state) => {
            for id in EFFECT_IDS {
                let params = extract_effect_params(&state, id);
                apply_enabled(w, id, &params);
                apply_params(w, id, &params);
                if id == "bg_replace" {
                    if let Some(bg) = params.get("background").and_then(value_as_string) {
                        w.bg_current.set_subtitle(&portrait_blur::bg_label(&bg));
                    }
                }
            }
        }
        UiUpdate::EffectChanged { id, params } => {
            apply_enabled(w, &id, &params);
            apply_params(w, &id, &params);
            if id == "bg_replace" {
                if let Some(bg) = params.get("background").and_then(value_as_string) {
                    w.bg_current.set_subtitle(&portrait_blur::bg_label(&bg));
                }
            }
        }
        UiUpdate::Status(status) => apply_status_banner(w, &status),
        UiUpdate::Capabilities(caps) => apply_capabilities(w, &caps),
        UiUpdate::Cameras { cameras, active } => apply_cameras(w, &cameras, &active),
        UiUpdate::Disconnected => apply_status_banner(w, "disconnected"),
    }
}

/// Banners are for states the user should act on; running/idle/starting are
/// normal so the banner stays hidden for them.
fn apply_status_banner(w: &Widgets, status: &str) {
    let message = match status {
        "stopped" => Some("Pipeline stopped"),
        "error" => Some("The pipeline reported an error"),
        "disconnected" => Some("Disconnected from the OpenEffects daemon — retrying…"),
        _ => None,
    };
    match message {
        Some(text) => {
            w.banner.set_title(text);
            w.banner.set_revealed(true);
        }
        None => w.banner.set_revealed(false),
    }
}

fn apply_capabilities(w: &Widgets, caps: &VariantMap) {
    let tier = caps
        .get("tier")
        .and_then(value_as_string)
        .unwrap_or_else(|| "—".into());
    let ep = caps
        .get("ep")
        .and_then(value_as_string)
        .unwrap_or_else(|| "—".into());
    let ready = caps
        .get("models_ready")
        .and_then(value_as_bool)
        .unwrap_or(false);
    let vcam = caps
        .get("virtual_camera")
        .and_then(value_as_string)
        .unwrap_or_else(|| "—".into());

    w.prefs.tier.set_subtitle(&tier.to_uppercase());
    w.prefs.ep.set_subtitle(&ep.to_uppercase());
    w.camera.virtual_cam_row.set_subtitle(&vcam);

    let pill_text = if ready { "Ready" } else { "Missing" };
    for row in w.prefs.model_rows.values() {
        if let Some(pill) = unsafe { row.data::<gtk::Label>("pill") } {
            let pill = unsafe { pill.as_ref() };
            pill.set_label(pill_text);
        }
    }
}

fn apply_cameras(w: &Widgets, cameras: &[CameraInfo], active: &str) {
    let labels: Vec<&str> = if cameras.is_empty() {
        vec!["No cameras found"]
    } else {
        cameras.iter().map(|c| c.name.as_str()).collect()
    };
    let model = gtk::StringList::new(&labels);
    *w.camera.ids.borrow_mut() = cameras.iter().map(|c| c.id.clone()).collect();

    let active_idx = cameras.iter().position(|c| c.id == active).unwrap_or(0) as u32;
    w.camera.combo.block_signal(&w.camera.handler);
    w.camera.combo.set_model(Some(&model));
    w.camera.combo.set_selected(active_idx);
    w.camera.combo.unblock_signal(&w.camera.handler);
}

/// `GetAllState()` returns keys prefixed with `"{effect_id}."`; strip the prefix
/// so the result matches an `EffectChanged` payload's shape.
fn extract_effect_params(state: &VariantMap, id: &str) -> VariantMap {
    let prefix = format!("{id}.");
    state
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix(prefix.as_str())
                .and_then(|key| v.try_clone().ok().map(|v| (key.to_string(), v)))
        })
        .collect()
}

fn apply_enabled(w: &Widgets, id: &str, params: &VariantMap) {
    let Some((switch, handler)) = w.switches.get(id) else {
        return;
    };
    if let Some(on) = params.get("enabled").and_then(value_as_bool) {
        switch.block_signal(handler);
        switch.set_active(on);
        switch.unblock_signal(handler);
    }
}

fn apply_params(w: &Widgets, id: &str, params: &VariantMap) {
    for (key, value) in params {
        if key == "enabled" {
            continue;
        }
        let Some(widget) = w.params.get(&format!("{id}.{key}")) else {
            continue;
        };
        match widget {
            ParamWidget::Combo(combo) => {
                if let Some(s) = value_as_string(value) {
                    if let Some(idx) = combo.options.iter().position(|(v, _)| *v == s) {
                        combo.row.block_signal(&combo.handler);
                        combo.row.set_selected(idx as u32);
                        combo.row.unblock_signal(&combo.handler);
                    }
                }
            }
            ParamWidget::Spin(SpinParam::U32 { row, handler }) => {
                if let Some(v) = value_as_u32(value) {
                    row.block_signal(handler);
                    row.set_value(v as f64);
                    row.unblock_signal(handler);
                }
            }
            ParamWidget::Spin(SpinParam::I32 { row, handler }) => {
                if let Some(v) = value_as_i32(value) {
                    row.block_signal(handler);
                    row.set_value(v as f64);
                    row.unblock_signal(handler);
                }
            }
        }
    }
}
