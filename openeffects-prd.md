# OpenEffects — Product Requirements Document

> Components: **`openeffectsd`** (daemon) · **`openeffects`** (GUI) · **`openeffectsctl`** (CLI) · **`openeffects-tray`** (tray applet)

---

## 1. Document control

| Field        | Value                                                              |
| ------------ | ------------------------------------------------------------------ |
| Version      | 0.4 (implementation-ready)                                         |
| Status       | Ready for development                                              |
| Owner        | Aryan                                                              |
| Last updated | 2026-05-22                                                         |
| Prev version | 0.3 — design decisions inlined into relevant sections; §20 removed |

---

## 2. Executive summary

OpenEffects is a Linux-native webcam effects engine that brings macOS-class features — Center Stage, Portrait mode, Studio Light, Background Replacement, and Reactions — to any Linux desktop, transparently, for any consumer of a PipeWire camera node or `/dev/video*` device (Zoom, Chrome, Firefox, Slack, OBS, etc.).

The system is architected as a **headless GPU-accelerated daemon** (`openeffectsd`) that owns the capture → process → publish pipeline, plus three client surfaces: a **tray applet** (`openeffects-tray`) as the primary day-to-day control surface, a **preferences GUI** (`openeffects`) for advanced configuration, and a **CLI** (`openeffectsctl`) for scripting and tiling-WM keybinds.

ML inference is unified behind a single **ONNX Runtime** abstraction with an Execution Provider (EP) priority chain that degrades from vendor-specific accelerators (TensorRT, CUDA, ROCm, OpenVINO) to vendor-neutral GPU (Vulkan) to CPU to heuristic fallbacks, so the app is useful across hardware from an 11th-gen Intel laptop to an RTX 4090. Heavier/higher-quality models are opt-in downloads; a functional base set is bundled.

Wayland is the only supported display protocol. X11 sessions are not a target.

**Development platform:** Primary development is on **Arch Linux** with **KDE Plasma 6**. The project is designed to be straightforward to build and deploy on Arch; it uses standard Arch packages and follows Linux best practices.

---

## 3. Goals and non-goals

### 3.1 Goals

- **G1** — Provide a virtual webcam that any Linux app can consume, with real-time effects applied.
- **G2** — Integrate naturally on GNOME, KDE, and tiling compositors (Hyprland, Sway, river) without a mandatory GUI window.
- **G3** — Use the GPU wherever available, via a unified ONNX Runtime inference path that works across NVIDIA, AMD, and Intel.
- **G4** — Degrade gracefully on systems without GPU acceleration, kernel-modular virtual camera, or recent PipeWire.
- **G5** — Tray applet is the primary control surface; GUI is advanced configuration only. CLI is first-class for power users.
- **G6** — Sub-50 ms added end-to-end latency on Tier 1 hardware; sub-100 ms on Tier 2.

### 3.2 Non-goals (v1.0)

- No profiles or saved presets (current effect state is live; camera on → tray available → adjust inline).
- No per-app auto-switching of effect configurations.
- No virtual studio / scene compositing (that's OBS's job).
- No remote or cloud inference.
- No X11 session support.
- No neural face relighting (planned post-v1).
- No Windows or macOS support.

---

## 4. Target users and personas

| Persona                    | DE / setup                  | Primary surface                                              | Notes                                                           |
| -------------------------- | --------------------------- | ------------------------------------------------------------ | --------------------------------------------------------------- |
| **Mira** — design lead     | Fedora + GNOME, Intel iGPU  | Tray quick toggles; `openeffects` GUI for advanced settings  | Wants toggle-on-join; fine-tunes blur strength once, leaves it. |
| **Rohit** — KDE user       | KDE 6 on AMD discrete GPU   | Tray applet in system tray, KWin integration                 | Expects quick toggles without opening a window.                 |
| **Aryan** — tiling WM user | Arch + Hyprland, NVIDIA     | `openeffectsctl` bound to keybinds; waybar module for status | Never opens a GUI window; drives everything from keybinds.      |
| **Sam** — older laptop     | Ubuntu LTS, no discrete GPU | Tray toggle                                                  | Cares that *something* works; happy with CPU-path blur.         |

---

## 5. Functional requirements

### 5.1 Center Stage (P0)

Frames a person centered in the output by detecting their face/body bounding box and applying a smoothed crop+zoom over time.

- **5.1.1** Track up to N=4 faces; user-selectable "primary face follow" vs "group framing" (quick toggle in tray submenu).
- **5.1.2** Smoothing avoids visible jitter on micro-movements while reacting within ~400 ms to deliberate motion.
- **5.1.3** Zoom level user-configurable: `off`, `subtle`, `normal`, `tight` — exposed as tray submenu and GUI slider.
- **5.1.4** Must preserve the aspect ratio of the consumer's requested format.

### 5.2 Portrait mode (P0)

Blurs the background while keeping the subject crisp.

- **5.2.1** Segmentation mask refreshed every frame; feathered edges; temporally stable across frames.
- **5.2.2** Blur strength exposed as a tray submenu shortcut (`light`, `medium`, `heavy`) and a continuous slider in GUI.
- **5.2.3** v1.0 ships Gaussian blur; disc/bokeh kernel is a stretch goal.

### 5.3 Background replacement (P0)

Replaces background with a user asset or solid color.

- **5.3.1** User assets in `~/.local/share/openeffects/backgrounds/`. Ships with 6 built-in defaults (gradients, abstract, neutral).
- **5.3.2** Background selection exposed in tray submenu (thumbnail grid, max 8 shown).
- **5.3.3** Edge refinement (guided filter) on Tier 1/2 hardware.

### 5.4 Studio Light (P1)

Subtly brightens and separates the subject.

- **5.4.1** Face-region-aware brightness/contrast lift on T1/T2; global tone curve fallback otherwise.
- **5.4.2** Intensity slider in GUI; tray quick toggle on/off only.

### 5.5 Reactions (P1)

Hand-gesture-triggered animated overlays.

- **5.5.1** Built-in gestures: thumbs-up → 👍 burst, peace sign → confetti, heart (two-hand) → hearts, open palm → wave, fist → fireworks.
- **5.5.2** Debounce: same gesture cannot retrigger within 3 s.
- **5.5.3** **Off by default.** Explicitly enable via tray toggle or GUI.

### 5.6 Quick controls (tray-defined, not a separate feature)

The tray applet is the control surface for all real-time adjustments. No profile concept exists — state is live and immediate.

- **5.6.1** Tray appears as soon as `openeffectsd` is running; does not require a consumer to be active.
- **5.6.2** Each effect has a top-level toggle (checkbox) and a submenu for its fast parameters.
- **5.6.3** "Open OpenEffects…" opens the GUI for advanced settings (model selection, background library, calibration).
- **5.6.4** Indicator icon reflects daemon state: running+active (colored), running+idle (dim), error (warning symbol).

---

## 6. System architecture

### 6.1 Process model

```
  openeffectsd  (systemd --user service)
  ┌──────────────────────────────────────────────────────────┐
  │  GStreamer pipeline                                      │
  │  pipewiresrc → glupload → effects-bin → pipewiresink    │
  │  ↳ fallback: pipewiresrc → v4l2sink (/dev/videoN)       │
  │                                                          │
  │  D-Bus service: org.openeffects.Daemon (session bus)     │
  └──────────────────────────────────────────────────────────┘
         ▲                  ▲                  ▲
         │                  │                  │
  openeffects-tray    openeffects (GUI)   openeffectsctl
  (systemd --user,    (on-demand,         (ad-hoc / keybinds)
   companion unit)    launched by tray
                      or directly)
```

- `openeffectsd` and `openeffects-tray` are both `--user` systemd units; they start together.
- `openeffects` (the GUI) is launched on demand from the tray menu or directly. Closing the window does not affect the daemon or tray.
- All surfaces are stateless D-Bus clients. Killing any one of them does not affect the pipeline.
- `openeffects-tray` is a **separate binary** from `openeffects` (see §6.2 for rationale).

## 6.2 Tray applet: separate process rationale

The tray applet is implemented as a lightweight Qt 6 process (`openeffects-tray`) separate from the main preferences GUI.

Qt's native StatusNotifierItem support integrates naturally with KDE Plasma and works reliably under Wayland compositors supporting SNI.

The tray applet remains intentionally separate from the main GUI process for robustness and low idle overhead.

| Concern                 | Separate process    | Tray inside GUI process                   |
| ----------------------- | ------------------- | ----------------------------------------- |
| Tray survives GUI close | Yes                 | Requires hidden-window behavior           |
| Tray survives GUI crash | Yes                 | No                                        |
| Memory footprint        | ~15–25 MB           | Higher due to full UI stack always loaded |
| Auto-start model        | Simple systemd unit | Requires GUI background startup           |
| Crash isolation         | Strong              | Weak                                      |
| Startup latency         | Minimal             | GUI initialization required               |

The tray process uses:
- Qt 6
- QSystemTrayIcon / StatusNotifierItem
- D-Bus IPC to `openeffectsd`
- QMenu-based dynamic menus

`openeffects-tray.service`:

```ini
[Unit]
Description=OpenEffects tray applet
PartOf=openeffectsd.service
After=openeffectsd.service

[Service]
ExecStart=%h/.local/bin/openeffects-tray
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

### 6.3 Pipeline data flow

The hot path is designed for **zero CPU copies** of full frames on Tier 1/2 hardware:

1. **Capture** — `pipewiresrc` delivers DMA-BUF-backed `GstBuffer`s.
2. **Upload to GL/Vulkan** — `glupload` wraps the DMA-BUF as a texture; no memcpy.
3. **Pre-process for inference** — shader downsamples + normalizes a copy to inference resolution (e.g., 256×256); small readback to ONNX input tensor.
4. **Inference** — ONNX session runs on selected EP; produces mask/landmarks.
5. **Upload result** — small output tensor → GPU texture.
6. **Composite** — single shader pass: segmentation mask + blur + background + face-region brightening + reaction sprites.
7. **Publish** — `pipewiresink` (virtual camera node) or `v4l2sink` (/dev/videoN) as fallback.

### 6.4 Threading model

- GStreamer pipeline runs on its own threadpool.
- D-Bus handler runs on a `zbus` async runtime.
- Dedicated **inference thread** with a single-slot ring buffer — if a new frame arrives while inference is still running on the previous one, the old request is dropped. Masks are reused for up to 2 frames (segmentation is temporally coherent; invisible at ≤2 frames).
- Reaction gesture detector runs at 1/3 the pipeline frame rate (every 3rd frame) to save budget.

---

| Layer                  | Choice                                            | Rationale                                                                                                                                                     |
| ---------------------- | ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Language (daemon, CLI) | **Rust**                                          | Memory safety in a long-running media daemon; strong ecosystem for PipeWire, GStreamer, Vulkan, and D-Bus.                                                    |
| Language (GUI + tray)  | **Qt 6 + QML (C++)**                              | Native KDE integration, mature Wayland support, high-performance GPU-accelerated UI, battle-tested multimedia tooling, excellent Linux desktop compatibility. |
| Media pipeline         | **GStreamer 1.24+** via `gstreamer-rs`            | Mature; first-class PipeWire, Vulkan, VAAPI, GL integration.                                                                                                  |
| Camera I/O (primary)   | **PipeWire** via `pipewiresrc` / virtual node API | Modern; portal-friendly; Wayland-native; no kernel module needed.                                                                                             |
| Camera I/O (fallback)  | **v4l2loopback** + `v4l2src`/`v4l2sink`           | For apps that bypass PipeWire or systems on older PipeWire versions.                                                                                          |
| GPU compute            | **Vulkan** compute shaders + GLSL via `gst-gl`    | Vendor-neutral; `gst-gl` is well-exercised; Vulkan compute for custom kernels.                                                                                |
| ML inference           | **ONNX Runtime** via `ort` crate                  | Single API; broadest EP coverage.                                                                                                                             |
| Tray                   | **Qt StatusNotifierItem / QSystemTrayIcon**       | Native KDE integration; SNI support across Plasma, GNOME (extension), XFCE, Hyprland waybar tray.                                                             |
| IPC                    | **D-Bus (session bus)** via `zbus`                | Universal Linux IPC; works under Flatpak.                                                                                                                     |
| Config                 | **TOML** via `serde`                              | Human-readable; power users can edit directly.                                                                                                                |
| Build                  | `cargo` + `cmake` + `meson` + `flatpak-builder`   | Standard Linux tooling.                                                                                                                                       |
| Display protocol       | **Wayland only**                                  | X11 is deprecated on all major DEs targeted; simplifies GPU context management.                                                                               |
---

## 7.1 Frontend architecture

The frontend stack is intentionally separated from the Rust media pipeline.

Architecture:

```
Rust daemon (openeffectsd)
        ▲
        │ D-Bus
        ▼
Qt 6 frontend processes
 ├── openeffects
 └── openeffects-tray
```

Rationale:

- GUI crashes cannot affect the media pipeline
- Qt UI iteration remains independent from the Rust backend
- Native Linux desktop behavior
- Reduced coupling between media/inference code and presentation layer

The GUI layer contains:

- QML UI
- View models
- D-Bus proxy bindings
- Preview rendering surfaces

The Rust backend remains responsible for:

- Media processing
- ONNX inference
- GStreamer pipelines
- Device management
- Configuration/state persistence
- Performance management

These changes align the PRD with:
- KDE-first Linux UX
- Qt-native Wayland integration
- lower frontend maintenance burden
- better tray reliability
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

    /// Exposed to tray/GUI for the "Running on: CUDA" status badge.
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

Every `infer()` call records wall time. A rolling P95 is maintained per (model, EP). If P95 exceeds the per-feature budget for **30 consecutive frames**, the engine emits a `BudgetExceeded` event; the `FeatureManager` downgrades one tier (§10) and persists the decision to config. The tray icon shows a brief "quality reduced" tooltip.

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
PipeWire ≥ 0.3.65 running?
├── Yes → Can we create a PipeWire virtual camera node?
│         ├── Yes → PRIMARY PATH. pipewiresrc → virtual node.
│         └── No  → try v4l2loopback ↓
└── No  → Is v4l2loopback available (module present + permissions)?
          ├── Yes → FALLBACK PATH. pipewiresrc → v4l2sink → /dev/videoN.
          └── No  → Virtual camera disabled. Surface actionable error in tray
                    icon tooltip and GUI: "Install v4l2loopback or upgrade
                    PipeWire to enable virtual camera."
                    openeffectsd stays running; effects can still be previewed
                    in the GUI's built-in preview pane.
```

### 10.4 Runtime auto-degradation

A `FeatureManager` watches a rolling 5-second frame-time window per feature. If P95 frame time exceeds budget for **30 consecutive frames**:

1. Downgrade feature one tier.
2. Persist the decision to `~/.cache/openeffects/tier-overrides.toml`.
3. Show tray tooltip: "Portrait blur quality reduced — GPU budget exceeded."

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
| `GetAllState()`                       | method → `a{sv}` | All effects + params in one call (used by tray on init).                                        |
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

The selected default is stored in state and surfaced in the tray as `Camera: <name>`. Users can override at any time via `openeffectsctl camera select "<name>"` or the GUI's Camera page; the override is persisted.

All four binaries share generated D-Bus proxy types from a single `interface.xml`, ensuring the API stays in sync.

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

### 14.1 Tray applet (`openeffects-tray`)

The tray applet is implemented in Qt 6 and serves as the primary real-time interaction surface.

Features:
- Native KDE Plasma integration
- Wayland-native rendering
- Dynamic menus via QMenu
- StatusNotifierItem support
- Zero dependency on GTK or libadwaita
- Fast startup and low idle memory usage

Supported environments:
- KDE Plasma (native)
- GNOME with AppIndicator extension
- Hyprland/Sway via waybar tray
- XFCE and other SNI-capable desktops

Tray behavior:
- Starts automatically with the daemon
- Survives GUI crashes/restarts
- Operates independently from the preferences window

**GNOME without AppIndicator extension:** At startup, the tray probes for an SNI host via D-Bus (`org.kde.StatusNotifierWatcher`). If no host is found, the tray process remains running but invisible. On the next launch of `openeffects` (the GUI), a dismissible banner is shown: *"Tray applet not available — install the AppIndicator extension for quick toggles."* with a direct link to `extensions.gnome.org`. Once the extension is installed, the tray becomes visible on the next login without any manual restart of the daemon.

**Camera portal permission:** `openeffects-tray` requests camera access via `xdg-desktop-portal-camera` at startup. The user grants or denies once via the standard portal dialog; the decision is persisted by the portal. This unifies camera permission under one access model across daemon and tray, and preps for a future live thumbnail in the tray icon without requiring a second permission prompt later.

**Tray menu structure:**

```
● OpenEffects  [icon: colored=active, dim=idle, ⚠=error]
───────────────────────────────────────
☑  Center Stage
   ↳ Framing: Subtle / Normal / Tight
   ↳ Mode: Single face / Group
☑  Portrait Blur
   ↳ Strength: Light / Medium / Heavy
□  Background Replace
   ↳ [thumbnail] None
   ↳ [thumbnail] Blur only
   ↳ [thumbnail] Office.jpg
   ↳ [thumbnail] Abstract Blue
   ↳ [thumbnail] Gradient Warm
   ↳ Browse…
□  Studio Light
☑  Reactions
───────────────────────────────────────
   Camera: Logitech C920 ▸
   Running on: CUDA (T1 · 1080p60)
───────────────────────────────────────
   Open OpenEffects…
   Quit
```

Parameters changed via the tray take effect within one frame. State is persisted to `~/.config/openeffects/state.toml` on every change.

## 14.2 Preferences GUI (openeffects)

The preferences GUI is implemented using:

- Qt 6
- Qt Quick (QML)
- Qt Multimedia
- Qt Wayland
- GPU-accelerated scene rendering

The GUI is launched on-demand from the tray or directly from the application launcher.

Design goals:

- Native KDE feel
- Acceptable GNOME appearance
-  Smooth GPU-accelerated animations
-  Low latency preview rendering
-  Responsive controls
-  Consistent Wayland behavior

Main window layout:

- Sidebar navigation
- Central configuration panel
- Live preview pane

Pages:

- Effects
- Model Library
- Camera
- Backgrounds
- System
- About

The live preview pane consumes the virtual camera node at reduced resolution (240p default) to minimize additional GPU load.

Qt Multimedia + GStreamer integration is used for preview rendering.

## 14.3 CLI (openeffectsctl)

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

## 14.4 DE integration points

| DE              | Integration in v1.0                                                                 |
| --------------- | ----------------------------------------------------------------------------------- |
| KDE Plasma      | Native Qt integration; StatusNotifierItem works out of the box.                     |
| GNOME           | Tray support via AppIndicator extension; GUI remains fully functional without tray. |
| Hyprland / Sway | Tray supported through waybar tray module; CLI-first workflows fully supported.     |
| XFCE            | Standard SNI/AppIndicator integration.                                              |

Qt 6 provides consistent Wayland-native behavior across all supported environments.
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
- Cargo workspace: crates `daemon`, `cli`, `tray`, `gui`, `shared`.
- CI: GitHub Actions; Fedora + Ubuntu + Arch containers; `cargo test`, `cargo clippy`, formatting.
- D-Bus interface XML committed; `zbus` proxy codegen in build.
- Empty daemon: registers `org.openeffects.Daemon` on session bus, responds to `Status()`.
- State persistence: read/write `~/.config/openeffects/state.toml`.
- Workspace structure:
  - Rust crates: `daemon`, `cli`, `shared`
  - Qt frontend projects: `gui`, `tray`
- Qt 6 build integration via CMake
- D-Bus interface XML committed; proxy generation shared between Rust and Qt layers

**Exit criteria:** `openeffectsctl status` returns `stopped` against a manually launched daemon.

---

### Phase 1 — Camera plumbing MVP (weeks 3–6)

**Scope:**
- GStreamer pass-through pipeline: `pipewiresrc → identity → pipewiresink` (virtual node).
- Fallback: detect missing PipeWire virtual node; switch to `v4l2sink` via v4l2loopback.
- GPU effects scaffolding (currently identity).
- Basic non-ML effects: brightness/contrast, manual rectangular crop.
- **Tray applet is the core deliverable of this phase.** Simple icon + menu with toggles for the basic effects, tested and working on **Arch Linux + KDE Plasma 6**. Uses Qt 6 StatusNotifierItem / QSystemTrayIcon integration
- Systemd user service units for both daemon and tray.
- CLI: `enable`, `disable`, `set`, `status`.
- Daemon auto-pause when no virtual camera consumer for > 30 s.

**Exit criteria:**
- Build on Arch Linux with `cargo build --release` and `meson install`.
- Virtual camera visible in `wpctl status` (PipeWire) and/or `v4l2-ctl --list-devices` (loopback).
- Tray applet appears in KDE Plasma system tray; effects toggle on/off via menu with live feedback.
- Open virtual camera in Chrome `getUserMedia` demo, Firefox, OBS, Zoom — frames appear.
- Adjust brightness live via CLI.
- Daemon restarts gracefully via systemd.

**Testing:**
- Unit: config (de)serialization, D-Bus method dispatch with mocked pipeline.
- Pipeline: `videotestsrc → identity → fakesink`; byte-identical frames.
- System: virtual camera enumerated and readable in Chrome, Firefox, OBS, Zoom, `cheese` on Arch + KDE Plasma.
- System: **tray applet is visible in KDE Plasma system tray and clickable; menu responds to clicks**.
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
- Tray: blur toggle + strength submenu; background submenu + "Browse…".
- GUI: Effects page with sliders; System page with EP/tier readout.
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
- Tray: center stage toggle + zoom submenu; studio light toggle.

**Exit criteria:** Center Stage tracks a deliberate subject move within 400 ms on T2 hardware; jitter < 1.5 px stddev for a stationary subject. Enabled simultaneously with portrait blur within frame budget.

**Testing:**
- Unit: smoother convergence on synthetic bbox trajectories.
- Quality: jitter test (static subject → bbox stddev < 1.5 px); responsiveness test (step input → 80% convergence within 12 frames at 30 fps).
- Edge cases: no face → graceful no-op; 4 faces → primary-face follow; face fully exits frame → zoom resets smoothly.
- Integration: Center Stage + Portrait Blur simultaneously; frame budget respected on T2.
- EP: ROCm path validated on AMD RX 6600; OpenVINO path validated on Intel UHD 770.

---

### Phase 4 — Reactions, opt-in models, full tray (weeks 17–22)

**Scope:**
- MediaPipe Hands model integrated; gesture classifier MLP head.
- Overlay/sprite compositor system.
- Opt-in model download: GUI "Model Library" download button, SHA256 verification, progress feedback.
- Reaction tray toggle; debounce logic.
- Complete tray menu (all effects + camera selection + "Running on" status line).
- GUI: full preferences app including Background library manager, Model Library with download.
- `openeffects-tray` systemd service unit and companion configuration.

**Exit criteria:** End-to-end: open tray, enable reactions, trigger thumbs-up gesture → burst overlay appears in Zoom video call. Download RVM from Model Library → portrait blur quality visibly improves.

**Testing:**
- Unit: gesture debounce — same gesture within 3 s does not retrigger.
- Quality: gesture classifier accuracy ≥ 95% on held-out test set; false positive rate < 1 per 10 min on "negative" footage (normal webcam, no intended gestures).
- Integration: model download aborted halfway → partial file cleaned up; no broken model state.
- Integration: SHA256 mismatch post-download → model rejected, error surfaced in GUI.
- Tray: switching camera mid-session via tray menu completes within 2 s with no consumer app visible glitch.
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

**Exit criteria:** Install via Flatpak on a clean Fedora 42 and Ubuntu 24.04 VM; no manual steps; all bundled-model effects work; opt-in model download completes; tray appears; GUI opens. All cells marked P in §17.3 pass.

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
| GNOME + NVIDIA discrete | P          | P             | S              | S            |
| GNOME + Intel iGPU      | P          | P             | S              | S            |
| KDE 6 + AMD discrete    | P          | P             | S              | T            |
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

| DE              | Integration in v1.0                                                                                                                           |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| GNOME           | XDG autostart `.desktop` for tray; GNOME AppIndicator extension required for tray visibility (surfaced as an in-app tip if extension absent). |
| KDE Plasma      | SNI tray works natively; no additional integration needed.                                                                                    |
| Hyprland / Sway | No DE integration; tray via `waybar` `tray` module; CLI for keybinds.                                                                         |
| XFCE            | Generic AppIndicator tray.                                                                                                                    |

Post-v1 targets: GNOME Quick Settings tile; KDE Plasmoid.

## 18.4 Wayland considerations

- Qt 6 renders natively on Wayland.
- No XWayland dependency.
- Vulkan/OpenGL rendering paths operate directly through Wayland-native surfaces.
- GUI preview rendering uses Qt Multimedia + GStreamer integration.
- No `DISPLAY` environment variable assumptions anywhere in the stack.
- PipeWire integration is fully Wayland-native.
---

## 19. Risks and mitigations

| Risk                                                                        | Likelihood | Impact | Mitigation                                                                     |
| --------------------------------------------------------------------------- | ---------- | ------ | ------------------------------------------------------------------------------ |
| PipeWire virtual node API instability across distro versions                | Medium     | High   | v4l2loopback fallback; pin to feature-tested PW versions.                      |
| ORT Vulkan EP immature → gaps in vendor-neutral GPU coverage                | High       | Medium | Vulkan EP marked experimental; CPU+XNNPACK is the universal floor.             |
| Apps (especially Electron-based) cache device list and miss the virtual cam | Medium     | Medium | Recommend daemon auto-start at session login; documented workaround.           |
| Flatpak sandbox prevents v4l2loopback                                       | Confirmed  | Medium | Flatpak variant is PipeWire-only; documented clearly.                          |
| NVIDIA driver/CUDA version mismatch causes silent inference error           | Medium     | High   | Probe with test model at startup; expose EP status in tray tooltip.            |
| GNOME tray support requires AppIndicator extension                          | High       | Medium | Detect missing SNI host at startup; surface guidance in GUI and documentation. |
| Opt-in model download from CDN blocked (corporate firewalls)                | Low        | Low    | Document manual install path; models are plain ONNX files.                     |
| Battery drain from always-on daemon                                         | Medium     | Medium | Auto-pause pipeline when no consumer; unload models after 5 min idle.          |

---

## 20. Appendix — glossary

| Term                | Definition                                                                             |
| ------------------- | -------------------------------------------------------------------------------------- |
| **EP**              | Execution Provider — ONNX Runtime's backend for a specific hardware target.            |
| **ORT**             | ONNX Runtime.                                                                          |
| **RVM**             | Robust Video Matting — high-quality temporally stable segmentation model.              |
| **YuNet**           | Small, fast face detection model from OpenCV's model zoo.                              |
| **TensorRT**        | NVIDIA's optimizing inference runtime; typically 2–4× faster than CUDA EP.             |
| **DMA-BUF**         | Linux kernel mechanism for sharing GPU buffers between processes without CPU copy.     |
| **PipeWire portal** | The `xdg-desktop-portal` Camera interface; used by sandboxed apps for camera access.   |
| **v4l2loopback**    | Kernel module creating virtual `/dev/video*` devices fed from userspace.               |
| **SNI**             | StatusNotifierItem — the D-Bus protocol used by tray applets on modern Linux desktops. |
| **Tier 1–4**        | Hardware capability classes defined in §10.1.                                          |

---

*End of document — OpenEffects PRD v0.4*
