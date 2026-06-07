mod dbus_server;
mod pipeline;
mod state;

use std::sync::Arc;

use anyhow::Context;
use shared::{
    config::AppState,
    dbus::{OBJECT_PATH, SERVICE_NAME},
};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info};

use crate::{
    dbus_server::{Daemon1Iface, Devices1Iface, Effects1Iface},
    pipeline::{PipelineCommand, PipelineEvent},
    state::{DaemonState, DaemonStatus},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let start_pipeline = std::env::args().any(|arg| arg == "--start");
    let app = AppState::load_or_default().unwrap_or_else(|err| {
        error!(%err, "failed to load config, using defaults");
        AppState::default()
    });
    let state = Arc::new(RwLock::new(DaemonState::new(app)));
    let (pipeline_tx, pipeline_rx) = mpsc::channel(32);
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let worker = pipeline::spawn_worker(pipeline_rx, event_tx);

    let event_state = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let mut state = event_state.write().await;
            match event {
                PipelineEvent::Started { sink } => {
                    state.status = DaemonStatus::Running;
                    state.output_sink = sink;
                    state.last_error = None;
                    // Note: virtual_camera_verified is intentionally left as-is.
                    // It is a one-time fact set when the native provide node
                    // registers, and Started now fires on every consumer connect.
                }
                PipelineEvent::Idle => {
                    state.status = DaemonStatus::Idle;
                }
                PipelineEvent::Stopped => {
                    state.status = DaemonStatus::Stopped;
                    state.virtual_camera_verified = None;
                }
                PipelineEvent::Error(error) => {
                    state.status = DaemonStatus::Error;
                    state.last_error = Some(error);
                }
                PipelineEvent::VirtualCameraVerified { node_name, found } => {
                    state.virtual_camera_name = node_name;
                    state.virtual_camera_verified = Some(found);
                }
            }
        }
    });

    let _conn = zbus::connection::Builder::session()?
        .serve_at(
            OBJECT_PATH,
            Daemon1Iface::new(Arc::clone(&state), pipeline_tx.clone()),
        )?
        .serve_at(
            OBJECT_PATH,
            Effects1Iface::new(Arc::clone(&state), pipeline_tx.clone()),
        )?
        .serve_at(
            OBJECT_PATH,
            Devices1Iface::new(Arc::clone(&state), pipeline_tx.clone()),
        )?
        .name(SERVICE_NAME)?
        .build()
        .await
        .context("connect to session bus")?;

    if start_pipeline {
        let app = state.read().await.app.clone();
        {
            let mut state = state.write().await;
            state.status = DaemonStatus::Starting;
        }
        pipeline_tx.send(PipelineCommand::Start(app)).await?;
    }

    info!("openeffectsd is running on the session bus");
    tokio::signal::ctrl_c().await?;
    let _ = pipeline_tx.send(PipelineCommand::Shutdown).await;
    let _ = worker.join();
    Ok(())
}
