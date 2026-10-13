# Packaging & Release Guide

How vinoOpenEffects is built into native packages (`.deb`, `.rpm`, Arch `.pkg.tar.zst`),
how ML models and background images become available after install, and how to cut
a release.

## Package layout

vinoOpenEffects ships as **three** packages so users install only what they need:

| Package          | Contents                                                            | Depends on                           |
| ---------------- | ------------------------------------------------------------------- | ------------------------------------ |
| `vinoopeneffects-daemon`   | daemon binary, D-Bus service + systemd user unit, bundled ML models | pipewire/gstreamer stack, dbus       |
| `vinoopeneffects-ctl` | CLI binary                                                          | **`vinoopeneffects-daemon`**, dbus             |
| `vinoopeneffects`    | GTK4/libadwaita GUI binary, `.desktop` + icon                       | **`vinoopeneffects-daemon`**, gtk4, libadwaita |

The daemon is the only process that touches the pipeline, so both clients declare a
hard dependency on it. The dependency is unversioned (`Depends: vinoopeneffects-daemon`) and is
resolved automatically whether installing from local package files (pass them on the
same command line) or from a configured repository — see [Install](#install-the-built-packages).

Each package is built from its own nfpm config: `packaging/nfpm-daemon.yaml`,
`nfpm-cli.yaml`, `nfpm-gui.yaml`.

## Upgrading from openeffects

vinoOpenEffects is a hard fork of `openeffects`; every installed name changed:

| Old | New |
| --- | --- |
| packages `openeffectsd` / `openeffectsctl` / `openeffects` | `vinoopeneffects-daemon` / `vinoopeneffects-ctl` / `vinoopeneffects` |
| `openeffectsd.service` | `vinoopeneffects-daemon.service` |
| `in.co.funinkina.Daemon.service` (D-Bus) | `io.github.karanshukla.vinoopeneffects.Daemon.service` |
| `in.co.funinkina.OpenEffects.desktop` + icon | `io.github.karanshukla.vinoopeneffects.desktop` + icon |
| `/usr/share/openeffects/models` | `/usr/share/vinoopeneffects/models` |
| `OE_EP`, `OE_MODELS_DEST` | `VINOOPENEFFECTS_EP`, `VINOOPENEFFECTS_MODELS_DEST` |

The camera picker in apps shows "vinoOpenEffects Virtual Camera" instead of "OpenEffects Virtual
Camera". `openeffectsctl` is gone; there is no alias, use `vinoopeneffects-ctl`.

### Migration

On first start the daemon and the GUI move existing per-user state to the new
names, logging each move:

- `~/.config/openeffects/` (`state.toml`, `gui.toml`) to `~/.config/vinoopeneffects/`
- `~/.local/share/openeffects/` (models, backgrounds) to `~/.local/share/vinoopeneffects/`
- `~/.config/systemd/user/openeffectsd.service.d/` to
  `~/.config/systemd/user/vinoopeneffects-daemon.service.d/`, with `OE_EP` and
  `OE_MODELS_DEST` in its `*.conf` files renamed to `VINOOPENEFFECTS_EP` and
  `VINOOPENEFFECTS_MODELS_DEST`
- user enablement links `*.wants/openeffectsd.service` are re-created as
  `*.wants/vinoopeneffects-daemon.service`

An item is skipped, with a warning, if its destination already exists; old and
new are never merged. A user-installed `~/.config/systemd/user/openeffectsd.service`
is left alone with a warning. `~/.local/lib/openeffects-ort` (a benchmark
cache) is not migrated. Run `systemctl --user daemon-reload` after upgrading.

Each new package declares `replaces`/`conflicts` (deb/Arch) and `Obsoletes` (rpm,
via nfpm `replaces`) on its old counterpart, and `packaging/postinstall.sh` sweeps
whatever the old packages (or a source install) left behind: the old user unit,
D-Bus service file and interface XMLs, desktop entry, icon, binaries and bundled
models. It is POSIX sh and idempotent (`rm -f`). Arch packages rely on
`replaces`/`conflicts` only; they have no install script.

Per-user config (`~/.config/openeffects`), data (`~/.local/share/openeffects`) and
the `npu.conf` systemd drop-in (with `OE_EP` rewritten to `VINOOPENEFFECTS_EP`) are
migrated automatically on first start of the new daemon/GUI; nothing is
overwritten if the new location already exists.

## How runtime data resolves after install

You do **not** package these by hand — they resolve automatically:

- **Background images.** Compiled into the GUI binary via `include_bytes!`
  (`gui/src/constants.rs`). On first GUI launch `ensure_preset_images()`
  (`gui/src/pages/background.rs`) extracts them to
  `~/.local/share/vinoopeneffects/backgrounds/`. The daemon receives the absolute path
  over D-Bus (`bg_replace.background`). Nothing to install.
- **ML models.** Bundled into the package at build time. `scripts/build-packages.sh`
  downloads + sha256-verifies the 4 ONNX models and installs them to
  `/usr/share/vinoopeneffects/models/<id>/`. The daemon's `search_dirs()`
  (`core/src/inference/registry.rs`) already searches:
  1. `~/.local/share/vinoopeneffects/models` (per-user)
  2. `/usr/share/vinoopeneffects/models` (system — where packages put them)

  So an installed package reports `Capabilities.models_ready = true` immediately,
  offline, with no user step. Model licenses are Apache-2.0 / MIT (redistributable).
- **Config.** Per-user at `~/.config/vinoopeneffects/state.toml` — never packaged.

## Build dependencies

Same as a normal source build; the packages link against these at build time.

| Component | Build deps (pkg-config / headers)                                                                  |
| --------- | -------------------------------------------------------------------------------------------------- |
| daemon    | GStreamer + plugins-base, PipeWire (`libpipewire-0.3`, `libspa-0.2`), `clang`/`libclang` (bindgen) |
| gui       | GTK4, libadwaita, GStreamer                                                                        |
| all       | Rust ≥ 1.92, `cargo`                                                                               |
| packaging | [nfpm](https://nfpm.goreleaser.com), `curl`, `python3`                                             |

ONNX Runtime is **loaded at runtime** (`npu` feature, on by default). Packages ship the
sha256-verified CPU build (`scripts/fetch-ort.sh`, staged into `dist/ort` by
`build-packages.sh`) in `/usr/lib/vinoopeneffects/ort`; the daemon prefers a per-user
OpenVINO build in `~/.local/lib/vinoopeneffects/ort` (`ovfetch install`) when an NPU is
present. Bumping onnxruntime means updating the version and hashes in `fetch-ort.sh`.
`--no-default-features` is the statically linked CPU-only build.

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
3. `ldd` guard — fail if `vinoopeneffects-daemon` dynamically links onnxruntime.
4. Render the `@bindir@` service templates (`data/{dbus-services,systemd}/*.in`)
   into `dist/` with `@bindir@ → /usr/bin`.
5. Stage models: `VINOOPENEFFECTS_MODELS_DEST=dist/models ./scripts/fetch-models.sh`
   (downloads + sha256-verifies into the package staging tree).
6. `nfpm pkg` for each of the 3 configs × each format → `dist/`.

Output (3 packages × N formats):

```
dist/vinoopeneffects-daemon_<ver>_amd64.deb     vinoopeneffects-ctl_<ver>_amd64.deb     vinoopeneffects_<ver>_amd64.deb
dist/vinoopeneffects-daemon-<ver>-1.x86_64.rpm  vinoopeneffects-ctl-<ver>-1.x86_64.rpm  vinoopeneffects-<ver>-1.x86_64.rpm
dist/vinoopeneffects-daemon-<ver>-1-x86_64.pkg.tar.zst  ...ctl...  ...vinoopeneffects...
```

`dist/` is gitignored.

### What lands in each package

**`vinoopeneffects-daemon`** (daemon + everything the pipeline needs):

| Path                                                        | Source                                 |
| ----------------------------------------------------------- | -------------------------------------- |
| `/usr/bin/vinoopeneffects-daemon`                                     | `target/release/vinoopeneffects-daemon`          |
| `/usr/share/dbus-1/interfaces/io.github.karanshukla.vinoopeneffects.*.xml`        | `data/dbus/*.xml`                      |
| `/usr/share/dbus-1/services/io.github.karanshukla.vinoopeneffects.Daemon.service` | rendered template                      |
| `/usr/lib/systemd/user/vinoopeneffects-daemon.service`                | rendered template                      |
| `/usr/share/vinoopeneffects/models/<id>/`                       | `dist/models/` (downloaded + verified) |

**`vinoopeneffects-ctl`** (CLI): `/usr/bin/vinoopeneffects-ctl`.

**`vinoopeneffects`** (GUI):

| Path                                                                     | Source                                                  |
| ------------------------------------------------------------------------ | ------------------------------------------------------- |
| `/usr/bin/vinoopeneffects`                                                   | `target/release/vinoopeneffects`                            |
| `/usr/share/icons/hicolor/scalable/apps/io.github.karanshukla.vinoopeneffects.svg` | `data/icons/io.github.karanshukla.vinoopeneffects.svg`            |
| `/usr/share/applications/io.github.karanshukla.vinoopeneffects.desktop`            | `data/applications/io.github.karanshukla.vinoopeneffects.desktop` |

The GUI package's deb/rpm run `packaging/postinstall.sh` / `postremove.sh` (icon cache +
`update-desktop-database`). Arch handles those via pacman hooks — no scriptlet.

## Install the built packages

The CLI and GUI packages declare `Depends: vinoopeneffects-daemon`. There are two ways the
dependency is satisfied:

**From a repository** — the daemon is in a configured repo, so installing only a
client pulls it in automatically:

```bash
sudo apt install vinoopeneffects          # Debian/Ubuntu — pulls vinoopeneffects-daemon
sudo dnf install vinoopeneffects          # Fedora/RHEL
sudo pacman -S vinoopeneffects            # Arch
```

**From local package files** — pass the daemon file on the same command line so the
resolver sees it. The package managers' file-install mode resolves inter-file deps:

```bash
# Debian/Ubuntu (use apt, not `dpkg -i` — dpkg does NOT resolve deps)
sudo apt install ./dist/vinoopeneffects-daemon_<ver>_amd64.deb ./dist/vinoopeneffects_<ver>_amd64.deb

# Fedora/RHEL
sudo dnf install ./dist/vinoopeneffects-daemon-<ver>-1.x86_64.rpm ./dist/vinoopeneffects-<ver>-1.x86_64.rpm

# Arch (pacman -U resolves deps among the files given, or from the sync db)
sudo pacman -U dist/vinoopeneffects-daemon-<ver>-1-x86_64.pkg.tar.zst dist/vinoopeneffects-<ver>-1-x86_64.pkg.tar.zst
```

Install just `vinoopeneffects-daemon` for a headless/CLI-only setup, or add `vinoopeneffects-ctl`
the same way. Installing a client without the daemon (e.g. `dpkg -i vinoopeneffects.deb`
alone) fails the dependency check — that's the resolver doing its job.

Then enable the daemon (D-Bus-activated; the user unit auto-starts it on demand):

```bash
systemctl --user daemon-reload
systemctl --user enable --now vinoopeneffects-daemon.service   # optional; D-Bus activation works without it
```

## Version bump

Version lives in one place, the workspace version in `Cargo.toml`; all crates
inherit via `version.workspace = true`, and nfpm and `build-packages.sh` read it from
there. One script bumps it:

```bash
./scripts/bump-version.sh 0.2.0
```

It rewrites `Cargo.toml`, runs `cargo update --workspace` to sync `Cargo.lock`,
commits `chore: release v0.2.0`, and tags `v0.2.0`. It does **not** push.

```bash
git push && git push origin v0.2.0   # pushing the tag fires the release CI
```

## Release CI

`.github/workflows/release.yml` triggers on `v*` tags. On `ubuntu-latest` it installs
build deps + Rust + nfpm, runs `scripts/build-packages.sh`, and attaches
`dist/*.{deb,rpm,pkg.tar.zst}` to the GitHub Release via `softprops/action-gh-release`.
nfpm builds the Arch `.zst` fine on Ubuntu — no Arch host required.

## Source install (no packaging)

`scripts/install.sh` builds from the checkout and installs for the current user:

```bash
scripts/install.sh                          # ~/.local: binaries, D-Bus, systemd unit, desktop entry, icons, models
scripts/install.sh --no-gui                 # daemon and CLI only (no GTK needed)
sudo scripts/install.sh --prefix /usr/local # system-wide
scripts/uninstall.sh [--prefix DIR] [--purge]
```

It renders `@bindir@` into the D-Bus activation file and the unit, fetches the
sha256-verified models (`fetch-models.sh`), and asks the bus to reload so D-Bus
activation works immediately. `--purge` also removes models and per-user config.
The `npu` build is not handled here; see #21.

The GUI needs libadwaita 1.7 or newer (`AdwToggleGroup`); on older systems use
`--no-gui`.

## Verifying a package without installing

```bash
# Arch / zst — inspect one package (e.g. the GUI, to confirm it depends on vinoopeneffects-daemon)
tar tf dist/vinoopeneffects-*.pkg.tar.zst | grep usr/
tar xOf dist/vinoopeneffects-*.pkg.tar.zst .PKGINFO | grep depend

# deb
ar p dist/vinoopeneffects_*.deb data.tar.gz | bsdtar tf -                 # file list
ar p dist/vinoopeneffects_*.deb control.tar.gz | bsdtar xOf - ./control   # deps + metadata
```

## Adding architectures

Currently x86_64 only. For aarch64: parameterize `ARCH` in `build-packages.sh`,
cross-compile or use a native runner, and add a CI matrix.
