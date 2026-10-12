//! Background card: the effect switch in the header, then a Blur/Replace mode
//! control and, in Blur mode, a strength row. The card drives the two
//! underlying effects — `portrait_blur` and `bg_replace`. Only the controls
//! for the active mode are shown, and everything is disabled while it's off.
//!
//! The mode is *virtual*: `bg_replace.enabled` means Replace, otherwise
//! Blur, and changing it just toggles those two effects' `enabled` flags
//! (mutually exclusive). Replace mode offers a grid of built-in image
//! presets (plus a "choose your own" button) and a row of solid-color
//! swatches with a custom-color button.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk::RGBA;
use gtk::gio;
use shared::dbus::{VariantMap, value_as_bool, value_as_string, value_as_u32};

use crate::constants::{BG_BLUR, BG_IMAGE_PRESETS, BG_MODES, BG_PRESETS, BG_REPLACE, BLUR_LEVELS};
use crate::dbus_client::{CmdTx, GuiCommand};
use crate::widgets::{EffectCard, ToggleCtl, plain_row, section_label, toggle_row};

const CUSTOM_SWATCH_CLASS: &str = "oe-custom-swatch";
const CUSTOM_IMAGE_CLASS: &str = "oe-custom-image";

/// Size of the background-image preset thumbnails, sized so five fit across
/// the controls column.
const IMAGE_THUMB_WIDTH: i32 = 60;
const IMAGE_THUMB_HEIGHT: i32 = 40;

/// Widgets the state-sync layer keeps in sync with the daemon.
pub struct Widgets {
    pub card: EffectCard,
    mode_row: adw::ActionRow,
    mode: ToggleCtl,
    /// Blur-strength row, shown only in Blur mode.
    blur_row: adw::ActionRow,
    blur_strength: ToggleCtl,
    /// Rows shown only in Replace mode (image picker + color swatches).
    replace_rows: Vec<gtk::ListBoxRow>,
    /// Shows the file name of a custom (non-preset) image background.
    image_row: adw::ActionRow,
    /// Built-in image backgrounds: label, on-disk path and toggle button.
    image_presets: Vec<(&'static str, PathBuf, gtk::ToggleButton)>,
    swatches: Vec<gtk::ToggleButton>,
    custom_provider: gtk::CssProvider,
    /// Display name of the current replacement background, for the summary.
    bg_name: RefCell<String>,
    /// Guards programmatic widget updates so they don't echo to the daemon.
    applying: Rc<Cell<bool>>,
    blur_enabled: Cell<bool>,
    bg_enabled: Cell<bool>,
}

pub fn build(cmd_tx: &CmdTx) -> Widgets {
    install_swatch_css();
    let applying = Rc::new(Cell::new(false));

    // Mode toggle (blur / replace) and, for Blur, a strength row. Built
    // before the card so the switch closure can capture the toggle.
    let (mode_row, mode) = {
        let cmd_tx = cmd_tx.clone();
        let applying = applying.clone();
        toggle_row("Mode", "", BG_MODES, move |value| {
            if applying.get() {
                return;
            }
            set_mode(&cmd_tx, value == BG_REPLACE);
        })
    };
    let (blur_row, blur_strength) = {
        let cmd_tx = cmd_tx.clone();
        let applying = applying.clone();
        toggle_row("Strength", "", BLUR_LEVELS, move |value| {
            if applying.get() {
                return;
            }
            let strength: u32 = value.parse().unwrap_or(66);
            let _ = cmd_tx.send(GuiCommand::SetParam {
                id: "portrait_blur".into(),
                key: "strength".into(),
                value: shared::dbus::u32_value(strength),
            });
        })
    };

    let card = {
        let cmd_tx = cmd_tx.clone();
        let applying = applying.clone();
        let mode_toggle = mode.group.clone();
        EffectCard::new("Background", "view-conceal-symbolic", move |on| {
            if applying.get() {
                return;
            }
            if on {
                let replace = mode_toggle.active_name().as_deref() == Some(BG_REPLACE);
                set_mode(&cmd_tx, replace);
            } else {
                let _ = cmd_tx.send(GuiCommand::SetEnabled {
                    id: "portrait_blur".into(),
                    on: false,
                });
                let _ = cmd_tx.send(GuiCommand::SetEnabled {
                    id: "bg_replace".into(),
                    on: false,
                });
            }
        })
    };
    card.add_row(&mode_row);
    card.add_row(&blur_row);

    // Image presets and color swatches share one radio group, so picking
    // either kind of background visually deselects the other.
    let mut group_leader: Option<gtk::ToggleButton> = None;

    // replace: image presets
    let image_box = wrapping_grid(5);

    let mut image_presets = Vec::new();
    for (label, path) in ensure_preset_images() {
        let button = image_preset_button(label, &path);
        match &group_leader {
            Some(leader) => button.set_group(Some(leader)),
            None => group_leader = Some(button.clone()),
        }
        {
            let cmd_tx = cmd_tx.clone();
            let applying = applying.clone();
            let path_str = path.to_string_lossy().into_owned();
            button.connect_toggled(move |b| {
                if applying.get() || !b.is_active() {
                    return;
                }
                set_background(&cmd_tx, &path_str);
            });
        }
        image_box.append(&button);
        image_presets.push((label, path, button));
    }

    // "Add your own" button (not part of the radio group): always opens a
    // file picker.
    let browse = gtk::Button::builder()
        .tooltip_text("Choose Image…")
        .css_classes(["oe-swatch-btn", "flat"])
        .build();
    let browse_child = image_box_child(CUSTOM_IMAGE_CLASS);
    browse_child.append(&plus_icon());
    browse.set_child(Some(&browse_child));
    {
        let cmd_tx = cmd_tx.clone();
        browse.connect_clicked(move |b| open_image_dialog(b, &cmd_tx));
    }
    image_box.append(&browse);
    let images_row = plain_row(&labelled(&section_label("Images"), &image_box));
    card.add_row(&images_row);

    let image_row = adw::ActionRow::builder()
        .title("No image selected")
        .subtitle("—")
        .visible(false)
        .build();
    card.add_row(&image_row);

    // replace: color swatches
    let swatch_box = wrapping_grid(7);

    let mut swatches = Vec::new();
    for (i, (label, hex)) in BG_PRESETS.iter().enumerate() {
        let button = swatch_button(&format!("oe-swatch-{i}"), label);
        match &group_leader {
            Some(leader) => button.set_group(Some(leader)),
            None => group_leader = Some(button.clone()),
        }
        {
            let cmd_tx = cmd_tx.clone();
            let applying = applying.clone();
            let hex = hex.to_string();
            button.connect_toggled(move |b| {
                if applying.get() || !b.is_active() {
                    return;
                }
                set_background(&cmd_tx, &hex);
            });
        }
        swatch_box.append(&button);
        swatches.push(button);
    }

    // Custom-color button (not part of the radio group): always opens a picker.
    let custom_provider = gtk::CssProvider::new();
    add_provider(&custom_provider);
    let custom = gtk::Button::builder()
        .tooltip_text("Custom color…")
        .css_classes(["oe-swatch-btn", "flat"])
        .build();
    let custom_child = swatch_box_child(CUSTOM_SWATCH_CLASS);
    custom_child.append(&plus_icon());
    custom.set_child(Some(&custom_child));
    {
        let cmd_tx = cmd_tx.clone();
        let provider = custom_provider.clone();
        custom.connect_clicked(move |b| open_color_dialog(b, &cmd_tx, &provider));
    }
    swatch_box.append(&custom);
    let colors_row = plain_row(&labelled(&section_label("Solid colors"), &swatch_box));
    card.add_row(&colors_row);

    let widgets = Widgets {
        card,
        mode_row,
        mode,
        blur_row,
        blur_strength,
        replace_rows: vec![images_row, colors_row],
        image_row,
        image_presets,
        swatches,
        custom_provider,
        bg_name: RefCell::new(String::new()),
        applying,
        blur_enabled: Cell::new(false),
        bg_enabled: Cell::new(false),
    };
    widgets.update_visibility();
    widgets
}

impl Widgets {
    /// Apply an `EffectChanged`/`AllState` payload for `portrait_blur` or
    /// `bg_replace`.
    pub fn apply(&self, id: &str, params: &VariantMap) {
        self.applying.set(true);
        match id {
            "portrait_blur" => {
                if let Some(on) = params.get("enabled").and_then(value_as_bool) {
                    self.blur_enabled.set(on);
                }
                if let Some(strength) = params.get("strength").and_then(value_as_u32) {
                    self.blur_strength.set_value(&nearest_blur_level(strength));
                }
            }
            "bg_replace" => {
                if let Some(on) = params.get("enabled").and_then(value_as_bool) {
                    self.bg_enabled.set(on);
                }
                if let Some(bg) = params.get("background").and_then(value_as_string) {
                    self.apply_background(&bg);
                }
            }
            _ => {}
        }
        self.update_visibility();
        self.applying.set(false);
    }

    /// `bg_replace` wins; otherwise Blur. Used as the default mode shown
    /// while neither effect is enabled (switch off).
    fn current_mode(&self) -> &'static str {
        if self.bg_enabled.get() {
            return BG_REPLACE;
        }
        if self.blur_enabled.get() {
            return BG_BLUR;
        }
        match self.mode.group.active_name().as_deref() {
            Some(BG_REPLACE) => BG_REPLACE,
            _ => BG_BLUR,
        }
    }

    fn update_visibility(&self) {
        let on = self.blur_enabled.get() || self.bg_enabled.get();
        let mode = self.current_mode();

        self.card.set_on(on);
        self.mode.set_value(mode);
        self.mode_row.set_sensitive(on);
        self.blur_row.set_sensitive(on);
        self.blur_row.set_visible(mode == BG_BLUR);
        for row in &self.replace_rows {
            row.set_sensitive(on);
            row.set_visible(mode == BG_REPLACE);
        }
        self.image_row.set_sensitive(on);
        self.update_summary(on, mode);
    }

    fn update_summary(&self, on: bool, mode: &str) {
        let summary = if !on {
            "Off".to_string()
        } else if mode == BG_REPLACE {
            let name = self.bg_name.borrow();
            if name.is_empty() {
                "Replace".to_string()
            } else {
                format!("Replace · {name}")
            }
        } else {
            let strength = self.blur_strength.group.active_name();
            let label = BLUR_LEVELS
                .iter()
                .find(|(n, _)| Some(*n) == strength.as_deref())
                .map_or("Medium", |(_, label)| *label);
            format!("Blur · {label}")
        };
        self.card.set_summary(&summary);
    }

    /// Reflect the current `bg_replace.background` value in the swatch/image
    /// selection. Called with `applying` already set.
    fn apply_background(&self, bg: &str) {
        for button in &self.swatches {
            button.set_active(false);
        }
        for (_, _, button) in &self.image_presets {
            button.set_active(false);
        }
        let name = if bg.starts_with('#') {
            self.image_row.set_visible(false);
            if let Some(idx) = BG_PRESETS.iter().position(|(_, h)| *h == bg) {
                self.swatches[idx].set_active(true);
                BG_PRESETS[idx].0.to_string()
            } else {
                set_custom_color(&self.custom_provider, bg);
                "Custom color".to_string()
            }
        } else if !bg.is_empty() {
            if let Some((label, _, button)) = self
                .image_presets
                .iter()
                .find(|(_, path, _)| path.to_str() == Some(bg))
            {
                button.set_active(true);
                self.image_row.set_visible(false);
                label.to_string()
            } else {
                let path = std::path::Path::new(bg);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| bg.to_string());
                self.image_row.set_title(&name);
                self.image_row.set_subtitle(bg);
                self.image_row.set_visible(true);
                name
            }
        } else {
            self.image_row.set_visible(false);
            String::new()
        };
        *self.bg_name.borrow_mut() = name;
    }
}

/// Enable the chosen mode's effect and disable the other one.
fn set_mode(cmd_tx: &CmdTx, replace: bool) {
    let _ = cmd_tx.send(GuiCommand::SetEnabled {
        id: "bg_replace".into(),
        on: replace,
    });
    let _ = cmd_tx.send(GuiCommand::SetEnabled {
        id: "portrait_blur".into(),
        on: !replace,
    });
}

/// Set the background value and switch to Replace mode.
fn set_background(cmd_tx: &CmdTx, value: &str) {
    let _ = cmd_tx.send(GuiCommand::SetParam {
        id: "bg_replace".into(),
        key: "background".into(),
        value: shared::dbus::str_value(value),
    });
    set_mode(cmd_tx, true);
}

fn open_image_dialog(widget: &impl IsA<gtk::Widget>, cmd_tx: &CmdTx) {
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
    let parent = widget.root().and_downcast::<gtk::Window>();
    dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
        if let Ok(file) = res
            && let Some(path) = file.path()
        {
            set_background(&cmd_tx, &path.to_string_lossy());
        }
    });
}

fn open_color_dialog(button: &gtk::Button, cmd_tx: &CmdTx, provider: &gtk::CssProvider) {
    let dialog = gtk::ColorDialog::builder().title("Custom Color").build();
    let cmd_tx = cmd_tx.clone();
    let provider = provider.clone();
    let parent = button.root().and_downcast::<gtk::Window>();
    dialog.choose_rgba(parent.as_ref(), None, gio::Cancellable::NONE, move |res| {
        if let Ok(rgba) = res {
            let hex = rgba_to_hex(&rgba);
            set_custom_color(&provider, &hex);
            set_background(&cmd_tx, &hex);
        }
    });
}

/// Nearest blur-level toggle name for a raw strength value.
fn nearest_blur_level(strength: u32) -> String {
    BLUR_LEVELS
        .iter()
        .min_by_key(|(name, _)| {
            let level: i64 = name.parse().unwrap_or(66);
            (level - strength as i64).abs()
        })
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| "66".into())
}

fn rgba_to_hex(rgba: &RGBA) -> String {
    let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        to_u8(rgba.red()),
        to_u8(rgba.green()),
        to_u8(rgba.blue())
    )
}

// swatch widgets & CSS

/// A grid that wraps its swatches/thumbnails onto extra rows when the column
/// is narrow, instead of forcing a minimum window width like a plain `Box`.
fn wrapping_grid(max_per_line: u32) -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(max_per_line)
        .row_spacing(8)
        .column_spacing(8)
        .halign(gtk::Align::Start)
        .build()
}

/// A section heading stacked over its content.
fn labelled(label: &gtk::Label, content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    body.append(label);
    body.append(content);
    body
}

fn swatch_button(class: &str, tooltip: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .tooltip_text(tooltip)
        .css_classes(["oe-swatch-btn", "flat"])
        .build();
    button.set_child(Some(&swatch_box_child(class)));
    button
}

fn swatch_box_child(class: &str) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .width_request(32)
        .height_request(32)
        .css_classes(["oe-swatch", class])
        .build()
}

/// A "+"-style icon that fills and centers itself in its parent box,
/// regardless of the box's own size. Used by the custom-color and
/// custom-image buttons.
fn plus_icon() -> gtk::Image {
    let icon = gtk::Image::from_icon_name("list-add-symbolic");
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_hexpand(true);
    icon.set_vexpand(true);
    icon
}

/// A toggle button showing a built-in background-image thumbnail.
fn image_preset_button(label: &str, path: &std::path::Path) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .tooltip_text(label)
        .css_classes(["oe-swatch-btn", "flat"])
        .build();
    let picture = gtk::Picture::for_filename(path);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_width_request(IMAGE_THUMB_WIDTH);
    picture.set_height_request(IMAGE_THUMB_HEIGHT);
    picture.add_css_class("oe-bg-thumb");
    button.set_child(Some(&picture));
    button
}

/// Thumbnail-sized box for the "add your own image" button.
fn image_box_child(class: &str) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .width_request(IMAGE_THUMB_WIDTH)
        .height_request(IMAGE_THUMB_HEIGHT)
        .css_classes(["oe-bg-thumb", class])
        .build()
}

/// Write the bundled preset background images to the user's data dir (if not
/// already present) and return their on-disk paths. The daemon loads
/// `bg_replace.background` as a filesystem path, so the presets need to live
/// somewhere stable on disk rather than just in memory.
fn ensure_preset_images() -> Vec<(&'static str, PathBuf)> {
    let dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("vinoopeneffects/backgrounds");
    let _ = std::fs::create_dir_all(&dir);
    BG_IMAGE_PRESETS
        .iter()
        .map(|(label, filename, bytes)| {
            let path = dir.join(filename);
            if !path.exists() {
                let _ = std::fs::write(&path, bytes);
            }
            (*label, path)
        })
        .collect()
}

fn set_custom_color(provider: &gtk::CssProvider, hex: &str) {
    provider.load_from_string(&format!(
        ".{CUSTOM_SWATCH_CLASS} {{ background-color: {hex}; }}"
    ));
}

/// Install the static swatch CSS (base shape + one class per preset color).
fn install_swatch_css() {
    let mut css = String::from(
        ".oe-swatch { border-radius: 50%; border: 1px solid alpha(currentColor, 0.2); } \
         .oe-swatch-btn { padding: 4px; min-width: 0; min-height: 0; } \
         .oe-bg-thumb { border-radius: 10px; border: 1px solid alpha(currentColor, 0.2); \
         overflow: hidden; } \
         .oe-custom-image { background-color: alpha(currentColor, 0.08); }",
    );
    for (i, (_, hex)) in BG_PRESETS.iter().enumerate() {
        css.push_str(&format!(" .oe-swatch-{i} {{ background-color: {hex}; }}"));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    add_provider(&provider);
}

fn add_provider(provider: &gtk::CssProvider) {
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
