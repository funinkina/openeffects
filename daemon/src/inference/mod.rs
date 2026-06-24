pub mod engine;
pub mod gesture;
pub mod hand;
pub mod manifest;
pub mod registry;

/// ONNX Runtime execution providers, in PRD §8 priority order
/// (TensorRT > CUDA > ROCm > OpenVINO > CPU). Surfaced to clients via the
/// `Capabilities.ep` D-Bus property for the "Running on: X" GUI badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpKind {
    // Reserved for future GPU EP probing (PRD §8); `probe_ep` only returns
    // `Cpu` today since `download-binaries` ships the CPU EP binary only.
    #[allow(dead_code)]
    TensorRt,
    #[allow(dead_code)]
    Cuda,
    #[allow(dead_code)]
    Rocm,
    #[allow(dead_code)]
    OpenVino,
    Cpu,
}

impl EpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EpKind::TensorRt => "tensorrt",
            EpKind::Cuda => "cuda",
            EpKind::Rocm => "rocm",
            EpKind::OpenVino => "openvino",
            EpKind::Cpu => "cpu",
        }
    }
}

/// Probe the best available ONNX Runtime execution provider.
///
/// Only CPU is selected for now: the bundled models (≤1 MB each, running at
/// video frame rate) do not benefit from GPU EP — H2D/D2H transfer overhead
/// exceeds inference time for models this small. `EpKind` carries the full
/// PRD §8 priority chain so GPU probing can be added later without changing
/// callers.
pub fn probe_ep() -> EpKind {
    EpKind::Cpu
}

/// Initialize the ONNX Runtime environment once at daemon startup, before any
/// model session is created. Returns `true` if this call configured the
/// environment, `false` if one was already committed (e.g. by a prior call).
pub fn init_runtime() -> bool {
    ort::init().with_name("openeffectsd").commit()
}

/// Hardware capability tier (PRD §10.1). Drives model/quality selection and is
/// surfaced to clients via the `Capabilities.tier` D-Bus property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Discrete GPU.
    T1,
    /// Modern integrated GPU (Intel Xe/UHD 11th-gen+, AMD Vega).
    T2,
    /// Older iGPU/APU.
    T3,
    /// No GPU acceleration.
    T4,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::T1 => "t1",
            Tier::T2 => "t2",
            Tier::T3 => "t3",
            Tier::T4 => "t4",
        }
    }

    pub fn from_override(value: &str) -> Option<Tier> {
        match value.trim().to_ascii_lowercase().as_str() {
            "t1" => Some(Tier::T1),
            "t2" => Some(Tier::T2),
            "t3" => Some(Tier::T3),
            "t4" => Some(Tier::T4),
            _ => None,
        }
    }
}

/// Detect the hardware tier from the DRM render nodes (PRD §10.1). A discrete
/// GPU (NVIDIA `nvidia*` node, or a non-Intel/non-virtual render node) maps to
/// T1; an Intel/AMD integrated render node to T2; anything else to T4. This is
/// a coarse heuristic independent of the ONNX EP; it exists so the GUI About
/// page and degradation logic can report a tier.
pub fn detect_tier() -> Tier {
    // Discrete NVIDIA GPU.
    if std::path::Path::new("/proc/driver/nvidia/gpus").exists() {
        return Tier::T1;
    }
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return Tier::T4;
    };
    let mut has_render_node = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("renderD") {
            has_render_node = true;
        }
    }
    if has_render_node {
        // A render node exists (Intel/AMD iGPU or discrete). We can't cheaply
        // tell discrete from integrated here, so treat as a modern iGPU (T2);
        // the user can force T1 via `pipeline.tier_override`.
        Tier::T2
    } else {
        Tier::T4
    }
}
