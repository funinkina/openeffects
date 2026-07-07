#!/bin/sh
# Flatpak entry point for OpenEffects.
#
# No systemd inside the sandbox, so this launcher starts both binaries. The
# process layout is load-bearing: libflatpak's flatpak_instance_is_running()
# checks the host-side `flatpak run` pid, and `flatpak run` exits when the
# sandbox's MAIN child exits. xdg-desktop-portal drops "not running" instances
# from GNOME's Background Apps menu (src/background.c, check_background_apps),
# even though bwrap keeps orphaned children alive until the namespace empties.
# So the daemon — the process meant to outlive the window — must be the main
# child, and the GUI runs as the background child. The obvious
# `daemon & exec gui` layout makes the app vanish from Background Apps the
# moment the window closes, while the daemon silently keeps running.
#
# Quit paths: with "keep running in the background" off, the GUI sends Quit
# over D-Bus and the daemon exits cleanly. GNOME Shell's Background Apps ->
# Quit tries the org.freedesktop.Application `quit` action (not exported) and
# falls back to `flatpak kill`, i.e. SIGKILL.
#
# To run the daemon headless (no GUI):
#   flatpak run --command=openeffectsd in.co.funinkina.OpenEffects --start
set -eu

BUS_NAME=in.co.funinkina.Daemon

owned() {
    gdbus call --session \
        --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner "$BUS_NAME" 2>/dev/null \
        | grep -q true
}

if owned; then
    # Daemon already running in another instance: GUI-only launch.
    exec openeffects "$@"
fi

openeffects "$@" &
exec openeffectsd --start
