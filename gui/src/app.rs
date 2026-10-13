use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::ApplicationWindow;
use adw::glib;
use adw::prelude::*;
use gtk::gio;
use shared::dbus::{EFFECT_IDS, VariantMap, value_as_bool, value_as_str_array, value_as_string};

use crate::about;
use crate::config::GuiConfig;
use crate::dbus_client::{CameraInfo, CmdTx, GuiCommand, UiUpdate};
use crate::pages::{background, center_stage, reactions, studio_light};
use crate::preferences;
use crate::preview::{self, Preview};

/// Width of the controls column beside the preview in the wide layout.
const CONTROLS_WIDTH: i32 = 420;
/// Preview height when the layout stacks it above the controls.
const NARROW_PREVIEW_HEIGHT: i32 = 224;
/// Below this window width the preview moves above the controls.
const NARROW_BREAKPOINT: &str = "max-width: 760sp";

struct Widgets {
    banner: adw::Banner,
    center_stage: center_stage::Widgets,
    background: background::Widgets,
    studio_light: studio_light::Widgets,
    reactions: reactions::Widgets,
    prefs: preferences::Widgets,
    preview: Rc<Preview>,
    bypass_switch: gtk::Switch,
    bypass_handler: glib::SignalHandlerId,
    bypass_label: gtk::Label,
    /// The effect cards, dimmed while every effect is bypassed.
    cards: gtk::Box,
    /// True while "Hold to compare" is pressed; the resulting bypass echoes
    /// are transient and must not flip the master switch.
    comparing: Rc<Cell<bool>>,
}

pub fn build_window(
    app: &adw::Application,
    cmd_tx: CmdTx,
    update_rx: async_channel::Receiver<UiUpdate>,
    config: GuiConfig,
) {
    install_css();
    let gui_config = Rc::new(RefCell::new(config));

    // Preferences dialog (camera, engine info, model library, startup)
    let (prefs_dialog, prefs_widgets) = preferences::build(&cmd_tx, Rc::clone(&gui_config));

    // Effect cards, stacked in the controls column
    let cs_widgets = center_stage::build(&cmd_tx);
    let bg_widgets = background::build(&cmd_tx);
    let sl_widgets = studio_light::build(&cmd_tx);
    let rx_widgets = reactions::build(&cmd_tx);
    bg_widgets.card.set_expanded(true);

    let cards = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(8)
        .margin_end(16)
        .build();
    cards.append(&cs_widgets.card.list);
    cards.append(&bg_widgets.card.list);
    cards.append(&sl_widgets.card.list);
    cards.append(&rx_widgets.card.list);

    let controls = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(CONTROLS_WIDTH)
        .vexpand(true)
        .child(&cards)
        .build();

    // Live preview, leading the window
    let preview = preview::build(&cmd_tx, Rc::clone(&prefs_widgets.camera_ids));
    preview.frame.set_margin_top(16);
    preview.frame.set_margin_bottom(16);
    preview.frame.set_margin_start(16);
    preview.frame.set_margin_end(8);
    preview.frame.set_hexpand(true);
    preview.frame.set_vexpand(true);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    body.append(&preview.frame);
    body.append(&controls);

    // Primary menu (Preferences / About), right side of the header
    let menu = gio::Menu::new();
    menu.append(Some("Preferences"), Some("win.preferences"));
    menu.append(Some("About vinoOpenEffects"), Some("win.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();

    // Master effects switch (left of header): off = passthrough camera
    let bypass_label = gtk::Label::builder()
        .label("Effects on")
        .css_classes(["heading"])
        .build();
    let bypass_switch = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .active(true)
        .tooltip_text("Toggle all effects. Off passes the camera through untouched.")
        .build();
    bypass_switch.update_property(&[gtk::accessible::Property::Label("All effects")]);
    let bypass_handler = {
        let cmd_tx = cmd_tx.clone();
        let bypass_label = bypass_label.clone();
        bypass_switch.connect_active_notify(move |sw| {
            set_effects_label(&bypass_label, sw.is_active());
            // Switch on = effects active = bypass off.
            let _ = cmd_tx.send(GuiCommand::SetBypass {
                on: !sw.is_active(),
            });
        })
    };
    let bypass_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    bypass_box.append(&bypass_switch);
    bypass_box.append(&bypass_label);

    let header = adw::HeaderBar::new();
    header.pack_start(&bypass_box);
    header.pack_end(&menu_button);

    // Connection-trouble banner, sits just under the header.
    let banner = adw::Banner::builder().revealed(false).build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(&body));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("vinoOpenEffects")
        .default_width(1120)
        .default_height(720)
        .width_request(360)
        .height_request(560)
        .content(&toolbar)
        .build();

    // Narrow windows: stack a shorter preview above the controls.
    if let Ok(condition) = adw::BreakpointCondition::parse(NARROW_BREAKPOINT) {
        let breakpoint = adw::Breakpoint::new(condition);
        breakpoint.add_setter(
            &body,
            "orientation",
            Some(&gtk::Orientation::Vertical.to_value()),
        );
        breakpoint.add_setter(&preview.frame, "vexpand", Some(&false.to_value()));
        breakpoint.add_setter(
            &preview.frame,
            "height-request",
            Some(&NARROW_PREVIEW_HEIGHT.to_value()),
        );
        breakpoint.add_setter(&preview.frame, "margin-end", Some(&16.to_value()));
        breakpoint.add_setter(&preview.frame, "margin-bottom", Some(&0.to_value()));
        breakpoint.add_setter(&controls, "width-request", Some(&(-1).to_value()));
        breakpoint.add_setter(&cards, "margin-start", Some(&16.to_value()));
        breakpoint.add_setter(&preview.live_label, "label", Some(&"Live".to_value()));
        breakpoint.add_setter(&preview.node_pill, "visible", Some(&false.to_value()));
        window.add_breakpoint(breakpoint);
    }

    // Primary menu actions
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

    let comparing = Rc::new(Cell::new(false));
    wire_compare(&preview.compare, &bypass_switch, &cmd_tx, &comparing);

    let widgets = Rc::new(Widgets {
        banner,
        center_stage: cs_widgets,
        background: bg_widgets,
        studio_light: sl_widgets,
        reactions: rx_widgets,
        prefs: prefs_widgets,
        preview,
        bypass_switch,
        bypass_handler,
        bypass_label,
        cards,
        comparing,
    });

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
            let keep_running = gui_config.borrow().keep_running_in_background;
            if keep_running || shared::systemd::is_enabled() {
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

/// Press-and-hold on `button` bypasses every effect until release, so the
/// preview shows the untouched camera. Does nothing while effects are already
/// off. The gesture claims the press so the release always reaches it, even
/// when the pointer leaves the button.
fn wire_compare(
    button: &gtk::Button,
    bypass_switch: &gtk::Switch,
    cmd_tx: &CmdTx,
    comparing: &Rc<Cell<bool>>,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let bypass_switch = bypass_switch.clone();
        let cmd_tx = cmd_tx.clone();
        let comparing = Rc::clone(comparing);
        gesture.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if !bypass_switch.is_active() || comparing.get() {
                return;
            }
            comparing.set(true);
            if let Some(widget) = gesture.widget() {
                widget.add_css_class("oe-comparing");
            }
            let _ = cmd_tx.send(GuiCommand::SetBypass { on: true });
        });
    }
    let end = {
        let cmd_tx = cmd_tx.clone();
        let comparing = Rc::clone(comparing);
        move |gesture: &gtk::GestureClick| {
            if !comparing.replace(false) {
                return;
            }
            if let Some(widget) = gesture.widget() {
                widget.remove_css_class("oe-comparing");
            }
            let _ = cmd_tx.send(GuiCommand::SetBypass { on: false });
        }
    };
    {
        let end = end.clone();
        gesture.connect_released(move |gesture, _, _, _| end(gesture));
    }
    gesture.connect_cancel(move |gesture, _| end(gesture));
    button.add_controller(gesture);
}

fn set_effects_label(label: &gtk::Label, on: bool) {
    label.set_label(if on { "Effects on" } else { "Effects off" });
}

/// App-wide styling for the preview frame, its floating pills and the
/// effect-card icons.
fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".oe-preview-frame { border-radius: 18px; background-color: #0b0b0d; } \
         .oe-preview-status { color: #d8d8de; } \
         .oe-pill, .oe-pill-button, .oe-pill-dropdown > button { \
           border-radius: 999px; min-height: 34px; padding: 0 12px; \
           background-color: rgba(14, 14, 17, 0.66); color: #f2f2f5; \
           border: 1px solid rgba(255, 255, 255, 0.14); font-size: 13px; } \
         .oe-pill-button:hover, .oe-pill-dropdown > button:hover { \
           background-color: rgba(30, 30, 36, 0.8); } \
         .oe-pill-button.oe-comparing { background-color: rgba(60, 60, 70, 0.9); } \
         .oe-live-dot { min-width: 8px; min-height: 8px; border-radius: 50%; \
           background-color: @success_color; } \
         .oe-effect-icon { min-width: 38px; min-height: 38px; border-radius: 10px; \
           background-color: alpha(currentColor, 0.08); } \
         .oe-effect-icon.oe-on { background-color: alpha(@success_color, 0.16); \
           color: @success_color; } \
         .oe-emoji-btn { font-size: 24px; min-height: 52px; padding: 0; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

// Apply daemon state updates

fn apply_update(w: &Widgets, update: UiUpdate) {
    match update {
        UiUpdate::AllState(state) => {
            for id in EFFECT_IDS {
                let params = extract_effect_params(&state, id);
                apply_effect(w, id, &params);
            }
            if let Some(on) = state.get("bypass").and_then(value_as_bool) {
                apply_bypass(w, on);
            }
        }
        UiUpdate::EffectChanged { id, params } => apply_effect(w, &id, &params),
        UiUpdate::Bypass(on) => apply_bypass(w, on),
        UiUpdate::Status(status) => apply_status_banner(w, &status),
        UiUpdate::InferenceChanged { active, detail } => {
            apply_inference_changed(w, &active, &detail);
        }
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
        "reactions" => w.reactions.apply(params),
        _ => {}
    }
}

/// Banners are for states the user should act on; running/idle/starting are
/// normal so the banner stays hidden for them.
fn apply_status_banner(w: &Widgets, status: &str) {
    let message = match status {
        "stopped" => Some("Pipeline stopped"),
        "error" => Some("The pipeline reported an error"),
        "disconnected" => Some("vinoOpenEffects daemon is not running, please start it..."),
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

    w.prefs.virtual_cam_label.set_label(&vcam);
    w.preview.node_pill.set_label(&vcam);

    let (pill_text, pill_class, stale_class) = if ready {
        ("Ready", "success", "warning")
    } else {
        ("Missing", "warning", "success")
    };
    for row in w.prefs.model_rows.values() {
        if let Some(pill) = unsafe { row.data::<gtk::Label>("pill") } {
            let pill = unsafe { pill.as_ref() };
            pill.set_label(pill_text);
            pill.remove_css_class("dim-label");
            pill.remove_css_class(stale_class);
            pill.add_css_class(pill_class);
        }
    }

    let inf_req = caps.get("inference_requested").and_then(value_as_string);
    let inf_act = caps
        .get("inference_active")
        .and_then(value_as_string)
        .unwrap_or_else(|| "cpu".into());
    let inf_avail = caps
        .get("inference_available")
        .and_then(value_as_str_array)
        .unwrap_or_default();
    let inf_detail = caps.get("inference_detail").and_then(value_as_string);
    apply_inference(
        w,
        inf_req.as_deref(),
        &inf_act,
        &inf_avail,
        inf_detail.as_deref(),
    );
}

fn apply_inference(
    w: &Widgets,
    requested: Option<&str>,
    active: &str,
    available: &[String],
    detail: Option<&str>,
) {
    let has_npu = available.iter().any(|s| s == "npu");
    w.prefs.inference_toggle_npu.set_enabled(has_npu);
    if !has_npu {
        let tip = detail.unwrap_or("Intel NPU hardware or OpenVINO execution provider unavailable");
        w.prefs.inference_toggle_npu.set_tooltip(tip);
    } else {
        w.prefs.inference_toggle_npu.set_tooltip("");
    }

    if let Some(req) = requested {
        w.prefs
            .inference_group
            .block_signal(&w.prefs.inference_handler);
        w.prefs.inference_group.set_active_name(Some(req));
        w.prefs
            .inference_group
            .unblock_signal(&w.prefs.inference_handler);
    }

    let (badge_text, badge_class) = match active {
        "npu" => ("Active: NPU", "success"),
        "mixed" => ("Active: Mixed", "accent"),
        _ => ("Active: CPU", "dim-label"),
    };
    w.prefs.inference_badge.set_label(badge_text);
    w.prefs.inference_badge.remove_css_class("dim-label");
    w.prefs.inference_badge.remove_css_class("success");
    w.prefs.inference_badge.remove_css_class("warning");
    w.prefs.inference_badge.remove_css_class("accent");
    w.prefs.inference_badge.add_css_class(badge_class);

    if let Some(d) = detail {
        if !d.is_empty() {
            w.prefs
                .inference_row
                .set_subtitle(&format!("Falling back to CPU: {d}"));
        } else {
            w.prefs
                .inference_row
                .set_subtitle("Auto selects NPU when available, falls back to CPU");
        }
    } else {
        w.prefs
            .inference_row
            .set_subtitle("Auto selects NPU when available, falls back to CPU");
    }
}

fn apply_inference_changed(w: &Widgets, active: &str, detail: &str) {
    let (badge_text, badge_class) = match active {
        "npu" => ("Active: NPU", "success"),
        "mixed" => ("Active: Mixed", "accent"),
        _ => ("Active: CPU", "dim-label"),
    };
    w.prefs.inference_badge.set_label(badge_text);
    w.prefs.inference_badge.remove_css_class("dim-label");
    w.prefs.inference_badge.remove_css_class("success");
    w.prefs.inference_badge.remove_css_class("warning");
    w.prefs.inference_badge.remove_css_class("accent");
    w.prefs.inference_badge.add_css_class(badge_class);

    if !detail.is_empty() {
        w.prefs
            .inference_row
            .set_subtitle(&format!("Falling back to CPU: {detail}"));
    } else {
        w.prefs
            .inference_row
            .set_subtitle("Auto selects NPU when available, falls back to CPU");
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

    let dropdown = &w.preview.camera_dropdown;
    dropdown.block_signal(&w.preview.camera_handler);
    dropdown.set_model(Some(&model));
    dropdown.set_selected(active_idx);
    dropdown.unblock_signal(&w.preview.camera_handler);
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

fn apply_bypass(w: &Widgets, on: bool) {
    // Transient bypass from "Hold to compare": leave the controls alone.
    if w.comparing.get() {
        return;
    }
    w.bypass_switch.block_signal(&w.bypass_handler);
    w.bypass_switch.set_active(!on);
    w.bypass_switch.unblock_signal(&w.bypass_handler);
    set_effects_label(&w.bypass_label, !on);
    w.cards.set_sensitive(!on);
    w.preview.compare.set_sensitive(!on);
}
