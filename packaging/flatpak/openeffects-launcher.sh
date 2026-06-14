#!/bin/sh
# Flatpak entry point for OpenEffects.
#
# There is no systemd inside the sandbox, so the daemon is not started by a user
# unit or D-Bus activation. This launcher starts openeffectsd in the background
# (if its session-bus name is unowned), then hands the foreground to the GUI.
#
# The daemon is deliberately NOT killed when the GUI window closes: it keeps
# running as a background app (visible in GNOME Shell's "Background Apps" menu via
# the Background portal) and continues applying effects to the virtual camera.
# It exits on SIGTERM — which is what the shell's "Quit" action sends — tearing
# down the sandbox with it.
#
# To run the daemon headless (no GUI):
#   flatpak run --command=openeffectsd org.openeffects.OpenEffects --start
set -eu

BUS_NAME=org.openeffects.Daemon

owned() {
    gdbus call --session \
        --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner "$BUS_NAME" 2>/dev/null \
        | grep -q true
}

if ! owned; then
    openeffectsd --start &
fi

# Replace the shell with the GUI. The backgrounded daemon reparents to the
# sandbox init and keeps running after the GUI exits.
exec openeffects "$@"
