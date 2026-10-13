use std::path::{Path, PathBuf};

use super::types::{Check, Status};
use crate::config::AppState;

pub fn find_unknown_keys(value: &toml::Value) -> Vec<String> {
    let mut unknown = Vec::new();
    let toml::Value::Table(top) = value else {
        return unknown;
    };

    for (k, v) in top {
        match k.as_str() {
            "bypass" => {}
            "inference" => {
                if let toml::Value::Table(inf) = v {
                    for ik in inf.keys() {
                        if ik != "provider" {
                            unknown.push(format!("inference.{ik}"));
                        }
                    }
                }
            }
            "camera" => {
                if let toml::Value::Table(cam) = v {
                    for ck in cam.keys() {
                        if ck != "selected" {
                            unknown.push(format!("camera.{ck}"));
                        }
                    }
                }
            }
            "effects" => {
                if let toml::Value::Table(effs) = v {
                    for (ek, ev) in effs {
                        match ek.as_str() {
                            "center_stage" => {
                                if let toml::Value::Table(t) = ev {
                                    for key in t.keys() {
                                        if !matches!(
                                            key.as_str(),
                                            "enabled" | "zoom" | "mode" | "crop"
                                        ) {
                                            unknown.push(format!("effects.center_stage.{key}"));
                                        }
                                    }
                                }
                            }
                            "portrait_blur" => {
                                if let toml::Value::Table(t) = ev {
                                    for key in t.keys() {
                                        if !matches!(key.as_str(), "enabled" | "strength") {
                                            unknown.push(format!("effects.portrait_blur.{key}"));
                                        }
                                    }
                                }
                            }
                            "bg_replace" => {
                                if let toml::Value::Table(t) = ev {
                                    for key in t.keys() {
                                        if !matches!(key.as_str(), "enabled" | "background") {
                                            unknown.push(format!("effects.bg_replace.{key}"));
                                        }
                                    }
                                }
                            }
                            "studio_light" => {
                                if let toml::Value::Table(t) = ev {
                                    for key in t.keys() {
                                        if !matches!(
                                            key.as_str(),
                                            "enabled"
                                                | "intensity"
                                                | "brightness"
                                                | "contrast"
                                                | "bg_brightness"
                                                | "auto_light"
                                                | "auto_strength"
                                        ) {
                                            unknown.push(format!("effects.studio_light.{key}"));
                                        }
                                    }
                                }
                            }
                            "reactions" => {
                                if let toml::Value::Table(t) = ev {
                                    for key in t.keys() {
                                        if key != "enabled" {
                                            unknown.push(format!("effects.reactions.{key}"));
                                        }
                                    }
                                }
                            }
                            other => unknown.push(format!("effects.{other}")),
                        }
                    }
                }
            }
            other => unknown.push(other.to_string()),
        }
    }

    unknown
}

pub fn config_verdict(path: &Path, content: Result<&str, &std::io::Error>) -> Check {
    match content {
        Ok(text) => {
            let parsed_app: Result<AppState, _> = toml::from_str(text);
            let parsed_val: Result<toml::Value, _> = toml::from_str(text);

            match (parsed_app, parsed_val) {
                (Ok(_), Ok(val)) => {
                    let unknown = find_unknown_keys(&val);
                    if unknown.is_empty() {
                        Check::new(
                            "config",
                            "state.toml",
                            Status::Ok,
                            format!("{} is valid", path.display()),
                            None,
                        )
                    } else {
                        Check::new(
                            "config",
                            "state.toml",
                            Status::Warn,
                            format!(
                                "{} is valid, but contains unrecognized key(s): {}",
                                path.display(),
                                unknown.join(", ")
                            ),
                            None,
                        )
                    }
                }
                (Err(e), _) => Check::new(
                    "config",
                    "state.toml",
                    Status::Fail,
                    format!("parse error in {}: {e}", path.display()),
                    Some("check state.toml syntax or remove invalid keys".into()),
                ),
                _ => Check::new(
                    "config",
                    "state.toml",
                    Status::Fail,
                    format!("invalid TOML in {}", path.display()),
                    Some("check state.toml syntax".into()),
                ),
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Check::new(
            "config",
            "state.toml",
            Status::Ok,
            format!("default config ({} does not exist yet)", path.display()),
            None,
        ),
        Err(err) => Check::new(
            "config",
            "state.toml",
            Status::Fail,
            format!("could not read {}: {err}", path.display()),
            None,
        ),
    }
}

pub fn probe_config_path(home: Option<&Path>) -> PathBuf {
    match home {
        Some(h) => h.join(".config/vinoopeneffects/state.toml"),
        None => dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("vinoopeneffects/state.toml"),
    }
}

pub fn probe_config(home: Option<&Path>) -> (PathBuf, Result<String, std::io::Error>) {
    let path = probe_config_path(home);
    let res = std::fs::read_to_string(&path);
    (path, res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_verdict_missing_file() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let c = config_verdict(Path::new("/missing/state.toml"), Err(&err));
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("default config"));
    }

    #[test]
    fn test_config_verdict_valid() {
        let toml = r#"
            bypass = false
            [effects.portrait_blur]
            enabled = true
            strength = 50
        "#;
        let c = config_verdict(Path::new("/config/state.toml"), Ok(toml));
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("valid"));
    }

    #[test]
    fn test_config_verdict_unknown_keys() {
        let toml = r#"
            unknown_root = true
            [effects.unknown_effect]
            enabled = true
        "#;
        let c = config_verdict(Path::new("/config/state.toml"), Ok(toml));
        assert_eq!(c.status, Status::Warn);
        assert!(c.detail.contains("unknown_root"));
        assert!(c.detail.contains("effects.unknown_effect"));
    }

    #[test]
    fn test_config_verdict_syntax_error() {
        let toml = "this is not valid toml = = =";
        let c = config_verdict(Path::new("/config/state.toml"), Ok(toml));
        assert_eq!(c.status, Status::Fail);
        assert!(c.fix.is_some());
    }
}
