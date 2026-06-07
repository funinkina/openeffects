pub mod bridge;
pub mod builder;
pub mod cameras;
pub mod effects;
pub mod probe;
pub mod provider;

use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use pipewire as pw;
use shared::config::AppState;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{error, info};
use zvariant::OwnedValue;

use crate::pipeline::bridge::Bridge;
use crate::pipeline::probe::PIPEWIRE_NODE_NAME;

/// Fixed virtual-camera format, shared by the capture appsink and the native
/// provide node so frames are byte-compatible without per-frame conversion.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
pub const FPS: i32 = 30;
/// I420 Y-plane stride.
pub const STRIDE: u32 = WIDTH;
/// I420 frame size = W*H (Y) + 2 * (W/2 * H/2) (U,V) = W*H*3/2.
pub const FRAME_SIZE: usize = (WIDTH as usize * HEIGHT as usize * 3) / 2;

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

/// Internal gating signal: the provide node's consumer state drives the capture
/// pipeline's lifecycle. Emitted from the provider thread's `state_changed`.
#[derive(Debug)]
pub(crate) enum CaptureCmd {
    Start,
    Stop,
}

/// Handle to the running native provide node (its own `pw_main_loop` thread).
struct ProviderHandle {
    quit: pw::channel::Sender<()>,
    join: std::thread::JoinHandle<()>,
}

impl ProviderHandle {
    fn stop(self) {
        let _ = self.quit.send(());
        let _ = self.join.join();
    }
}

pub fn spawn_worker(
    commands: mpsc::Receiver<PipelineCommand>,
    events: mpsc::Sender<PipelineEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(err) = gstreamer::init() {
            let _ = events.blocking_send(PipelineEvent::Error(err.to_string()));
            return;
        }
        let bridge = Arc::new(Bridge::new());
        worker_loop(commands, events, bridge);
    })
}

fn worker_loop(
    mut commands: mpsc::Receiver<PipelineCommand>,
    events: mpsc::Sender<PipelineEvent>,
    bridge: Arc<Bridge>,
) {
    // Provider -> worker capture gating channel.
    let (capture_tx, capture_rx) = std_mpsc::channel::<CaptureCmd>();

    let mut stored_app: Option<AppState> = None;
    let mut provider: Option<ProviderHandle> = None;
    let mut capture: Option<builder::BuiltCapture> = None;
    // Whether a consumer is currently pulling (provide node STREAMING).
    let mut consumer_streaming = false;

    loop {
        match commands.try_recv() {
            Ok(PipelineCommand::Start(app)) => {
                stored_app = Some(app);
                // Tear any running capture so a config/camera change takes effect.
                if let Some(c) = capture.take() {
                    c.stop();
                }
                bridge.clear();

                // Arm the provide node if not already advertised. The node sits
                // in PAUSED with the real camera untouched until a consumer links.
                if provider.is_none() {
                    provider = Some(spawn_provider(&bridge, &capture_tx, &events));
                    let _ = events.blocking_send(PipelineEvent::Started {
                        sink: format!("pipewire:{PIPEWIRE_NODE_NAME}"),
                    });
                }

                // If a consumer is already streaming (live camera switch), rebuild
                // the capture immediately rather than waiting for a state change.
                if consumer_streaming {
                    if let Some(app) = stored_app.clone() {
                        start_capture(&app, &bridge, &events, &mut capture);
                    }
                }
            }
            Ok(PipelineCommand::Stop) => {
                if let Some(c) = capture.take() {
                    c.stop();
                }
                bridge.clear();
                consumer_streaming = false;
                if let Some(p) = provider.take() {
                    p.stop();
                }
                let _ = events.blocking_send(PipelineEvent::Stopped);
            }
            Ok(PipelineCommand::SetEnabled { id, on }) => {
                if let Some(app) = stored_app.as_mut() {
                    apply_enabled(app, &id, on);
                }
                if let Some(c) = capture.as_ref() {
                    c.set_enabled(&id, on);
                }
            }
            Ok(PipelineCommand::SetParam { id, key, value }) => {
                if let Some(app) = stored_app.as_mut() {
                    apply_param(app, &id, &key, &value);
                }
                if let Some(c) = capture.as_ref() {
                    c.set_param(&id, &key, &value);
                }
            }
            Ok(PipelineCommand::Shutdown) => {
                if let Some(c) = capture.take() {
                    c.stop();
                }
                if let Some(p) = provider.take() {
                    p.stop();
                }
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
        }

        // Capture gating driven by the provide node's consumer state.
        match capture_rx.try_recv() {
            Ok(CaptureCmd::Start) => {
                consumer_streaming = true;
                if capture.is_none() {
                    if let Some(app) = stored_app.clone() {
                        start_capture(&app, &bridge, &events, &mut capture);
                    }
                }
            }
            Ok(CaptureCmd::Stop) => {
                consumer_streaming = false;
                if let Some(c) = capture.take() {
                    c.stop();
                    bridge.clear();
                    info!("capture stopped: no consumer, camera released");
                    let _ = events.blocking_send(PipelineEvent::Idle);
                }
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
            Err(std_mpsc::TryRecvError::Disconnected) => {}
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_provider(
    bridge: &Arc<Bridge>,
    capture_tx: &std_mpsc::Sender<CaptureCmd>,
    events: &mpsc::Sender<PipelineEvent>,
) -> ProviderHandle {
    let (quit_tx, quit_rx) = pw::channel::channel::<()>();
    let bridge = Arc::clone(bridge);
    let capture_tx = capture_tx.clone();
    let events = events.clone();
    let join = std::thread::spawn(move || {
        provider::run(bridge, capture_tx, events, quit_rx);
    });
    ProviderHandle {
        quit: quit_tx,
        join,
    }
}

/// Build + start the capture pipeline (opens the real camera) and report status.
fn start_capture(
    app: &AppState,
    bridge: &Arc<Bridge>,
    events: &mpsc::Sender<PipelineEvent>,
    capture: &mut Option<builder::BuiltCapture>,
) {
    match builder::build_capture_pipeline(app, Arc::clone(bridge)).and_then(|c| {
        c.start()?;
        Ok(c)
    }) {
        Ok(c) => {
            info!("capture started: consumer connected, camera opened");
            *capture = Some(c);
            let _ = events.blocking_send(PipelineEvent::Started {
                sink: format!("pipewire:{PIPEWIRE_NODE_NAME}"),
            });
        }
        Err(err) => {
            error!(%err, "capture start failed");
            let _ = events.blocking_send(PipelineEvent::Error(err.to_string()));
        }
    }
}

/// Mirror an effect toggle into the stored config so a later capture rebuild
/// (consumer reconnect, camera switch) reflects the current state. Only the
/// effects the Phase-1 pipeline actually renders are tracked here.
fn apply_enabled(app: &mut AppState, id: &str, on: bool) {
    match id {
        "center_stage" => app.effects.center_stage.enabled = on,
        "portrait_blur" => app.effects.portrait_blur.enabled = on,
        "bg_replace" => app.effects.bg_replace.enabled = on,
        "studio_light" => app.effects.studio_light.enabled = on,
        "reactions" => app.effects.reactions.enabled = on,
        _ => {}
    }
}

fn apply_param(app: &mut AppState, id: &str, key: &str, value: &OwnedValue) {
    if id == "studio_light" {
        match key {
            "brightness" => {
                if let Some(v) = shared::dbus::value_as_i32(value) {
                    app.effects.studio_light.brightness = v.clamp(-100, 100) as i8;
                }
            }
            "contrast" => {
                if let Some(v) = shared::dbus::value_as_u32(value) {
                    app.effects.studio_light.contrast = v.min(100) as u8;
                }
            }
            _ => {}
        }
    }
}
