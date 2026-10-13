pub mod hand;
pub mod manifest;
pub mod models;
pub mod registry;

const INTRA_THREADS: usize = 2;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once the NPU/GPU path has failed in `auto` mode, so later model loads go
/// straight to CPU instead of paying for another failed compile each.
static ACCEL_FAILED: AtomicBool = AtomicBool::new(false);

pub fn reset_accel_failed() {
    ACCEL_FAILED.store(false, Ordering::Relaxed);
}

#[derive(Debug, Clone)]
pub struct NpuProbe {
    pub available: bool,
    pub reason: Option<String>,
}

#[cfg(feature = "npu")]
pub fn probe_npu() -> NpuProbe {
    use ort::ep::ExecutionProvider;
    if !npu_present() {
        return NpuProbe {
            available: false,
            reason: Some("no Intel NPU hardware detected (/dev/accel has no accel nodes)".into()),
        };
    }
    if let Err(e) = ensure_runtime() {
        return NpuProbe {
            available: false,
            reason: Some(format!("onnxruntime library unavailable: {e}")),
        };
    }
    let ep = ort::ep::OpenVINO::default().with_device_type("NPU");
    if !ep.is_available().unwrap_or(false) {
        return NpuProbe {
            available: false,
            reason: Some("onnxruntime has no OpenVINO execution provider".into()),
        };
    }
    NpuProbe {
        available: true,
        reason: None,
    }
}

#[cfg(not(feature = "npu"))]
pub fn probe_npu() -> NpuProbe {
    NpuProbe {
        available: false,
        reason: Some("built without `npu` feature (CPU-only build)".into()),
    }
}

static PROBED_NPU: std::sync::OnceLock<NpuProbe> = std::sync::OnceLock::new();

pub fn probe_npu_cached() -> &'static NpuProbe {
    PROBED_NPU.get_or_init(probe_npu)
}

pub fn available_providers() -> Vec<&'static str> {
    if probe_npu_cached().available {
        vec!["cpu", "npu"]
    } else {
        vec!["cpu"]
    }
}

pub fn env_ep() -> Option<String> {
    std::env::var("VINOOPENEFFECTS_EP")
        .map(|v| v.trim().to_ascii_lowercase())
        .ok()
        .filter(|v| !v.is_empty())
}

pub fn requested_ep() -> String {
    env_ep().unwrap_or_else(|| "auto".into())
}

pub fn effective_ep(configured: &str) -> String {
    env_ep().unwrap_or_else(|| configured.trim().to_ascii_lowercase())
}

/// The provider models will land on for `provider` before any is loaded, plus
/// why it differs from the request (an `npu` request with no usable NPU).
pub fn predicted_active(provider: &str) -> (&'static str, Option<String>) {
    let probe = probe_npu_cached();
    match provider {
        "npu" | "auto" if probe.available => ("npu", None),
        "npu" => ("cpu", probe.reason.clone()),
        "gpu" => ("gpu", None),
        _ => ("cpu", None),
    }
}

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

pub fn build_session(provider: &str) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    with_execution_provider(base_builder()?, provider)
}

pub struct LoadedSession {
    pub session: ort::session::Session,
    pub active_ep: &'static str,
    pub fallback: Option<String>,
}

/// Load `model` on the requested execution provider (`auto`, `cpu`, `npu`, `gpu`).
/// In `auto` mode, an NPU compile failure falls back to CPU. An explicit `npu`
/// with no usable NPU runs on CPU and reports why in `fallback` (the request is
/// kept, so it takes effect once the NPU is there); an NPU that is present but
/// fails to compile the model is an error, as is any explicit `gpu` failure.
pub fn load_session(model: &Path, provider: &str) -> anyhow::Result<LoadedSession> {
    use anyhow::Context;
    let load = |mut builder: ort::session::builder::SessionBuilder| {
        builder
            .commit_from_file(model)
            .with_context(|| format!("load {}", model.display()))
    };

    let target = provider.trim().to_ascii_lowercase();
    match target.as_str() {
        "cpu" => {
            let session = load(base_builder()?)?;
            Ok(LoadedSession {
                session,
                active_ep: "cpu",
                fallback: None,
            })
        }
        "npu" if !probe_npu_cached().available => {
            let session = load(base_builder()?)?;
            Ok(LoadedSession {
                session,
                active_ep: "cpu",
                fallback: probe_npu_cached().reason.clone(),
            })
        }
        "npu" => {
            let session = load(build_session("npu")?)?;
            Ok(LoadedSession {
                session,
                active_ep: "npu",
                fallback: None,
            })
        }
        "gpu" => {
            let session = load(build_session("gpu")?)?;
            Ok(LoadedSession {
                session,
                active_ep: "gpu",
                fallback: None,
            })
        }
        "auto" => {
            if !ACCEL_FAILED.load(Ordering::Relaxed) && probe_npu_cached().available {
                match build_session("npu").and_then(load) {
                    Ok(session) => {
                        return Ok(LoadedSession {
                            session,
                            active_ep: "npu",
                            fallback: None,
                        });
                    }
                    Err(err) => {
                        let msg = format!("NPU compile failed for {}: {err}", model.display());
                        if !ACCEL_FAILED.swap(true, Ordering::Relaxed) {
                            tracing::warn!("{msg}; falling back to CPU");
                        }
                        let session = load(base_builder()?)?;
                        return Ok(LoadedSession {
                            session,
                            active_ep: "cpu",
                            fallback: Some(msg),
                        });
                    }
                }
            }
            let session = load(base_builder()?)?;
            Ok(LoadedSession {
                session,
                active_ep: "cpu",
                fallback: None,
            })
        }
        other => {
            anyhow::bail!("unknown execution provider '{other}': expected auto, cpu, npu or gpu")
        }
    }
}

/// Load `model` on the NPU or fail: for models that are only usable there
/// (too slow on CPU), where CPU fallback would be a trap.
#[cfg(feature = "npu")]
pub fn load_session_npu(model: &Path, provider: &str) -> anyhow::Result<LoadedSession> {
    use anyhow::Context;
    use ort::ep::{ExecutionProvider, OpenVINO};

    let target = provider.trim().to_ascii_lowercase();
    anyhow::ensure!(
        matches!(target.as_str(), "auto" | "npu"),
        "needs the NPU, but provider is {target}"
    );
    let probe = probe_npu_cached();
    anyhow::ensure!(
        probe.available,
        "NPU unavailable: {}",
        probe.reason.as_deref().unwrap_or("unknown reason")
    );
    ensure_runtime()?;
    let ep = OpenVINO::default().with_device_type("NPU");
    anyhow::ensure!(
        ep.is_available().unwrap_or(false),
        "onnxruntime has no OpenVINO execution provider"
    );
    let mut builder = base_builder()?
        .with_execution_providers([ep.build().error_on_failure()])
        .map_err(|e| anyhow::anyhow!("register OpenVINO EP (NPU): {e}"))?;
    let session = builder
        .commit_from_file(model)
        .with_context(|| format!("load {} on the NPU", model.display()))?;
    Ok(LoadedSession {
        session,
        active_ep: "npu",
        fallback: None,
    })
}

#[cfg(not(feature = "npu"))]
pub fn load_session_npu(_model: &Path, _provider: &str) -> anyhow::Result<LoadedSession> {
    anyhow::bail!("needs a build with --features npu")
}

#[cfg(feature = "npu")]
/// Whether the kernel exposes an Intel NPU (`intel_vpu` registers `accel` nodes).
pub(crate) fn npu_present() -> bool {
    std::fs::read_dir("/dev/accel")
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("accel"))
        })
        .unwrap_or(false)
}

#[cfg(not(feature = "npu"))]
pub(crate) fn npu_present() -> bool {
    false
}

/// Directories searched for `libonnxruntime.so` when `ORT_DYLIB_PATH` is unset,
/// most specific first: user data directory, user local lib, executable prefix,
/// and system library paths.
#[cfg(feature = "npu")]
fn runtime_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("vinoopeneffects/ort"));
    }
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
    dirs.push("/usr/local/lib/vinoopeneffects/ort".into());
    dirs
}

/// Load the onnxruntime shared library once, failing with a message instead of
/// ort's panic when none can be found.
#[cfg(feature = "npu")]
pub(crate) fn ensure_runtime() -> anyhow::Result<()> {
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
pub(crate) fn ensure_runtime() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(feature = "npu")]
fn with_execution_provider(
    builder: ort::session::builder::SessionBuilder,
    requested: &str,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    use ort::ep::{ExecutionProvider, OpenVINO};

    let (device, strict) = match requested {
        "cpu" => return Ok(builder),
        "auto" => {
            if ACCEL_FAILED.load(Ordering::Relaxed) || !probe_npu_cached().available {
                return Ok(builder);
            }
            ("NPU", false)
        }
        "npu" => {
            let probe = probe_npu_cached();
            if !probe.available {
                anyhow::bail!(
                    "NPU requested but unavailable: {}",
                    probe.reason.as_deref().unwrap_or("unknown error")
                );
            }
            ("NPU", true)
        }
        "gpu" => ("GPU", true),
        other => {
            anyhow::bail!("unknown execution provider '{other}': expected auto, cpu, npu or gpu")
        }
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
    requested: &str,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    match requested {
        "cpu" | "auto" => Ok(builder),
        other => anyhow::bail!("execution provider '{other}' needs a build with --features npu"),
    }
}
