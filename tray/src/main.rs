mod dbus_client;
mod tray_item;

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
    SetEnabled { id: String, on: bool },
    SetParam { id: String, key: String, value: OwnedValue },
    Start,
    Stop,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<TrayCommand>(32);

    let tray = tray_item::OpenEffectsTray::new(cmd_tx);
    let service = ksni::TrayService::new(tray);
    let handle = service.handle();
    service.spawn();

    dbus_client::run(handle, cmd_rx).await;

    Ok(())
}
