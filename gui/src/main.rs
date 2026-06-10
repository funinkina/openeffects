mod dbus_client;

use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use adw::{glib, ApplicationWindow};
use dbus_client::{GuiCommand, UiUpdate};
use shared::dbus::{
    value_as_bool, value_as_i32, value_as_string, value_as_u32, VariantMap, EFFECT_IDS,
};
use tokio::sync::mpsc;

const APP_ID: &str = "org.openeffects.OpenEffects";

/// (stored value, display label)
const ZOOM_LEVELS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("subtle", "Subtle"),
    ("normal", "Normal"),
    ("tight", "Tight"),
];

const FRAMING_MODES: &[(&str, &str)] = &[("single", "Single Face"), ("group", "Group Framing")];

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

/// A combo-row parameter: maps a finite set of `(stored_value, label)` pairs to
/// a selection index.
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
    window_title: adw::WindowTitle,
    /// effect id -> (enable switch, its notify::active handler)
    switches: HashMap<&'static str, (adw::SwitchRow, glib::SignalHandlerId)>,
    /// "{effect_id}.{param_key}" -> param widget
    params: HashMap<String, ParamWidget>,
}

impl Widgets {
    fn new(window_title: adw::WindowTitle) -> Self {
        Self {
            window_title,
            switches: HashMap::new(),
            params: HashMap::new(),
        }
    }
}

fn build_window(
    app: &adw::Application,
    cmd_tx: mpsc::UnboundedSender<GuiCommand>,
    update_rx: async_channel::Receiver<UiUpdate>,
) {
    let window_title = adw::WindowTitle::new("OpenEffects", "Connecting…");
    let header = adw::HeaderBar::builder()
        .title_widget(&window_title)
        .build();

    let page = adw::PreferencesPage::new();
    let mut widgets = Widgets::new(window_title);

    build_center_stage_group(&page, &mut widgets, &cmd_tx);
    build_portrait_blur_group(&page, &mut widgets, &cmd_tx);
    build_bg_replace_group(&page, &mut widgets, &cmd_tx);
    build_studio_light_group(&page, &mut widgets, &cmd_tx);
    build_reactions_group(&page, &mut widgets, &cmd_tx);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&page));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("OpenEffects")
        .default_width(480)
        .default_height(640)
        .content(&toolbar_view)
        .build();

    let widgets = Rc::new(widgets);
    glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = update_rx.recv().await {
            apply_update(&widgets, update);
        }
    });

    window.present();
}

fn build_center_stage_group(
    page: &adw::PreferencesPage,
    w: &mut Widgets,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
) {
    let group = adw::PreferencesGroup::builder()
        .title("Center Stage")
        .description("Keep the subject framed and centered as they move")
        .build();
    add_switch_row(
        &group,
        "Center Stage",
        "Track and crop to keep the subject in frame",
        "center_stage",
        cmd_tx,
        w,
    );
    add_combo_row(
        &group,
        "Framing",
        ZOOM_LEVELS,
        "center_stage",
        "zoom",
        cmd_tx,
        w,
    );
    add_combo_row(
        &group,
        "Mode",
        FRAMING_MODES,
        "center_stage",
        "mode",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Crop Top",
        "Pixels removed from the top edge",
        0.0,
        512.0,
        1.0,
        "center_stage",
        "crop_top",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Crop Bottom",
        "Pixels removed from the bottom edge",
        0.0,
        512.0,
        1.0,
        "center_stage",
        "crop_bottom",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Crop Left",
        "Pixels removed from the left edge",
        0.0,
        512.0,
        1.0,
        "center_stage",
        "crop_left",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Crop Right",
        "Pixels removed from the right edge",
        0.0,
        512.0,
        1.0,
        "center_stage",
        "crop_right",
        cmd_tx,
        w,
    );
    page.add(&group);
}

fn build_portrait_blur_group(
    page: &adw::PreferencesPage,
    w: &mut Widgets,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
) {
    let group = adw::PreferencesGroup::builder()
        .title("Portrait Blur")
        .description("Blur the background while keeping the subject sharp")
        .build();
    add_switch_row(
        &group,
        "Portrait Blur",
        "Blur everything behind the subject",
        "portrait_blur",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Strength",
        "Blur intensity, 0–100",
        0.0,
        100.0,
        1.0,
        "portrait_blur",
        "strength",
        cmd_tx,
        w,
    );
    page.add(&group);
}

fn build_bg_replace_group(
    page: &adw::PreferencesPage,
    w: &mut Widgets,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
) {
    let group = adw::PreferencesGroup::builder()
        .title("Background Replace")
        .description("Replace the background with an image or solid color")
        .build();
    add_switch_row(
        &group,
        "Background Replace",
        "Background picker is available on the Backgrounds page",
        "bg_replace",
        cmd_tx,
        w,
    );
    page.add(&group);
}

fn build_studio_light_group(
    page: &adw::PreferencesPage,
    w: &mut Widgets,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
) {
    let group = adw::PreferencesGroup::builder()
        .title("Studio Light")
        .description("Subtly brighten and separate the subject")
        .build();
    add_switch_row(
        &group,
        "Studio Light",
        "Brighten and add contrast to the subject",
        "studio_light",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Intensity",
        "Overall effect strength, 0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "intensity",
        cmd_tx,
        w,
    );
    add_spin_row_i32(
        &group,
        "Brightness",
        "-100 (darker) to 100 (brighter)",
        -100.0,
        100.0,
        1.0,
        "studio_light",
        "brightness",
        cmd_tx,
        w,
    );
    add_spin_row_u32(
        &group,
        "Contrast",
        "0–100",
        0.0,
        100.0,
        1.0,
        "studio_light",
        "contrast",
        cmd_tx,
        w,
    );
    page.add(&group);
}

fn build_reactions_group(
    page: &adw::PreferencesPage,
    w: &mut Widgets,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
) {
    let group = adw::PreferencesGroup::builder()
        .title("Reactions")
        .description("Trigger animated overlays with hand gestures")
        .build();
    add_switch_row(
        &group,
        "Reactions",
        "Off by default — enable to react to hand gestures",
        "reactions",
        cmd_tx,
        w,
    );
    page.add(&group);
}

fn add_switch_row(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    id: &'static str,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
    w: &mut Widgets,
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
    w.switches.insert(id, (row, handler));
}

fn add_combo_row(
    group: &adw::PreferencesGroup,
    title: &str,
    options: &'static [(&'static str, &'static str)],
    id: &'static str,
    key: &'static str,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
    w: &mut Widgets,
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
    w.params.insert(
        format!("{id}.{key}"),
        ParamWidget::Combo(ComboParam {
            row,
            options,
            handler,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn add_spin_row_u32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
    w: &mut Widgets,
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
    w.params.insert(
        format!("{id}.{key}"),
        ParamWidget::Spin(SpinParam::U32 { row, handler }),
    );
}

#[allow(clippy::too_many_arguments)]
fn add_spin_row_i32(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    id: &'static str,
    key: &'static str,
    cmd_tx: &mpsc::UnboundedSender<GuiCommand>,
    w: &mut Widgets,
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
    w.params.insert(
        format!("{id}.{key}"),
        ParamWidget::Spin(SpinParam::I32 { row, handler }),
    );
}

fn apply_update(w: &Widgets, update: UiUpdate) {
    match update {
        UiUpdate::AllState(state) => {
            for id in EFFECT_IDS {
                let params = extract_effect_params(&state, id);
                apply_enabled(w, id, &params);
                apply_params(w, id, &params);
            }
        }
        UiUpdate::EffectChanged { id, params } => {
            apply_enabled(w, &id, &params);
            apply_params(w, &id, &params);
        }
        UiUpdate::Status(status) => {
            w.window_title.set_subtitle(&status_subtitle(&status));
        }
        UiUpdate::Disconnected => {
            w.window_title.set_subtitle("Disconnected — retrying…");
        }
    }
}

/// `GetAllState()` returns keys prefixed with `"{effect_id}."`; strip that
/// prefix so the result has the same shape as an `EffectChanged` payload.
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
