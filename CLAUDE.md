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
- `src/pipeline/` — GStreamer lives entirely in a `std::thread::spawn_blocking` thread (GStreamer elements are not `Send` across await points):
  - `probe.rs` — detects whether to output to PipeWire virtual node, v4l2loopback, or fakesink. Force v4l2 path with `OPENEFFECTS_FORCE_V4L2=1`.
  - `builder.rs` — constructs the pipeline: `pipewiresrc → capsfilter → effects_bin → pipewiresink` (or fallbacks). Source falls back to `videotestsrc` if `pipewiresrc` plugin is absent.
  - `effects.rs` — the effects bin: `queue → videobalance(oe_videobalance) → videocrop(oe_videocrop) → videoconvert → videoscale`. Phase 1 only; ML effects come in Phase 2.

Auto-pause: the pipeline worker polls consumer connectivity every 200 ms. After 30 s without a consumer linked to the PipeWire sink, the pipeline pauses and emits `PipelineEvent::Idle`.

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

- `gst-plugin-pipewire` must be installed for the PipeWire source/sink. Without it the daemon falls back to `videotestsrc → fakesink`.
- `v4l2loopback-dkms` (AUR) needed for the `/dev/video*` fallback path.
- The daemon registers `Type=dbus` in its systemd unit so `openeffects-tray.service` (which is `After=openeffectsd.service`) waits for the bus name before starting.

## Notes

- The README.md mentions GTK4/libadwaita for the GUI — this is outdated. The tray uses `ksni` (pure Rust), and the GUI crate is currently a stub. No GTK dependencies exist anywhere in the workspace.
- The `--start` flag on `openeffectsd` auto-starts the pipeline on launch (useful for manual testing without a D-Bus `Start()` call).
- `openeffectsctl status --short` is the Waybar-compatible one-liner output.
