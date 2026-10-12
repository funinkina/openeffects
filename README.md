<img src="data/icons/in.co.funinkina.OpenEffects.svg" align="left" width="80" alt="OpenEffects icon">

# vinoOpenEffects

[![Latest Release](https://img.shields.io/github/v/release/funinkina/openeffects?style=flat-square)](https://github.com/funinkina/openeffects/releases/latest)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)
[![Language](https://img.shields.io/badge/language-Rust-orange?style=flat-square)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Linux-important?style=flat-square)](https://www.linux.org/)
[![GitHub Stars](https://img.shields.io/github/stars/funinkina/openeffects?style=flat-square)](https://github.com/funinkina/openeffects/stargazers)
[![GitHub Issues](https://img.shields.io/github/issues/funinkina/openeffects?style=flat-square)](https://github.com/funinkina/openeffects/issues)

vinoOpenEffects is a Linux-native webcam effects engine powered by ONNX Runtime. It brings advanced camera features like Center Stage, Portrait blur, Background Replacement, and gesture-triggered Reactions to any Wayland Linux desktop. It works transparently with any app that consumes a PipeWire camera node or `/dev/video*` device (Zoom, OBS, WebRTC, etc.).

## Features

- **Portrait Mode:** High-quality background blur with feathered edges and temporal stability.
- **Center Stage:** Intelligent face and body tracking that smoothly crops and zooms to keep you (or your group) centered.
- **Background Replacement:** Replace your messy room with a solid color or custom image.
- **Studio Light:** Subtly brightens and separates the subject from the background using face-region-aware tone mapping.
- **Reactions:** Hand-gesture-triggered animated overlays (e.g., thumbs-up for 👍 and thumbs down and heart emojies based on gestures). *Off by default.* Can be trigerred manually via the GUI or CLI anytime. 

## Components

vinoOpenEffects is designed to be lightweight and modular, avoiding the need for an always-open GUI window:

- **`vinoopeneffects-daemon` (Daemon):** Headless GStreamer pipeline that handles capture, ML inference, compositing, and publishing via PipeWire.
- **`vinoopeneffects` (GUI):** A GTK4/libadwaita app and primary control surface — live effect toggles and adjustments, camera settings, opt-in model downloads, and background asset management.
- **`vinoopeneffects-ctl` (CLI):** First-class command-line interface for styling WMs (Hyprland, Sway), scripting, and keybinds.

## ML Inference

Inference runs through **ONNX Runtime** (`ort` crate). Four bundled models: selfie segmentation (256²), YuNet face detection (640²), palm detection (192²) and hand landmarks (224²).

The default build is **CPU only** (`download-binaries`), which is what the packages ship. An opt-in **`npu` build** runs the same models on Intel NPUs (or GPUs) through the OpenVINO execution provider:

```bash
# onnxruntime-openvino matched to the installed NPU driver
ovfetch install --prefix ~/.local/lib/vinoopeneffects-ort

cargo build --release -p vinoopeneffects-daemon --features npu
ORT_DYLIB_PATH=~/.local/lib/vinoopeneffects-ort/libonnxruntime.so \
  VINOOPENEFFECTS_EP=npu vinoopeneffects-daemon --start
```

`VINOOPENEFFECTS_EP` is `cpu` (default), `npu` or `gpu`. A non-CPU request that can't be satisfied is an error, never a silent CPU fallback.

### Measured latency

Per call, mean ms over 100 iterations after warmup, 1280×720 frame, Dell XPS 13 (Wildcat Lake NPU, driver 1.35.0, OpenVINO 2025.4.1). Includes the RGBA resample and output decode, not just `session.run`.

| Model | CPU | NPU | NPU load (first run) |
|---|---|---|---|
| Selfie segmentation | 3.4 | 1.4 | 35 ms |
| YuNet | 8.1 | 4.9 | 4 ms |
| Palm detection | 4.9 | 2.1 | 533 ms |
| Hand landmarks | 2.6 | 1.2 | 426 ms |

Reproduce with `vinoopeneffects-daemon --bench-inference [ITERS] [FRAME.png]` under each `VINOOPENEFFECTS_EP`. The NPU run was confirmed on hardware via `npu_busy_time_us`.

Palm and hand loaded in 37 ms and 68 ms on a later run, so the first-run compile cost is not paid every launch. This is one machine and a synthetic frame (no face or hand in it), so read it as a snapshot. It measures inference only, not the camera and GStreamer/PipeWire pipeline.

## Installation

Download the packages for your distro from the [latest GitHub release](https://github.com/karanshukla/vinoOpenEffects/releases/latest).

vinoOpenEffects ships as three packages — `vinoopeneffects-daemon` (daemon, bundles the ML models), `vinoopeneffects-ctl` (CLI), and `vinoopeneffects` (GUI). The clients depend on the daemon, so install `vinoopeneffects-daemon` alongside whichever client you want; pass both files on the same command line so the dependency resolves.

### Arch Linux
```bash
sudo pacman -U vinoopeneffects-daemon-*-x86_64.pkg.tar.zst vinoopeneffects-*-x86_64.pkg.tar.zst
```

### Fedora / RHEL
```bash
sudo dnf install ./vinoopeneffects-daemon-*.x86_64.rpm ./vinoopeneffects-*.x86_64.rpm
```

### Ubuntu / Debian
```bash
sudo apt install ./vinoopeneffects-daemon_*_amd64.deb ./vinoopeneffects_*_amd64.deb
```

For a headless / CLI-only setup, install `vinoopeneffects-daemon` plus `vinoopeneffects-ctl` instead. x86_64 only for now.

### Upgrading from openeffects

This project is a hard fork of `openeffects` and everything was renamed. Installing the new packages removes the old ones and their leftover files (old unit, D-Bus service, desktop entry, icon, binaries).

| Before | Now |
|---|---|
| packages `openeffectsd`, `openeffectsctl`, `openeffects` | `vinoopeneffects-daemon`, `vinoopeneffects-ctl`, `vinoopeneffects` |
| `systemctl --user ... openeffectsd` | `systemctl --user ... vinoopeneffects-daemon` |
| `OE_EP` | `VINOOPENEFFECTS_EP` |
| camera picker entry "OpenEffects Virtual Camera" | "vinoOpenEffects" |
| `openeffectsctl` | `vinoopeneffects-ctl` (there is no `openeffectsctl` alias) |

Your config and downloaded models are migrated automatically on first start of the new daemon/GUI. If you set `OE_EP` in a systemd drop-in, the migration rewrites it to `VINOOPENEFFECTS_EP`. Update any scripts, keybinds or Waybar modules that call `openeffectsctl`, and re-select the camera in apps (Zoom, OBS, browsers) since the device name changed.

### Remote repositories (coming soon)

- **Flatpak (Flathub)** — _coming soon_ (PipeWire virtual camera only; `v4l2loopback` is unavailable inside the sandbox).
- **Arch Linux (AUR)** — _coming soon_: `yay -S vinoopeneffects`
- **Fedora (COPR)** — _coming soon_: `dnf copr enable <user>/vinoopeneffects && dnf install vinoopeneffects`
- **Ubuntu / Debian (PPA)** — _coming soon_: `add-apt-repository ppa:<user>/vinoopeneffects && apt install vinoopeneffects`

## Usage

Start the daemon (usually handled automatically by the systemd user service):
```bash
systemctl --user start vinoopeneffects-daemon
```

### CLI Examples

Control effects via `vinoopeneffects-ctl` (see the full [CLI reference](docs/cli.md)):
```bash
vinoopeneffects-ctl status
vinoopeneffects-ctl portrait-blur --on --strength 70
vinoopeneffects-ctl center-stage --zoom tight
vinoopeneffects-ctl background ~/Pictures/office.jpg
vinoopeneffects-ctl react thumbs-up
vinoopeneffects-ctl toggle reactions
```

Use with Waybar:
```json
"custom/vinoopeneffects": {
    "exec": "vinoopeneffects-ctl status --short",
    "interval": 5,
    "on-click": "vinoopeneffects-ctl toggle portrait-blur"
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

## Credits & License

vinoOpenEffects is a hard fork of [funinkina/openeffects](https://github.com/funinkina/openeffects) by Aryan, and keeps its GPL-3.0-or-later license. See [NOTICE](NOTICE) for attribution and [LICENSE](LICENSE) for the license text.
