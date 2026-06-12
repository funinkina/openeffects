pub mod bridge;
pub mod builder;
pub mod cameras;
pub mod effects;
pub mod filters;
pub mod probe;
pub mod provider;

use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use pipewire as pw;
use shared::config::AppState;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{error, info};

use crate::pipeline::bridge::Bridge;
use crate::pipeline::probe::PIPEWIRE_NODE_NAME;

/// Default virtual-camera format, used when the camera's native modes can't be
/// probed (e.g. `videotestsrc` fallback).
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
pub const FPS: i32 = 30;

/// The I420 format shared by the capture appsink and the native provide node so
/// frames are byte-compatible without per-frame conversion. The dimensions are
/// taken from the physical camera's preferred native mode
/// (`cameras::preferred_format`) and pinned on the *source* too, so the capture
/// pipeline never rescales — rescaling a 4:3 camera mode into a fixed 16:9
/// output is what made the feed look horizontally stretched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineFormat {
    pub width: u32,
    pub height: u32,
    pub fps: i32,
}

impl PipelineFormat {
    pub const DEFAULT: PipelineFormat = PipelineFormat {
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
    };

    /// I420 Y-plane stride.
    pub fn stride(&self) -> u32 {
        self.width
    }

    /// I420 frame size = W*H (Y) + 2 * (W/2 * H/2) (U,V) = W*H*3/2.
    pub fn frame_size(&self) -> usize {
        (self.width as usize * self.height as usize * 3) / 2
    }
}

/// Grace period before releasing the real camera once the provide node leaves
/// STREAMING. Consumers that go through xdg-desktop-portal (browsers) probe the
/// node with rapid connect/disconnect blips and retry if the first frames are
/// the warmup placeholder; tearing the capture pipeline on every blip would
/// thrash the camera (open takes ~250 ms) and never deliver a stable stream.
/// Holding briefly keeps the camera warm across blips while still releasing it
/// promptly — and the LED off — when the consumer is really gone.
const CAPTURE_RELEASE_GRACE: Duration = Duration::from_millis(300);

#[derive(Debug)]
pub enum PipelineCommand {
    Start(AppState),
    Stop,
    /// Replace the worker's stored config and push it onto the live capture
    /// elements. Sent after any effect toggle or param change, carrying the
    /// daemon's canonical `AppState` so the worker never re-parses per-key
    /// values (a duplicated mapping here once let param changes go stale until
    /// a restart).
    ApplyEffects(AppState),
    /// Fire a one-shot reaction burst (a [`shared::dbus::REACTION_IDS`] id) into
    /// the live `oe_effects` element. A no-op when no capture pipeline is up
    /// (nothing is consuming the virtual camera).
    TriggerReaction(String),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Advertised { sink: String },
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
        if let Err(err) = filters::register() {
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
    // The format the provide node currently advertises; follows the selected
    // camera's preferred native mode.
    let mut current_fmt = PipelineFormat::DEFAULT;
    // Whether a consumer is currently pulling (provide node STREAMING).
    let mut consumer_streaming = false;
    // When set, the capture pipeline is released once this instant passes and no
    // consumer has reconnected (debounce against portal probe/retry blips).
    let mut stop_deadline: Option<Instant> = None;

    loop {
        match commands.try_recv() {
            Ok(PipelineCommand::Start(app)) => {
                // Tear any running capture so a config/camera change takes effect.
                if let Some(c) = capture.take() {
                    c.stop();
                }
                bridge.clear();
                stop_deadline = None;

                // Adopt the selected camera's preferred native mode. A camera
                // switch can change the advertised format, which requires
                // re-advertising the provide node (consumers renegotiate on
                // reconnect).
                let fmt = builder::probe_format(&app);
                stored_app = Some(app);
                if provider.is_some() && fmt != current_fmt {
                    info!(?fmt, "virtual camera format changed; re-advertising");
                    if let Some(p) = provider.take() {
                        p.stop();
                    }
                    consumer_streaming = false;
                }
                current_fmt = fmt;

                // Arm the provide node if not already advertised. The node sits
                // in PAUSED with the real camera untouched until a consumer links.
                if provider.is_none() {
                    provider = Some(spawn_provider(&bridge, current_fmt, &capture_tx, &events));
                    let _ = events.blocking_send(PipelineEvent::Advertised {
                        sink: format!("pipewire:{PIPEWIRE_NODE_NAME}"),
                    });
                }

                // If a consumer is already streaming (live camera switch), rebuild
                // the capture immediately rather than waiting for a state change.
                if consumer_streaming && let Some(app) = stored_app.clone() {
                    start_capture(&app, current_fmt, &bridge, &events, &mut capture);
                }
            }
            Ok(PipelineCommand::Stop) => {
                if let Some(c) = capture.take() {
                    c.stop();
                }
                bridge.clear();
                consumer_streaming = false;
                stop_deadline = None;
                if let Some(p) = provider.take() {
                    p.stop();
                }
                let _ = events.blocking_send(PipelineEvent::Stopped);
            }
            Ok(PipelineCommand::ApplyEffects(app)) => {
                if let Some(c) = capture.as_ref() {
                    c.apply_app_state(&app);
                }
                stored_app = Some(app);
            }
            Ok(PipelineCommand::TriggerReaction(id)) => {
                // Only meaningful while a consumer is streaming; otherwise drop.
                if let Some(c) = capture.as_ref() {
                    c.trigger_reaction(&id);
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
                // A consumer is back -> cancel any pending release and ensure the
                // camera is running (it may still be warm from a recent blip).
                stop_deadline = None;
                if capture.is_none()
                    && let Some(app) = stored_app.clone()
                {
                    start_capture(&app, current_fmt, &bridge, &events, &mut capture);
                }
            }
            Ok(CaptureCmd::Stop) => {
                consumer_streaming = false;
                // Debounce: don't release the camera yet — a portal probe/retry
                // may reconnect within the grace window. Arm a deadline instead.
                if capture.is_some() && stop_deadline.is_none() {
                    stop_deadline = Some(Instant::now() + CAPTURE_RELEASE_GRACE);
                }
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
            Err(std_mpsc::TryRecvError::Disconnected) => {}
        }

        // Release the camera once the grace window elapses with no consumer.
        if let Some(deadline) = stop_deadline {
            if !consumer_streaming
                && Instant::now() >= deadline
                && let Some(c) = capture.take()
            {
                c.stop();
                bridge.clear();
                info!("capture stopped: no consumer, camera released");
                let _ = events.blocking_send(PipelineEvent::Idle);
            }
            if consumer_streaming || Instant::now() >= deadline {
                stop_deadline = None;
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_provider(
    bridge: &Arc<Bridge>,
    fmt: PipelineFormat,
    capture_tx: &std_mpsc::Sender<CaptureCmd>,
    events: &mpsc::Sender<PipelineEvent>,
) -> ProviderHandle {
    let (quit_tx, quit_rx) = pw::channel::channel::<()>();
    let bridge = Arc::clone(bridge);
    let capture_tx = capture_tx.clone();
    let events = events.clone();
    let join = std::thread::spawn(move || {
        provider::run(bridge, fmt, capture_tx, events, quit_rx);
    });
    ProviderHandle {
        quit: quit_tx,
        join,
    }
}

/// Build + start the capture pipeline (opens the real camera) and report status.
fn start_capture(
    app: &AppState,
    fmt: PipelineFormat,
    bridge: &Arc<Bridge>,
    events: &mpsc::Sender<PipelineEvent>,
    capture: &mut Option<builder::BuiltCapture>,
) {
    match builder::build_capture_pipeline(app, fmt, Arc::clone(bridge)).and_then(|c| {
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
