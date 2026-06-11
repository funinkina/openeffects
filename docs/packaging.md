# Packaging & Release Guide

How OpenEffects is built into native packages (`.deb`, `.rpm`, Arch `.pkg.tar.zst`),
how ML models and background images become available after install, and how to cut
a release.

## How runtime data resolves after install

You do **not** package these by hand — they resolve automatically:

- **Background images.** Compiled into the GUI binary via `include_bytes!`
  (`gui/src/constants.rs`). On first GUI launch `ensure_preset_images()`
  (`gui/src/pages/background.rs`) extracts them to
  `~/.local/share/openeffects/backgrounds/`. The daemon receives the absolute path
  over D-Bus (`bg_replace.background`). Nothing to install.
- **ML models.** Bundled into the package at build time. `scripts/build-packages.sh`
  downloads + sha256-verifies the 4 ONNX models and installs them to
  `/usr/share/openeffects/models/<id>/`. The daemon's `search_dirs()`
  (`daemon/src/inference/registry.rs`) already searches:
  1. `~/.local/share/openeffects/models` (per-user)
  2. `/usr/share/openeffects/models` (system — where packages put them)
  3. `/app/share/openeffects/models` (Flatpak)

  So an installed package reports `Capabilities.models_ready = true` immediately,
  offline, with no user step. Model licenses are Apache-2.0 / MIT (redistributable).
- **Config.** Per-user at `~/.config/openeffects/state.toml` — never packaged.

## Build dependencies

Same as a normal source build; the packages link against these at build time.

| Component | Build deps (pkg-config / headers)                                                                  |
| --------- | -------------------------------------------------------------------------------------------------- |
| daemon    | GStreamer + plugins-base, PipeWire (`libpipewire-0.3`, `libspa-0.2`), `clang`/`libclang` (bindgen) |
| gui       | GTK4, libadwaita, GStreamer                                                                        |
| all       | Rust ≥ 1.88, `cargo`                                                                               |
| packaging | [nfpm](https://nfpm.goreleaser.com), `curl`, `python3`                                             |

ONNX Runtime is pulled by `ort`'s `download-binaries` feature and **statically
linked** — the resulting binaries carry no `libonnxruntime.so` runtime dependency.
`build-packages.sh` asserts this with an `ldd` guard; if a future toolchain links it
dynamically the build fails loudly so the `.so` can be added to `nfpm.yaml` contents.

Distro **runtime** deps (what users need installed) are declared per-packager in
`packaging/nfpm.yaml` under `overrides:` (different package names per distro).

## Install nfpm

Single Go binary; CI installs it from the released `.deb`. Locally:

```bash
go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest
export PATH="$HOME/go/bin:$PATH"
```

## Build packages

```bash
./scripts/build-packages.sh            # deb + rpm + archlinux
./scripts/build-packages.sh deb        # one format
```

What it does:
1. Read `VERSION` from `Cargo.toml` (`[workspace.package].version`), `ARCH=amd64`.
2. `cargo build --release --workspace`.
3. `ldd` guard — fail if `openeffectsd` dynamically links onnxruntime.
4. Render the `@bindir@` service templates (`data/{dbus-services,systemd}/*.in`)
   into `dist/` with `@bindir@ → /usr/bin`.
5. Stage models: `OE_MODELS_DEST=dist/models ./scripts/fetch-models.sh`
   (downloads + sha256-verifies into the package staging tree).
6. `nfpm pkg` for each format → `dist/`.

Output:

```
dist/openeffects_<ver>_amd64.deb
dist/openeffects-<ver>-1.x86_64.rpm
dist/openeffects-<ver>-1-x86_64.pkg.tar.zst
```

`dist/` is gitignored.

### What lands in the package

| Path                                                        | Source                                  |
| ----------------------------------------------------------- | --------------------------------------- |
| `/usr/bin/{openeffectsd,openeffectsctl,openeffects}`        | `target/release/*`                      |
| `/usr/share/dbus-1/interfaces/org.openeffects.*.xml`        | `data/dbus/*.xml`                       |
| `/usr/share/dbus-1/services/org.openeffects.Daemon.service` | rendered template                       |
| `/usr/lib/systemd/user/openeffectsd.service`                | rendered template                       |
| `/usr/share/icons/hicolor/scalable/apps/openeffects.svg`    | `data/icons/openeffects.svg`            |
| `/usr/share/applications/openeffects.desktop`               | `data/applications/openeffects.desktop` |
| `/usr/share/openeffects/models/<id>/`                       | `dist/models/` (downloaded + verified)  |

deb/rpm run `packaging/postinstall.sh` / `postremove.sh` (icon cache +
`update-desktop-database`). Arch handles those via pacman hooks — no scriptlet.

## Install the built packages

```bash
# Debian/Ubuntu
sudo apt install ./dist/openeffects_<ver>_amd64.deb
# Fedora/RHEL
sudo dnf install ./dist/openeffects-<ver>-1.x86_64.rpm
# Arch
sudo pacman -U dist/openeffects-<ver>-1-x86_64.pkg.tar.zst
```

Then enable the daemon (D-Bus-activated; the user unit auto-starts it on demand):

```bash
systemctl --user daemon-reload
systemctl --user enable --now openeffectsd.service   # optional; D-Bus activation works without it
```

## Version bump

Version lives in two hard-coded spots (`Cargo.toml`, `meson.build`); all crates
inherit via `version.workspace = true`. One script keeps them in sync:

```bash
./scripts/bump-version.sh 0.2.0
```

It rewrites both files, runs `cargo update --workspace` to sync `Cargo.lock`,
commits `chore: release v0.2.0`, and tags `v0.2.0`. It does **not** push.

```bash
git push && git push origin v0.2.0   # pushing the tag fires the release CI
```

## Release CI

`.github/workflows/release.yml` triggers on `v*` tags. On `ubuntu-latest` it installs
build deps + Rust + nfpm, runs `scripts/build-packages.sh`, and attaches
`dist/*.{deb,rpm,pkg.tar.zst}` to the GitHub Release via `softprops/action-gh-release`.
nfpm builds the Arch `.zst` fine on Ubuntu — no Arch host required.

## Source build (no packaging)

For dev / non-packaged installs use meson; models come from `fetch-models.sh`:

```bash
meson setup build && meson install -C build   # binaries + dbus/systemd/desktop/icon
./scripts/fetch-models.sh                      # models → ~/.local/share/openeffects/models
```

`meson install` does **not** ship models (they aren't in the source tree) — run
`fetch-models.sh` for the per-user model dir.

## Verifying a package without installing

```bash
# Arch / zst
tar tf dist/*.pkg.tar.zst | grep usr/
tar xOf dist/*.pkg.tar.zst .PKGINFO | grep depend

# deb
ar p dist/*.deb data.tar.gz | bsdtar tf -                 # file list
ar p dist/*.deb control.tar.gz | bsdtar xOf - ./control   # deps + metadata
```

## Adding architectures

Currently x86_64 only. For aarch64: parameterize `ARCH` in `build-packages.sh`,
cross-compile or use a native runner, and add a CI matrix.
