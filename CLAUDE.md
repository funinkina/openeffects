# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & test commands

```bash
# Build everything
cargo build --workspace
cargo build --workspace --release

# Build a single crate
cargo build -p openeffectsd
cargo build -p openeffects
cargo build -p openeffectsctl

# Run all tests
cargo test --workspace

# Run tests for one crate
cargo test -p shared
cargo test -p openeffectsd

# Run a single test by name
cargo test -p shared config::tests::toml_round_trip
cargo test -p openeffectsd pipeline::effects::tests::effects_bin_builds_without_panic

# Integration tests that require a live daemon are marked #[ignore]; run them with:
cargo test -p openeffects-integration-tests -- --include-ignored

# Lint / format
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

The CI workflow (`.github/workflows/ci.yml`) runs `cargo fmt --check` and `cargo test --workspace`. Building the `gui` crate requires `gtk4` and `libadwaita` dev packages (pkg-config) in addition to the GStreamer/PipeWire deps below.

## Architecture

### Process model

Three binaries communicate exclusively over D-Bus session bus (`org.openeffects.Daemon`, `/org/openeffects/Daemon`):

```
openeffectsd ──D-Bus──► openeffectsctl      (ad-hoc CLI)
             ──D-Bus──► openeffects         (GTK4/libadwaita GUI, on-demand)
```

The daemon is the only process that touches GStreamer, PipeWire, or cameras. All clients are stateless D-Bus consumers.

### D-Bus interfaces

Three interfaces live at the same object path, all defined in `data/dbus/*.xml`:

| Interface                  | Purpose                                                                                     |
| -------------------------- | ------------------------------------------------------------------------------------------- |
| `org.openeffects.Daemon1`  | Pipeline lifecycle (`Start`, `Stop`), `Status` property, `StatusChanged` signal             |
| `org.openeffects.Effects1` | Effect toggles and params (`SetEnabled`, `SetParam`, `GetAllState`), `EffectChanged` signal |
| `org.openeffects.Devices1` | Camera enumeration and selection, `VirtualCameraInfo` property                              |

String constants for all three are in `shared/src/dbus.rs`. **When you modify a `.xml` file, `build.rs` in `daemon`, `cli`, and `gui` automatically regenerates proxy code into `$OUT_DIR/proxies.rs`** via `zbus_xmlgen`. You do not need to hand-edit generated code.

### Daemon internals (`daemon/`)

- `src/main.rs` — registers three zbus `#[interface]` structs on the session bus and drives the pipeline event loop
- `src/state.rs` — `DaemonState` holds `AppState` (config) + runtime fields; `DaemonStatus` enum guards valid state transitions
- `src/dbus_server.rs` — implements all three D-Bus interfaces; state mutations go through `Arc<RwLock<DaemonState>>`; pipeline commands go through `mpsc::Sender<PipelineCommand>`
- `src/pipeline/` — the virtual camera is a **two-stream + userspace bridge** design (see the "On-Demand PipeWire Virtual Camera" model). The provide side is a **native libpipewire** node; the capture side is GStreamer (elements are not `Send`, so they stay on the worker thread):
  - `provider.rs` — native `pw_stream` `Video/Source` node (`media.class=Video/Source`, `node.name=openeffects`), runs its own `pw_main_loop` on a dedicated thread. **The on-demand hinge**: its `state_changed` callback maps `STREAMING → CaptureCmd::Start` (open camera, LED on) and `PAUSED`/`UNCONNECTED` → `CaptureCmd::Stop` (tear capture, LED off). `process()` serves the latest frame from the bridge (black placeholder until the first frame) and stamps the `SPA_META_Header` meta; `param_changed` answers the `Buffers` **and** `Meta(Header)` params after format negotiation.
  - `bridge.rs` — `Bridge`: a `Mutex<Option<Vec<u8>>>` latest-frame slot, `Arc`-shared between the appsink writer and the provider reader. Newest frame overwrites the previous; `clear()` on capture stop so no stale frame is served on reconnect.
  - `builder.rs` — builds the capture pipeline only: `source → capsfilter(native WxH@fps, raw|jpeg) → decodebin → videoconvert → videoscale → capsfilter(I420 WxH@fps) → effects_bin → appsink`, where the appsink callback writes each processed frame into the bridge. The **source capsfilter pins the camera to the same mode the virtual camera advertises** (MJPEG allowed — decodebin decodes it), so videoscale never rescales and the aspect ratio is preserved; letting negotiation pick a default raw mode (e.g. 4:3 640x480) and scaling it to the output is what stretched the feed. Source falls back to `videotestsrc` if no camera is available. `resolve_camera()` + `probe_format()` decide camera and mode; `cameras::preferred_format()` probes modes via DeviceMonitor caps (no device open) and prefers exact 1280x720, else the mode with area closest to it at ≥ 24 fps.
  - `probe.rs` — now just holds `PIPEWIRE_NODE_NAME` (`"openeffects"`); there is no GStreamer output sink to probe anymore.
  - `effects.rs` — the effects bin: `queue → videobalance(oe_videobalance, Studio Light) → videoconvert → capsfilter(RGBA) → oe_effects → videoconvert → videoscale`. Studio Light is the stock `videobalance`; the three ML effects (Portrait Blur, Background Replace, Center Stage) all live in the single **CPU** `oe_effects` filter (`pipeline/filters/oe_effects.rs`), which runs on RGBA system memory. `apply_app_state_to_elements` pushes config onto `videobalance` + `oe_effects` properties (kebab-case: `portrait-blur-enabled`, `bg-replace-path`, `center-stage-zoom`, …).
  - `filters/oe_effects.rs` — the `oe_effects` `gst_video::VideoFilter` (in-place, RGBA). Per frame it runs **selfie segmentation once** (reused for one extra frame), then composites in order: portrait-blur / bg-replace (`out = fg·mask + bg·(1-mask)`, where `bg` is a separable box-blur of the frame, or a user image/`#RRGGBB` color), then an EMA-smoothed center-stage crop+zoom driven by a YuNet face box (falling back to the mask's foreground extent). Idle effects → `set_passthrough(true)`. Registered via `filters::register()` as `oe_effects`.
  - `inference/` — the ONNX layer (CPU EP via `ort`'s `download-binaries`). `engine.rs` holds `SelfieSeg` (`pixel_values` NHWC→`alphas` 256², the foreground mask) and `YuNet` (`input` 640² BGR → anchor-free cls/obj/bbox at strides 8/16/32, decoded like OpenCV `FaceDetectorYN` + NMS). `manifest.rs`/`registry.rs` parse model manifests and resolve sha256-verified variant files under `~/.local/share/openeffects/models/` (install with `scripts/fetch-models.sh`). `probe_ep()` returns `Cpu`; `detect_tier()` reports T1–T4 from DRM render nodes (surfaced as `Capabilities.tier`).
  - The format is a runtime `PipelineFormat { width, height, fps }` (`mod.rs`), probed from the selected camera's native modes at `Start` (default `1280x720@30` when nothing can be probed). It is shared by the source capsfilter, the appsink, and the provide node so frames are byte-compatible without per-frame conversion. A camera switch that changes the format re-advertises the provide node (consumers reconnect).

On-demand lifecycle: `Start` arms the provide node (advertised, `PAUSED`, real camera untouched → status `Idle`). When a consumer links, `STREAMING` opens the capture pipeline (status `Running`). When the consumer leaves, the capture pipeline is torn to `NULL`, releasing the real camera. There is no auto-pause polling — gating is event-driven from the native node's `state_changed`.

Four details make this work with real consumers:
- **Driving**: a virtual camera has no hardware clock and camera consumers expect the *source* to drive the graph, so the provide node connects with `StreamFlags::DRIVER` and a loop timer calls `trigger_process()` at `FPS` (only while `is_driving()`, i.e. a consumer is streaming). Without this the graph never cycles and consumers see a black "no active stream". Requires the `pipewire` crate's `v0_3_34` feature.
- **Sync scheduling** (`StreamFlags::RT_PROCESS`): without it the stream processes on its pw_main_loop and PipeWire ≥ 1.2 marks the node `node.async = true`. An async *driver* flips connecting consumers to async scheduling, which WebRTC consumers don't survive — Chromium's video-capture service dies in a connect/crash/retry loop and the page sees a black 2×2 track. `RT_PROCESS` keeps processing on the data loop, synchronous, like a real v4l2 camera node.
- **Header meta** (`SPA_META_Header`): browsers' `video_capture_pipewire.cc` requests this meta and dereferences it **without a null check** (`h->flags`). The meta region is only allocated when the producer announces it too, so `param_changed` must answer with a `Meta(Header)` param alongside `Buffers`, and `process()` fills pts/seq. Omitting it segfaults the browser's capture process on the first frame.
- **Release debounce** (`CAPTURE_RELEASE_GRACE`, 5 s in `mod.rs`): consumers reached via xdg-desktop-portal (browsers) probe the node with rapid connect/disconnect blips and retry if the first frames are the warmup placeholder. Releasing on every `PAUSED` would thrash the camera (open ≈250 ms) and never deliver a stable stream, so release is deferred by the grace window and cancelled if a consumer reconnects.

Headless verification of the full browser path (auto-grants camera, picks the OpenEffects device, samples pixel luma — mean ≈ 0 means black, varying ≈ 110+ means live video):

```bash
google-chrome-stable --headless=new --use-fake-ui-for-media-stream \
  --enable-features=WebRtcPipeWireCamera --enable-logging=stderr \
  "file://$PWD/scripts/camtest.html" 2>&1 | grep CONSOLE
# scripts/camtest.html: getUserMedia → canvas → console.log luma samples
```

### GUI (`gui/`)

`openeffects` is a single **GTK4 + libadwaita** (`gtk4-rs` / `adw` crates) `AdwApplicationWindow`, the primary control surface (Arch + GNOME is the dev target). It is on-demand — launched from the GNOME Activities/app grid via a `.desktop` entry, with no companion systemd unit; the daemon's own lifecycle is unaffected by whether the GUI is open.

- `build.rs` runs the same `zbus_xmlgen` codegen as `daemon`/`cli`, generating proxies into `$OUT_DIR/proxies.rs` from `data/dbus/*.xml`.
- D-Bus client pattern (`dbus_client.rs`): a background tokio task owns the `zbus::Connection` and an `mpsc` command channel; `tokio::select!` merges GUI→daemon commands (`SetEnabled`, `SetParam`, `SelectCamera`) with `EffectChanged`/`StatusChanged` signals plus `Capabilities`/`ActiveCamera` property-change streams, after an initial `GetAllState()` + `Status` + `Capabilities` + `ListCameras()` on connect. Updates are pushed to the GTK main loop via an `async_channel` drained by `glib::MainContext::spawn_local`.
- Layout: `AdwNavigationSplitView` with a sidebar `gtk::ListBox` switching a `gtk::Stack` of five `AdwPreferencesPage`s; the content header `AdwWindowTitle` shows the current page + daemon status subtitle.
  - **Effects**: one `AdwPreferencesGroup` per effect — `AdwSwitchRow` enable + `AdwComboRow`/`AdwSpinRow` params (Center Stage zoom/mode, Portrait Blur strength, Studio Light intensity/brightness/contrast). Programmatic updates block the row's signal handler to avoid echo.
  - **Camera**: `Devices1.ListCameras()`/`SelectCamera()` picker (`AdwComboRow`) + virtual-camera node readout.
  - **Backgrounds**: "None" + built-in solid-color swatches + a `gtk::FileDialog` "Browse…" (needs gtk4 feature `v4_10`); selecting one sends `bg_replace.background` + enables the effect.
  - **Model Library**: bundled models with a Ready/Missing pill from `Capabilities.models_ready`.
  - **About**: version + `Capabilities` readout (hardware tier, "Running on" EP, models, virtual-camera node).
- Live preview on the Camera page (`gtk4paintablesink`) is **deferred to Phase 4**; the GUI has no GStreamer dependency yet.

### Shared library (`shared/`)

- `src/config.rs` — `AppState` (TOML serde) stored at `~/.config/openeffects/state.toml`. Use `AppState::load_or_default()` / `save()` for XDG paths, or `load_or_default_from(path)` / `save_to(path)` in tests.
- `src/dbus.rs` — shared D-Bus constants (`SERVICE_NAME`, `OBJECT_PATH`, interface name strings, `EFFECT_IDS` array) and `VariantMap` type alias + helpers for converting `OwnedValue`.

### Config and state

`AppState` is the single source of truth for persisted config. The daemon loads it on startup; every `SetEnabled` / `SetParam` D-Bus call immediately saves the updated state. Effect IDs are the five strings in `shared::dbus::EFFECT_IDS`: `center_stage`, `portrait_blur`, `bg_replace`, `studio_light`, `reactions`.

## Runtime requirements (Arch Linux / GNOME)

- A running **PipeWire** session (≥ 1.0) and **WirePlumber** (≥ 0.5). The provide node is published via native libpipewire (the `pipewire` crate), so `libpipewire-0.3` + `libspa-0.2` dev headers (pkg-config) and `clang`/`libclang` (bindgen) are needed at **build** time.
- `gst-plugin-pipewire` / `gst-plugin-good` for the camera **source** (`pipewiresrc` / `v4l2src`); without a camera the daemon falls back to `videotestsrc`.
- Output is **PipeWire-only** (`media.class=Video/Source`). Consumer reach is limited to PipeWire-camera-aware apps: Firefox (`media.webrtc.camera.allow-pipewire=true`), flagged Chromium (`--enable-features=WebRtcPipeWireCamera`), OBS.
- The `gui` crate needs `gtk4` and `libadwaita` dev headers (pkg-config) at **build** time.
- The daemon registers `Type=dbus` in its systemd unit so D-Bus clients (`openeffects`, `openeffectsctl`) can rely on the bus name being available once the unit is active.

## Notes

- The `tray` crate (ksni-based) has been removed from the workspace; `openeffects` (GTK4/libadwaita) is now the primary GUI surface and is being built out from a stub.
- The `--start` flag on `openeffectsd` auto-starts the pipeline on launch (useful for manual testing without a D-Bus `Start()` call).
- `openeffectsctl status --short` is the Waybar-compatible one-liner output.
