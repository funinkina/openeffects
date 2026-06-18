<img src="data/icons/in.co.funinkina.OpenEffects.svg" align="left" width="80" alt="OpenEffects icon">

# OpenEffects

[![Latest Release](https://img.shields.io/github/v/release/funinkina/openeffects?style=flat-square)](https://github.com/funinkina/openeffects/releases/latest)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange?style=flat-square)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Linux-important?style=flat-square)](https://www.linux.org/)
[![GitHub Stars](https://img.shields.io/github/stars/funinkina/openeffects?style=flat-square)](https://github.com/funinkina/openeffects/stargazers)
[![GitHub Issues](https://img.shields.io/github/issues/funinkina/openeffects?style=flat-square)](https://github.com/funinkina/openeffects/issues)

OpenEffects is a Linux-native webcam effects engine powered by ONNX Runtime. It brings advanced camera features like Center Stage, Portrait blur, Background Replacement, and gesture-triggered Reactions to any Wayland Linux desktop. It works transparently with any app that consumes a PipeWire camera node or `/dev/video*` device (Zoom, OBS, WebRTC, etc.).

## Features

- **Portrait Mode:** High-quality background blur with feathered edges and temporal stability.
- **Center Stage:** Intelligent face and body tracking that smoothly crops and zooms to keep you (or your group) centered.
- **Background Replacement:** Replace your messy room with a solid color or custom image.
- **Studio Light:** Subtly brightens and separates the subject from the background using face-region-aware tone mapping.
- **Reactions:** Hand-gesture-triggered animated overlays (e.g., thumbs-up for 👍 and thumbs down and heart emojies based on gestures). *Off by default.* Can be trigerred manually via 

## Components

OpenEffects is designed to be lightweight and modular, avoiding the need for an always-open GUI window:

- **`openeffectsd` (Daemon):** Headless GStreamer pipeline that handles capture, ML inference, compositing, and publishing via PipeWire.
- **`openeffects` (GUI):** A GTK4/libadwaita app and primary control surface — live effect toggles and adjustments, camera settings, opt-in model downloads, and background asset management.
- **`openeffectsctl` (CLI):** First-class command-line interface for styling WMs (Hyprland, Sway), scripting, and keybinds.

## ML Inference

All models run on the **CPU** via **ONNX Runtime** (`ort` crate with `download-binaries` feature). The bundled models (selfie segmentation, face detection, hand/palm landmarks) are lightweight MobileNet-class architectures — 256² and 640² — and complete inference in 2–5 ms on modern x86_64 CPUs. No GPU is required.

> [!NOTE]
> GPU execution providers (CUDA, OpenVINO, Vulkan) were evaluated and rejected: for
> sub-1 MB models the kernel-launch and transfer latency of a discrete GPU exceeds
> the total CPU inference time. Camera capture and the GStreamer/PipeWire pipeline
> are the actual bottlenecks, not inference.

## Installation

Download the packages for your distro from the [latest GitHub release](https://github.com/funinkina/openeffects/releases/latest).

OpenEffects ships as three packages — `openeffectsd` (daemon, bundles the ML models), `openeffectsctl` (CLI), and `openeffects` (GUI). The clients depend on the daemon, so install `openeffectsd` alongside whichever client you want; pass both files on the same command line so the dependency resolves.

### Arch Linux
```bash
sudo pacman -U openeffectsd-*-x86_64.pkg.tar.zst openeffects-*-x86_64.pkg.tar.zst
```

### Fedora / RHEL
```bash
sudo dnf install ./openeffectsd-*.x86_64.rpm ./openeffects-*.x86_64.rpm
```

### Ubuntu / Debian
```bash
sudo apt install ./openeffectsd_*_amd64.deb ./openeffects_*_amd64.deb
```

For a headless / CLI-only setup, install `openeffectsd` plus `openeffectsctl` instead. x86_64 only for now.

### Remote repositories (coming soon)

- **Flatpak (Flathub)** — _coming soon_ (PipeWire virtual camera only; `v4l2loopback` is unavailable inside the sandbox).
- **Arch Linux (AUR)** — _coming soon_: `yay -S openeffects`
- **Fedora (COPR)** — _coming soon_: `dnf copr enable <user>/openeffects && dnf install openeffects`
- **Ubuntu / Debian (PPA)** — _coming soon_: `add-apt-repository ppa:<user>/openeffects && apt install openeffects`

## Usage

Start the daemon (usually handled automatically by the systemd user service):
```bash
systemctl --user start openeffectsd
```

### CLI Examples

Control effects via `openeffectsctl` (see the full [CLI reference](docs/cli.md)):
```bash
openeffectsctl status
openeffectsctl portrait-blur --on --strength 70
openeffectsctl center-stage --zoom tight
openeffectsctl background ~/Pictures/office.jpg
openeffectsctl react thumbs-up
openeffectsctl toggle reactions
```

Use with Waybar:
```json
"custom/openeffects": {
    "exec": "openeffectsctl status --short",
    "interval": 5,
    "on-click": "openeffectsctl toggle portrait-blur"
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
