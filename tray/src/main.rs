mod dbus_client;
mod indicator;
mod menu;

use std::sync::mpsc;

use shared::dbus::VariantMap;
use zvariant::OwnedValue;

#[derive(Debug, Clone)]
pub enum TrayUpdate {
    Status(String),
    AllEffects(VariantMap),
    EffectChanged { id: String, params: VariantMap },
    Error(String),
}

#[derive(Debug)]
pub enum TrayCommand {
    SetEnabled {
        id: String,
        on: bool,
    },
    SetParam {
        id: String,
        key: String,
        value: OwnedValue,
    },
    Start,
    Stop,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    gtk::init().expect("failed to initialize GTK");

    #[allow(deprecated)]
    let (state_tx, state_rx) = glib::MainContext::channel(glib::Priority::DEFAULT);
    let (cmd_tx, cmd_rx) = mpsc::channel::<TrayCommand>();

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        runtime.block_on(dbus_client::run(state_tx, cmd_rx));
    });

    let _indicator = indicator::build_and_show(state_rx, cmd_tx);
    gtk::main();
}
