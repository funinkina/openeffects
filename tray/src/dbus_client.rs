use futures_util::StreamExt;
use ksni::Handle;
use shared::dbus::{VariantMap, DAEMON_INTERFACE, EFFECTS_INTERFACE, OBJECT_PATH, SERVICE_NAME};
use tokio::sync::mpsc;
use zbus::{Connection, Proxy};
use zvariant::Value;

use crate::{tray_item::OpenEffectsTray, TrayCommand, TrayUpdate};

pub async fn run(handle: Handle<OpenEffectsTray>, mut cmd_rx: mpsc::Receiver<TrayCommand>) {
    loop {
        match run_once(&handle, &mut cmd_rx).await {
            Ok(()) => {}
            Err(err) => {
                tracing::error!(%err, "D-Bus session lost, reconnecting in 2s");
                handle.update(|tray| tray.apply_update(TrayUpdate::Error(err.to_string())));
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

async fn run_once(
    handle: &Handle<OpenEffectsTray>,
    cmd_rx: &mut mpsc::Receiver<TrayCommand>,
) -> anyhow::Result<()> {
    let conn = Connection::session().await?;
    let daemon = daemon_proxy(&conn).await?;
    let effects = effects_proxy(&conn).await?;

    // Fetch initial state
    let status: String = daemon.get_property("Status").await?;
    let all_state: VariantMap = effects.call("GetAllState", &()).await?;
    handle.update(|t| t.apply_update(TrayUpdate::Status(status)));
    handle.update(|t| t.apply_update(TrayUpdate::AllEffects(all_state)));

    // Subscribe to EffectChanged signal
    let sig_proxy = Proxy::new(&conn, SERVICE_NAME, OBJECT_PATH, EFFECTS_INTERFACE).await?;
    let mut effect_signals = sig_proxy.receive_signal("EffectChanged").await?;

    let mut status_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    // Skip the first immediate tick
    status_tick.tick().await;

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                if let Err(err) = dispatch_command(&daemon, &effects, cmd).await {
                    tracing::warn!(%err, "D-Bus command failed");
                }
            }
            _ = status_tick.tick() => {
                if let Ok(s) = daemon.get_property::<String>("Status").await {
                    handle.update(|t| t.apply_update(TrayUpdate::Status(s)));
                }
            }
            msg = effect_signals.next() => {
                let Some(msg) = msg else {
                    anyhow::bail!("EffectChanged signal stream ended");
                };
                let (id, params): (String, VariantMap) = msg.body().deserialize()?;
                handle.update(|t| t.apply_update(TrayUpdate::EffectChanged { id, params }));
            }
        }
    }
}

async fn dispatch_command(
    daemon: &Proxy<'_>,
    effects: &Proxy<'_>,
    cmd: TrayCommand,
) -> anyhow::Result<()> {
    match cmd {
        TrayCommand::SetEnabled { id, on } => {
            effects
                .call::<_, _, ()>("SetEnabled", &(id.as_str(), on))
                .await?;
        }
        TrayCommand::SetParam { id, key, value } => {
            let v: Value<'_> = value.into();
            effects
                .call::<_, _, ()>("SetParam", &(id.as_str(), key.as_str(), v))
                .await?;
        }
        TrayCommand::Start => {
            daemon.call::<_, _, ()>("Start", &()).await?;
        }
        TrayCommand::Stop => {
            daemon.call::<_, _, ()>("Stop", &()).await?;
        }
    }
    Ok(())
}

async fn daemon_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, DAEMON_INTERFACE).await
}

async fn effects_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, EFFECTS_INTERFACE).await
}
