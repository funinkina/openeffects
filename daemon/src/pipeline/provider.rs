//! Native libpipewire `Video/Source` provide node — the on-demand hinge.
//!
//! At daemon start this node is advertised to the graph and sits in `PAUSED`
//! with the real camera untouched (webcam LED off). When a consumer (browser,
//! OBS, ...) selects and starts the virtual camera, the provide stream moves to
//! `STREAMING`; that transition tells the GStreamer worker to open the capture
//! pipeline (LED on). When the consumer leaves, the stream drops back to
//! `PAUSED`/`UNCONNECTED` and the worker tears the capture pipeline down (LED
//! off). Graph laziness alone does *not* gate a hardware capture stream, so this
//! explicit `state_changed` gating is what makes "LED off until an app opens the
//! camera" actually hold.
//!
//! libpipewire objects are not `Send`, so the whole node runs on its own thread
//! driving a `pw_main_loop`. It talks to the rest of the daemon only through
//! channels (`CaptureCmd` to the worker, `PipelineEvent` to the D-Bus state) and
//! the shared `Arc<Bridge>` latest-frame slot.

use std::io::Cursor;
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;
use std::time::Duration;

use pipewire as pw;
use pw::spa::pod::Pod;
use pw::{properties::properties, spa};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::bridge::Bridge;
use super::probe::PIPEWIRE_NODE_NAME;
use super::{CaptureCmd, PipelineEvent, FPS, FRAME_SIZE, HEIGHT, STRIDE, WIDTH};

/// State carried through the stream callbacks.
struct UserData {
    /// Emit `VirtualCameraVerified` only once, on first registration.
    announced: bool,
    capture_tx: StdSender<CaptureCmd>,
    events: mpsc::Sender<PipelineEvent>,
    bridge: Arc<Bridge>,
    /// Monotonic frame counter for the buffer header meta (`seq`/`pts`).
    frame_count: u64,
}

/// Run the provide node until a unit message is received on `quit_rx`.
///
/// On fatal setup failure (e.g. no PipeWire daemon) an `Error` event is emitted
/// and the function returns; the worker treats the node as down.
pub fn run(
    bridge: Arc<Bridge>,
    capture_tx: StdSender<CaptureCmd>,
    events: mpsc::Sender<PipelineEvent>,
    quit_rx: pw::channel::Receiver<()>,
) {
    if let Err(err) = run_inner(bridge, capture_tx, events.clone(), quit_rx) {
        error!(%err, "provide node failed");
        let _ = events.blocking_send(PipelineEvent::Error(format!("provide node: {err}")));
    }
}

fn run_inner(
    bridge: Arc<Bridge>,
    capture_tx: StdSender<CaptureCmd>,
    events: mpsc::Sender<PipelineEvent>,
    quit_rx: pw::channel::Receiver<()>,
) -> Result<(), pw::Error> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    // Quit the loop when the worker drops/asks the node to stop.
    let _quit = quit_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let data = UserData {
        announced: false,
        capture_tx,
        events,
        bridge,
        frame_count: 0,
    };

    let stream = pw::stream::StreamRc::new(
        core,
        "openeffects-source",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Source",
            *pw::keys::MEDIA_ROLE => "Camera",
            // media.class=Video/Source is what makes WirePlumber surface this as
            // a *camera* that consumers can pick, rather than a generic stream.
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::NODE_NAME => PIPEWIRE_NODE_NAME,
            *pw::keys::NODE_DESCRIPTION => "OpenEffects Virtual Camera",
        },
    )?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(|_stream, ud, old, new| {
            use pw::stream::StreamState;
            info!(?old, ?new, "provide node state changed");
            match new {
                StreamState::Streaming => {
                    // A consumer linked and the graph is cycling -> camera on.
                    let _ = ud.capture_tx.send(CaptureCmd::Start);
                }
                StreamState::Paused => {
                    // First PAUSED = the node is registered and advertised.
                    if !ud.announced {
                        ud.announced = true;
                        let _ = ud
                            .events
                            .blocking_send(PipelineEvent::VirtualCameraVerified {
                                node_name: PIPEWIRE_NODE_NAME.into(),
                                found: true,
                            });
                    }
                    // No consumer pulling (resting state) -> release the camera.
                    let _ = ud.capture_tx.send(CaptureCmd::Stop);
                }
                StreamState::Unconnected | StreamState::Error(_) => {
                    let _ = ud.capture_tx.send(CaptureCmd::Stop);
                }
                StreamState::Connecting => {}
            }
        })
        .param_changed(|stream, _ud, id, param| {
            // When the format is fixated, answer with our buffer requirements so
            // the consumer/server allocates buffers big enough for one I420 frame.
            //
            // The Meta(Header) param is not optional: WebRTC consumers (Chromium,
            // Firefox) request SPA_META_Header and then dereference it without a
            // null check (video_capture_pipewire.cc, `h->flags`). The meta region
            // is only allocated when *both* ports announce it, so omitting it
            // here segfaults the browser's video-capture process on the first
            // delivered frame.
            if param.is_none() || id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let buffers = build_buffers_pod();
            let meta_header = build_meta_header_pod();
            let (Some(buffers_pod), Some(meta_pod)) =
                (Pod::from_bytes(&buffers), Pod::from_bytes(&meta_header))
            else {
                return;
            };
            let mut params = [buffers_pod, meta_pod];
            if let Err(err) = stream.update_params(&mut params) {
                warn!(%err, "update_params(Buffers, Meta) failed");
            }
        })
        .process(|stream, ud| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }
                let data = &mut datas[0];

                let written = if let Some(slice) = data.data() {
                    match ud.bridge.copy_latest(slice) {
                        Some(n) => n,
                        None => {
                            // No processed frame yet -> emit black so a freshly
                            // connected consumer sees a valid signal immediately.
                            let n = FRAME_SIZE.min(slice.len());
                            fill_black_i420(&mut slice[..n]);
                            n
                        }
                    }
                } else {
                    0
                };

                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = STRIDE as i32;
                *chunk.size_mut() = written as u32;
            }

            // Stamp the header meta consumers negotiated (see param_changed).
            // pipewire-rs only exposes metas read-only, but the meta region
            // lives in the buffer's shared memory which the producer owns
            // during this cycle, so writing through the pointer is sound.
            if let Some(header) = buffer.find_meta::<spa::buffer::meta::MetaHeader>() {
                let frame_duration_ns = 1_000_000_000u64 / FPS as u64;
                let h = header as *const spa::buffer::meta::MetaHeader
                    as *mut pw::spa::sys::spa_meta_header;
                unsafe {
                    (*h).flags = 0;
                    (*h).offset = 0;
                    (*h).pts = (ud.frame_count * frame_duration_ns) as i64;
                    (*h).dts_offset = 0;
                    (*h).seq = ud.frame_count;
                }
                ud.frame_count += 1;
            }
        })
        .register()?;

    let format = build_format_pod();
    let Some(format_pod) = Pod::from_bytes(&format) else {
        return Err(pw::Error::CreationFailed);
    };
    let mut params = [format_pod];

    // OUTPUT direction = we implement a Source. No AUTOCONNECT: consumers come to
    // us via WirePlumber, we don't link ourselves to anything. DRIVER: a virtual
    // camera has no hardware clock, and camera consumers expect the *source* to
    // drive the graph — so we clock our own output cadence (see the timer below).
    // Without this the graph never cycles, process() never fires, and consumers
    // see a black "no active stream".
    //
    // RT_PROCESS is required, not an optimisation: without it the stream
    // processes on the main loop and PipeWire (>= 1.2) schedules the node
    // *asynchronously* (`node.async = true`). An async driver flips connecting
    // consumers to async scheduling too, which crashes Chromium's video-capture
    // service in a connect/crash/retry loop and leaves browsers on a black
    // frame. Synchronous data-loop processing keeps consumers on the normal
    // sync path, like a real v4l2 camera node.
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::DRIVER
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Drive output at the target framerate: each tick, if we are the active
    // driver (a consumer is streaming), trigger a graph cycle so process() runs
    // and serves the latest frame. When PAUSED, is_driving() is false and the
    // tick is a cheap no-op, so we never spin the camera with no consumer.
    let _timer = {
        let stream = stream.clone();
        let timer = mainloop.loop_().add_timer(move |_expirations| {
            if stream.is_driving() {
                let _ = stream.trigger_process();
            }
        });
        let interval = Duration::from_nanos(1_000_000_000 / FPS as u64);
        timer.update_timer(Some(interval), Some(interval));
        timer
    };

    info!(
        node = PIPEWIRE_NODE_NAME,
        "virtual camera advertised; waiting for consumers"
    );
    mainloop.run();
    info!("provide node loop exited");
    Ok(())
}

/// The single video format we advertise to consumers: I420 at the fixed
/// resolution/framerate the capture pipeline normalises to.
fn build_format_pod() -> Vec<u8> {
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Id,
            pw::spa::param::video::VideoFormat::I420
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: WIDTH,
                height: HEIGHT
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Fraction,
            pw::spa::utils::Fraction {
                num: FPS as u32,
                denom: 1
            }
        ),
    );
    pw::spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .unwrap()
    .0
    .into_inner()
}

/// Buffer requirements answered during format negotiation. The buffer-param keys
/// have no typed enum in libspa-rs, so we build the object from raw `spa_sys`
/// constants.
fn build_buffers_pod() -> Vec<u8> {
    use pw::spa::pod::{ChoiceValue, Object, Property, Value};
    use pw::spa::sys;
    use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags};

    let mem_ptr = 1i32 << sys::SPA_DATA_MemPtr;

    let obj = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamBuffers,
        id: sys::SPA_PARAM_Buffers,
        properties: vec![
            Property::new(
                sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 8,
                        min: 2,
                        max: 16,
                    },
                ))),
            ),
            Property::new(sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(sys::SPA_PARAM_BUFFERS_size, Value::Int(FRAME_SIZE as i32)),
            Property::new(sys::SPA_PARAM_BUFFERS_stride, Value::Int(STRIDE as i32)),
            Property::new(
                sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: mem_ptr,
                        flags: vec![mem_ptr],
                    },
                ))),
            ),
        ],
    };
    pw::spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .unwrap()
        .0
        .into_inner()
}

/// Announce support for the buffer header meta (`SPA_META_Header`). WebRTC
/// consumers request it and crash if the negotiated buffers don't carry it; the
/// region is only allocated when the producer side lists it too.
fn build_meta_header_pod() -> Vec<u8> {
    use pw::spa::pod::{Object, Property, Value};
    use pw::spa::sys;
    use pw::spa::utils::Id;

    let obj = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamMeta,
        id: sys::SPA_PARAM_Meta,
        properties: vec![
            Property::new(
                sys::SPA_PARAM_META_type,
                Value::Id(Id(sys::SPA_META_Header)),
            ),
            Property::new(
                sys::SPA_PARAM_META_size,
                Value::Int(std::mem::size_of::<sys::spa_meta_header>() as i32),
            ),
        ],
    };
    pw::spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .unwrap()
        .0
        .into_inner()
}

/// Fill an I420 buffer with limited-range black (Y=16, Cb=Cr=128).
fn fill_black_i420(buf: &mut [u8]) {
    let y_size = (WIDTH as usize) * (HEIGHT as usize);
    let chroma = y_size / 4;
    let end = (y_size + 2 * chroma).min(buf.len());
    let y_end = y_size.min(buf.len());
    for b in &mut buf[..y_end] {
        *b = 16;
    }
    if y_end < end {
        for b in &mut buf[y_end..end] {
            *b = 128;
        }
    }
}
