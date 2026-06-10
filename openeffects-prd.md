# OpenEffects — Product Requirements Document

> Components: **`openeffectsd`** (daemon) · **`openeffects`** (GUI) · **`openeffectsctl`** (CLI)

---

## 1. Document control

| Field        | Value                                                                                                              |
| ------------ | ------------------------------------------------------------------------------------------------------------------ |
| Version      | 0.5 (implementation-ready)                                                                                         |
| Status       | Ready for development                                                                                              |
| Owner        | Aryan                                                                                                              |
| Last updated | 2026-06-10                                                                                                         |
| Prev version | 0.4 — Qt6/tray-primary architecture for KDE; superseded by GTK4/libadwaita GUI-primary architecture for Arch+GNOME |

---

## 2. Executive summary

OpenEffects is a Linux-native webcam effects engine that brings macOS-class features — Center Stage, Portrait mode, Studio Light, Background Replacement, and Reactions — to any Linux desktop, transparently, for any consumer of a PipeWire camera node or `/dev/video*` device (Zoom, Chrome, Firefox, Slack, OBS, etc.).

The system is architected as a **headless GPU-accelerated daemon** (`openeffectsd`) that owns the capture → process → publish pipeline, plus two client surfaces: a **GTK4/libadwaita GUI** (`openeffects`) as the primary day-to-day control surface for live effect toggles, params, and camera/device settings, and a **CLI** (`openeffectsctl`) for scripting and tiling-WM keybinds.

ML inference is unified behind a single **ONNX Runtime** abstraction with an Execution Provider (EP) priority chain that degrades from vendor-specific accelerators (TensorRT, CUDA, ROCm, OpenVINO) to vendor-neutral GPU (Vulkan) to CPU to heuristic fallbacks, so the app is useful across hardware from an 11th-gen Intel laptop to an RTX 4090. Heavier/higher-quality models are opt-in downloads; a functional base set is bundled.

Wayland is the only supported display protocol. X11 sessions are not a target.

**Development platform:** Primary development is on **Arch Linux** with **GNOME** (GNOME Shell, Wayland). The project is designed to be straightforward to build and deploy on Arch; it uses standard Arch packages and follows Linux best practices.

---

## 3. Goals and non-goals

### 3.1 Goals

- **G1** — Provide a virtual webcam that any Linux app can consume, with real-time effects applied.
- **G2** — Integrate naturally on GNOME (primary target); remain usable on KDE and tiling compositors (Hyprland, Sway, river) via the GTK4 GUI and CLI without requiring a tray.
- **G3** — Use the GPU wherever available, via a unified ONNX Runtime inference path that works across NVIDIA, AMD, and Intel.
- **G4** — Degrade gracefully on systems without GPU acceleration, kernel-modular virtual camera, or recent PipeWire.
- **G5** — `openeffects` (GTK4/libadwaita) is the primary control surface for live effect toggles, params, and camera settings. CLI is first-class for power users.
- **G6** — Sub-50 ms added end-to-end latency on Tier 1 hardware; sub-100 ms on Tier 2.

### 3.2 Non-goals (v1.0)

- No profiles or saved presets (current effect state is live; camera on → `openeffects` GUI available → adjust inline).
- No per-app auto-switching of effect configurations.
- No virtual studio / scene compositing (that's OBS's job).
- No remote or cloud inference.
- No X11 session support.
- No neural face relighting (planned post-v1).
- No Windows or macOS support.

---

## 4. Target users and personas

| Persona                    | DE / setup                      | Primary surface                                                             | Notes                                                           |
| -------------------------- | ------------------------------- | --------------------------------------------------------------------------- | --------------------------------------------------------------- |
| **Mira** — design lead     | Arch/Fedora + GNOME, Intel iGPU | `openeffects` GUI: Effects page for quick toggles, Camera page for settings | Wants toggle-on-join; fine-tunes blur strength once, leaves it. |
| **Rohit** — KDE user       | KDE 6 on AMD discrete GPU       | `openeffects` GUI (GTK4/libadwaita, runs via Adwaita styling)               | Secondary target; functional but non-native look on Plasma.     |
| **Aryan** — tiling WM user | Arch + Hyprland, NVIDIA         | `openeffectsctl` bound to keybinds; waybar module for status                | Never opens a GUI window; drives everything from keybinds.      |
| **Sam** — older laptop     | Ubuntu LTS, no discrete GPU     | `openeffects` GUI toggle                                                    | Cares that *something* works; happy with CPU-path blur.         |

---

## 5. Functional requirements

### 5.1 Center Stage (P0)

Frames a person centered in the output by detecting their face/body bounding box and applying a smoothed crop+zoom over time.

- **5.1.1** Track up to N=4 faces; user-selectable "primary face follow" vs "group framing" (toggle in the `openeffects` GUI's Effects page).
- **5.1.2** Smoothing avoids visible jitter on micro-movements while reacting within ~400 ms to deliberate motion.
- **5.1.3** Zoom level user-configurable: `off`, `subtle`, `normal`, `tight` — exposed as a GUI slider (`AdwSpinRow`).
- **5.1.4** Must preserve the aspect ratio of the consumer's requested format.

### 5.2 Portrait mode (P0)

Blurs the background while keeping the subject crisp.

- **5.2.1** Segmentation mask refreshed every frame; feathered edges; temporally stable across frames.
- **5.2.2** Blur strength exposed as a continuous slider in the GUI, with `light`/`medium`/`heavy` presets.
- **5.2.3** v1.0 ships Gaussian blur; disc/bokeh kernel is a stretch goal.

### 5.3 Background replacement (P0)

Replaces background with a user asset or solid color.

- **5.3.1** User assets in `~/.local/share/openeffects/backgrounds/`. Ships with 6 built-in defaults (gradients, abstract, neutral).
- **5.3.2** Background selection exposed in the GUI's Effects page (thumbnail grid, max 8 shown, "Browse…" for more).
- **5.3.3** Edge refinement (guided filter) on Tier 1/2 hardware.

### 5.4 Studio Light (P1)

Subtly brightens and separates the subject.

- **5.4.1** Face-region-aware brightness/contrast lift on T1/T2; global tone curve fallback otherwise.
- **5.4.2** Enable toggle and intensity slider both in the GUI's Effects page.

### 5.5 Reactions (P1)

Hand-gesture-triggered animated overlays.

- **5.5.1** Built-in gestures: thumbs-up → 👍 burst, peace sign → confetti, heart (two-hand) → hearts, open palm → wave, fist → fireworks.
- **5.5.2** Debounce: same gesture cannot retrigger within 3 s.
- **5.5.3** **Off by default.** Explicitly enable via the GUI's Effects page.

### 5.6 Live controls (`openeffects` GUI)

The `openeffects` GUI is the control surface for all real-time adjustments. No profile concept exists — state is live and immediate.

- **5.6.1** The GUI can be opened at any time, independent of whether `openeffectsd` has an active consumer; it connects to the daemon over D-Bus on launch and reflects current state via `GetAllState()`.
- **5.6.2** Each effect has a top-level enable switch (`AdwSwitchRow`) and inline rows for its fast parameters.
- **5.6.3** Model selection, background library management, and calibration live alongside the live toggles in the same window (Camera/Backgrounds/Model Library pages).
- **5.6.4** The header bar subtitle reflects daemon state: running+active, running+idle, or error (via `Daemon1.Status`/`StatusChanged`).

---

## 6. System architecture

### 6.1 Process model

```
  openeffectsd  (systemd --user service, Type=dbus)
  ┌──────────────────────────────────────────────────────────┐
  │  GStreamer capture pipeline → effects-bin                │
  │  Native PipeWire provide node (pw_stream,                │
  │  media.class=Video/Source, node.name=openeffects)        │
  │  On-demand: capture opens when a consumer links          │
  │                                                          │
  │  D-Bus service: org.openeffects.Daemon (session bus)     │
  └──────────────────────────────────────────────────────────┘
         ▲                              ▲
         │                              │
  openeffects (GUI)              openeffectsctl
  (on-demand, launched           (ad-hoc / keybinds)
   from app grid)
```

- `openeffectsd` is a `--user` systemd unit (`Type=dbus`); it autostarts at login independent of any client.
- `openeffects` (the GUI) is launched on demand from the GNOME app grid/Activities or directly. Closing the window does not affect the daemon.
- Both surfaces are stateless D-Bus clients. Killing either does not affect the pipeline.

## 6.2 GUI process: connection & lifecycle

`openeffects` is a single GTK4 + libadwaita process, connecting to `openeffectsd` over the same D-Bus session-bus interfaces as the CLI.

D-Bus client pattern:
- `gui/build.rs` runs the same `zbus_xmlgen` codegen as `daemon`/`cli`, generating proxies into `$OUT_DIR/proxies.rs` from `data/dbus/*.xml`.
- A background tokio task owns the `zbus::Connection` and an `mpsc` command channel, `tokio::select!`ing between: GUI→daemon commands (`SetEnabled`, `SetParam`, `SelectCamera`, `Start`/`Stop`), `EffectChanged`/`StatusChanged`/Devices1 property-change signal streams, and an initial `GetAllState()` + `ListCameras()` on connect.
- Updates are pushed to the GTK main loop via a `glib::MainContext` channel, where they update `AdwSwitchRow`/`AdwSpinRow`/etc. state.

Lifecycle:
- `openeffectsd` keeps its existing `systemd --user` `Type=dbus` autostart; it runs headless regardless of whether the GUI is open.
- `openeffects` is launched on demand via its `.desktop` entry from GNOME Activities/the app grid. Closing the window does not stop the daemon — the existing on-demand virtual-camera + release-debounce design (see CLAUDE.md) already handles idling when no GUI/consumer is present.
- No companion systemd unit for the GUI.

### 6.3 Pipeline data flow

The hot path is designed for **zero CPU copies** of full frames on Tier 1/2 hardware:

1. **Capture** — `pipewiresrc` delivers DMA-BUF-backed `GstBuffer`s.
2. **Upload to GL/Vulkan** — `glupload` wraps the DMA-BUF as a texture; no memcpy.
3. **Pre-process for inference** — shader downsamples + normalizes a copy to inference resolution (e.g., 256×256); small readback to ONNX input tensor.
4. **Inference** — ONNX session runs on selected EP; produces mask/landmarks.
5. **Upload result** — small output tensor → GPU texture.
6. **Composite** — single shader pass: segmentation mask + blur + background + face-region brightening + reaction sprites.
7. **Publish** — `v4l2sink` → `/dev/videoN` (v4l2loopback virtual device).

### 6.4 Threading model

- GStreamer pipeline runs on its own threadpool.
- D-Bus handler runs on a `zbus` async runtime.
- Dedicated **inference thread** with a single-slot ring buffer — if a new frame arrives while inference is still running on the previous one, the old request is dropped. Masks are reused for up to 2 frames (segmentation is temporally coherent; invisible at ≤2 frames).
- Reaction gesture detector runs at 1/3 the pipeline frame rate (every 3rd frame) to save budget.

---

| Layer                  | Choice                                                    | Rationale                                                                                                  |
| ---------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Language (daemon, CLI) | **Rust**                                                  | Memory safety in a long-running media daemon; strong ecosystem for PipeWire, GStreamer, Vulkan, and D-Bus. |
| Language (GUI)         | **Rust + `gtk4-rs` + `libadwaita`**                       | Native GNOME integration, single-language workspace, memory-safe, Wayland-native rendering.                |
| Media pipeline         | **GStreamer 1.24+** via `gstreamer-rs`                    | Mature; first-class PipeWire, Vulkan, VAAPI, GL integration.                                               |
| Camera capture         | **PipeWire** via `pipewiresrc` (or `v4l2src`)             | `pipewiresrc` preferred for shared camera access; `v4l2src` for `/dev/videoN` paths.                       |
| Virtual camera output  | **Native PipeWire provide node** via the `pipewire` crate | `pw_stream` `Video/Source` node (`media.class=Video/Source`); on-demand, no kernel module required.        |
| GPU compute            | **Vulkan** compute shaders + GLSL via `gst-gl`            | Vendor-neutral; `gst-gl` is well-exercised; Vulkan compute for custom kernels.                             |
| ML inference           | **ONNX Runtime** via `ort` crate                          | Single API; broadest EP coverage.                                                                          |
| IPC                    | **D-Bus (session bus)** via `zbus`                        | Universal Linux IPC; works under Flatpak.                                                                  |
| Config                 | **TOML** via `serde`                                      | Human-readable; power users can edit directly.                                                             |
| Build                  | `cargo` + `meson` + `flatpak-builder`                     | Single Rust/cargo build for all binaries; `meson`/`flatpak-builder` for packaging only.                    |
| Display protocol       | **Wayland only**                                          | X11 is deprecated on all major DEs targeted; simplifies GPU context management.                            |
---

## 7.1 Frontend architecture

The frontend stack is intentionally separated from the Rust media pipeline.

Architecture:

```
Rust daemon (openeffectsd)
        ▲
        │ D-Bus
        ▼
openeffects (GTK4 + libadwaita)
```

Rationale:

- GUI crashes cannot affect the media pipeline
- Single language (Rust) across the whole workspace
- Native GNOME look and feel via libadwaita; GTK4 renders Wayland-natively
- Reduced coupling between media/inference code and presentation layer

The GUI layer contains:

- GTK4/libadwaita widgets (`AdwApplicationWindow`, `AdwViewStack`, preferences rows)
- View models
- zbus proxy bindings (D-Bus)
- Preview rendering surfaces (`gtk4paintablesink`)

The Rust backend remains responsible for:

- Media processing
- ONNX inference
- GStreamer pipelines
- Device management
- Configuration/state persistence
- Performance management

These changes align the PRD with:
- GNOME-first Linux UX
- GTK4-native Wayland integration
- lower frontend maintenance burden (single language, no tray)
- cleaner multimedia integration
- stronger long-term Linux desktop compatibility


## 8. Unified GPU inference layer (ONNX Runtime)

All ML models — segmentation, face detection, landmarks, gesture classification — flow through one abstraction. Feature modules never touch ONNX Runtime, EPs, or device handles directly.

### 8.1 The `InferenceEngine` abstraction

```rust
pub struct InferenceEngine {
    available_eps: Vec<ExecutionProvider>,
    loaded_models: HashMap<ModelId, LoadedModel>,
    config: InferenceConfig,
}

impl InferenceEngine {
    /// Probe available EPs at startup. Caches result.
    pub fn probe() -> Self;

    /// Load model using best available EP for this model.
    pub fn load(&mut self, manifest: &ModelManifest) -> Result<ModelHandle>;

    /// Run synchronous inference. Blocks inference thread.
    pub fn infer(&self, h: ModelHandle, input: &TensorView) -> Result<TensorView>;

    /// Free GPU/CPU memory when a feature is disabled or daemon idles.
    pub fn unload(&mut self, h: ModelHandle);

    /// Exposed to the GUI for the "Running on: CUDA" status badge.
    pub fn capabilities(&self) -> EngineCapabilities;
}
```

### 8.2 Execution Provider priority chain

Probed at daemon startup. Default priority (user-overridable in config):

| Rank | EP                  | Activated when                                                   |
| ---- | ------------------- | ---------------------------------------------------------------- |
| 1    | **TensorRT**        | NVIDIA GPU + TRT runtime installed                               |
| 2    | **CUDA**            | NVIDIA GPU + CUDA runtime                                        |
| 3    | **ROCm / MIGraphX** | AMD discrete GPU + ROCm stack                                    |
| 4    | **OpenVINO**        | Intel CPU/iGPU/NPU + OpenVINO runtime                            |
| 5    | **Vulkan**          | Any Vulkan 1.2+ GPU (experimental; ORT Vulkan EP still maturing) |
| 6    | **CPU**             | Always. XNNPACK-accelerated.                                     |

**Probing procedure:** For each candidate EP, attempt to create a session with a 1-op test model. Any failure (missing shared library, driver version mismatch, permission denied) removes it from the available list. Result cached to `~/.cache/openeffects/ep-probe.json` keyed by `(driver_version, gpu_pci_id)`, TTL 24 h.

### 8.3 Per-model EP selection

Each model manifest (§9.1) declares which EPs it has been validated against. The engine selects:

```
available_eps ∩ model.supported_eps, highest priority wins
```

This matters because:
- INT8 quantized models have limited op support on some EPs.
- Some models exceed VRAM on low-memory EPs.
- Certain model/EP/driver combinations have known correctness bugs.

```rust
fn pick_ep(model: &ModelManifest, available: &[EP]) -> EP {
    for ep in &PRIORITY_CHAIN {
        if !available.contains(ep) { continue; }
        if !model.supported_eps.contains(ep) { continue; }
        if ep.estimated_vram_mb() + model.min_vram_mb > ep.available_vram_mb() { continue; }
        return *ep;
    }
    EP::Cpu
}
```

### 8.4 Tensor zero-copy

For EPs that share memory with GStreamer's GL/Vulkan context (CUDA via CUcontext interop, Vulkan EP via `VkBuffer`), input/output tensors are bound to GPU buffers without CPU roundtrip. For CPU-path EPs, one staging buffer per session is reused. The `TensorView` type hides this distinction from callers.

### 8.5 Inference budget enforcement

Every `infer()` call records wall time. A rolling P95 is maintained per (model, EP). If P95 exceeds the per-feature budget for **30 consecutive frames**, the engine emits a `BudgetExceeded` event; the `FeatureManager` downgrades one tier (§10) and persists the decision to config. The GUI shows a brief "quality reduced" toast.

---

## 9. Model management

### 9.1 Model manifest

Every model ships with a TOML manifest that is the single source of truth for how to use it:

```toml
[model]
id          = "selfie_seg"
version     = "1.2.0"
sha256      = "a3f1...e9c4"
purpose     = "segmentation"
license     = "Apache-2.0"
bundled     = true          # false = opt-in download

[model.input]
name        = "input_1"
shape       = [1, 256, 256, 3]
dtype       = "float32"
preprocess  = "scale_0_1"

[model.output.mask]
name        = "segmentation_mask"
shape       = [1, 256, 256, 1]
dtype       = "float32"
postprocess = "sigmoid_then_resize"

[model.execution]
supported_eps   = ["tensorrt", "cuda", "rocm", "openvino", "vulkan", "cpu"]
min_vram_mb     = 128

[model.variants]
fp32 = { path = "selfie_seg_v1.2.0_fp32.onnx", size_mb = 14 }
fp16 = { path = "selfie_seg_v1.2.0_fp16.onnx", size_mb = 7 }
int8 = { path = "selfie_seg_v1.2.0_int8.onnx", size_mb = 4, supported_eps = ["cpu", "openvino"] }
```

### 9.2 Bundled vs. opt-in models

| Model                      | Purpose                     | Size       | Ship status         |
| -------------------------- | --------------------------- | ---------- | ------------------- |
| MediaPipe Selfie Seg       | Segmentation (fast)         | 7 MB FP16  | **Bundled**         |
| YuNet                      | Face detection              | 2 MB       | **Bundled**         |
| MediaPipe Face Mesh (lite) | Studio Light landmarks      | 4 MB       | **Bundled**         |
| MediaPipe Hands            | Gesture recognition         | 8 MB       | **Bundled**         |
| RVM (Robust Video Matting) | Segmentation (high quality) | 14 MB FP16 | **Opt-in download** |
| MODNet                     | Segmentation (balanced)     | 6 MB FP16  | **Opt-in download** |
| MiDaS Small                | Depth estimation for bokeh  | 80 MB      | **Opt-in download** |

Opt-in models are listed in the GUI's "Model Library" tab with a download button and disk/quality tradeoff description. Downloads are verified against manifest SHA256 before use.

### 9.3 Model registry

Searched in priority order:

1. `~/.local/share/openeffects/models/` — user-installed and downloaded.
2. `/usr/share/openeffects/models/` — system-installed (distro packages).
3. Flatpak read-only mount `/app/share/openeffects/models/`.

SHA256 is verified lazily on first session creation per boot, cached by (path, mtime).

### 9.4 Variant selection

For each model, the engine selects the best variant matching the chosen EP and hardware tier:

| Tier                              | NVIDIA discrete | Intel iGPU  | No GPU                   |
| --------------------------------- | --------------- | ----------- | ------------------------ |
| Quality (user opt-in heavy model) | RVM FP16        | RVM FP16    | Selfie INT8              |
| Standard (default)                | Selfie FP16     | Selfie FP16 | Selfie INT8              |
| Performance (user override)       | Selfie INT8     | Selfie INT8 | Selfie INT8 + frame skip |

---

## 10. Fallback strategy

Every feature has at least one path that works on a system with no GPU, no PipeWire virtual node, and no ONNX EP beyond CPU.

### 10.1 Hardware capability tiers

| Tier   | Definition                                                         | Target                                |
| ------ | ------------------------------------------------------------------ | ------------------------------------- |
| **T1** | Discrete GPU (NVIDIA RTX, AMD RX 6000+, Intel Arc) + PipeWire 1.0+ | All features, 1080p60, full quality   |
| **T2** | Modern iGPU (Intel Xe/UHD 11th gen+, AMD Vega) + PipeWire 1.0+     | All features, 1080p30, FP16 models    |
| **T3** | Older iGPU or APU + PipeWire 0.3+                                  | Core features, 720p30, INT8 models    |
| **T4** | No GPU acceleration                                                | Heuristic effects only, 720p30, no ML |

Detected at startup; re-detected on daemon restart. User can force a lower tier for battery savings.

### 10.2 Per-feature degradation matrix

| Feature            | T1 (best)                    | T2                          | T3                             | T4 (heuristic)                                             |
| ------------------ | ---------------------------- | --------------------------- | ------------------------------ | ---------------------------------------------------------- |
| **Center Stage**   | YuNet GPU + Kalman smoother  | YuNet GPU + EMA             | YuNet CPU + EMA                | Center-weighted static crop, no tracking                   |
| **Portrait blur**  | RVM FP16 GPU + guided filter | Selfie Seg FP16 GPU         | Selfie Seg INT8 CPU            | Radial blur around face bbox (or center if no face detect) |
| **BG replacement** | RVM FP16 + edge refinement   | Selfie Seg FP16 GPU         | Selfie Seg INT8 CPU            | Chroma-key only (user must set physical green screen)      |
| **Studio Light**   | Face mesh + region tone map  | Face bbox + region brighten | Global tone curve, no face     | Auto-exposure only                                         |
| **Reactions**      | MediaPipe Hands GPU          | MediaPipe Hands CPU         | Reduced-rate (every 3rd frame) | **Disabled** (no viable heuristic)                         |

### 10.3 Camera I/O fallback tree

```
Is v4l2loopback module present + /dev/videoN writable?
├── Yes → PRIMARY PATH. pipewiresrc/v4l2src → effects-bin → v4l2sink → /dev/videoN.
└── No  → Virtual camera disabled. Surface actionable error in the
          GUI: "Load v4l2loopback to enable virtual camera."
          openeffectsd stays running; effects can still be previewed
          in the GUI's built-in preview pane.
```

### 10.4 Runtime auto-degradation

A `FeatureManager` watches a rolling 5-second frame-time window per feature. If P95 frame time exceeds budget for **30 consecutive frames**:

1. Downgrade feature one tier.
2. Persist the decision to `~/.cache/openeffects/tier-overrides.toml`.
3. Show a GUI toast: "Portrait blur quality reduced — GPU budget exceeded."

Tier upgrade only happens on explicit user action (GUI or `openeffectsctl`); the system never silently raises quality on its own.

---

## 11. D-Bus interface specification

Service: `org.openeffects.Daemon` on the **session bus**. Object path: `/org/openeffects/Daemon`.

### 11.1 `org.openeffects.Daemon1` (lifecycle)

| Member          | Type             | Description                                                   |
| --------------- | ---------------- | ------------------------------------------------------------- |
| `Start()`       | method           | Start pipeline. No-op if running.                             |
| `Stop()`        | method           | Stop pipeline, release camera.                                |
| `Status`        | property `s`     | `running` / `idle` / `error` / `stopped`                      |
| `Capabilities`  | property `a{sv}` | Detected tier, active EP, PipeWire version, virtual cam path. |
| `StatusChanged` | signal           | Emitted on state transitions.                                 |

### 11.2 `org.openeffects.Effects1`

| Member                                | Type             | Description                                                                                     |
| ------------------------------------- | ---------------- | ----------------------------------------------------------------------------------------------- |
| `ListEffects()`                       | method → `as`    | Return effect IDs (`center_stage`, `portrait_blur`, `bg_replace`, `studio_light`, `reactions`). |
| `SetEnabled(id: s, on: b)`            | method           | Toggle effect on/off.                                                                           |
| `SetParam(id: s, key: s, value: v)`   | method           | Set a parameter. E.g. `blur_strength` = `u32(75)`, `background` = `s("/path/to/img.jpg")`.      |
| `GetParams(id: s)`                    | method → `a{sv}` | Current params for an effect.                                                                   |
| `GetAllState()`                       | method → `a{sv}` | All effects + params in one call (used by the GUI on init).                                     |
| `EffectChanged(id: s, params: a{sv})` | signal           | Emitted on any change; all clients stay in sync.                                                |

### 11.3 `org.openeffects.Devices1`

| Member                | Type              | Description                                             |
| --------------------- | ----------------- | ------------------------------------------------------- |
| `ListCameras()`       | method → `aa{sv}` | Available physical cameras with name, path, resolution. |
| `SelectCamera(id: s)` | method            | Switch input device.                                    |
| `VirtualCameraInfo`   | property `a{sv}`  | Path or PipeWire node name of virtual camera.           |
| `ActiveCamera`        | property `s`      | Currently selected camera ID.                           |

**Default camera selection heuristic:** On startup the daemon selects a default camera automatically without prompting:

1. Prefer cameras whose PipeWire `node.description` or `node.name` contains any of: `front`, `integrated`, `built-in`, `camera` (case-insensitive). The longest matching name wins (longer names are more descriptive and more likely to be a real camera rather than a capture card or virtual device).
2. Among equally-ranked candidates, prefer the lowest `/dev/video*` device number.
3. If no heuristic match is found, fall back to the first camera in the enumeration order.

The selected default is stored in state and surfaced in the GUI's Camera page as `Camera: <name>`. Users can override at any time via `openeffectsctl camera select "<name>"` or the GUI's Camera page; the override is persisted.

All three binaries share generated D-Bus proxy types from a single `interface.xml`, ensuring the API stays in sync.

---

## 12. Performance budgets

End-to-end = capture timestamp → publish timestamp.

| Resolution   | FPS | Total budget | Capture | ML inference | Compositing | Publish |
| ------------ | --- | ------------ | ------- | ------------ | ----------- | ------- |
| 1080p60 (T1) | 60  | 16.6 ms      | 2 ms    | 6 ms         | 5 ms        | 3 ms    |
| 1080p30 (T2) | 30  | 33.3 ms      | 3 ms    | 14 ms        | 10 ms       | 6 ms    |
| 720p30 (T3)  | 30  | 33.3 ms      | 3 ms    | 18 ms        | 8 ms        | 4 ms    |
| 720p30 (T4)  | 30  | 33.3 ms      | 3 ms    | 0 ms         | 25 ms       | 5 ms    |

Memory targets (RSS, daemon process):

- Idle (pipeline paused, models unloaded after 5 min): < 80 MB
- T1 active (bundled models): < 500 MB
- T1 active (opt-in RVM loaded): < 800 MB
- T4 active: < 150 MB
- VRAM (T1, bundled models): < 350 MB

Daemon CPU usage when pipeline is paused: < 1% of one core.

---

## 13. Security, privacy, and sandboxing

- Camera access mediated by `xdg-desktop-portal-camera` under Flatpak; daemon never opens raw `/dev/video*` in sandboxed builds.
- D-Bus service on **session bus only** — no system bus surface.
- No telemetry, no network calls (model downloads are explicit user action; SHA256-verified).
- Background images stored under `~/.local/share/openeffects/` (user-readable only).
- Reactions is **off by default** — hand tracking only runs when explicitly enabled.
- Opt-in model downloads: the daemon fetches from a manifest-declared URL; the URI is user-visible before download; SHA256 checked post-download before the model is used.

---

## 14. UI surfaces

### 14.1 `openeffects` GUI (primary surface)

The `openeffects` GUI is implemented using:

- GTK4 (`gtk4-rs`)
- libadwaita (`adw` crate)
- zbus D-Bus proxy bindings (see §6.2)
- `gst-plugin-gtk4`'s `gtk4paintablesink` for preview rendering

Launched on demand from the GNOME app grid/Activities (`.desktop` entry) or directly; connects to `openeffectsd` over D-Bus on launch and reflects current state via `GetAllState()` (§6.2).

Design goals:

- Native GNOME (libadwaita) look and feel
- Functional on KDE and other GTK4-capable desktops via Adwaita styling
- Low-latency preview rendering
- Responsive controls
- Consistent Wayland behavior

Main window layout:

- `AdwNavigationSplitView`: sidebar navigation + content pages
- Live preview pane on the Camera page

Pages:

- Effects
- Camera
- Backgrounds
- Model Library
- About

**Effects page** (the primary day-to-day surface — every effect's enable switch and fast parameters live here):

```
┌─ openeffects ──────────────────────────────────┐
│ ≡  Effects   Camera   Backgrounds   ⋯           │
├──────────────────────────────────────────────────┤
│ Center Stage                              ⏻ on  │
│    Framing: Subtle ▾    Mode: Single face ▾     │
│ Portrait Blur                             ⏻ on  │
│    Strength  ──────●──── 75%                    │
│ Background Replace                        ⏻ off │
│    [thumbnails: None · Blur · Office · …  Browse…]│
│ Studio Light                              ⏻ off │
│    Intensity ──●─────── 30%                     │
│ Reactions                                 ⏻ on  │
├──────────────────────────────────────────────────┤
│ Camera: Logitech C920          Running on: CUDA │
└──────────────────────────────────────────────────┘
```

**Camera portal permission:** `openeffects` requests access to the virtual camera's PipeWire node via `xdg-desktop-portal-camera` at startup, for its live preview pane on the Camera page. The user grants or denies once via the standard portal dialog; the decision is persisted by the portal.

The live preview pane consumes the virtual camera node at reduced resolution (240p default) to minimize additional GPU load, rendered via `gst-plugin-gtk4`'s `gtk4paintablesink` into a `gtk::Picture`.

Effect changes take effect within one frame. State is persisted to `~/.config/openeffects/state.toml` on every change.

### 14.2 CLI (openeffectsctl)

The CLI remains fully Rust-based and communicates exclusively through D-Bus.

Primary use cases:

- WM keybind integration
- scripting
- automation
- status monitoring

Examples:
```
openeffectsctl status
openeffectsctl status --json                     # structured output for scripting
openeffectsctl enable portrait_blur
openeffectsctl disable reactions
openeffectsctl toggle center_stage

openeffectsctl set portrait_blur.strength 75
openeffectsctl set bg_replace.background ~/bg.jpg
openeffectsctl set center_stage.zoom tight

openeffectsctl camera list
openeffectsctl camera select "Logitech C920"

openeffectsctl watch                             # tail D-Bus EffectChanged signals
```

Waybar module example (in `~/.config/waybar/config`):

```json
"custom/openeffects": {
    "exec": "openeffectsctl status --short",
    "interval": 5,
    "on-click": "openeffectsctl toggle portrait_blur"
}
```

### 14.3 DE integration points

| DE              | Integration in v1.0                                                                                                    |
| --------------- | ---------------------------------------------------------------------------------------------------------------------- |
| GNOME           | Native target. `openeffects` launches from Activities/app grid; daemon autostarts via systemd regardless of GUI state. |
| KDE Plasma      | `openeffects` runs via GTK4/Adwaita styling (Breeze icon theme honored); fully functional, non-native look.            |
| Hyprland / Sway | No DE integration needed; GUI launches as a normal Wayland toplevel; CLI for keybinds.                                 |
| XFCE            | GTK4/libadwaita runs natively (GTK-based DE).                                                                          |

GTK4 provides consistent Wayland-native behavior across all supported environments.
---

## 15. Distribution and packaging

| Target            | Format                                 | Notes                                                                                                           |
| ----------------- | -------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Flatpak (primary) | `.flatpak` on Flathub                  | Portal camera; bundles all bundled models. v4l2loopback unavailable inside sandbox; PipeWire virtual node only. |
| Fedora / RHEL     | `.rpm` (COPR → Fedora repo)            | Recommends `v4l2loopback-dkms` and `pipewire ≥ 1.0`.                                                            |
| Ubuntu / Debian   | `.deb` (PPA)                           | Same.                                                                                                           |
| Arch              | AUR (`openeffects`, `openeffects-git`) | Expected for the primary power-user persona.                                                                    |
| Generic           | Tarball + `meson install`              | For other distros.                                                                                              |

Opt-in models ship in a separate `openeffects-models-extra` package (RPM/DEB) so model updates don't require pulling a new daemon binary.

---

## 16. Phased implementation plan

### Phase 0 — Foundations (weeks 1–2)

**Scope:**
- Cargo workspace: crates `daemon`, `cli`, `gui`, `shared`.
- CI: GitHub Actions; Fedora + Ubuntu + Arch containers; `cargo test`, `cargo clippy`, formatting.
- D-Bus interface XML committed; `zbus` proxy codegen in build for `daemon`, `cli`, and `gui`.
- Empty daemon: registers `org.openeffects.Daemon` on session bus, responds to `Status()`.
- State persistence: read/write `~/.config/openeffects/state.toml`.
- Single Rust/cargo workspace build — no separate Qt/CMake build step.

**Exit criteria:** `openeffectsctl status` returns `stopped` against a manually launched daemon.

---

### Phase 1 — Camera plumbing MVP (weeks 3–6)

**Scope:**
- GStreamer pass-through pipeline: `pipewiresrc → identity → pipewiresink` (virtual node).
- GPU effects scaffolding (currently identity).
- Basic non-ML effects: brightness/contrast, manual rectangular crop.
- **GUI MVP is the core deliverable of this phase.** A GTK4/libadwaita `AdwApplicationWindow` listing the five effects with `AdwSwitchRow` toggles, tested and working on **Arch Linux + GNOME**. Toggling a row round-trips `SetEnabled` → `EffectChanged`.
- Systemd user service unit for the daemon (`Type=dbus` autostart).
- CLI: `enable`, `disable`, `set`, `status`.
- Daemon auto-pause when no virtual camera consumer for > 30 s.

**Exit criteria:**
- Build on Arch Linux with `cargo build --release` and `meson install`.
- Virtual camera visible in `wpctl status` (PipeWire) and/or `v4l2-ctl --list-devices` (loopback).
- `openeffects` GUI opens, lists the five effects, and toggling a switch enables/disables the effect with live feedback (round-trip `SetEnabled`/`EffectChanged`).
- Open virtual camera in Chrome `getUserMedia` demo, Firefox, OBS, Zoom — frames appear.
- Adjust brightness live via CLI.
- Daemon restarts gracefully via systemd.

**Testing:**
- Unit: config (de)serialization, D-Bus method dispatch with mocked pipeline.
- Pipeline: `videotestsrc → identity → fakesink`; byte-identical frames.
- System: virtual camera enumerated and readable in Chrome, Firefox, OBS, Zoom, `cheese` on Arch + GNOME.
- System: **`openeffects` GUI opens on Arch + GNOME, the effect list renders, and switches respond to clicks with `SetEnabled`/`EffectChanged` round-trip**.
- Fallback: disable PipeWire virtual node support via env var; confirm v4l2loopback path activates.
- Performance: pass-through latency < 20 ms; idle CPU < 1%.

---

### Phase 2 — Segmentation effects (weeks 7–12)

**Scope:**
- `InferenceEngine` with EP probing (initially: CUDA + CPU; ROCm + OpenVINO as detected).
- Model manifest parser + SHA256 verification + registry.
- Selfie Segmentation model: FP32, FP16, INT8 variants.
- Portrait blur effect: mask → gaussian blur composite shader.
- Background replacement effect: mask → composite over user image or solid color.
- Automatic tier detection (T1–T4).
- Auto-degradation watchdog.
- GUI: Effects page gains blur-strength slider and background picker.
- GUI: Effects page with sliders; About page with EP/tier readout.
- Model Library page with bundled models listed (no download UI yet).

**Exit criteria:** On NVIDIA laptop: portrait blur at 1080p30 stable within frame budget on CUDA EP. On no-GPU laptop: INT8 CPU path at 720p30. Killing CUDA mid-session cleanly falls back to CPU within 2 s with no crash or pipeline restart visible to consumer.

**Testing:**
- Unit: EP probe handles missing `libcuda.so` (via `LD_LIBRARY_PATH` isolation), `librocm_smi.so`; manifest parser rejects invalid TOML and SHA256 mismatch.
- Component: `pick_ep()` decision matrix — all combinations of (available EPs × model supported EPs).
- Component: simulate inference failure → verify fallback to next EP within 1 s.
- Quality: segmentation mask IoU > 0.92 on 200-image portrait dataset across varied skin tones, lighting, backgrounds.
- Performance: inference P95 measured per EP; asserted against frame budget in §12.
- Failure injection: kill GPU process mid-run → CPU fallback activates, consumer app sees no drop.
- E2E: portrait blur visible in Zoom video call (validate with screen recording).

---

### Phase 3 — Tracking and Studio Light (weeks 13–16)

**Scope:**
- YuNet face detection integrated through `InferenceEngine`.
- Tracking module: EMA smoother (default); Kalman filter (T1).
- Center Stage effect: smoothed crop+zoom shader; aspect ratio preservation.
- Studio Light: face mesh landmarks → region tone mapping on T1/T2; global tone curve on T3/T4.
- ROCm and OpenVINO EPs validated and added to CI matrix.
- GUI: Effects page gains Center Stage zoom control and Studio Light toggle.

**Exit criteria:** Center Stage tracks a deliberate subject move within 400 ms on T2 hardware; jitter < 1.5 px stddev for a stationary subject. Enabled simultaneously with portrait blur within frame budget.

**Testing:**
- Unit: smoother convergence on synthetic bbox trajectories.
- Quality: jitter test (static subject → bbox stddev < 1.5 px); responsiveness test (step input → 80% convergence within 12 frames at 30 fps).
- Edge cases: no face → graceful no-op; 4 faces → primary-face follow; face fully exits frame → zoom resets smoothly.
- Integration: Center Stage + Portrait Blur simultaneously; frame budget respected on T2.
- EP: ROCm path validated on AMD RX 6600; OpenVINO path validated on Intel UHD 770.

---

### Phase 4 — Reactions, opt-in models, full GUI (weeks 17–22)

**Scope:**
- MediaPipe Hands model integrated; gesture classifier MLP head.
- Overlay/sprite compositor system.
- Opt-in model download: GUI "Model Library" download button, SHA256 verification, progress feedback.
- Reaction toggle (GUI Effects page); debounce logic.
- GUI: Camera page with camera selection; About page shows "Running on" status line; full effect set on Effects page.
- GUI: full preferences app including Background library manager, Model Library with download.
- `.desktop` launcher entry and icon for `openeffects`.

**Exit criteria:** End-to-end: open `openeffects`, enable reactions, trigger thumbs-up gesture → burst overlay appears in Zoom video call. Download RVM from Model Library → portrait blur quality visibly improves.

**Testing:**
- Unit: gesture debounce — same gesture within 3 s does not retrigger.
- Quality: gesture classifier accuracy ≥ 95% on held-out test set; false positive rate < 1 per 10 min on "negative" footage (normal webcam, no intended gestures).
- Integration: model download aborted halfway → partial file cleaned up; no broken model state.
- Integration: SHA256 mismatch post-download → model rejected, error surfaced in GUI.
- GUI: switching camera mid-session via the Camera page completes within 2 s with no consumer app visible glitch.
- GUI: window open < 500 ms; preview pane shows processed frames within 1 s.

---

### Phase 5 — Distribution, polish, beta (weeks 23–28)

**Scope:**
- Flatpak manifest + Flathub submission.
- COPR, PPA, AUR packaging.
- Compatibility test matrix execution (§17.3).
- Performance regression suite in CI.
- Documentation site: user guide + developer reference + D-Bus API reference.
- Localization scaffolding (gettext).
- Public beta release.

**Exit criteria:** Install via Flatpak on a clean Fedora 42 and Ubuntu 24.04 VM; no manual steps; all bundled-model effects work; opt-in model download completes; GUI opens. All cells marked P in §17.3 pass.

---

## 17. Testing strategy

### 17.1 Test taxonomy

| Level             | Scope                                                                              | Tooling                                                |
| ----------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------ |
| **Unit**          | Pure functions: EP selection, smoother, manifest parser, config                    | `cargo test`                                           |
| **Component**     | Subsystem with mocked dependencies (e.g., `InferenceEngine` with fake ORT session) | `cargo test` with feature flags                        |
| **Pipeline**      | Full GStreamer pipeline with `videotestsrc` / `fakesink`                           | Custom Rust harness                                    |
| **Integration**   | Daemon + D-Bus + CLI together                                                      | Python pytest + `dbus-next`                            |
| **System / E2E**  | Real camera → consumer app                                                         | Manual checklist + scripted via `selenium` for browser |
| **Performance**   | Frame time, memory, latency under load                                             | `criterion` + custom frame-time harness                |
| **Quality**       | Mask IoU, tracking jitter, gesture accuracy                                        | Golden datasets                                        |
| **Compatibility** | Distro × DE × GPU                                                                  | VM / container farm                                    |

CI runs unit + component + pipeline + integration on every PR. Performance suite runs nightly. System tests run per phase milestone.

### 17.2 Performance regression gates

CI fails on any of the following regressions vs the previous release tag (10% threshold):

- Per-EP inference P95 per model.
- Total frame time P95 at each resolution target.
- End-to-end latency.
- RSS memory (idle + active states).
- VRAM usage.
- Daemon startup time.
- Time-to-first-frame from `Start()` D-Bus call.

### 17.3 Hardware × distro compatibility matrix

**P = primary** (tested in CI nightly), **S = secondary** (weekly), **T = tertiary** (release milestone only, must not crash).

|                         | Fedora 42+ | Ubuntu 24.04+ | Arch (rolling) | Ubuntu 22.04 |
| ----------------------- | ---------- | ------------- | -------------- | ------------ |
| GNOME + NVIDIA discrete | P          | P             | P              | S            |
| GNOME + Intel iGPU      | P          | P             | P              | S            |
| KDE 6 + AMD discrete    | S          | S             | S              | T            |
| Hyprland + NVIDIA       | S          | S             | P              | T            |
| Sway + Intel iGPU       | S          | P             | P              | T            |
| XFCE + no GPU           | T          | T             | T              | T            |

### 17.4 Third-party app verification checklist

Run at the end of each phase that changes the virtual camera or pipeline:

| App                            | Test                                                        |
| ------------------------------ | ----------------------------------------------------------- |
| Firefox                        | `getUserMedia` test; virtual cam listed and delivers frames |
| Chromium                       | `chrome://settings/content/camera`; WebRTC test             |
| Zoom (Linux client)            | Settings → Video; virtual cam selectable; frames visible    |
| Slack                          | Huddle video                                                |
| Discord                        | Voice video                                                 |
| OBS                            | Add Video Capture Device → frames                           |
| `gst-launch-1.0 pipewiresrc …` | Direct PipeWire pipeline verification                       |
| `cheese`                       | Sanity check                                                |

---

## 18. Integration plan

### 18.1 PipeWire integration

- Build against `libpipewire-0.3 ≥ 0.3.65`. Test against PipeWire 1.0, 1.2, 1.4.
- Virtual camera via `pw_filter` API with properties `node.name=openeffects`, `media.class=Video/Source`, `node.description=OpenEffects Virtual Camera`.
- Camera enumeration via `pw_registry` watching for `media.class=Video/Source/Camera` objects.
- Consumer detection: watch for `pw_link` objects connecting to the virtual node; daemon idles when no links exist for > 30 s.

### 18.2 v4l2loopback integration

- Detection: `modinfo v4l2loopback` + check `/sys/module/v4l2loopback`.
- Device creation: prefer pre-created via udev rule shipped with the package. On-demand fallback: run `modprobe v4l2loopback exclusive_caps=1 card_label="OpenEffects"` via polkit prompt.
- Package ships a udev rule granting the `video` group access; post-install script adds user to the group.
- Device removed on daemon shutdown if created dynamically.

### 18.3 DE integration points

| DE              | Integration in v1.0                                                                                                    |
| --------------- | ---------------------------------------------------------------------------------------------------------------------- |
| GNOME           | Native target. `openeffects` launches from Activities/app grid; daemon autostarts via systemd regardless of GUI state. |
| KDE Plasma      | `openeffects` runs via GTK4/Adwaita styling (Breeze icon theme honored); fully functional, non-native look.            |
| Hyprland / Sway | No DE integration needed; GUI launches as a normal Wayland toplevel; CLI for keybinds.                                 |
| XFCE            | GTK4/libadwaita runs natively (GTK-based DE).                                                                          |

Post-v1 target: GNOME Shell Quick Settings extension for live effect toggles (complements the GTK4 app).

## 18.4 Wayland considerations

- GTK4 renders natively on Wayland (default GDK Wayland backend).
- No XWayland dependency.
- Vulkan/OpenGL rendering paths operate directly through Wayland-native surfaces.
- `gst-plugin-gtk4`'s `gtk4paintablesink` renders the virtual camera feed into a `gtk::Picture`/`gtk::Paintable`.
- No `DISPLAY` environment variable assumptions anywhere in the stack.
- PipeWire integration is fully Wayland-native.
---

## 19. Risks and mitigations

| Risk                                                                        | Likelihood | Impact | Mitigation                                                                  |
| --------------------------------------------------------------------------- | ---------- | ------ | --------------------------------------------------------------------------- |
| PipeWire virtual node API instability across distro versions                | Medium     | High   | v4l2loopback fallback; pin to feature-tested PW versions.                   |
| ORT Vulkan EP immature → gaps in vendor-neutral GPU coverage                | High       | Medium | Vulkan EP marked experimental; CPU+XNNPACK is the universal floor.          |
| Apps (especially Electron-based) cache device list and miss the virtual cam | Medium     | Medium | Recommend daemon auto-start at session login; documented workaround.        |
| Flatpak sandbox prevents v4l2loopback                                       | Confirmed  | Medium | Flatpak variant is PipeWire-only; documented clearly.                       |
| NVIDIA driver/CUDA version mismatch causes silent inference error           | Medium     | High   | Probe with test model at startup; expose EP status in the GUI's About page. |
| GTK4/libadwaita looks non-native on KDE/other DEs                           | Low        | Low    | Document as known limitation.                                               |
| Opt-in model download from CDN blocked (corporate firewalls)                | Low        | Low    | Document manual install path; models are plain ONNX files.                  |
| Battery drain from always-on daemon                                         | Medium     | Medium | Auto-pause pipeline when no consumer; unload models after 5 min idle.       |

---

## 20. Appendix — glossary

| Term                | Definition                                                                           |
| ------------------- | ------------------------------------------------------------------------------------ |
| **EP**              | Execution Provider — ONNX Runtime's backend for a specific hardware target.          |
| **ORT**             | ONNX Runtime.                                                                        |
| **RVM**             | Robust Video Matting — high-quality temporally stable segmentation model.            |
| **YuNet**           | Small, fast face detection model from OpenCV's model zoo.                            |
| **TensorRT**        | NVIDIA's optimizing inference runtime; typically 2–4× faster than CUDA EP.           |
| **DMA-BUF**         | Linux kernel mechanism for sharing GPU buffers between processes without CPU copy.   |
| **PipeWire portal** | The `xdg-desktop-portal` Camera interface; used by sandboxed apps for camera access. |
| **v4l2loopback**    | Kernel module creating virtual `/dev/video*` devices fed from userspace.             |
| **Tier 1–4**        | Hardware capability classes defined in §10.1.                                        |

---

*End of document — OpenEffects PRD v0.5*
