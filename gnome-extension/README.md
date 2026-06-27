# OpenEffects Control Center (GNOME Shell extension)

A top-bar control center for the [OpenEffects](https://github.com/funinkina/openeffects)
webcam daemon. Click the app icon in the panel to get a popup that mirrors the desktop
app — toggle the global bypass, enable Center Stage / Background / Studio Light /
Reactions, tune each one, switch cameras, and fire reaction emojis. It talks straight
to `openeffectsd` over D-Bus; no app window required.

The popup stays in sync: change anything from the GTK app or `openeffectsctl` and the
panel reflects it live.

## How it maps to the daemon

Everything is the existing session-bus API (`in.co.funinkina.Daemon` at
`/in/co/funinkina/Daemon`) — no daemon changes.

| UI | D-Bus |
|----|-------|
| Master switch (header) | `Effects1.SetBypass` — **on = effects active** (`bypass=false`), off = camera passthrough. The daemon and webcam stay up. |
| Circular icon next to each effect name | `Effects1.SetEnabled(<id>, on)` |
| Center Stage: Subtle/Normal/Tight, Single/Group | `SetParam('center_stage','zoom'\|'mode', …)` |
| Background: Blur level / color / image | `SetEnabled('portrait_blur'\|'bg_replace', …)` + `SetParam(…,'strength'\|'background', …)` |
| Studio Light sliders | `SetParam('studio_light','intensity'\|'brightness'\|'contrast'\|'bg_brightness', …)` |
| Reaction emoji buttons | `Effects1.TriggerReaction(<id>)` |
| Camera row | `Devices1.ListCameras` / `SelectCamera` |
| "Open OpenEffects" | launches `in.co.funinkina.OpenEffects.desktop` |

When the daemon isn't running the panel icon dims and the popup shows a notice; flipping
the master switch on asks the bus to activate `openeffectsd`.

## Requirements

- GNOME Shell **45–50** (ESM).
- `openeffectsd` installed and reachable on the session bus (the `openeffectsd`
  system package). Confirm with `busctl --user list | grep funinkina`.

## Install

```sh
cd gnome-extension
make install      # copies to ~/.local/share/gnome-shell/extensions/
# Wayland: log out and back in.   Xorg: Alt+F2 -> r -> Enter.
make enable
```

Uninstall: `make disable && make uninstall`.

### From a zip

```sh
make pack
gnome-extensions install --force openeffects@funinkina.co.in.shell-extension.zip
```

## Layout

```
openeffects@funinkina.co.in/
  extension.js     entry point (enable/disable)
  indicator.js     panel button + popup + live refresh
  sections.js      St widgets + per-effect builders
  dbus.js          daemon proxies, state snapshot, signal plumbing
  stylesheet.css   app-matching theme (#56A7EA accent)
  icons/           app logo + background presets
```

## Develop

- `make lint` — syntax-checks the JS (`node --check`).
- `make nested` — opens a throwaway nested GNOME Shell to test without disturbing your session.
- `make logs` — follows shell logs (`journalctl -f -o cat /usr/bin/gnome-shell`); the
  extension logs under `OpenEffects:` prefixes.
- Inspect the live API: `busctl --user introspect in.co.funinkina.Daemon /in/co/funinkina/Daemon`.

## Troubleshooting

- **Icon missing / extension off** — version mismatch. Check `gnome-extensions info
  openeffects@funinkina.co.in`; ensure your GNOME is 45–50, then re-enable.
- **Controls greyed with "daemon not running"** — start it: `systemctl --user start
  openeffectsd` (or flip the master switch on).
- **Nothing happens on toggle** — watch `make logs` for `OpenEffects:` errors while clicking.
