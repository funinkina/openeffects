mod about;
mod app;
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
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    if let Err(err) = gstreamer::init() {
        tracing::warn!("GStreamer init failed, live preview disabled: {err}");
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<GuiCommand>();
    let (update_tx, update_rx) = async_channel::unbounded::<UiUpdate>();
    dbus_client::spawn(cmd_rx, update_tx);

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| app::build_window(app, cmd_tx.clone(), update_rx.clone()));
    app.run()
}
