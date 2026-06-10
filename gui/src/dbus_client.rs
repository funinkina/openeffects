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
use proxies::{Daemon1Proxy, Devices1Proxy, Effects1Proxy};

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
    SelectCamera {
        id: String,
    },
}

/// A physical camera as shown in the Camera page picker.
#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug)]
pub enum UiUpdate {
    AllState(VariantMap),
    EffectChanged {
        id: String,
        params: VariantMap,
    },
    Status(String),
    Capabilities(VariantMap),
    Cameras {
        cameras: Vec<CameraInfo>,
        active: String,
    },
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
    let devices = Devices1Proxy::new(&conn).await?;

    // Initial snapshot.
    update_tx
        .send(UiUpdate::AllState(effects.get_all_state().await?))
        .await
        .ok();
    update_tx
        .send(UiUpdate::Status(daemon.status().await?))
        .await
        .ok();
    push_capabilities(&daemon, update_tx).await;
    push_cameras(&devices, update_tx).await;

    let mut effect_changed = effects.receive_effect_changed().await?;
    let mut status_changed = daemon.receive_daemon_status_changed().await?;
    let mut caps_changed = daemon.receive_capabilities_changed().await;
    let mut active_cam_changed = devices.receive_active_camera_changed().await;

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
                    Some(GuiCommand::SelectCamera { id }) => {
                        devices.select_camera(&id).await?;
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
                // Status transitions (idle/running) may change the active EP/tier readout.
                push_capabilities(&daemon, update_tx).await;
            }
            change = caps_changed.next() => {
                if change.is_none() {
                    return Err(anyhow::anyhow!("Capabilities property stream closed"));
                }
                push_capabilities(&daemon, update_tx).await;
            }
            change = active_cam_changed.next() => {
                if change.is_none() {
                    return Err(anyhow::anyhow!("ActiveCamera property stream closed"));
                }
                push_cameras(&devices, update_tx).await;
            }
        }
    }
}

async fn push_capabilities(daemon: &Daemon1Proxy<'_>, update_tx: &async_channel::Sender<UiUpdate>) {
    if let Ok(caps) = daemon.capabilities().await {
        update_tx.send(UiUpdate::Capabilities(caps)).await.ok();
    }
}

async fn push_cameras(devices: &Devices1Proxy<'_>, update_tx: &async_channel::Sender<UiUpdate>) {
    let Ok(raw) = devices.list_cameras().await else {
        return;
    };
    let active = devices.active_camera().await.unwrap_or_default();
    let cameras = raw
        .into_iter()
        .filter_map(|m| {
            let id = shared::dbus::value_as_string(m.get("id")?)?;
            let name = m
                .get("name")
                .and_then(shared::dbus::value_as_string)
                .unwrap_or_else(|| id.clone());
            Some(CameraInfo { id, name })
        })
        .collect();
    update_tx
        .send(UiUpdate::Cameras { cameras, active })
        .await
        .ok();
}
