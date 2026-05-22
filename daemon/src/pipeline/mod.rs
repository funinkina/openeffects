pub mod builder;
pub mod effects;
pub mod probe;

use shared::config::AppState;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use tokio::sync::mpsc::error::TryRecvError;
use tracing::{error, info, warn};
use zvariant::OwnedValue;

#[derive(Debug)]
pub enum PipelineCommand {
    Start(AppState),
    Stop,
    SetEnabled {
        id: String,
        on: bool,
    },
    SetParam {
        id: String,
        key: String,
        value: OwnedValue,
    },
    SelectCamera(String),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Started { sink: String },
    Idle,
    Stopped,
    Error(String),
}

pub fn spawn_worker(
    mut commands: mpsc::Receiver<PipelineCommand>,
    events: mpsc::Sender<PipelineEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(err) = gstreamer::init() {
            let _ = events.blocking_send(PipelineEvent::Error(err.to_string()));
            return;
        }

        let mut current: Option<builder::BuiltPipeline> = None;
        let mut disconnected_since: Option<Instant> = None;
        let mut idle = false;

        loop {
            match commands.try_recv() {
                Ok(command) => match command {
                    PipelineCommand::Start(app) => {
                        if let Some(pipeline) = current.take() {
                            pipeline.stop();
                        }
                        disconnected_since = None;
                        idle = false;
                        match builder::build_pipeline(&app).and_then(|pipeline| {
                            pipeline.start()?;
                            Ok(pipeline)
                        }) {
                            Ok(pipeline) => {
                                let sink = pipeline.output_sink().to_string();
                                info!(sink, "pipeline started");
                                let _ = events.blocking_send(PipelineEvent::Started { sink });
                                current = Some(pipeline);
                            }
                            Err(err) => {
                                error!(%err, "pipeline start failed");
                                let _ = events.blocking_send(PipelineEvent::Error(err.to_string()));
                            }
                        }
                    }
                    PipelineCommand::Stop => {
                        if let Some(pipeline) = current.take() {
                            pipeline.stop();
                        }
                        disconnected_since = None;
                        idle = false;
                        let _ = events.blocking_send(PipelineEvent::Stopped);
                    }
                    PipelineCommand::SetEnabled { id, on } => {
                        if let Some(pipeline) = current.as_ref() {
                            pipeline.set_enabled(&id, on);
                        }
                    }
                    PipelineCommand::SetParam { id, key, value } => {
                        if let Some(pipeline) = current.as_ref() {
                            pipeline.set_param(&id, &key, &value);
                        }
                    }
                    PipelineCommand::SelectCamera(camera) => {
                        warn!(
                            camera,
                            "camera switching will restart the pipeline in a later phase"
                        );
                    }
                    PipelineCommand::Shutdown => {
                        if let Some(pipeline) = current.take() {
                            pipeline.stop();
                        }
                        break;
                    }
                },
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }

            if let Some(pipeline) = current.as_ref() {
                let connected = pipeline.consumer_connected().unwrap_or(true);
                if connected {
                    disconnected_since = None;
                    if idle {
                        if let Err(err) = pipeline.start() {
                            let _ = events.blocking_send(PipelineEvent::Error(err.to_string()));
                        } else {
                            idle = false;
                            let _ = events.blocking_send(PipelineEvent::Started {
                                sink: pipeline.output_sink().to_string(),
                            });
                        }
                    }
                } else {
                    let since = disconnected_since.get_or_insert_with(Instant::now);
                    if !idle && since.elapsed() >= Duration::from_secs(30) {
                        pipeline.pause();
                        idle = true;
                        let _ = events.blocking_send(PipelineEvent::Idle);
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(200));
        }
    })
}
