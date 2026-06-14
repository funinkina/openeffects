# Flatpak packaging

A single sandboxed app (`org.openeffects.OpenEffects`) bundling the **GUI + daemon**
as one tightly-coupled unit. This is the Flathub-targeted build.

The **CLI (`openeffectsctl`) is intentionally not shipped** in the Flatpak — it's a
scripting/WM-styling tool that belongs in the native package. The sandbox ships
only the GUI and the daemon it controls.

## Files

| File                                       | Purpose                                                          |
| ------------------------------------------ | ---------------------------------------------------------------- |
| `org.openeffects.OpenEffects.yaml`         | flatpak-builder manifest                                         |
| `org.openeffects.OpenEffects.metainfo.xml` | AppStream metadata (Flathub-required)                            |
| `openeffects-launcher.sh`                  | starts the daemon, then runs the GUI (no systemd in the sandbox) |
| `Cargo.lock`                               | lockfile resolved for the `load-dynamic` ort build               |
| `cargo-sources.json`                       | vendored crate sources for the offline build                     |
| `generate-cargo-sources.sh`                | regenerates `Cargo.lock` + `cargo-sources.json`                  |

## Build & test locally

```bash
flatpak install -y flathub org.gnome.Platform//50 org.gnome.Sdk//50 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08 \
    org.freedesktop.Sdk.Extension.llvm20//25.08    # 25.08 = GNOME 50's freedesktop base

cd packaging/flatpak
flatpak-builder --user --install --force-clean build-dir org.openeffects.OpenEffects.yaml

flatpak run org.openeffects.OpenEffects                                # GUI (+ daemon)
flatpak run --command=openeffectsd org.openeffects.OpenEffects --start # headless daemon only
```

The `type: dir` source builds from the working tree, so local rebuilds pick up
your changes with no commit needed.

## How the hard parts are solved

- **Offline build.** Flathub builds have no network during build steps. Crates
  are vendored (`cargo-sources.json`); ONNX Runtime and the ML models are
  declared `archive`/`file` sources (url + sha256), which flatpak-builder fetches
  before the offline phase.
- **ONNX Runtime.** The repo links it statically via ort's `download-binaries`,
  which downloads at build time — impossible offline. The manifest switches ort
  to `load-dynamic` (`sed` on `Cargo.toml`, plus the matching `Cargo.lock` here)
  and ships Microsoft's prebuilt `libonnxruntime.so`, pointed at by
  `ORT_DYLIB_PATH`. **CPU execution provider only** — see "GPU" below.
- **Camera (`org.freedesktop.portal.Camera`).** In the sandbox the daemon calls
  the camera portal (`AccessCamera` + `OpenPipeWireRemote`) and feeds the returned
  PipeWire fd to `pipewiresrc`, so the user gets a proper camera-permission prompt
  and the shell's camera indicator. Gated on `FLATPAK_ID` — native installs keep
  capturing directly. See `daemon/src/portal.rs`.
- **Background app (`org.freedesktop.portal.Background`).** The daemon is the
  persistent process: it calls `SetStatus`, so after the GUI window closes it
  shows up in GNOME Shell's **Background Apps** menu and keeps applying effects.
  The shell's "Quit" there sends `SIGTERM`, which the daemon handles for a clean
  shutdown (tearing down the sandbox).
- **No systemd.** The daemon is started by `openeffects-launcher.sh` (it does
  **not** kill it on GUI exit). The GUI's systemd autostart toggle is replaced by
  an explanatory row in the sandbox.
- **Models.** All four ONNX models (~8 MB) are bundled into
  `/app/share/openeffects/models`, which `registry::search_dirs()` already
  searches under `FLATPAK_ID`. No runtime network, consistent with the app's
  "no network calls" privacy claim.

## GPU acceleration

The Flatpak is **CPU-only** (XNNPACK). The models are ≤4 MB and meet the
sub-50ms budget on CPU. GPU execution providers were evaluated and rejected for
the Flathub build:

| EP                   | Feasible in Flatpak? | Why not shipped                                                                                      |
| -------------------- | -------------------- | ---------------------------------------------------------------------------------------------------- |
| NVIDIA CUDA/TensorRT | Technically yes      | +~2 GB bundled CUDA/cuDNN, NVIDIA-only, fragile vs host driver, cuDNN redistribution review friction |
| AMD ROCm             | Barely               | Multi-GB, version-locked to host amdgpu                                                              |
| Intel OpenVINO       | Yes (lighter)        | Hundreds of MB, Intel-only                                                                           |
| Vulkan               | No                   | ONNX Runtime has no production Vulkan EP                                                             |

For GPU acceleration, use the native distro packages. If a single-vendor GPU
build is later wanted, NVIDIA CUDA (via the `org.freedesktop.Platform.GL.nvidia-*`
extension + bundled CUDA libs) is the only path worth its size.

## Regenerating Cargo inputs

After any dependency change in the repo, refresh both lock + sources:

```bash
./generate-cargo-sources.sh
```

It patches a throwaway copy to `load-dynamic`, re-resolves the lockfile, writes
`Cargo.lock` + `cargo-sources.json`, and restores the repo's pristine files.

## Before submitting to Flathub

1. **Screenshot.** `metainfo.xml` points at a placeholder
   (`docs/screenshots/main.png`). Commit a real PNG/JPG there (or host it) — a
   working screenshot URL is mandatory.
2. **Pin the source.** Replace the `type: dir` source with a tagged release:
   ```yaml
   - type: git
     url: https://github.com/funinkina/openeffects.git
     tag: v0.1.2
     commit: <full 40-char sha>
   ```
3. **Runtime version.** Bump `runtime-version` (and the SDK-extension branches in
   the install command above) to the GNOME runtime Flathub currently ships.
4. **Validate.**
   ```bash
   appstreamcli validate org.openeffects.OpenEffects.metainfo.xml
   desktop-file-validate ../../data/applications/org.openeffects.OpenEffects.desktop
   flatpak run org.flatpak.Builder --show-manifest org.openeffects.OpenEffects.yaml
   ```
   Flathub CI also runs `flatpak-builder-lint manifest` and `...lint repo`.
5. **Submit.** Open a PR adding `org.openeffects.OpenEffects.yaml` (+ this dir's
   sources) to the [`flathub/flathub`](https://github.com/flathub/flathub)
   `new-pr` branch. See <https://docs.flathub.org/docs/for-app-authors/submission>.

## Why `--socket=pipewire` (and not just the Camera portal)

OpenEffects **creates** a virtual PipeWire camera node — that's the whole point.
The Camera portal only grants restricted *read* access to existing camera nodes;
it cannot create nodes. So the full PipeWire socket is required for the output
side regardless. Capture still goes through the Camera portal (above) for the
permission prompt + indicator; the socket is justified to Flathub by the
virtual-output requirement.

## Known limitations

- **Virtual camera is PipeWire-only.** `v4l2loopback` can't be created from the
  sandbox, so consumers must read the PipeWire camera node (most modern apps do;
  some legacy `/dev/video*`-only apps won't see it).
- **CPU inference only** (see GPU section). For GPU, use the native packages.
- **Camera picker selection is best-effort in the sandbox.** Capture uses the
  portal's granted camera; with multiple cameras the portal default is used.
- **Needs on-device testing.** The portal flows (Camera, Background) can't be
  validated outside a real sandbox + desktop session — verify them on first build.
