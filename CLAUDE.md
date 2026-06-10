# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & test commands

```bash
# Build everything
cargo build --workspace
cargo build --workspace --release

# Build a single crate
cargo build -p openeffectsd
cargo build -p openeffects-tray
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

The CI workflow (`.github/workflows/ci.yml`) runs `cargo fmt --check` and `cargo test --workspace`. The CI file references older GTK system packages that are **no longer needed** — the tray no longer depends on GTK.

## Architecture

### Process model

Four binaries communicate exclusively over D-Bus session bus (`org.openeffects.Daemon`, `/org/openeffects/Daemon`):

```
openeffectsd ──D-Bus──► openeffects-tray   (companion systemd unit, always running)
             ──D-Bus──► openeffectsctl      (ad-hoc CLI)
             ──D-Bus──► openeffects         (GUI, on-demand stub for now)
```

The daemon is the only process that touches GStreamer, PipeWire, or cameras. All clients are stateless D-Bus consumers.

### D-Bus interfaces

Three interfaces live at the same object path, all defined in `data/dbus/*.xml`:

| Interface | Purpose |
|---|---|
| `org.openeffects.Daemon1` | Pipeline lifecycle (`Start`, `Stop`), `Status` property, `StatusChanged` signal |
| `org.openeffects.Effects1` | Effect toggles and params (`SetEnabled`, `SetParam`, `GetAllState`), `EffectChanged` signal |
| `org.openeffects.Devices1` | Camera enumeration and selection, `VirtualCameraInfo` property |

String constants for all three are in `shared/src/dbus.rs`. **When you modify a `.xml` file, `build.rs` in `daemon`, `cli`, and `tray` automatically regenerates proxy code into `$OUT_DIR/proxies.rs`** via `zbus_xmlgen`. You do not need to hand-edit generated code.

### Daemon internals (`daemon/`)

- `src/main.rs` — registers three zbus `#[interface]` structs on the session bus and drives the pipeline event loop
- `src/state.rs` — `DaemonState` holds `AppState` (config) + runtime fields; `DaemonStatus` enum guards valid state transitions
- `src/dbus_server.rs` — implements all three D-Bus interfaces; state mutations go through `Arc<RwLock<DaemonState>>`; pipeline commands go through `mpsc::Sender<PipelineCommand>`
- `src/pipeline/` — the virtual camera is a **two-stream + userspace bridge** design (see the "On-Demand PipeWire Virtual Camera" model). The provide side is a **native libpipewire** node; the capture side is GStreamer (elements are not `Send`, so they stay on the worker thread):
  - `provider.rs` — native `pw_stream` `Video/Source` node (`media.class=Video/Source`, `node.name=openeffects`), runs its own `pw_main_loop` on a dedicated thread. **The on-demand hinge**: its `state_changed` callback maps `STREAMING → CaptureCmd::Start` (open camera, LED on) and `PAUSED`/`UNCONNECTED` → `CaptureCmd::Stop` (tear capture, LED off). `process()` serves the latest frame from the bridge (black placeholder until the first frame) and stamps the `SPA_META_Header` meta; `param_changed` answers the `Buffers` **and** `Meta(Header)` params after format negotiation.
  - `bridge.rs` — `Bridge`: a `Mutex<Option<Vec<u8>>>` latest-frame slot, `Arc`-shared between the appsink writer and the provider reader. Newest frame overwrites the previous; `clear()` on capture stop so no stale frame is served on reconnect.
  - `builder.rs` — builds the capture pipeline only: `source → decodebin → videoconvert → videoscale → capsfilter(I420 1280x720@30) → effects_bin → appsink`, where the appsink callback writes each processed frame into the bridge. Source falls back to `videotestsrc` if no camera is available.
  - `probe.rs` — now just holds `PIPEWIRE_NODE_NAME` (`"openeffects"`); there is no GStreamer output sink to probe anymore.
  - `effects.rs` — the effects bin: `queue → videobalance(oe_videobalance) → videocrop(oe_videocrop) → videoconvert → videoscale`. Phase 1 only; ML effects come in Phase 2.
  - Fixed format `I420 1280x720@30` (`WIDTH`/`HEIGHT`/`FPS`/`STRIDE`/`FRAME_SIZE` consts in `mod.rs`) is shared by the appsink and the provide node so frames are byte-compatible without per-frame conversion.

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

### Tray (`tray/`)

Uses **`ksni`** (pure-Rust StatusNotifierItem) — no GTK, no C dependencies. Works natively with KDE Plasma's SNI protocol and any SNI-capable compositor (waybar, XFCE, etc.).

- `src/tray_item.rs` — implements `ksni::Tray`; holds `status` + per-effect `enabled` map; `apply_update()` is called from the D-Bus thread via `ksni::Handle::update()`
- `src/dbus_client.rs` — async tokio loop that subscribes to `EffectChanged` signals and polls `Status` every 5 s; pushes updates to the tray via `handle.update(|t| t.apply_update(...))`
- Threading: `ksni::TrayService::spawn()` runs the SNI service internally; the `tokio` runtime drives D-Bus; `TrayCommand`s flow from ksni callbacks → tokio channel → D-Bus calls using `blocking_send()`

### Shared library (`shared/`)

- `src/config.rs` — `AppState` (TOML serde) stored at `~/.config/openeffects/state.toml`. Use `AppState::load_or_default()` / `save()` for XDG paths, or `load_or_default_from(path)` / `save_to(path)` in tests.
- `src/dbus.rs` — shared D-Bus constants (`SERVICE_NAME`, `OBJECT_PATH`, interface name strings, `EFFECT_IDS` array) and `VariantMap` type alias + helpers for converting `OwnedValue`.

### Config and state

`AppState` is the single source of truth for persisted config. The daemon loads it on startup; every `SetEnabled` / `SetParam` D-Bus call immediately saves the updated state. Effect IDs are the five strings in `shared::dbus::EFFECT_IDS`: `center_stage`, `portrait_blur`, `bg_replace`, `studio_light`, `reactions`.

## Runtime requirements (Arch Linux / KDE Plasma 6)

- A running **PipeWire** session (≥ 1.0) and **WirePlumber** (≥ 0.5). The provide node is published via native libpipewire (the `pipewire` crate), so `libpipewire-0.3` + `libspa-0.2` dev headers (pkg-config) and `clang`/`libclang` (bindgen) are needed at **build** time.
- `gst-plugin-pipewire` / `gst-plugin-good` for the camera **source** (`pipewiresrc` / `v4l2src`); without a camera the daemon falls back to `videotestsrc`.
- Output is **PipeWire-only** (`media.class=Video/Source`). Consumer reach is limited to PipeWire-camera-aware apps: Firefox (`media.webrtc.camera.allow-pipewire=true`), flagged Chromium (`--enable-features=WebRtcPipeWireCamera`), OBS. Legacy V4L2-only apps are not supported (no v4l2loopback bridge).
- The daemon registers `Type=dbus` in its systemd unit so `openeffects-tray.service` (which is `After=openeffectsd.service`) waits for the bus name before starting.

## Notes

- The README.md mentions GTK4/libadwaita for the GUI — this is outdated. The tray uses `ksni` (pure Rust), and the GUI crate is currently a stub. No GTK dependencies exist anywhere in the workspace.
- The `--start` flag on `openeffectsd` auto-starts the pipeline on launch (useful for manual testing without a D-Bus `Start()` call).
- `openeffectsctl status --short` is the Waybar-compatible one-liner output.
