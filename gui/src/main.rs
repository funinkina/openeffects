mod dbus_client;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use adw::{glib, ApplicationWindow};
use dbus_client::{CameraInfo, GuiCommand, UiUpdate};
use gtk::gio;
use shared::dbus::{
    value_as_bool, value_as_i32, value_as_string, value_as_u32, VariantMap, EFFECT_IDS,
};
use tokio::sync::mpsc;

const APP_ID: &str = "org.openeffects.OpenEffects";

type CmdTx = mpsc::UnboundedSender<GuiCommand>;

/// (stored value, display label)
const ZOOM_LEVELS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("subtle", "Subtle"),
    ("normal", "Normal"),
    ("tight", "Tight"),
];

const FRAMING_MODES: &[(&str, &str)] = &[("single", "Single Face"), ("group", "Group Framing")];

/// Built-in solid-color backgrounds (label, `#RRGGBB`).
const BG_PRESETS: &[(&str, &str)] = &[
    ("Charcoal", "#1e1e2e"),
    ("Slate", "#2e3440"),
    ("Deep Blue", "#1b3a5b"),
    ("Forest", "#1f3d2b"),
    ("Plum", "#3b2e4a"),
    ("Warm Gray", "#3a3a3a"),
];

/// Bundled models listed on the Model Library page (id, display name, purpose).
const BUNDLED_MODELS: &[(&str, &str, &str)] = &[
    (
        "selfie_segmentation",
        "MediaPipe Selfie Segmentation",
        "Portrait blur & background replace",
    ),
    ("yunet", "YuNet", "Face detection for Center Stage"),
];

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<GuiCommand>();
    let (update_tx, update_rx) = async_channel::unbounded::<UiUpdate>();
    dbus_client::spawn(cmd_rx, update_tx);

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| build_window(app, cmd_tx.clone(), update_rx.clone()));
    app.run()
}

/// A combo-row parameter: maps `(stored_value, label)` pairs to a selection index.
struct ComboParam {
    row: adw::ComboRow,
    options: &'static [(&'static str, &'static str)],
    handler: glib::SignalHandlerId,
}

/// A spin-row parameter holding either an unsigned or signed integer.
enum SpinParam {
    U32 {
        row: adw::SpinRow,
        handler: glib::SignalHandlerId,
    },
    I32 {
        row: adw::SpinRow,
        handler: glib::SignalHandlerId,
    },
}

enum ParamWidget {
    Combo(ComboParam),
    Spin(SpinParam),
}

struct Widgets {
    page_title: adw::WindowTitle,
    status: RefCell<String>,
    /// effect id -> (enable switch, its notify::active handler)
    switches: HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>,
    /// "{effect_id}.{param_key}" -> param widget
    params: HashMap<String, ParamWidget>,
    // Camera page
    camera_combo: adw::ComboRow,
    camera_handler: glib::SignalHandlerId,
    camera_ids: RefCell<Vec<String>>,
    virtual_cam_row: adw::ActionRow,
    // Backgrounds page
    bg_current: adw::ActionRow,
    // Model Library page
    model_rows: HashMap<&'static str, adw::ActionRow>,
    // About page
    about_tier: adw::ActionRow,
    about_ep: adw::ActionRow,
    about_models: adw::ActionRow,
    about_vcam: adw::ActionRow,
}

fn build_window(
    app: &adw::Application,
    cmd_tx: CmdTx,
    update_rx: async_channel::Receiver<UiUpdate>,
) {
    let page_title = adw::WindowTitle::new("Effects", "Connecting…");

    // ── Content stack (one widget per page) ──────────────────────────────────
    let content_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();

    let mut params = HashMap::new();
    let mut switches = HashMap::new();

    let effects_page = build_effects_page(&cmd_tx, &mut switches, &mut params);
    content_stack.add_named(&effects_page, Some("effects"));

    let (camera_page, camera_combo, camera_handler, virtual_cam_row) = build_camera_page(&cmd_tx);
    content_stack.add_named(&camera_page, Some("camera"));

    let (backgrounds_page, bg_current) = build_backgrounds_page(&cmd_tx);
    content_stack.add_named(&backgrounds_page, Some("backgrounds"));

    let (models_page, model_rows) = build_models_page();
    content_stack.add_named(&models_page, Some("models"));

    let (about_page, about_tier, about_ep, about_models, about_vcam) = build_about_page();
    content_stack.add_named(&about_page, Some("about"));

    // ── Sidebar (page switcher) ──────────────────────────────────────────────
    let pages: &[(&str, &str)] = &[
        ("effects", "Effects"),
        ("camera", "Camera"),
        ("backgrounds", "Backgrounds"),
        ("models", "Model Library"),
        ("about", "About"),
    ];
    let listbox = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    for (name, label) in pages {
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(
            &gtk::Label::builder().label(*label).xalign(0.0).build(),
        ));
        unsafe { row.set_data("page", name.to_string()) };
        listbox.append(&row);
    }
    {
        let content_stack = content_stack.clone();
        let page_title = page_title.clone();
        let pages_map: HashMap<String, String> = pages
            .iter()
            .map(|(n, l)| (n.to_string(), l.to_string()))
            .collect();
        listbox.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let name = unsafe { row.data::<String>("page") };
            if let Some(name) = name {
                let name = unsafe { name.as_ref() };
                content_stack.set_visible_child_name(name);
                if let Some(label) = pages_map.get(name) {
                    page_title.set_title(label);
                }
            }
        });
    }
    listbox.select_row(listbox.row_at_index(0).as_ref());

    let sidebar_header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("OpenEffects", ""))
        .build();
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&listbox));
    let sidebar_page = adw::NavigationPage::builder()
        .title("OpenEffects")
        .child(&sidebar_view)
        .build();

    let content_header = adw::HeaderBar::builder().title_widget(&page_title).build();
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
    content_view.set_content(Some(&content_stack));
    let content_page = adw::NavigationPage::builder()
        .title("Effects")
        .child(&content_view)
        .build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .build();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("OpenEffects")
        .default_width(720)
        .default_height(640)
        .content(&split)
        .build();

    let widgets = Rc::new(Widgets {
        page_title,
        status: RefCell::new("connecting".into()),
        switches,
        params,
        camera_combo,
        camera_handler,
        camera_ids: RefCell::new(Vec::new()),
        virtual_cam_row,
        bg_current,
        model_rows,
        about_tier,
        about_ep,
        about_models,
        about_vcam,
    });

    glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = update_rx.recv().await {
            apply_update(&widgets, update);
        }
    });

    window.present();
}

// ── Effects page ─────────────────────────────────────────────────────────────

fn build_effects_page(
    cmd_tx: &CmdTx,
    switches: &mut HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>,
    params: &mut HashMap<String, ParamWidget>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let cs = pref_group(
        "Center Stage",
        "Keep the subject framed and centered as they move",
    );
    add_switch(
        &cs,
        "Center Stage",
        "Track and crop to keep the subject in frame",
        "center_stage",
        cmd_tx,
        switches,
    );
    add_combo(
        &cs,
        "Framing",
        ZOOM_LEVELS,
        "center_stage",
        "zoom",
        cmd_tx,
        params,
    );
    add_combo(
        &cs,
        "Mode",
        FRAMING_MODES,
        "center_stage",
        "mode",
        cmd_tx,
        params,
    );
    page.add(&cs);

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
        "Pick a background on the Backgrounds page",
        "bg_replace",
        cmd_tx,
        switches,
    );
    page.add(&bg);

    let sl = pref_group("Studio Light", "Subtly brighten and separate the subject");
    add_switch(
        &sl,
        "Studio Light",
        "Brighten and add contrast to the subject",
        "studio_light",
        cmd_tx,
        switches,
    );
    add_spin_u32(
        &sl,
        "Intensity",
        "Overall effect strength, 0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "intensity",
        cmd_tx,
        params,
    );
    add_spin_i32(
        &sl,
        "Brightness",
        "-100 (darker) to 100 (brighter)",
        -100.0,
        100.0,
        1.0,
        "studio_light",
        "brightness",
        cmd_tx,
        params,
    );
    add_spin_u32(
        &sl,
        "Contrast",
        "0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "contrast",
        cmd_tx,
        params,
    );
    page.add(&sl);

    let rx = pref_group("Reactions", "Trigger animated overlays with hand gestures");
    add_switch(
        &rx,
        "Reactions",
        "Off by default — enable to react to hand gestures",
        "reactions",
        cmd_tx,
        switches,
    );
    page.add(&rx);

    page
}

// ── Camera page ────────────────────────────────────────────────────────────────

fn build_camera_page(
    cmd_tx: &CmdTx,
) -> (
    adw::PreferencesPage,
    adw::ComboRow,
    glib::SignalHandlerId,
    adw::ActionRow,
) {
    let page = adw::PreferencesPage::new();

    let group = pref_group(
        "Camera",
        "Choose the physical camera OpenEffects captures from",
    );
    let camera_combo = adw::ComboRow::builder()
        .title("Source")
        .model(&gtk::StringList::new(&["No cameras found"]))
        .build();
    let camera_ids = Rc::new(RefCell::new(Vec::<String>::new()));
    let handler = {
        let cmd_tx = cmd_tx.clone();
        let camera_ids = Rc::clone(&camera_ids);
        camera_combo.connect_selected_notify(move |row| {
            let ids = camera_ids.borrow();
            if let Some(id) = ids.get(row.selected() as usize) {
                let _ = cmd_tx.send(GuiCommand::SelectCamera { id: id.clone() });
            }
        })
    };
    // Stash the ids vec on the combo so the update handler can refresh it.
    unsafe { camera_combo.set_data("ids", camera_ids) };
    group.add(&camera_combo);
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

    (page, camera_combo, handler, virtual_cam_row)
}

// ── Backgrounds page ─────────────────────────────────────────────────────────

fn build_backgrounds_page(cmd_tx: &CmdTx) -> (adw::PreferencesPage, adw::ActionRow) {
    let page = adw::PreferencesPage::new();

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

// ── Model Library page ───────────────────────────────────────────────────────

fn build_models_page() -> (adw::PreferencesPage, HashMap<&'static str, adw::ActionRow>) {
    let page = adw::PreferencesPage::new();
    let group = pref_group(
        "Bundled Models",
        "Run scripts/fetch-models.sh to install missing models",
    );
    let mut rows = HashMap::new();
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
        group.add(&row);
        rows.insert(*id, row);
    }
    page.add(&group);
    (page, rows)
}

// ── About page ───────────────────────────────────────────────────────────────

fn build_about_page() -> (
    adw::PreferencesPage,
    adw::ActionRow,
    adw::ActionRow,
    adw::ActionRow,
    adw::ActionRow,
) {
    let page = adw::PreferencesPage::new();

    let group = pref_group("OpenEffects", "Linux-native webcam effects engine");
    let version = adw::ActionRow::builder()
        .title("Version")
        .subtitle(env!("CARGO_PKG_VERSION"))
        .build();
    group.add(&version);
    page.add(&group);

    let hw = pref_group("Engine", "Detected at daemon startup");
    let tier = adw::ActionRow::builder()
        .title("Hardware tier")
        .subtitle("—")
        .build();
    let ep = adw::ActionRow::builder()
        .title("Running on")
        .subtitle("—")
        .build();
    let models = adw::ActionRow::builder()
        .title("Models")
        .subtitle("—")
        .build();
    let vcam = adw::ActionRow::builder()
        .title("Virtual camera")
        .subtitle("—")
        .build();
    hw.add(&tier);
    hw.add(&ep);
    hw.add(&models);
    hw.add(&vcam);
    page.add(&hw);

    (page, tier, ep, models, vcam)
}

// ── Row builders ───────────────────────────────────────────────────────────────

fn pref_group(title: &str, description: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder()
        .title(title)
        .description(description)
        .build()
}

fn add_switch(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    id: &'static str,
    cmd_tx: &CmdTx,
    switches: &mut HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>,
) {
    let row = adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_active_notify(move |row| {
        let _ = cmd_tx.send(GuiCommand::SetEnabled {
            id: id.to_string(),
            on: row.is_active(),
        });
    });
    group.add(&row);
    switches.insert(id, (row, handler));
}

fn add_combo(
    group: &adw::PreferencesGroup,
    title: &str,
    options: &'static [(&'static str, &'static str)],
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    params: &mut HashMap<String, ParamWidget>,
) {
    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
    let model = gtk::StringList::new(&labels);
    let row = adw::ComboRow::builder().title(title).model(&model).build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_selected_notify(move |row| {
        let Some((value, _)) = options.get(row.selected() as usize) else {
            return;
        };
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::str_value(*value),
        });
    });
    group.add(&row);
    params.insert(
        format!("{id}.{key}"),
        ParamWidget::Combo(ComboParam {
            row,
            options,
            handler,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn add_spin_u32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    params: &mut HashMap<String, ParamWidget>,
) {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
    let row = adw::SpinRow::builder()
        .title(title)
        .subtitle(subtitle)
        .adjustment(&adjustment)
        .digits(0)
        .build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_value_notify(move |row| {
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::u32_value(row.value() as u32),
        });
    });
    group.add(&row);
    params.insert(
        format!("{id}.{key}"),
        ParamWidget::Spin(SpinParam::U32 { row, handler }),
    );
}

#[allow(clippy::too_many_arguments)]
fn add_spin_i32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &CmdTx,
    params: &mut HashMap<String, ParamWidget>,
) {
    let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
    let row = adw::SpinRow::builder()
        .title(title)
        .subtitle(subtitle)
        .adjustment(&adjustment)
        .digits(0)
        .build();
    let cmd_tx = cmd_tx.clone();
    let handler = row.connect_value_notify(move |row| {
        let _ = cmd_tx.send(GuiCommand::SetParam {
            id: id.to_string(),
            key: key.to_string(),
            value: shared::dbus::i32_value(row.value() as i32),
        });
    });
    group.add(&row);
    params.insert(
        format!("{id}.{key}"),
        ParamWidget::Spin(SpinParam::I32 { row, handler }),
    );
}

// ── Update application ─────────────────────────────────────────────────────────

fn apply_update(w: &Widgets, update: UiUpdate) {
    match update {
        UiUpdate::AllState(state) => {
            for id in EFFECT_IDS {
                let params = extract_effect_params(&state, id);
                apply_enabled(w, id, &params);
                apply_params(w, id, &params);
                if id == "bg_replace" {
                    if let Some(bg) = params.get("background").and_then(value_as_string) {
                        w.bg_current.set_subtitle(&bg_label(&bg));
                    }
                }
            }
        }
        UiUpdate::EffectChanged { id, params } => {
            apply_enabled(w, &id, &params);
            apply_params(w, &id, &params);
            if id == "bg_replace" {
                if let Some(bg) = params.get("background").and_then(value_as_string) {
                    w.bg_current.set_subtitle(&bg_label(&bg));
                }
            }
        }
        UiUpdate::Status(status) => {
            *w.status.borrow_mut() = status.clone();
            w.page_title.set_subtitle(&status_subtitle(&status));
        }
        UiUpdate::Capabilities(caps) => apply_capabilities(w, &caps),
        UiUpdate::Cameras { cameras, active } => apply_cameras(w, &cameras, &active),
        UiUpdate::Disconnected => {
            *w.status.borrow_mut() = "disconnected".into();
            w.page_title.set_subtitle("Disconnected — retrying…");
        }
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

    w.about_tier.set_subtitle(&tier.to_uppercase());
    w.about_ep.set_subtitle(&ep.to_uppercase());
    w.about_models.set_subtitle(if ready {
        "Installed"
    } else {
        "Not installed — run fetch-models.sh"
    });
    w.about_vcam.set_subtitle(&vcam);
    w.virtual_cam_row.set_subtitle(&vcam);

    let pill_text = if ready { "Ready" } else { "Missing" };
    for row in w.model_rows.values() {
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
    *w.camera_ids.borrow_mut() = cameras.iter().map(|c| c.id.clone()).collect();
    // Keep the combo's stashed ids in sync with the selection handler's copy.
    if let Some(ids) = unsafe { w.camera_combo.data::<Rc<RefCell<Vec<String>>>>("ids") } {
        let ids = unsafe { ids.as_ref() };
        *ids.borrow_mut() = cameras.iter().map(|c| c.id.clone()).collect();
    }

    let active_idx = cameras.iter().position(|c| c.id == active).unwrap_or(0) as u32;
    w.camera_combo.block_signal(&w.camera_handler);
    w.camera_combo.set_model(Some(&model));
    w.camera_combo.set_selected(active_idx);
    w.camera_combo.unblock_signal(&w.camera_handler);
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
            ParamWidget::Combo(ComboParam {
                row,
                options,
                handler,
            }) => {
                if let Some(s) = value_as_string(value) {
                    if let Some(idx) = options.iter().position(|(v, _)| *v == s) {
                        row.block_signal(handler);
                        row.set_selected(idx as u32);
                        row.unblock_signal(handler);
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

fn bg_label(bg: &str) -> String {
    if bg.is_empty() {
        "None".into()
    } else if let Some((label, _)) = BG_PRESETS.iter().find(|(_, hex)| *hex == bg) {
        format!("{label} ({bg})")
    } else {
        bg.to_string()
    }
}

fn status_subtitle(status: &str) -> String {
    match status {
        "running" => "Running".into(),
        "idle" => "Idle — no active consumer".into(),
        "starting" => "Starting…".into(),
        "stopped" => "Stopped".into(),
        "error" => "Error".into(),
        other => other.to_string(),
    }
}
