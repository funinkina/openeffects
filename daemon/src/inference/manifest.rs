// Fields mirror the full PRD §9.1 manifest schema; most are read by the
// model-loading code added in Stage 3/4 (Portrait Blur / Center Stage
// inference), not yet present.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Model manifest, per PRD §9.1: describes a model's I/O tensors, supported
/// execution providers, and downloadable variants (with sha256 for
/// verification by `scripts/fetch-models.sh` and [`ModelManifest::resolve_variant`]).
#[derive(Debug, Clone, Deserialize)]
pub struct ModelManifest {
    pub model: ModelSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelSection {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: String,
    pub input: TensorSpec,
    #[serde(default)]
    pub output: HashMap<String, TensorSpec>,
    pub execution: ExecutionSpec,
    pub variants: Vec<VariantSpec>,
    /// Upstream download URL, used by `scripts/fetch-models.sh`. Not part of
    /// the strict PRD §9.1 schema, but harmless extra metadata.
    #[serde(default)]
    pub source: Option<SourceSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TensorSpec {
    pub name: String,
    pub shape: Vec<i64>,
    #[serde(default)]
    pub layout: String,
    #[serde(default)]
    pub preprocessing: String,
    #[serde(default)]
    pub postprocessing: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionSpec {
    pub supported_eps: Vec<String>,
    #[serde(default)]
    pub min_vram_mb: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantSpec {
    pub name: String,
    pub file: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceSpec {
    pub url: String,
}

impl ModelManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse manifest {}", path.display()))
    }

    /// The first variant whose file exists under `dir` and whose contents
    /// hash to the manifest's recorded sha256.
    pub fn resolve_variant(&self, dir: &Path) -> Option<(&VariantSpec, PathBuf)> {
        self.model.variants.iter().find_map(|variant| {
            let path = dir.join(&variant.file);
            let digest = sha256_hex(&path).ok()?;
            (digest == variant.sha256).then_some((variant, path))
        })
    }
}

pub(crate) fn sha256_hex(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SELFIE_SEG_TOML: &str = r#"
[model]
id = "selfie_segmentation"
name = "MediaPipe Selfie Segmentation"
version = "1"
license = "Apache-2.0"

[model.input]
name = "input_1"
shape = [1, 256, 256, 3]
layout = "NHWC"
preprocessing = "scale_0_1"

[model.output.mask]
name = "activation_10"
shape = [1, 256, 256, 1]
postprocessing = "sigmoid"

[model.execution]
supported_eps = ["cpu"]
min_vram_mb = 0

[[model.variants]]
name = "fp32"
file = "selfie_segmentation.onnx"
sha256 = "deadbeef"

[model.source]
url = "https://example.invalid/selfie_segmentation.onnx"
"#;

    const YUNET_TOML: &str = r#"
[model]
id = "yunet"
name = "YuNet Face Detector"
version = "2023mar"
license = "MIT"

[model.input]
name = "input"
shape = [1, 3, 120, 160]
layout = "NCHW"
preprocessing = "none"

[model.output.cls_8]
name = "cls_8"
shape = [1, 1200, 1]
postprocessing = "sigmoid"

[model.output.bbox_8]
name = "bbox_8"
shape = [1, 1200, 4]
postprocessing = "decode_bbox"

[model.execution]
supported_eps = ["cpu"]
min_vram_mb = 0

[[model.variants]]
name = "fp32"
file = "face_detection_yunet_2023mar.onnx"
sha256 = "cafebabe"
"#;

    #[test]
    fn parses_selfie_seg_manifest() {
        let manifest: ModelManifest = toml::from_str(SELFIE_SEG_TOML).unwrap();
        assert_eq!(manifest.model.id, "selfie_segmentation");
        assert_eq!(manifest.model.input.shape, vec![1, 256, 256, 3]);
        assert_eq!(manifest.model.output["mask"].postprocessing, "sigmoid");
        assert_eq!(manifest.model.variants[0].file, "selfie_segmentation.onnx");
        assert_eq!(
            manifest.model.source.unwrap().url,
            "https://example.invalid/selfie_segmentation.onnx"
        );
    }

    #[test]
    fn parses_multi_output_yunet_manifest() {
        let manifest: ModelManifest = toml::from_str(YUNET_TOML).unwrap();
        assert_eq!(manifest.model.output.len(), 2);
        assert_eq!(manifest.model.output["bbox_8"].shape, vec![1, 1200, 4]);
        assert_eq!(manifest.model.input.layout, "NCHW");
    }

    #[test]
    fn resolve_variant_returns_none_on_sha_mismatch() {
        let manifest: ModelManifest = toml::from_str(SELFIE_SEG_TOML).unwrap();
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("selfie_segmentation.onnx"), b"hello").unwrap();

        assert!(manifest.resolve_variant(dir.path()).is_none());
    }

    #[test]
    fn resolve_variant_returns_none_on_missing_file() {
        let manifest: ModelManifest = toml::from_str(SELFIE_SEG_TOML).unwrap();
        let dir = TempDir::new().unwrap();

        assert!(manifest.resolve_variant(dir.path()).is_none());
    }

    #[test]
    fn resolve_variant_returns_match_on_sha_match() {
        let mut manifest: ModelManifest = toml::from_str(SELFIE_SEG_TOML).unwrap();
        let dir = TempDir::new().unwrap();
        let model_path = dir.path().join("selfie_segmentation.onnx");
        std::fs::write(&model_path, b"hello").unwrap();
        manifest.model.variants[0].sha256 = sha256_hex(&model_path).unwrap();

        let (variant, path) = manifest.resolve_variant(dir.path()).unwrap();
        assert_eq!(variant.name, "fp32");
        assert_eq!(path, model_path);
    }

    #[test]
    fn load_reads_manifest_from_disk() {
        let dir = TempDir::new().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, SELFIE_SEG_TOML).unwrap();

        let manifest = ModelManifest::load(&manifest_path).unwrap();
        assert_eq!(manifest.model.id, "selfie_segmentation");
    }
}
