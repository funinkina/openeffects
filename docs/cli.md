# `openeffectsctl` — CLI reference

`openeffectsctl` is the command-line control surface for the OpenEffects daemon.
It is a stateless D-Bus client: every command talks to a running `openeffectsd`
over the session bus, so the daemon must be up first.

```bash
systemctl --user start openeffectsd   # or: openeffectsd --start
```

Every command prints help with `--help`; effect subcommands print their current
state when run with no flags.

```bash
openeffectsctl --help
openeffectsctl studio-light --help
```

## Quick start

```bash
openeffectsctl start                       # arm the virtual camera
openeffectsctl portrait-blur --on --strength 70
openeffectsctl background ~/Pictures/office.jpg
openeffectsctl studio-light --on --intensity 60 --brightness 10
openeffectsctl status
```

## Daemon lifecycle

| Command                         | Description                                                                                         |
| ------------------------------- | --------------------------------------------------------------------------------------------------- |
| `openeffectsctl start`          | Arm the virtual camera. The real camera opens on demand when a consumer (browser, OBS, …) connects. |
| `openeffectsctl stop`           | Tear down the pipeline and release the camera.                                                      |
| `openeffectsctl status`         | Print status plus capabilities (tier, execution provider, model readiness, output sink).            |
| `openeffectsctl status --short` | One-word status, e.g. for Waybar.                                                                   |
| `openeffectsctl status --json`  | Machine-readable status.                                                                            |
| `openeffectsctl watch`          | Stream `EffectChanged` signals until interrupted (Ctrl-C).                                          |

Any command that talks to the daemon starts it first if it isn't already running
(via the systemd user unit, falling back to D-Bus activation).

## Autostart

Control whether the daemon starts automatically with your desktop session
(`systemctl --user enable/disable openeffectsd.service`).

| Command                        | Description                                                       |
| ------------------------------ | ---------------------------------------------------------------- |
| `openeffectsctl autostart enable`  | Start the daemon automatically when the graphical session begins. |
| `openeffectsctl autostart disable` | Stop starting it automatically.                                   |
| `openeffectsctl autostart status`  | Show whether autostart is enabled and whether the daemon is running. |

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
openeffectsctl effects
# [on ] center_stage
# [off] portrait_blur
# ...
```

### Generic enable / disable / toggle

Works for any effect (effect names are kebab-case):

```bash
openeffectsctl enable portrait-blur
openeffectsctl disable studio-light
openeffectsctl toggle reactions
```

## Per-effect commands

Each effect subcommand shares the `--on` / `--off` toggle and adds its own
settings. Pass any combination in one call; run with no flags to print the
current state.

### Center Stage

```bash
openeffectsctl center-stage --on --zoom tight --mode single
openeffectsctl center-stage --crop-left 40 --crop-right 40
openeffectsctl center-stage            # print current state
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
openeffectsctl portrait-blur --on --strength 75
```

| Flag             | Values    | Notes             |
| ---------------- | --------- | ----------------- |
| `--on` / `--off` | —         | Enable / disable. |
| `--strength`     | `0`–`100` | Blur strength.    |

### Background replace

```bash
openeffectsctl background "#1e1e2e"          # solid color, enables the effect
openeffectsctl background ~/Pictures/bg.jpg  # image, enables the effect
openeffectsctl background none               # clear background and disable
openeffectsctl background --off              # disable, keep stored background
openeffectsctl background                    # print current background
```

| Argument / flag | Values                              | Notes                                                                                   |
| --------------- | ----------------------------------- | --------------------------------------------------------------------------------------- |
| `VALUE`         | `none`, `#RRGGBB`, or an image path | `none` clears and disables; any other value sets the background and enables the effect. |
| `--off`         | —                                   | Disable without changing the stored background.                                         |

### Studio Light

```bash
openeffectsctl studio-light --on --intensity 60 --brightness 10 --contrast 55
```

| Flag             | Values       | Notes              |
| ---------------- | ------------ | ------------------ |
| `--on` / `--off` | —            | Enable / disable.  |
| `--intensity`    | `0`–`100`    | Overall intensity. |
| `--brightness`   | `-100`–`100` | Brightness lift.   |
| `--contrast`     | `0`–`100`    | Contrast.          |

### Reactions

Toggle the overlay effect, or fire a one-shot emoji burst with `react`:

```bash
openeffectsctl reactions --on
openeffectsctl react thumbs-up
openeffectsctl react heart
```

Reaction ids: `thumbs-up` `thumbs-down` `heart` `joy` `tada` `clap`.
(`thumbs-up`, `thumbs-down`, `heart` are also the three gesture-mapped reactions.)

## Camera

```bash
openeffectsctl camera list            # active camera marked with '*'
openeffectsctl camera select /dev/video0
openeffectsctl camera info            # virtual-camera node details
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
openeffectsctl set studio_light.brightness 10
openeffectsctl set center_stage.zoom tight
openeffectsctl set bg_replace.background "#000000"
```

Values are parsed as `true`/`false` → bool, integers → number, everything else →
string.

## Scripting

Waybar module:

```json
"custom/openeffects": {
    "exec": "openeffectsctl status --short",
    "interval": 5,
    "on-click": "openeffectsctl toggle portrait-blur"
}
```

Bind keys (Hyprland example):

```ini
bind = $mod, F1, exec, openeffectsctl toggle portrait-blur
bind = $mod, F2, exec, openeffectsctl react thumbs-up
```
