#!/usr/bin/env bash
# Build distributable packages (.deb, .rpm, Arch .pkg.tar.zst) for vinoOpenEffects.
#
# Steps: cargo release build -> render @bindir@ service templates -> stage
# sha256-verified ONNX models -> nfpm pack for each target into dist/.
#
# Usage:
#   ./scripts/build-packages.sh              # all three formats
#   ./scripts/build-packages.sh deb rpm      # selected formats
#
# Requires: cargo, nfpm (https://nfpm.goreleaser.com), curl, python3.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

dist="$repo_root/dist"
bindir="/usr/bin"

# nfpm arch label (deb/Arch use amd64/x86_64 conventions; nfpm normalizes).
ARCH="${ARCH:-amd64}"

# Single source of truth: workspace package version in the root Cargo.toml.
VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "([^"]+)"/\1/')"
if [ -z "$VERSION" ]; then
    echo "error: could not read version from Cargo.toml" >&2
    exit 1
fi

formats=("$@")
if [ "${#formats[@]}" -eq 0 ]; then
    formats=(deb rpm archlinux)
fi

if ! command -v nfpm >/dev/null 2>&1; then
    echo "error: nfpm not found. Install: https://nfpm.goreleaser.com/install/" >&2
    echo "  go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest" >&2
    exit 1
fi

echo "==> Building vinoopeneffects $VERSION ($ARCH)"

echo "==> cargo build --release --workspace"
cargo build --release --workspace

rm -rf "$dist"
mkdir -p "$dist"

echo "==> Rendering service templates (@bindir@ -> $bindir)"
sed "s|@bindir@|$bindir|g" data/dbus-services/io.github.karanshukla.vinoopeneffects.Daemon.service.in \
    > "$dist/io.github.karanshukla.vinoopeneffects.Daemon.service"
sed "s|@bindir@|$bindir|g" data/systemd/vinoopeneffects-daemon.service.in \
    > "$dist/vinoopeneffects-daemon.service"

echo "==> Staging onnxruntime into dist/ort (sha256-verified, CPU)"
./scripts/fetch-ort.sh "$dist/ort"

echo "==> Staging models into dist/models (sha256-verified)"
VINOOPENEFFECTS_MODELS_DEST="$dist/models" ./scripts/fetch-models.sh

# Three independent packages: the daemon, plus the CLI and GUI clients that
# both depend on it. Pack each config for each requested format.
configs=(daemon cli gui)
for cfg in "${configs[@]}"; do
    for fmt in "${formats[@]}"; do
        echo "==> nfpm pkg ($cfg) -p $fmt"
        VERSION="$VERSION" ARCH="$ARCH" nfpm pkg \
            --config "packaging/nfpm-$cfg.yaml" \
            --packager "$fmt" \
            --target "$dist/"
    done
done

echo
echo "==> Done. Artifacts in dist/:"
ls -1 "$dist"/*.deb "$dist"/*.rpm "$dist"/*.pkg.tar.zst 2>/dev/null || true
