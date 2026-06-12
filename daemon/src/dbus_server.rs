use std::sync::Arc;

use shared::dbus::{EFFECT_IDS, VariantMap};
use tokio::sync::{RwLock, mpsc};
use zbus::{fdo, interface, object_server::SignalEmitter};
use zvariant::OwnedValue;

use crate::{
    pipeline::{self, PipelineCommand},
    state::{DaemonState, DaemonStatus},
};

pub type SharedDaemonState = Arc<RwLock<DaemonState>>;

#[derive(Clone)]
pub struct Daemon1Iface {
    state: SharedDaemonState,
    pipeline_tx: mpsc::Sender<PipelineCommand>,
    shutdown_tx: mpsc::Sender<()>,
}

impl Daemon1Iface {
    pub fn new(
        state: SharedDaemonState,
        pipeline_tx: mpsc::Sender<PipelineCommand>,
        shutdown_tx: mpsc::Sender<()>,
    ) -> Self {
        Self {
            state,
            pipeline_tx,
            shutdown_tx,
        }
    }
}

#[interface(name = "org.openeffects.Daemon1")]
impl Daemon1Iface {
    async fn start(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> fdo::Result<()> {
        let app = {
            let mut state = self.state.write().await;
            if !state.status.can_start() {
                return Ok(());
            }
            state.status = DaemonStatus::Starting;
            state.last_error = None;
            state.app.clone()
        };

        Self::daemon_status_changed(&emitter, DaemonStatus::Starting.as_str())
            .await
            .map_err(to_fdo_error)?;
        self.pipeline_tx
            .send(PipelineCommand::Start(app))
            .await
            .map_err(|err| fdo::Error::Failed(err.to_string()))?;
        Ok(())
    }

    async fn stop(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> fdo::Result<()> {
        {
            let mut state = self.state.write().await;
            if !state.status.can_stop() {
                return Ok(());
            }
            state.status = DaemonStatus::Stopped;
        }
        self.pipeline_tx
            .send(PipelineCommand::Stop)
            .await
            .map_err(|err| fdo::Error::Failed(err.to_string()))?;
        Self::daemon_status_changed(&emitter, DaemonStatus::Stopped.as_str())
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    /// Tear down the pipeline and exit the process. Best-effort: the main loop
    /// performs the actual shutdown + clean exit when it receives the signal.
    async fn quit(&self) -> fdo::Result<()> {
        let _ = self.shutdown_tx.send(()).await;
        Ok(())
    }

    #[zbus(property)]
    async fn status(&self) -> String {
        self.state.read().await.status.as_str().into()
    }

    #[zbus(property)]
    async fn capabilities(&self) -> VariantMap {
        self.state.read().await.capabilities()
    }

    #[zbus(signal)]
    #[zbus(name = "StatusChanged")]
    pub async fn daemon_status_changed(
        emitter: &SignalEmitter<'_>,
        new_status: &str,
    ) -> zbus::Result<()>;
}

#[derive(Clone)]
pub struct Effects1Iface {
    state: SharedDaemonState,
    pipeline_tx: mpsc::Sender<PipelineCommand>,
}

impl Effects1Iface {
    pub fn new(state: SharedDaemonState, pipeline_tx: mpsc::Sender<PipelineCommand>) -> Self {
        Self { state, pipeline_tx }
    }
}

#[interface(name = "org.openeffects.Effects1")]
impl Effects1Iface {
    async fn list_effects(&self) -> Vec<String> {
        EFFECT_IDS.iter().map(|id| (*id).to_string()).collect()
    }

    async fn set_enabled(
        &self,
        id: &str,
        on: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let (params, app) = {
            let mut state = self.state.write().await;
            if !state.set_enabled(id, on) {
                return Err(fdo::Error::InvalidArgs(format!("unknown effect '{id}'")));
            }
            state
                .app
                .save()
                .map_err(|err| fdo::Error::Failed(err.to_string()))?;
            (
                state.effect_params(id).unwrap_or_default(),
                state.app.clone(),
            )
        };

        let _ = self
            .pipeline_tx
            .send(PipelineCommand::ApplyEffects(app))
            .await;
        Self::effect_changed(&emitter, id, params)
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    async fn set_param(
        &self,
        id: &str,
        key: &str,
        value: OwnedValue,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let (params, app) = {
            let mut state = self.state.write().await;
            if !state.set_param(id, key, &value) {
                return Err(fdo::Error::InvalidArgs(format!(
                    "unknown effect parameter '{id}.{key}'"
                )));
            }
            state
                .app
                .save()
                .map_err(|err| fdo::Error::Failed(err.to_string()))?;
            (
                state.effect_params(id).unwrap_or_default(),
                state.app.clone(),
            )
        };

        let _ = self
            .pipeline_tx
            .send(PipelineCommand::ApplyEffects(app))
            .await;
        Self::effect_changed(&emitter, id, params)
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    /// Fire a one-shot reaction burst. Stateless: no config is changed and no
    /// signal is emitted — it just hands the pipeline a trigger.
    async fn trigger_reaction(&self, id: &str) -> fdo::Result<()> {
        if !shared::dbus::REACTION_IDS.contains(&id) {
            return Err(fdo::Error::InvalidArgs(format!("unknown reaction '{id}'")));
        }
        self.pipeline_tx
            .send(PipelineCommand::TriggerReaction(id.to_string()))
            .await
            .map_err(|err| fdo::Error::Failed(err.to_string()))?;
        Ok(())
    }

    async fn get_params(&self, id: &str) -> fdo::Result<VariantMap> {
        self.state
            .read()
            .await
            .effect_params(id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown effect '{id}'")))
    }

    async fn get_all_state(&self) -> VariantMap {
        self.state.read().await.all_effect_state()
    }

    #[zbus(signal)]
    async fn effect_changed(
        emitter: &SignalEmitter<'_>,
        id: &str,
        params: VariantMap,
    ) -> zbus::Result<()>;
}

#[derive(Clone)]
pub struct Devices1Iface {
    state: SharedDaemonState,
    pipeline_tx: mpsc::Sender<PipelineCommand>,
}

impl Devices1Iface {
    pub fn new(state: SharedDaemonState, pipeline_tx: mpsc::Sender<PipelineCommand>) -> Self {
        Self { state, pipeline_tx }
    }
}

#[interface(name = "org.openeffects.Devices1")]
impl Devices1Iface {
    async fn list_cameras(&self) -> Vec<VariantMap> {
        let active = self.state.read().await.active_camera.clone();
        pipeline::cameras::enumerate()
            .into_iter()
            .map(|c| {
                let mut m = VariantMap::new();
                m.insert("id".into(), shared::dbus::str_value(&c.id));
                m.insert("name".into(), shared::dbus::str_value(&c.name));
                m.insert("path".into(), shared::dbus::str_value(&c.path));
                m.insert("api".into(), shared::dbus::str_value(&c.api));
                m.insert("active".into(), shared::dbus::bool_value(c.id == active));
                m.insert("width".into(), shared::dbus::u32_value(1280));
                m.insert("height".into(), shared::dbus::u32_value(720));
                m.insert("fps".into(), shared::dbus::u32_value(30));
                m
            })
            .collect()
    }

    async fn select_camera(&self, id: &str) -> fdo::Result<()> {
        let (was_running, app) = {
            let mut state = self.state.write().await;
            state.active_camera = id.into();
            state.app.camera.selected = id.into();
            state
                .app
                .save()
                .map_err(|err| fdo::Error::Failed(err.to_string()))?;
            (
                matches!(
                    state.status,
                    DaemonStatus::Running | DaemonStatus::Idle | DaemonStatus::Starting
                ),
                state.app.clone(),
            )
        };
        if was_running {
            let _ = self.pipeline_tx.send(PipelineCommand::Start(app)).await;
        }
        Ok(())
    }

    #[zbus(property)]
    async fn virtual_camera_info(&self) -> VariantMap {
        self.state.read().await.virtual_camera_info()
    }

    #[zbus(property)]
    async fn active_camera(&self) -> String {
        self.state.read().await.active_camera.clone()
    }
}

fn to_fdo_error(error: zbus::Error) -> fdo::Error {
    fdo::Error::Failed(error.to_string())
}
