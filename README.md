# OpenEffects (WIP)

OpenEffects is a Linux-native, GPU-accelerated webcam effects engine. It brings advanced camera features like Center Stage, Portrait blur, Background Replacement, and gesture-triggered Reactions to any Wayland Linux desktop. It works transparently with any app that consumes a PipeWire camera node or `/dev/video*` device (Zoom, OBS, WebRTC, etc.).

## Features

- **Portrait Mode:** High-quality background blur with feathered edges and temporal stability.
- **Center Stage:** Intelligent face and body tracking that smoothly crops and zooms to keep you (or your group) centered.
- **Background Replacement:** Replace your messy room with a solid color or custom image.
- **Studio Light:** Subtly brightens and separates the subject from the background using face-region-aware tone mapping.
- **Reactions:** Hand-gesture-triggered animated overlays (e.g., thumbs-up for 👍 burst, peace sign for confetti). *Off by default.*

## Components

OpenEffects is designed to be lightweight and modular, avoiding the need for an always-open GUI window:

- **`openeffectsd` (Daemon):** Headless, GPU-accelerated GStreamer pipeline that handles capture, ML inference, compositing, and publishing via PipeWire.
- **`openeffects` (GUI):** A GTK4/libadwaita app and primary control surface — live effect toggles and adjustments, camera settings, opt-in model downloads, and background asset management.
- **`openeffectsctl` (CLI):** First-class command-line interface for styling WMs (Hyprland, Sway), scripting, and keybinds.

## Hardware & ML Support

Inference is powered by **ONNX Runtime**, unified behind a single execution priority chain to leverage whatever hardware you have:

1. NVIDIA: TensorRT / CUDA
2. AMD: ROCm / MIGraphX
3. Intel: OpenVINO
4. Universal GPU: Vulkan
5. CPU Fallback: XNNPACK

OpenEffects automatically detects your hardware capabilities (Tier 1 to Tier 4) and gracefully degrades effect quality, resolution, or frame rates to maintain a sub-50ms latency budget.

## Installation

### Flatpak (Recommended)
Available on Flathub (PipeWire virtual camera support only; `v4l2loopback` is unavailable inside the sandbox).

### Arch Linux
```bash
yay -S openeffects
```

### Fedora
```bash
dnf copr enable <user>/openeffects
dnf install openeffects
```

### Ubuntu / Debian
```bash
add-apt-repository ppa:<user>/openeffects
apt install openeffects
```

## Usage

Start the daemon (usually handled automatically by the systemd user service):
```bash
systemctl --user start openeffectsd
```

### CLI Examples

Toggle effects via `openeffectsctl`:
```bash
openeffectsctl check status
openeffectsctl enable portrait_blur
openeffectsctl set center_stage.zoom tight
openeffectsctl toggle reactions
```

Use with Waybar:
```json
"custom/openeffects": {
    "exec": "openeffectsctl status --short",
    "interval": 5,
    "on-click": "openeffectsctl toggle portrait_blur"
}
```

## Security & Privacy

- Cameras are accessed securely via the `xdg-desktop-portal-camera`.
- IPC is restricted exclusively to the D-Bus **session bus**.
- No telemetry, network calls, or cloud inference. All ML processing happens locally. 
- Heavier models can be downloaded via the GUI, verified by SHA256 hashes defined in local manifests.

## Development Platform

- **Language:** Rust (Daemon, CLI, GUI)
- **UI:** GTK4 + libadwaita-rs
- **Media Pipeline:** GStreamer 1.24+ + PipeWire 
- **Display Protocol:** Wayland only (X11 is not supported)