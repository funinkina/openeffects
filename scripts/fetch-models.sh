#!/usr/bin/env bash
# Downloads the ML models referenced by data/models/*/manifest.toml into the
# user model directory (PRD §9.3), verifying sha256 against the committed
# manifest. Idempotent: skips models whose file is already present and
# verified.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
models_src="$repo_root/data/models"
dest_root="${XDG_DATA_HOME:-$HOME/.local/share}/openeffects/models"

for manifest in "$models_src"/*/manifest.toml; do
    id="$(basename "$(dirname "$manifest")")"
    dest="$dest_root/$id"
    mkdir -p "$dest"
    cp "$manifest" "$dest/manifest.toml"

    read -r url file sha256 < <(python3 - "$manifest" <<'EOF'
import sys, tomllib

with open(sys.argv[1], "rb") as f:
    model = tomllib.load(f)["model"]

variant = model["variants"][0]
url = model.get("source", {}).get("url", "")
print(url, variant["file"], variant["sha256"])
EOF
)

    target="$dest/$file"
    if [ -f "$target" ] && [ "$(sha256sum "$target" | cut -d' ' -f1)" = "$sha256" ]; then
        echo "[$id] already present and verified: $target"
        continue
    fi

    if [ -z "$url" ]; then
        echo "[$id] no source URL in manifest, cannot fetch $file" >&2
        exit 1
    fi

    echo "[$id] downloading $url"
    curl -fsSL "$url" -o "$target.tmp"

    actual_sha256="$(sha256sum "$target.tmp" | cut -d' ' -f1)"
    if [ "$actual_sha256" != "$sha256" ]; then
        echo "[$id] sha256 mismatch for $file: expected $sha256, got $actual_sha256" >&2
        rm -f "$target.tmp"
        exit 1
    fi

    mv "$target.tmp" "$target"
    echo "[$id] verified: $target"
done
