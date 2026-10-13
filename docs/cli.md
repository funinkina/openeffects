# `vinoopeneffects-ctl` — CLI reference

`vinoopeneffects-ctl` is the command-line control surface for the vinoOpenEffects daemon.
It is a stateless D-Bus client: every command talks to a running `vinoopeneffects-daemon`
over the session bus, so the daemon must be up first.

```bash
systemctl --user start vinoopeneffects-daemon   # or: vinoopeneffects-daemon --start
```

Every command prints help with `--help`; effect subcommands print their current
state when run with no flags.

```bash
vinoopeneffects-ctl --help
vinoopeneffects-ctl studio-light --help
```

## Quick start

```bash
vinoopeneffects-ctl start                       # arm the virtual camera
vinoopeneffects-ctl portrait-blur --on --strength 70
vinoopeneffects-ctl background ~/Pictures/office.jpg
vinoopeneffects-ctl studio-light --on --intensity 60 --brightness 10
vinoopeneffects-ctl status
```

## Daemon lifecycle

| Command                         | Description                                                                                         |
| ------------------------------- | --------------------------------------------------------------------------------------------------- |
| `vinoopeneffects-ctl start`          | Arm the virtual camera. The real camera opens on demand when a consumer (browser, OBS, …) connects. |
| `vinoopeneffects-ctl stop`           | Tear down the pipeline and release the camera.                                                      |
| `vinoopeneffects-ctl status`         | Print status plus capabilities (execution provider, model readiness, output sink).            |
| `vinoopeneffects-ctl status --short` | One-word status, e.g. for Waybar.                                                                   |
| `vinoopeneffects-ctl status --json`  | Machine-readable status.                                                                            |
| `vinoopeneffects-ctl inference [ep]` | View or switch the inference execution provider (`auto`, `cpu`, `npu`).                             |
| `vinoopeneffects-ctl doctor`         | Run diagnostic checks across daemon, NPU, models, cameras, and pipeline.                            |
| `vinoopeneffects-ctl watch`          | Stream `EffectChanged` signals until interrupted (Ctrl-C).                                          |

Any command that talks to the daemon starts it first if it isn't already running
(via the systemd user unit, falling back to D-Bus activation).

## Inference execution provider (`inference`)

View or switch the machine-learning execution provider between CPU and NPU at runtime:

```bash
vinoopeneffects-ctl inference          # show requested, active, available EPs, and any fallback detail
vinoopeneffects-ctl inference npu      # switch to Intel NPU (OpenVINO EP)
vinoopeneffects-ctl inference cpu      # switch to CPU
vinoopeneffects-ctl inference auto     # auto-select NPU if present, CPU otherwise
vinoopeneffects-ctl inference --json   # machine-readable output
```

Provider changes are persisted to `~/.config/vinoopeneffects/state.toml` under `[inference] provider`. Hot-swapping while streaming compiles models off-thread so the video pipeline never drops a frame.

## Autostart

Control whether the daemon starts automatically with your desktop session
(`systemctl --user enable/disable vinoopeneffects-daemon.service`).

| Command                        | Description                                                       |
| ------------------------------ | ---------------------------------------------------------------- |
| `vinoopeneffects-ctl autostart enable`  | Start the daemon automatically when the graphical session begins. |
| `vinoopeneffects-ctl autostart disable` | Stop starting it automatically.                                   |
| `vinoopeneffects-ctl autostart status`  | Show whether autostart is enabled and whether the daemon is running. |

## Diagnostics (`doctor`)

Inspect system environment, models, NPU hardware, camera classifications, and daemon health in one command:

```bash
vinoopeneffects-ctl doctor
```

Output checks:
- **Daemon**: Reachability over D-Bus, D-Bus activation service file validity, systemd user unit status (diagnoses why activation failed if daemon is stopped).
- **Inference**: NPU build feature, ONNX Runtime dynamic library and OpenVINO Execution Provider resolution, `/dev/accel` hardware nodes.
- **Models**: Mandatory and optional model file discovery and SHA256 integrity verification against `manifest.toml`.
- **Camera**: V4L2 device enumeration with format class tags (`RGB`, `IR`, `meta`), selected camera validation (warning and suggesting RGB alternative if an IR sensor is chosen).
- **Pipeline**: PipeWire session socket connectivity and virtual camera node presence.
- **GUI**: `gtk4paintablesink` GStreamer element availability (with distro-specific package suggestions if missing) and GTK 4 / libadwaita versions.
- **Config**: `state.toml` existence, syntax validity, and unrecognized keys detection.

Machine-readable JSON output for bug reports or scripts:

```bash
vinoopeneffects-ctl doctor --json
```

Exit status: exits non-zero (`1`) if any check returns `FAIL`, and `0` otherwise.

## Effects overview

| Command         | Effect id       | What it does                                   |
| --------------- | --------------- | ---------------------------------------------- |
| `center-stage`  | `center_stage`  | Auto crop + zoom that keeps you framed.        |
| `portrait-blur` | `portrait_blur` | Blurs the background behind you.               |
| `background`    | `bg_replace`    | Replaces the background with a color or image. |
| `studio-light`  | `studio_light`  | Brightens and adds contrast to your face.      |
| `reactions`     | `reactions`     | Gesture/emoji overlay.                         |

List every effect with its on/off state:

```bash
vinoopeneffects-ctl effects
# [on ] center_stage
# [off] portrait_blur
# ...
```

### Generic enable / disable / toggle

Works for any effect (effect names are kebab-case):

```bash
vinoopeneffects-ctl enable portrait-blur
vinoopeneffects-ctl disable studio-light
vinoopeneffects-ctl toggle reactions
```

### Master passthrough

One switch over everything. When on, all effects are bypassed and the virtual
camera passes the real camera through untouched. Per-effect settings are
preserved, so turning it back off restores exactly what was on before.

```bash
vinoopeneffects-ctl passthrough --on    # bypass all effects (raw camera)
vinoopeneffects-ctl passthrough --off   # re-enable effects
vinoopeneffects-ctl passthrough         # print current state
```

| Flag    | Notes                                          |
| ------- | ---------------------------------------------- |
| `--on`  | Bypass all effects — raw camera passthrough.   |
| `--off` | Re-enable effects (turn passthrough off).      |

## Per-effect commands

Each effect subcommand shares the `--on` / `--off` toggle and adds its own
settings. Pass any combination in one call; run with no flags to print the
current state.

### Center Stage

```bash
vinoopeneffects-ctl center-stage --on --zoom tight --mode single
vinoopeneffects-ctl center-stage --crop-left 40 --crop-right 40
vinoopeneffects-ctl center-stage            # print current state
```

| Flag             | Values                    | Notes                                        |
| ---------------- | ------------------------- | -------------------------------------------- |
| `--on` / `--off` | —                         | Enable / disable.                            |
| `--zoom`         | `subtle` `normal` `tight` | Zoom level.                                  |
| `--mode`         | `single` `group`          | Framing mode.                                |
| `--crop-top`     | `0`–`512`                 | Manual crop inset (px) from the top edge.    |
| `--crop-bottom`  | `0`–`512`                 | Manual crop inset (px) from the bottom edge. |
| `--crop-left`    | `0`–`512`                 | Manual crop inset (px) from the left edge.   |
| `--crop-right`   | `0`–`512`                 | Manual crop inset (px) from the right edge.  |

### Portrait Blur

```bash
vinoopeneffects-ctl portrait-blur --on --strength 75
```

| Flag             | Values    | Notes             |
| ---------------- | --------- | ----------------- |
| `--on` / `--off` | —         | Enable / disable. |
| `--strength`     | `0`–`100` | Blur strength.    |

### Background replace

```bash
vinoopeneffects-ctl background "#1e1e2e"          # solid color, enables the effect
vinoopeneffects-ctl background ~/Pictures/bg.jpg  # image, enables the effect
vinoopeneffects-ctl background none               # clear background and disable
vinoopeneffects-ctl background --off              # disable, keep stored background
vinoopeneffects-ctl background                    # print current background
```

| Argument / flag | Values                              | Notes                                                                                   |
| --------------- | ----------------------------------- | --------------------------------------------------------------------------------------- |
| `VALUE`         | `none`, `#RRGGBB`, or an image path | `none` clears and disables; any other value sets the background and enables the effect. |
| `--off`         | —                                   | Disable without changing the stored background.                                         |

### Studio Light

```bash
vinoopeneffects-ctl studio-light --on --intensity 60 --brightness 10 --contrast 55
```

| Flag             | Values       | Notes              |
| ---------------- | ------------ | ------------------ |
| `--on` / `--off` | —            | Enable / disable.  |
| `--intensity`    | `0`–`100`    | Overall intensity. |
| `--brightness`   | `-100`–`100` | Brightness lift on you, scaled by intensity. Default `30`. |
| `--contrast`     | `0`–`100`    | Contrast around your average brightness. `50` is neutral.  |

### Reactions

Toggle the overlay effect, or fire a one-shot emoji burst with `react`:

```bash
vinoopeneffects-ctl reactions --on
vinoopeneffects-ctl react thumbs-up
vinoopeneffects-ctl react heart
```

Reaction ids: `thumbs-up` `thumbs-down` `heart` `joy` `tada` `clap`.
(`thumbs-up`, `thumbs-down`, `heart` are also the three gesture-mapped reactions.)

## Camera

```bash
vinoopeneffects-ctl camera list            # active camera marked with '*'
vinoopeneffects-ctl camera select /dev/video0
vinoopeneffects-ctl camera info            # virtual-camera node details
```

| Command              | Description                                                  |
| -------------------- | ------------------------------------------------------------ |
| `camera list`        | List available cameras; the active one is prefixed with `*`. |
| `camera select <id>` | Select a camera by id or `/dev/videoN` path.                 |
| `camera info`        | Print the virtual-camera node info.                          |

## Low-level `set`

`set` writes any effect parameter directly using `EFFECT.KEY VALUE`, where
`EFFECT` is the underscore effect id. It bypasses CLI-side validation and is
mainly an escape hatch / scripting primitive.

```bash
vinoopeneffects-ctl set studio_light.brightness 10
vinoopeneffects-ctl set center_stage.zoom tight
vinoopeneffects-ctl set bg_replace.background "#000000"
```

Values are parsed as `true`/`false` → bool, integers → number, everything else →
string.

## Scripting

Waybar module:

```json
"custom/vinoopeneffects": {
    "exec": "vinoopeneffects-ctl status --short",
    "interval": 5,
    "on-click": "vinoopeneffects-ctl toggle portrait-blur"
}
```

Bind keys (Hyprland example):

```ini
bind = $mod, F1, exec, vinoopeneffects-ctl toggle portrait-blur
bind = $mod, F2, exec, vinoopeneffects-ctl react thumbs-up
```
