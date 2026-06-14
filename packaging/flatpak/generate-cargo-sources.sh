#!/usr/bin/env bash
# Regenerate the Flatpak Cargo inputs after a dependency change.
#
# Produces, in this directory:
#   * Cargo.lock          - the lockfile resolved for the `load-dynamic` ort
#                           feature set the Flatpak build uses (NOT the repo's
#                           download-binaries lock).
#   * cargo-sources.json  - vendored crate sources for the offline Flathub build,
#                           generated from that lock.
#
# The repo's own Cargo.toml / Cargo.lock are restored afterwards, untouched.
#
# Requires: cargo, python3 with aiohttp + toml + tomlkit + tomlkit (for the
# upstream flatpak-cargo-generator). Network access (crates.io) is needed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"
cd "$repo_root"

gen="${FLATPAK_CARGO_GENERATOR:-}"
if [ -z "$gen" ]; then
    gen="$(mktemp /tmp/flatpak-cargo-generator.XXXX.py)"
    echo "==> Fetching flatpak-cargo-generator.py"
    curl -fsSL -o "$gen" \
        https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
fi

backup_toml="$(mktemp)"; cp Cargo.toml "$backup_toml"
backup_lock="$(mktemp)"; cp Cargo.lock "$backup_lock"
restore() { cp "$backup_toml" Cargo.toml; cp "$backup_lock" Cargo.lock; }
trap restore EXIT

echo "==> Patching ort -> load-dynamic and resolving lockfile"
sed -i 's/"download-binaries"/"load-dynamic"/; /"tls-rustls",/d' Cargo.toml
cargo generate-lockfile

echo "==> Writing packaging/flatpak/Cargo.lock"
cp Cargo.lock "$here/Cargo.lock"

echo "==> Generating cargo-sources.json"
"${PYTHON:-python3}" "$gen" "$here/Cargo.lock" -o "$here/cargo-sources.json"

echo "==> Done. Repo Cargo.toml / Cargo.lock restored."
