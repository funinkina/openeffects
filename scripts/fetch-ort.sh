#!/usr/bin/env bash
# Fetch the sha256-verified CPU onnxruntime shared library the daemon loads at
# runtime (the OpenVINO build for NPUs comes from `ovfetch install` instead).
#
#   scripts/fetch-ort.sh DEST_DIR
set -euo pipefail

version=1.24.1
dest="${1:?usage: fetch-ort.sh DEST_DIR}"

case "$(uname -m)" in
    x86_64 | amd64) arch=x64 sha=9142552248b735920f9390027e4512a2cacf8946a1ffcbe9071a5c210531026f ;;
    aarch64 | arm64) arch=aarch64 sha=0f56edd68f7602df790b68b874a46b115add037e88385c6c842bb763b39b9f89 ;;
    *) echo "error: no onnxruntime build for $(uname -m)" >&2; exit 1 ;;
esac

target="$dest/libonnxruntime.so.$version"
if [ -s "$target" ]; then
    ln -sf "libonnxruntime.so.$version" "$dest/libonnxruntime.so.1"
    ln -sf "libonnxruntime.so.1" "$dest/libonnxruntime.so"
    echo "onnxruntime $version ($arch) already present: $target"
    exit 0
fi

name="onnxruntime-linux-$arch-$version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/ort.tgz" \
    "https://github.com/microsoft/onnxruntime/releases/download/v$version/$name.tgz"
echo "$sha  $tmp/ort.tgz" | sha256sum -c --quiet -
tar -xzf "$tmp/ort.tgz" -C "$tmp"

install -d "$dest"
install -m 0755 "$tmp/$name/lib/libonnxruntime.so.$version" "$dest/"
install -m 0755 "$tmp/$name/lib/libonnxruntime_providers_shared.so" "$dest/"
ln -sf "libonnxruntime.so.$version" "$dest/libonnxruntime.so.1"
ln -sf "libonnxruntime.so.1" "$dest/libonnxruntime.so"
echo "onnxruntime $version ($arch) -> $dest"
