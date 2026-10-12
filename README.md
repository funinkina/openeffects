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
- **Reactions:** Hand-gesture-triggered animated overlays (e.g., thumbs-up for 👍 and thumbs down and heart emojies based on gestures). *Off by default.* Can be trigerred manually via the GUI or CLI anytime. 

## Components

OpenEffects is designed to be lightweight and modular, avoiding the need for an always-open GUI window:

- **`openeffectsd` (Daemon):** Headless GStreamer pipeline that handles capture, ML inference, compositing, and publishing via PipeWire.
- **`openeffects` (GUI):** A GTK4/libadwaita app and primary control surface — live effect toggles and adjustments, camera settings, opt-in model downloads, and background asset management.
- **`openeffectsctl` (CLI):** First-class command-line interface for styling WMs (Hyprland, Sway), scripting, and keybinds.

## ML Inference

Inference runs through **ONNX Runtime** (`ort` crate). Four bundled models: selfie segmentation (256²), YuNet face detection (640²), palm detection (192²) and hand landmarks (224²).

The default build is **CPU only** (`download-binaries`), which is what the packages ship. An opt-in **`npu` build** runs the same models on Intel NPUs (or GPUs) through the OpenVINO execution provider:

```bash
# onnxruntime-openvino matched to the installed NPU driver
ovfetch install --prefix ~/.local/lib/openeffects-ort

cargo build --release -p openeffectsd --features npu
ORT_DYLIB_PATH=~/.local/lib/openeffects-ort/libonnxruntime.so \
  OE_EP=npu openeffectsd --start
```

`OE_EP` is `cpu` (default), `npu` or `gpu`. A non-CPU request that can't be satisfied is an error, never a silent CPU fallback.

### Measured latency

Per call, mean ms over 100 iterations after warmup, 1280×720 frame, Dell XPS 13 (Wildcat Lake NPU, driver 1.35.0, OpenVINO 2025.4.1). Includes the RGBA resample and output decode, not just `session.run`.

| Model | CPU | NPU | NPU load |
|---|---|---|---|
| Selfie segmentation | 3.4 | 1.4 | 35 ms |
| YuNet | 8.1 | 4.9 | 4 ms |
| Palm detection | 4.9 | 2.1 | 533 ms |
| Hand landmarks | 2.6 | 1.2 | 426 ms |

Reproduce with `openeffectsd --bench-inference [ITERS] [FRAME.png]` under each `OE_EP`. The NPU run was confirmed on hardware via `npu_busy_time_us`.

This is one machine and a synthetic frame (no face or hand in it), so read it as a snapshot. It measures inference only, not the camera and GStreamer/PipeWire pipeline.

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

## Development Platform

- **Language:** Rust (Daemon, CLI, GUI)
- **UI:** GTK4 + libadwaita-rs
- **Media Pipeline:** GStreamer 1.24+ + PipeWire 
- **Display Protocol:** Wayland only (X11 is not supported)
