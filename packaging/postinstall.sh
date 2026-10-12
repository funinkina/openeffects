#!/bin/sh
# Post-install/upgrade for the vinoOpenEffects packages (daemon, CLI, GUI).
#
# 1. Remove leftovers of the pre-fork "openeffects" install (funinkina/openeffects)
#    so nothing shadows the new files: old unit, D-Bus service/interfaces, desktop
#    entry, icon, binaries and bundled models. The old packages are also handled
#    via replaces/conflicts in the package metadata; this covers installs that
#    were not made from those packages (and RPM, which removes the old package
#    only after the new one is unpacked). Everything here is idempotent: rm -f,
#    nothing fails if a file is absent.
# 2. Refresh the icon cache and desktop database. Guarded so the package
#    installs cleanly on minimal systems.
set -e

# --- 1. old install cleanup -------------------------------------------------

if command -v systemctl >/dev/null 2>&1; then
    # Drop global (/etc/systemd/user) enablement of the old unit, then try to
    # stop it in every logged-in user manager. Best effort: per-user enablement
    # symlinks under ~/.config/systemd/user can't be reached from here, but they
    # dangle harmlessly once the unit file below is gone.
    systemctl --global disable openeffectsd.service >/dev/null 2>&1 || true
    for d in /run/user/*; do
        uid=${d##*/}
        case "$uid" in
            '' | *[!0-9]*) continue ;;
        esac
        systemctl --machine="${uid}@.host" --user stop openeffectsd.service >/dev/null 2>&1 || true
    done
fi

rm -f /usr/lib/systemd/user/openeffectsd.service
rm -f /etc/systemd/user/openeffectsd.service
rm -f /etc/systemd/user/*.wants/openeffectsd.service

rm -f /usr/share/dbus-1/services/in.co.funinkina.Daemon.service
rm -f /usr/share/dbus-1/interfaces/in.co.funinkina.Daemon1.xml
rm -f /usr/share/dbus-1/interfaces/in.co.funinkina.Effects1.xml
rm -f /usr/share/dbus-1/interfaces/in.co.funinkina.Devices1.xml

rm -f /usr/share/applications/in.co.funinkina.OpenEffects.desktop
rm -f /usr/share/icons/hicolor/scalable/apps/in.co.funinkina.OpenEffects.svg

rm -f /usr/bin/openeffectsd /usr/bin/openeffectsctl /usr/bin/openeffects

# Old bundled models; the new daemon reads /usr/share/vinoopeneffects/models.
rm -rf /usr/share/openeffects/models
rmdir /usr/share/openeffects >/dev/null 2>&1 || true

# --- 2. caches --------------------------------------------------------------

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications >/dev/null 2>&1 || true
fi

exit 0
