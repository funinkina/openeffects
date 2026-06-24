//! Top-level window. Follows the GNOME HIG multi-view pattern: an
//! `AdwViewStack` of `AdwPreferencesPage`s driven by an `AdwViewSwitcher` in
//! the header (wide) and an `AdwViewSwitcherBar` at the bottom (narrow,
//! revealed by a breakpoint). The primary menu (Preferences / About) lives on
//! the right of the header bar; daemon connection trouble surfaces in an
//! `AdwBanner`. Camera selection lives in the Preferences dialog.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::ApplicationWindow;
use adw::glib;
use adw::prelude::*;
use gtk::gio;
use shared::dbus::{EFFECT_IDS, VariantMap, value_as_bool, value_as_string};

use crate::about;
use crate::config::GuiConfig;
use crate::constants::NAV_PAGES;
use crate::dbus_client::{CameraInfo, CmdTx, GuiCommand, UiUpdate};
use crate::pages::{background, center_stage, reactions, studio_light};
use crate::preferences;
use crate::preview;
use crate::widgets::Switches;

struct Widgets {
    banner: adw::Banner,
    switches: Switches,
    center_stage: center_stage::Widgets,
    background: background::Widgets,
    studio_light: studio_light::Widgets,
    prefs: preferences::Widgets,
}

pub fn build_window(
    app: &adw::Application,
    cmd_tx: CmdTx,
    update_rx: async_channel::Receiver<UiUpdate>,
    config: GuiConfig,
) {
    let gui_config = Rc::new(RefCell::new(config));

    // ── Pages (each an AdwPreferencesPage) added to the view stack ───────────
    let stack = adw::ViewStack::new();

    let mut switches = HashMap::new();

    let (cs_page, cs_widgets) = center_stage::build(&cmd_tx);
    let (bg_page, bg_widgets) = background::build(&cmd_tx);
    let (sl_page, sl_widgets) = studio_light::build(&cmd_tx);
    let rx_page = reactions::build(&cmd_tx, &mut switches);

    let page_widgets: [&gtk::Widget; 4] = [
        cs_page.upcast_ref(),
        bg_page.upcast_ref(),
        sl_page.upcast_ref(),
        rx_page.upcast_ref(),
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

    // ── Preferences dialog (camera, engine info, model library, startup) ─────
    let (prefs_dialog, prefs_widgets) = preferences::build(&cmd_tx, Rc::clone(&gui_config));

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
    header.pack_start(&menu_button);

    // Connection-trouble banner, sits just under the header.
    let banner = adw::Banner::builder().revealed(false).build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(&stack));
    toolbar.add_bottom_bar(&switcher_bar);

    // Global live-preview side pane wraps the whole content; its header toggle
    // reveals it without covering the page controls or the narrow switcher bar.
    let preview = preview::build(&toolbar);
    header.pack_end(&preview.toggle);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("OpenEffects")
        .default_width(720)
        .default_height(640)
        .content(&preview.split)
        .build();

    // Adaptive: hide the header switcher and reveal the bottom bar when narrow.
    if let Ok(condition) = adw::BreakpointCondition::parse("max-width: 850sp") {
        let breakpoint = adw::Breakpoint::new(condition);
        breakpoint.add_setter(&switcher, "visible", Some(&false.to_value()));
        breakpoint.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
        // Narrow: overlay the preview pane instead of squeezing the page.
        // breakpoint.add_setter(&preview.split, "collapsed", Some(&true.to_value()));
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
        center_stage: cs_widgets,
        background: bg_widgets,
        studio_light: sl_widgets,
        prefs: prefs_widgets,
    });

    // ── Window close → optionally stop the daemon ────────────────────────────
    // Default behaviour (boot-autostart off, keep-running off): closing the
    // window quits the daemon so the camera is released. We keep the window
    // (and therefore the app) alive via Propagation::Stop until the client
    // confirms the Quit call landed (`UiUpdate::DaemonQuit`), then quit the app.
    // Returning here without waiting would let the app exit and kill the client
    // thread before the Quit was delivered.
    let quitting = Rc::new(Cell::new(false));
    {
        let cmd_tx = cmd_tx.clone();
        let gui_config = Rc::clone(&gui_config);
        let quitting = Rc::clone(&quitting);
        window.connect_close_request(move |_| {
            // Second close while the Quit is in flight: just let it close.
            if quitting.get() {
                return glib::Propagation::Proceed;
            }
            // In the Flatpak sandbox the daemon is the persistent background
            // process (it holds the shell's background-app status), so closing
            // the window always leaves it running. The launcher does not kill it.
            let in_flatpak = std::env::var_os("FLATPAK_ID").is_some();
            let keep_running = gui_config.borrow().keep_running_in_background;
            if in_flatpak || keep_running || shared::systemd::is_enabled() {
                return glib::Propagation::Proceed;
            }
            quitting.set(true);
            let _ = cmd_tx.send(GuiCommand::QuitDaemon);
            glib::Propagation::Stop
        });
    }

    let loop_app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = update_rx.recv().await {
            if matches!(update, UiUpdate::DaemonQuit) {
                loop_app.quit();
                continue;
            }
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
                apply_effect(w, id, &params);
            }
        }
        UiUpdate::EffectChanged { id, params } => apply_effect(w, &id, &params),
        UiUpdate::Status(status) => apply_status_banner(w, &status),
        UiUpdate::Capabilities(caps) => apply_capabilities(w, &caps),
        UiUpdate::Cameras { cameras, active } => apply_cameras(w, &cameras, &active),
        UiUpdate::Disconnected => apply_status_banner(w, "disconnected"),
        // Intercepted by the state-sync loop before reaching here.
        UiUpdate::DaemonQuit => {}
    }
}

/// Route an effect's params to whichever page owns it.
fn apply_effect(w: &Widgets, id: &str, params: &VariantMap) {
    match id {
        "center_stage" => w.center_stage.apply(params),
        "portrait_blur" | "bg_replace" => w.background.apply(id, params),
        "studio_light" => w.studio_light.apply(params),
        _ => apply_enabled(w, id, params),
    }
}

/// Banners are for states the user should act on; running/idle/starting are
/// normal so the banner stays hidden for them.
fn apply_status_banner(w: &Widgets, status: &str) {
    let message = match status {
        "stopped" => Some("Pipeline stopped"),
        "error" => Some("The pipeline reported an error"),
        "disconnected" => Some("OpenEffects daemon is not running, please start it..."),
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
    let ready = caps
        .get("models_ready")
        .and_then(value_as_bool)
        .unwrap_or(false);
    let vcam = caps
        .get("virtual_camera")
        .and_then(value_as_string)
        .unwrap_or_else(|| "—".into());

    w.prefs.virtual_cam_row.set_subtitle(&vcam);

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
    *w.prefs.camera_ids.borrow_mut() = cameras.iter().map(|c| c.id.clone()).collect();

    let active_idx = cameras.iter().position(|c| c.id == active).unwrap_or(0) as u32;
    w.prefs.camera_combo.block_signal(&w.prefs.camera_handler);
    w.prefs.camera_combo.set_model(Some(&model));
    w.prefs.camera_combo.set_selected(active_idx);
    w.prefs.camera_combo.unblock_signal(&w.prefs.camera_handler);
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
