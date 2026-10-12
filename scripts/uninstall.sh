#!/usr/bin/env bash
# Remove what scripts/install.sh installed.
#
#   scripts/uninstall.sh                  # from ~/.local
#   sudo scripts/uninstall.sh --prefix /usr/local
#   scripts/uninstall.sh --purge          # also models, config and backgrounds
set -euo pipefail

prefix="$HOME/.local"
purge=0
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --purge) purge=1; shift ;;
        -h | --help) sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
done

bindir="$prefix/bin"
datadir="$prefix/share"
app_id=io.github.karanshukla.vinoopeneffects

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now vinoopeneffects-daemon.service >/dev/null 2>&1 || true
fi

rm -f "$bindir/vinoopeneffects-daemon" "$bindir/vinoopeneffects-ctl" "$bindir/vinoopeneffects"
rm -f "$datadir"/dbus-1/interfaces/"$app_id".*.xml
rm -f "$datadir/dbus-1/services/$app_id.Daemon.service"
rm -f "$datadir/systemd/user/vinoopeneffects-daemon.service"
rm -f "$datadir/applications/$app_id.desktop"
rm -f "$datadir/icons/hicolor/scalable/apps/$app_id.svg"
rm -f "$datadir/icons/hicolor/symbolic/apps/$app_id-symbolic.svg"

if [ "$purge" = 1 ]; then
    rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/vinoopeneffects" \
        "${XDG_CONFIG_HOME:-$HOME/.config}/vinoopeneffects"
fi

if command -v busctl >/dev/null 2>&1; then
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
fi
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$datadir/icons/hicolor" 2>/dev/null || true
fi

echo "Removed vinoOpenEffects from $prefix."
