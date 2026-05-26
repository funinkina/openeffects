pub mod builder;
pub mod effects;
pub mod probe;

use shared::config::AppState;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use tokio::sync::mpsc::error::TryRecvError;
use tracing::{error, info, warn};
use zvariant::OwnedValue;

use crate::pipeline::probe::OutputSink;

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
    VirtualCameraVerified { node_name: String, found: bool },
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
                                if let OutputSink::PipeWire { node_name } = pipeline.sink_type() {
                                    spawn_pipewire_verifier(node_name.clone(), events.clone());
                                }
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

fn spawn_pipewire_verifier(node_name: String, events: mpsc::Sender<PipelineEvent>) {
    std::thread::spawn(move || {
        // Brief delay so pipewiresink finishes registering the node with the
        // PipeWire daemon before we probe; the GST PLAYING transition does not
        // imply the registry has caught up.
        std::thread::sleep(Duration::from_millis(300));

        let mut child = match Command::new("pw-cli")
            .args(["ls", "Node"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(err) => {
                warn!(%err, "pw-cli verification skipped (spawn failed)");
                let _ = events.blocking_send(PipelineEvent::VirtualCameraVerified {
                    node_name,
                    found: false,
                });
                return;
            }
        };

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        warn!("pw-cli timed out after 2s; killing");
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = events.blocking_send(PipelineEvent::VirtualCameraVerified {
                            node_name,
                            found: false,
                        });
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => {
                    warn!(%err, "pw-cli wait failed");
                    let _ = events.blocking_send(PipelineEvent::VirtualCameraVerified {
                        node_name,
                        found: false,
                    });
                    return;
                }
            }
        }

        let mut stdout = String::new();
        if let Some(mut handle) = child.stdout.take() {
            let _ = handle.read_to_string(&mut stdout);
        }

        let needle = format!("node.name = \"{node_name}\"");
        let found = stdout.lines().any(|l| l.contains(&needle));
        if found {
            info!(%node_name, "virtual camera registered in PipeWire graph");
        } else {
            warn!(%node_name, "virtual camera not found in PipeWire graph");
        }
        let _ = events.blocking_send(PipelineEvent::VirtualCameraVerified { node_name, found });
    });
}
