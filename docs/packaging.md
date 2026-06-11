# Packaging & Release Guide

How OpenEffects is built into native packages (`.deb`, `.rpm`, Arch `.pkg.tar.zst`),
how ML models and background images become available after install, and how to cut
a release.

## Package layout

OpenEffects ships as **three** packages so users install only what they need:

| Package | Contents | Depends on |
| --- | --- | --- |
| `openeffectsd` | daemon binary, D-Bus service + systemd user unit, bundled ML models | pipewire/gstreamer stack, dbus |
| `openeffectsctl` | CLI binary | **`openeffectsd`**, dbus |
| `openeffects` | GTK4/libadwaita GUI binary, `.desktop` + icon | **`openeffectsd`**, gtk4, libadwaita |

The daemon is the only process that touches the pipeline, so both clients declare a
hard dependency on it. The dependency is unversioned (`Depends: openeffectsd`) and is
resolved automatically whether installing from local package files (pass them on the
same command line) or from a configured repository — see [Install](#install-the-built-packages).

Each package is built from its own nfpm config: `packaging/nfpm-daemon.yaml`,
`nfpm-cli.yaml`, `nfpm-gui.yaml`.

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
the three `packaging/nfpm-*.yaml` configs under `overrides:` (different package names
per distro).

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
6. `nfpm pkg` for each of the 3 configs × each format → `dist/`.

Output (3 packages × N formats):

```
dist/openeffectsd_<ver>_amd64.deb     openeffectsctl_<ver>_amd64.deb     openeffects_<ver>_amd64.deb
dist/openeffectsd-<ver>-1.x86_64.rpm  openeffectsctl-<ver>-1.x86_64.rpm  openeffects-<ver>-1.x86_64.rpm
dist/openeffectsd-<ver>-1-x86_64.pkg.tar.zst  ...ctl...  ...openeffects...
```

`dist/` is gitignored.

### What lands in each package

**`openeffectsd`** (daemon + everything the pipeline needs):

| Path | Source |
| --- | --- |
| `/usr/bin/openeffectsd` | `target/release/openeffectsd` |
| `/usr/share/dbus-1/interfaces/org.openeffects.*.xml` | `data/dbus/*.xml` |
| `/usr/share/dbus-1/services/org.openeffects.Daemon.service` | rendered template |
| `/usr/lib/systemd/user/openeffectsd.service` | rendered template |
| `/usr/share/openeffects/models/<id>/` | `dist/models/` (downloaded + verified) |

**`openeffectsctl`** (CLI): `/usr/bin/openeffectsctl`.

**`openeffects`** (GUI):

| Path | Source |
| --- | --- |
| `/usr/bin/openeffects` | `target/release/openeffects` |
| `/usr/share/icons/hicolor/scalable/apps/org.openeffects.OpenEffects.svg` | `data/icons/org.openeffects.OpenEffects.svg` |
| `/usr/share/applications/org.openeffects.OpenEffects.desktop` | `data/applications/org.openeffects.OpenEffects.desktop` |

The GUI package's deb/rpm run `packaging/postinstall.sh` / `postremove.sh` (icon cache +
`update-desktop-database`). Arch handles those via pacman hooks — no scriptlet.

## Install the built packages

The CLI and GUI packages declare `Depends: openeffectsd`. There are two ways the
dependency is satisfied:

**From a repository** — the daemon is in a configured repo, so installing only a
client pulls it in automatically:

```bash
sudo apt install openeffects          # Debian/Ubuntu — pulls openeffectsd
sudo dnf install openeffects          # Fedora/RHEL
sudo pacman -S openeffects            # Arch
```

**From local package files** — pass the daemon file on the same command line so the
resolver sees it. The package managers' file-install mode resolves inter-file deps:

```bash
# Debian/Ubuntu (use apt, not `dpkg -i` — dpkg does NOT resolve deps)
sudo apt install ./dist/openeffectsd_<ver>_amd64.deb ./dist/openeffects_<ver>_amd64.deb

# Fedora/RHEL
sudo dnf install ./dist/openeffectsd-<ver>-1.x86_64.rpm ./dist/openeffects-<ver>-1.x86_64.rpm

# Arch (pacman -U resolves deps among the files given, or from the sync db)
sudo pacman -U dist/openeffectsd-<ver>-1-x86_64.pkg.tar.zst dist/openeffects-<ver>-1-x86_64.pkg.tar.zst
```

Install just `openeffectsd` for a headless/CLI-only setup, or add `openeffectsctl`
the same way. Installing a client without the daemon (e.g. `dpkg -i openeffects.deb`
alone) fails the dependency check — that's the resolver doing its job.

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
# Arch / zst — inspect one package (e.g. the GUI, to confirm it depends on openeffectsd)
tar tf dist/openeffects-*.pkg.tar.zst | grep usr/
tar xOf dist/openeffects-*.pkg.tar.zst .PKGINFO | grep depend

# deb
ar p dist/openeffects_*.deb data.tar.gz | bsdtar tf -                 # file list
ar p dist/openeffects_*.deb control.tar.gz | bsdtar xOf - ./control   # deps + metadata
```

## Adding architectures

Currently x86_64 only. For aarch64: parameterize `ARCH` in `build-packages.sh`,
cross-compile or use a native runner, and add a CI matrix.
