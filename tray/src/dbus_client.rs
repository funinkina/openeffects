use std::sync::mpsc;

use futures_util::StreamExt;
use glib::Sender;
use shared::dbus::{VariantMap, DAEMON_INTERFACE, EFFECTS_INTERFACE, OBJECT_PATH, SERVICE_NAME};
use zbus::{Connection, Proxy};
use zvariant::Value;

use crate::{TrayCommand, TrayUpdate};

pub async fn run(state_tx: Sender<TrayUpdate>, cmd_rx: mpsc::Receiver<TrayCommand>) {
    loop {
        match run_once(&state_tx, &cmd_rx).await {
            Ok(()) => {}
            Err(err) => {
                let _ = state_tx.send(TrayUpdate::Error(err.to_string()));
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

async fn run_once(
    state_tx: &Sender<TrayUpdate>,
    cmd_rx: &mpsc::Receiver<TrayCommand>,
) -> anyhow::Result<()> {
    let conn = Connection::session().await?;
    let daemon = daemon_proxy(&conn).await?;
    let effects = effects_proxy(&conn).await?;

    let _ = state_tx.send(TrayUpdate::Status(
        daemon.get_property::<String>("Status").await?,
    ));
    let _ = state_tx.send(TrayUpdate::AllEffects(
        effects.call("GetAllState", &()).await?,
    ));

    let proxy = Proxy::new(&conn, SERVICE_NAME, OBJECT_PATH, EFFECTS_INTERFACE).await?;
    let mut effect_signals = proxy.receive_signal("EffectChanged").await?;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    let mut status_tick = tokio::time::interval(std::time::Duration::from_secs(5));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                while let Ok(command) = cmd_rx.try_recv() {
                    handle_command(&daemon, &effects, command).await?;
                }
            }
            _ = status_tick.tick() => {
                let _ = state_tx.send(TrayUpdate::Status(daemon.get_property::<String>("Status").await?));
            }
            message = effect_signals.next() => {
                let Some(message) = message else {
                    anyhow::bail!("D-Bus effect signal stream ended");
                };
                let (id, params): (String, VariantMap) = message.body().deserialize()?;
                let _ = state_tx.send(TrayUpdate::EffectChanged { id, params });
            }
        }
    }
}

async fn handle_command(
    daemon: &Proxy<'_>,
    effects: &Proxy<'_>,
    command: TrayCommand,
) -> anyhow::Result<()> {
    match command {
        TrayCommand::SetEnabled { id, on } => {
            effects
                .call::<_, _, ()>("SetEnabled", &(id.as_str(), on))
                .await?
        }
        TrayCommand::SetParam { id, key, value } => {
            let value: Value<'_> = value.into();
            effects
                .call::<_, _, ()>("SetParam", &(id.as_str(), key.as_str(), value))
                .await?
        }
        TrayCommand::Start => daemon.call::<_, _, ()>("Start", &()).await?,
        TrayCommand::Stop => daemon.call::<_, _, ()>("Stop", &()).await?,
    }
    Ok(())
}

async fn daemon_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, DAEMON_INTERFACE).await
}

async fn effects_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, EFFECTS_INTERFACE).await
}
