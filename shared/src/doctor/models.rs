use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::types::{Check, Status};

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestToml {
    pub model: ModelHeader,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelHeader {
    pub id: String,
    pub variants: Vec<VariantHeader>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantHeader {
    pub name: String,
    pub file: String,
    pub sha256: String,
}

pub struct ModelDef {
    pub id: &'static str,
    pub name: &'static str,
    pub required: bool,
}

pub const KNOWN_MODELS: &[ModelDef] = &[
    ModelDef {
        id: "selfie_segmentation",
        name: "MediaPipe Selfie Segmentation",
        required: true,
    },
    ModelDef {
        id: "yunet",
        name: "YuNet Face Detector",
        required: true,
    },
    ModelDef {
        id: "palm_detection",
        name: "MediaPipe Palm Detection (Reactions)",
        required: false,
    },
    ModelDef {
        id: "hand_landmark",
        name: "MediaPipe Hand Landmark (Reactions)",
        required: false,
    },
    ModelDef {
        id: "iat_lowlight",
        name: "IAT Low-Light Enhancer (Studio Light)",
        required: false,
    },
];

pub fn model_verdict(
    id: &str,
    name: &str,
    required: bool,
    manifest_found: bool,
    file_found: bool,
    sha_matches: Option<bool>,
    model_path: Option<&Path>,
) -> Check {
    let check_name = id.to_string();
    if !manifest_found {
        if required {
            return Check::new(
                "models",
                check_name,
                Status::Fail,
                format!("{name} ({id}): manifest.toml missing"),
                Some("run `scripts/fetch-models.sh`".into()),
            );
        } else {
            return Check::new(
                "models",
                check_name,
                Status::Info,
                format!("{name} ({id}): not installed (optional)"),
                None,
            );
        }
    }

    if !file_found {
        if required {
            return Check::new(
                "models",
                check_name,
                Status::Fail,
                format!("{name} ({id}): model file missing"),
                Some("run `scripts/fetch-models.sh`".into()),
            );
        } else {
            return Check::new(
                "models",
                check_name,
                Status::Info,
                format!("{name} ({id}): model file not installed (optional)"),
                None,
            );
        }
    }

    match sha_matches {
        Some(true) => {
            let path_str = model_path
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| id.to_string());
            Check::new(
                "models",
                check_name,
                Status::Ok,
                format!("{path_str} verified"),
                None,
            )
        }
        Some(false) => Check::new(
            "models",
            check_name,
            Status::Fail,
            format!("{name} ({id}): SHA256 checksum mismatch (corrupted or modified)"),
            Some("re-fetch model with `scripts/fetch-models.sh`".into()),
        ),
        None => Check::new(
            "models",
            check_name,
            Status::Warn,
            format!("{name} ({id}): model file exists but manifest has no sha256 checksum"),
            None,
        ),
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
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
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn model_search_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = home {
        dirs.push(h.join(".local/share/vinoopeneffects/models"));
    }
    dirs.push(PathBuf::from("/usr/share/vinoopeneffects/models"));
    dirs.push(PathBuf::from("/usr/local/share/vinoopeneffects/models"));
    dirs
}

pub fn probe_model(search_dirs: &[PathBuf], id: &str) -> (bool, bool, Option<bool>, Option<PathBuf>) {
    for base in search_dirs {
        let model_dir = base.join(id);
        let manifest_path = model_dir.join("manifest.toml");
        if !manifest_path.exists() {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(&manifest_path) else {
            return (true, false, None, None);
        };
        let Ok(manifest) = toml::from_str::<ManifestToml>(&content) else {
            return (true, false, None, None);
        };

        for variant in &manifest.model.variants {
            let file_path = model_dir.join(&variant.file);
            if file_path.exists() {
                let actual_sha = sha256_file(&file_path).ok();
                let matches = actual_sha.map(|s| s.eq_ignore_ascii_case(&variant.sha256));
                return (true, true, matches, Some(file_path));
            }
        }
        return (true, false, None, None);
    }
    (false, false, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_verdict_verified() {
        let p = PathBuf::from("/models/selfie_segmentation/model.onnx");
        let c = model_verdict("selfie_segmentation", "Selfie", true, true, true, Some(true), Some(&p));
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("verified"));
    }

    #[test]
    fn test_model_verdict_corrupted() {
        let c = model_verdict("selfie_segmentation", "Selfie", true, true, true, Some(false), None);
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("checksum mismatch"));
        assert!(c.fix.is_some());
    }

    #[test]
    fn test_model_verdict_missing_required() {
        let c = model_verdict("yunet", "YuNet", true, false, false, None, None);
        assert_eq!(c.status, Status::Fail);
        assert!(c.fix.is_some());
    }

    #[test]
    fn test_model_verdict_missing_optional() {
        let c = model_verdict("iat_lowlight", "IAT", false, false, false, None, None);
        assert_eq!(c.status, Status::Info);
    }
}
