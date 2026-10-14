#!/usr/bin/env bash
# Bump the workspace version everywhere it is hard-coded, sync Cargo.lock, then
# commit and tag. Pushing the tag is left to you (it triggers the release CI).
#
# Usage:
#   ./scripts/bump-version.sh 0.2.0
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

new="${1:-}"
if [[ ! "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]]; then
    echo "usage: $0 <x.y.z>   (got: '${new:-}')" >&2
    exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "error: working tree not clean — commit or stash first." >&2
    exit 1
fi

old="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "([^"]+)"/\1/')"
echo "==> Bumping $old -> $new"

# Root workspace.package version (first version= line in the root manifest).
sed -i -E "0,/^version = \"[^\"]+\"/s//version = \"$new\"/" Cargo.toml

# meson.build project version.
sed -i -E "s/^  version: '[^']+',/  version: '$new',/" meson.build

# Website fallback for builds without git tags.
sed -i -E "s/^let version = 'v[^']+';/let version = 'v$new';/" website/astro.config.mjs

# Sync the lockfile entries for the workspace crates.
cargo update --workspace >/dev/null

git add Cargo.toml meson.build Cargo.lock website/astro.config.mjs
git commit -m "chore: release v$new"
git tag "v$new"

echo
echo "==> Tagged v$new. Push, then build + upload (docs/packaging.md#release):"
echo "    git push && git push origin v$new"
