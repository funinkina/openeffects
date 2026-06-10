//! Background D-Bus client task.
//!
//! Owns the `zbus::Connection` on a dedicated tokio runtime (GTK's main loop is
//! glib's, not tokio's). Commands flow GUI -> daemon over an unbounded mpsc
//! channel; state updates flow daemon -> GUI over an `async_channel`, which the
//! GTK side drains via `glib::MainContext::spawn_local`.

use std::time::Duration;

use futures_util::StreamExt;
use shared::dbus::VariantMap;
use tokio::sync::mpsc;
use zvariant::{OwnedValue, Value};

#[allow(dead_code)]
mod proxies {
    include!(concat!(env!("OUT_DIR"), "/proxies.rs"));
}
use proxies::{Daemon1Proxy, Effects1Proxy};

#[derive(Debug)]
pub enum GuiCommand {
    SetEnabled {
        id: String,
        on: bool,
    },
    SetParam {
        id: String,
        key: String,
        value: OwnedValue,
    },
}

#[derive(Debug)]
pub enum UiUpdate {
    AllState(VariantMap),
    EffectChanged { id: String, params: VariantMap },
    Status(String),
    Disconnected,
}

/// Spawn the D-Bus client on its own thread with its own tokio runtime.
pub fn spawn(
    cmd_rx: mpsc::UnboundedReceiver<GuiCommand>,
    update_tx: async_channel::Sender<UiUpdate>,
) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
        rt.block_on(run(cmd_rx, update_tx));
    });
}

async fn run(
    mut cmd_rx: mpsc::UnboundedReceiver<GuiCommand>,
    update_tx: async_channel::Sender<UiUpdate>,
) {
    loop {
        if let Err(err) = run_once(&mut cmd_rx, &update_tx).await {
            tracing::warn!(%err, "lost connection to openeffectsd, retrying");
            let _ = update_tx.send(UiUpdate::Disconnected).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

async fn run_once(
    cmd_rx: &mut mpsc::UnboundedReceiver<GuiCommand>,
    update_tx: &async_channel::Sender<UiUpdate>,
) -> anyhow::Result<()> {
    let conn = zbus::Connection::session().await?;
    let effects = Effects1Proxy::new(&conn).await?;
    let daemon = Daemon1Proxy::new(&conn).await?;

    let all_state = effects.get_all_state().await?;
    update_tx.send(UiUpdate::AllState(all_state)).await.ok();

    let status = daemon.status().await?;
    update_tx.send(UiUpdate::Status(status)).await.ok();

    let mut effect_changed = effects.receive_effect_changed().await?;
    let mut status_changed = daemon.receive_daemon_status_changed().await?;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(GuiCommand::SetEnabled { id, on }) => {
                        effects.set_enabled(&id, on).await?;
                    }
                    Some(GuiCommand::SetParam { id, key, value }) => {
                        let value: Value<'_> = value.into();
                        effects.set_param(&id, &key, &value).await?;
                    }
                    None => return Ok(()),
                }
            }
            signal = effect_changed.next() => {
                let Some(signal) = signal else {
                    return Err(anyhow::anyhow!("EffectChanged signal stream closed"));
                };
                let args = signal.args()?;
                let params: VariantMap = args
                    .params
                    .into_iter()
                    .map(|(k, v)| Ok((k.to_string(), OwnedValue::try_from(v)?)))
                    .collect::<anyhow::Result<_>>()?;
                update_tx
                    .send(UiUpdate::EffectChanged { id: args.id.to_string(), params })
                    .await
                    .ok();
            }
            signal = status_changed.next() => {
                let Some(signal) = signal else {
                    return Err(anyhow::anyhow!("StatusChanged signal stream closed"));
                };
                let args = signal.args()?;
                update_tx
                    .send(UiUpdate::Status(args.new_status.to_string()))
                    .await
                    .ok();
            }
        }
    }
}
