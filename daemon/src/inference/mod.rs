pub mod bench;
pub mod engine;
pub mod gesture;
pub mod hand;
pub mod manifest;
pub mod registry;

const INTRA_THREADS: usize = 2;

pub fn build_session() -> anyhow::Result<ort::session::builder::SessionBuilder> {
    use ort::memory::{AllocationDevice, AllocatorType, MemoryInfo, MemoryType};
    let cpu = MemoryInfo::new(
        AllocationDevice::CPU,
        0,
        AllocatorType::Device,
        MemoryType::Default,
    )
    .map_err(|e| anyhow::anyhow!("create CPU memory info: {e}"))?;
    let builder = ort::session::Session::builder()
        .map_err(|e| anyhow::anyhow!("create ORT session builder: {e}"))?
        .with_intra_threads(INTRA_THREADS)
        .map_err(|e| anyhow::anyhow!("set intra-op threads: {e}"))?
        .with_intra_op_spinning(false)
        .map_err(|e| anyhow::anyhow!("disable intra-op spinning: {e}"))?
        .with_memory_pattern(false)
        .map_err(|e| anyhow::anyhow!("disable memory pattern: {e}"))?
        .with_allocator(cpu)
        .map_err(|e| anyhow::anyhow!("set non-arena CPU allocator: {e}"))?;
    with_execution_provider(builder)
}

/// Execution provider selected by `OE_EP`: `cpu` (default), `npu`, or `gpu`.
/// Anything but `cpu` needs a build with `--features npu` and an
/// `ORT_DYLIB_PATH` pointing at an OpenVINO-enabled onnxruntime (see ovfetch).
pub fn requested_ep() -> String {
    std::env::var("OE_EP")
        .map(|v| v.trim().to_ascii_lowercase())
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "cpu".into())
}

#[cfg(feature = "npu")]
fn with_execution_provider(
    builder: ort::session::builder::SessionBuilder,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    use ort::ep::{ExecutionProvider, OpenVINO};

    let device = match requested_ep().as_str() {
        "cpu" => return Ok(builder),
        "npu" => "NPU",
        "gpu" => "GPU",
        other => anyhow::bail!("OE_EP={other:?}: expected cpu, npu or gpu"),
    };
    let ep = OpenVINO::default().with_device_type(device);
    // OpenVINO silently drops to CPU when it can't register, which would make a
    // CPU run look like an NPU run; refuse instead.
    anyhow::ensure!(
        ep.is_available().unwrap_or(false),
        "OpenVINO execution provider unavailable (is ORT_DYLIB_PATH an onnxruntime-openvino build?)"
    );
    builder
        .with_execution_providers([ep.build().error_on_failure()])
        .map_err(|e| anyhow::anyhow!("register OpenVINO EP ({device}): {e}"))
}

#[cfg(not(feature = "npu"))]
fn with_execution_provider(
    builder: ort::session::builder::SessionBuilder,
) -> anyhow::Result<ort::session::builder::SessionBuilder> {
    match requested_ep().as_str() {
        "cpu" => Ok(builder),
        other => anyhow::bail!("OE_EP={other:?} needs a build with --features npu"),
    }
}

