#!/usr/bin/env bash
# Build and install vinoOpenEffects from this checkout.
#
#   scripts/install.sh                    # to ~/.local, no root needed
#   sudo scripts/install.sh --prefix /usr/local
#   scripts/install.sh --no-gui           # daemon and CLI only
#
# Installs binaries, D-Bus interfaces and activation file, the systemd user
# unit, desktop entry and icons, fetches the sha256-verified ONNX models, and
# puts an onnxruntime in lib/vinoopeneffects/ort: the OpenVINO build (via
# ovfetch) when an NPU/GPU is detected, the CPU build otherwise. The daemon uses
# the NPU when present and falls back to CPU.
# Packages are built separately (scripts/build-packages.sh).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

prefix="$HOME/.local"
gui=1
models=1
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --no-gui) gui=0; shift ;;
        --no-models) models=0; shift ;;
        -h | --help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
done

bindir="$prefix/bin"
datadir="$prefix/share"
target="${CARGO_TARGET_DIR:-$repo_root/target}/release"
app_id=io.github.karanshukla.vinoopeneffects

bins=(vinoopeneffects-daemon vinoopeneffects-ctl)
build=(-p vinoopeneffects-daemon -p vinoopeneffects-ctl)
if [ "$gui" = 1 ]; then
    bins+=(vinoopeneffects)
    build+=(-p vinoopeneffects)
fi

echo "==> Building (release)"
cargo build --release --locked "${build[@]}"

echo "==> Installing to $prefix"
install -d "$bindir"
for bin in "${bins[@]}"; do
    install -m 0755 "$target/$bin" "$bindir/$bin"
done

install -d "$datadir/dbus-1/interfaces"
install -m 0644 data/dbus/*.xml "$datadir/dbus-1/interfaces/"

install -d "$datadir/dbus-1/services" "$datadir/systemd/user"
sed "s|@bindir@|$bindir|g" "data/dbus-services/$app_id.Daemon.service.in" \
    >"$datadir/dbus-1/services/$app_id.Daemon.service"
sed "s|@bindir@|$bindir|g" data/systemd/vinoopeneffects-daemon.service.in \
    >"$datadir/systemd/user/vinoopeneffects-daemon.service"

if [ "$gui" = 1 ]; then
    install -d "$datadir/applications" \
        "$datadir/icons/hicolor/scalable/apps" "$datadir/icons/hicolor/symbolic/apps"
    install -m 0644 "data/applications/$app_id.desktop" "$datadir/applications/"
    install -m 0644 "data/icons/$app_id.svg" "$datadir/icons/hicolor/scalable/apps/"
    install -m 0644 "data/icons/$app_id-symbolic.svg" "$datadir/icons/hicolor/symbolic/apps/"
fi

ortdir="$prefix/lib/vinoopeneffects/ort"
echo "==> onnxruntime"
if command -v ovfetch >/dev/null 2>&1 &&
    ovfetch detect 2>/dev/null | grep -q '"kind"' &&
    ovfetch install --prefix "$ortdir"; then
    :
else
    echo "no usable ovfetch/NPU: installing the CPU runtime"
    ./scripts/fetch-ort.sh "$ortdir"
fi

if [ "$models" = 1 ]; then
    echo "==> Fetching models"
    ./scripts/fetch-models.sh
fi

echo "==> Refreshing caches"
# The bus only reads new activation files on reload; without this the first
# D-Bus call fails with "name not activatable" until the next login.
if command -v busctl >/dev/null 2>&1; then
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
fi
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
fi
if [ "$gui" = 1 ]; then
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t -f "$datadir/icons/hicolor" || true
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q "$datadir/applications" || true
    fi
fi

case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "note: $bindir is not on PATH" >&2 ;;
esac
case ":${XDG_DATA_DIRS:-/usr/local/share:/usr/share}:" in
    *":$datadir:"*) ;;
    *) [ "$prefix" = "$HOME/.local" ] ||
        echo "note: $datadir is not in XDG_DATA_DIRS; D-Bus activation and the unit may not be found" >&2 ;;
esac

echo "==> Done. Try: vinoopeneffects-ctl status"
