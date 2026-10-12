mod about;
mod app;
mod config;
mod constants;
mod dbus_client;
mod pages;
mod preferences;
mod preview;
mod widgets;

use adw::glib;
use adw::prelude::*;
use dbus_client::{GuiCommand, UiUpdate};
use tokio::sync::mpsc;

use constants::APP_ID;

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Move pre-fork `openeffects` state first, before GuiConfig is read.
    shared::migrate::migrate_legacy_paths().log();

    if let Err(err) = gstreamer::init() {
        tracing::warn!("GStreamer init failed, live preview disabled: {err}");
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<GuiCommand>();
    let (update_tx, update_rx) = async_channel::unbounded::<UiUpdate>();
    dbus_client::spawn(cmd_rx, update_tx);

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        app::build_window(
            app,
            cmd_tx.clone(),
            update_rx.clone(),
            config::GuiConfig::load_or_default(),
        )
    });
    app.run()
}
