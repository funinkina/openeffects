pub mod bench;
pub mod engine;
pub mod gesture;
pub mod hand;
pub mod manifest;
pub mod registry;

const INTRA_THREADS: usize = 2;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once the NPU/GPU path has failed in `auto` mode, so later model loads go
/// straight to CPU instead of paying for another failed compile each.
static ACCEL_FAILED: AtomicBool = AtomicBool::new(false);

/// A session builder with the shared CPU settings and no accelerator attached.
fn base_builder() -> anyhow::Result<ort::session::builder::SessionBuilder> {
    use ort::memory::{AllocationDevice, AllocatorType, MemoryInfo, MemoryType};
    ensure_runtime()?;
    let cpu = MemoryInfo::new(
        AllocationDevice::CPU,
        0,
        AllocatorType::Device,
        MemoryType::Default,
    )
    .map_err(|e| anyhow::anyhow!("create CPU memory info: {e}"))?;
    ort::session::Session::builder()
        .map_err(|e| anyhow::anyhow!("create ORT session builder: {e}"))?
        .with_intra_threads(INTRA_THREADS)
        .map_err(|e| anyhow::anyhow!("set intra-op threads: {e}"))?
        .with_intra_op_spinning(false)
        .map_err(|e| anyhow::anyhow!("disable intra-op spinning: {e}"))?
        .with_memory_pattern(false)
        .map_err(|e| anyhow::anyhow!("disable memory pattern: {e}"))?
        .with_allocator(cpu)
        .map_err(|e| anyhow::anyhow!("set non-arena CPU allocator: {e}"))
}

pub fn build_session() -> anyhow::Result<ort::session::builder::SessionBuilder> {
    with_execution_provider(base_builder()?)
}

/// Load `model` on the requested execution provider. In `auto` mode a failure
/// on the NPU (driver, compile, unsupported op) falls back to CPU for this and
/// every later model; an explicit `npu`/`gpu`/`cpu` request never falls back.
pub fn load_session(model: &Path) -> anyhow::Result<ort::session::Session> {
    use anyhow::Context;
    let load = |mut builder: ort::session::builder::SessionBuilder| {
        builder
            .commit_from_file(model)
            .with_context(|| format!("load {}", model.display()))
    };
    if requested_ep() != "auto" {
        return load(build_session()?);
    }
    if !ACCEL_FAILED.load(Ordering::Relaxed)
        && let Ok(session) = build_session().and_then(load)
    {
        return Ok(session);
    }
    if !ACCEL_FAILED.swap(true, Ordering::Relaxed) {
        tracing::warn!("NPU/GPU unavailable for {}; using CPU", model.display());
    }
    load(base_builder()?)
}

/// Execution provider selected by `VINOOPENEFFECTS_EP`: `auto` (default: the
/// NPU when one is present and usable, otherwise CPU), `cpu`, `npu`, or `gpu`.
/// An explicit `npu`/`gpu` is strict: it errors rather than silently running
/// on CPU. Accelerators need the `npu` feature and an OpenVINO-enabled
/// onnxruntime (see [`ensure_runtime`]).
pub fn requested_ep() -> String {
    std::env::var("VINOOPENEFFECTS_EP")
        .map(|v| v.trim().to_ascii_lowercase())
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "auto".into())
}

#[cfg(feature = "npu")]
/// Whether the kernel exposes an Intel NPU (`intel_vpu` registers `accel` nodes).
fn npu_present() -> bool {
    std::fs::read_dir("/dev/accel")
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("accel"))
        })
        .unwrap_or(false)
}

/// Directories searched for `libonnxruntime.so` when `ORT_DYLIB_PATH` is unset,
/// most specific first: a per-user OpenVINO build (`ovfetch install --prefix
/// ~/.local/lib/vinoopeneffects/ort`), the prefix this binary was installed
/// under, then the packaged CPU runtime.
#[cfg(feature = "npu")]
fn runtime_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/lib/vinoopeneffects/ort"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(Path::parent)
    {
        dirs.push(prefix.join("lib/vinoopeneffects/ort"));
        dirs.push(prefix.join("lib64/vinoopeneffects/ort"));
    }
    dirs.push("/usr/lib/vinoopeneffects/ort".into());
    dirs.push("/usr/lib64/vinoopeneffects/ort".into());
    dirs
}

/// Load the onnxruntime shared library once, failing with a message instead of
/// ort's panic when none can be found.
#[cfg(feature = "npu")]
fn ensure_runtime() -> anyhow::Result<()> {
    use std::path::PathBuf;
    use std::sync::OnceLock;
    static LOADED: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let loaded = LOADED.get_or_init(|| {
        let path = match std::env::var_os("ORT_DYLIB_PATH").filter(|v| !v.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => runtime_dirs()
                .into_iter()
                .map(|d| d.join("libonnxruntime.so"))
                .find(|p| p.exists())
                .unwrap_or_else(|| "libonnxruntime.so".into()),
        };
        match ort::init_from(&path) {
            Ok(builder) => {
                builder.commit();
                tracing::info!(runtime = %path.display(), "loaded onnxruntime");
                Ok(path)
            }
            Err(e) => Err(format!(
                "cannot load onnxruntime ({}): {e}; set ORT_DYLIB_PATH or install libonnxruntime.so \
                 to ~/.local/lib/vinoopeneffects/ort",
                path.display()
            )),
        }
    });
    loaded
        .as_ref()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(not(feature = "npu"))]
fn ensure_runtime() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(feature = "npu")]
fn with_execution_provider(
    builder: ort::session::builder::SessionBuilder,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    use ort::ep::{ExecutionProvider, OpenVINO};

    let requested = requested_ep();
    let (device, strict) = match requested.as_str() {
        "cpu" => return Ok(builder),
        "auto" => {
            if ACCEL_FAILED.load(Ordering::Relaxed) || !npu_present() {
                return Ok(builder);
            }
            ("NPU", false)
        }
        "npu" => ("NPU", true),
        "gpu" => ("GPU", true),
        other => anyhow::bail!("VINOOPENEFFECTS_EP={other:?}: expected auto, cpu, npu or gpu"),
    };
    let ep = OpenVINO::default().with_device_type(device);
    if !ep.is_available().unwrap_or(false) {
        // The CPU-only runtime has no OpenVINO EP: fine for auto, and the whole
        // point of the strict modes otherwise (a CPU run must not pass as NPU).
        anyhow::ensure!(
            !strict,
            "OpenVINO execution provider unavailable (is ORT_DYLIB_PATH an onnxruntime-openvino build?)"
        );
        ACCEL_FAILED.store(true, Ordering::Relaxed);
        tracing::info!("onnxruntime has no OpenVINO EP; using CPU");
        return Ok(builder);
    }
    let registered = if strict {
        ep.build().error_on_failure()
    } else {
        ep.build()
    };
    builder
        .with_execution_providers([registered])
        .map_err(|e| anyhow::anyhow!("register OpenVINO EP ({device}): {e}"))
}

#[cfg(not(feature = "npu"))]
fn with_execution_provider(
    builder: ort::session::builder::SessionBuilder,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    match requested_ep().as_str() {
        "cpu" | "auto" => Ok(builder),
        other => anyhow::bail!("VINOOPENEFFECTS_EP={other:?} needs a build with --features npu"),
    }
}
