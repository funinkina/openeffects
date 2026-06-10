use std::path::PathBuf;

use super::manifest::ModelManifest;

/// A model whose manifest parsed and whose variant file is present and
/// sha256-verified.
// Consumed by the Stage 3/4 model loaders, not yet present.
#[allow(dead_code)]
pub struct ResolvedModel {
    pub manifest: ModelManifest,
    pub model_path: PathBuf,
}

/// Model search directories, in PRD §9.3 priority order: per-user data dir,
/// system-wide data dir, then the Flatpak app mount when sandboxed.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data_dir) = dirs::data_dir() {
        dirs.push(data_dir.join("openeffects/models"));
    }
    dirs.push(PathBuf::from("/usr/share/openeffects/models"));
    if std::env::var_os("FLATPAK_ID").is_some() {
        dirs.push(PathBuf::from("/app/share/openeffects/models"));
    }
    dirs
}

/// Find `<dir>/<id>/manifest.toml` across `dirs`, returning the first whose
/// manifest parses and resolves a variant in the same directory.
pub fn find_model_in(dirs: &[PathBuf], id: &str) -> Option<ResolvedModel> {
    for dir in dirs {
        let model_dir = dir.join(id);
        let Ok(manifest) = ModelManifest::load(&model_dir.join("manifest.toml")) else {
            continue;
        };
        if let Some((_, model_path)) = manifest.resolve_variant(&model_dir) {
            return Some(ResolvedModel {
                manifest,
                model_path,
            });
        }
    }
    None
}

/// Find a model by id across the standard search directories ([`search_dirs`]).
pub fn find_model(id: &str) -> Option<ResolvedModel> {
    find_model_in(&search_dirs(), id)
}

#[cfg(test)]
mod tests {
    use super::super::manifest::sha256_hex;
    use super::*;
    use tempfile::TempDir;

    const MANIFEST_TOML: &str = r#"
[model]
id = "selfie_segmentation"
name = "Test"
version = "1"
license = "Apache-2.0"

[model.input]
name = "input_1"
shape = [1, 256, 256, 3]
layout = "NHWC"
preprocessing = "scale_0_1"

[model.execution]
supported_eps = ["cpu"]
min_vram_mb = 0

[[model.variants]]
name = "fp32"
file = "model.onnx"
sha256 = "REPLACED"
"#;

    fn write_resolvable_model(dir: &std::path::Path, id: &str) -> PathBuf {
        let model_dir = dir.join(id);
        std::fs::create_dir_all(&model_dir).unwrap();
        let model_path = model_dir.join("model.onnx");
        std::fs::write(&model_path, b"hello").unwrap();
        let digest = sha256_hex(&model_path).unwrap();
        std::fs::write(
            model_dir.join("manifest.toml"),
            MANIFEST_TOML.replace("REPLACED", &digest),
        )
        .unwrap();
        model_path
    }

    #[test]
    fn finds_model_with_valid_manifest_and_matching_file() {
        let dir = TempDir::new().unwrap();
        let model_path = write_resolvable_model(dir.path(), "selfie_segmentation");

        let resolved = find_model_in(&[dir.path().to_path_buf()], "selfie_segmentation").unwrap();
        assert_eq!(resolved.manifest.model.id, "selfie_segmentation");
        assert_eq!(resolved.model_path, model_path);
    }

    #[test]
    fn missing_manifest_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(find_model_in(&[dir.path().to_path_buf()], "selfie_segmentation").is_none());
    }

    #[test]
    fn falls_through_to_second_dir_when_first_unresolved() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let model_path = write_resolvable_model(dir2.path(), "selfie_segmentation");

        let resolved = find_model_in(
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
            "selfie_segmentation",
        )
        .unwrap();
        assert_eq!(resolved.model_path, model_path);
    }
}
