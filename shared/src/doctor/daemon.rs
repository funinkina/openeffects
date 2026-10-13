use std::path::{Path, PathBuf};

use super::types::{Check, Status};

pub const DBUS_SERVICE_NAME: &str = "io.github.karanshukla.vinoopeneffects.Daemon";
pub const SYSTEMD_UNIT: &str = "vinoopeneffects-daemon.service";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationFileInfo {
    pub path: PathBuf,
    pub exists: bool,
    pub exec_path: Option<PathBuf>,
    pub exec_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    Active,
    Inactive,
    Failed,
    NotFound,
}

pub fn dbus_verdict(
    status_result: Result<&str, &str>,
    act_files: Option<&[ActivationFileInfo]>,
    unit_state: Option<UnitState>,
) -> Check {
    match status_result {
        Ok(status) => Check::new(
            "daemon",
            "dbus",
            Status::Ok,
            format!("reachable (daemon reports {status})"),
            None,
        ),
        Err(err) => {
            if let Some(files) = act_files {
                let existing = files.iter().find(|f| f.exists);
                match existing {
                    None => {
                        return Check::new(
                            "daemon",
                            "dbus",
                            Status::Fail,
                            format!("unreachable ({err}); no D-Bus activation .service file found"),
                            Some("install or link dist/io.github.karanshukla.vinoopeneffects.Daemon.service to ~/.local/share/dbus-1/services/".into()),
                        );
                    }
                    Some(f) if !f.exec_exists && f.exec_path.is_some() => {
                        let exec = f.exec_path.as_ref().unwrap().display();
                        return Check::new(
                            "daemon",
                            "dbus",
                            Status::Fail,
                            format!("unreachable ({err}); activation service file points to non-existent binary '{exec}'"),
                            Some(format!("install {exec} or check package installation")),
                        );
                    }
                    Some(_) if unit_state == Some(UnitState::Failed) => {
                        return Check::new(
                            "daemon",
                            "dbus",
                            Status::Fail,
                            format!("unreachable ({err}); daemon failed on startup"),
                            Some("journalctl --user -u vinoopeneffects-daemon.service -e".into()),
                        );
                    }
                    Some(_) => {
                        return Check::new(
                            "daemon",
                            "dbus",
                            Status::Fail,
                            format!("unreachable ({err}); D-Bus activation service file exists but daemon was not activated"),
                            Some("run 'busctl --user call org.freedesktop.DBus /org/freedesktop/DBus ReloadConfig' or 'systemctl --user start vinoopeneffects-daemon.service'".into()),
                        );
                    }
                }
            }
            Check::new(
                "daemon",
                "dbus",
                Status::Fail,
                format!("unreachable ({err})"),
                Some("systemctl --user start vinoopeneffects-daemon.service".into()),
            )
        }
    }
}

pub fn activation_file_verdict(files: &[ActivationFileInfo]) -> Check {
    let existing = files.iter().find(|f| f.exists);
    match existing {
        Some(f) if !f.exec_exists && f.exec_path.is_some() => {
            let exec = f.exec_path.as_ref().unwrap().display();
            Check::new(
                "daemon",
                "activation",
                Status::Fail,
                format!(
                    "{} present, but binary '{exec}' does not exist on disk",
                    f.path.display()
                ),
                Some(format!("install {exec} or check package installation")),
            )
        }
        Some(f) => Check::new(
            "daemon",
            "activation",
            Status::Ok,
            format!("{} present", f.path.display()),
            None,
        ),
        None => Check::new(
            "daemon",
            "activation",
            Status::Warn,
            "no D-Bus activation service file found in search paths",
            Some("install or link dist/io.github.karanshukla.vinoopeneffects.Daemon.service to ~/.local/share/dbus-1/services/".into()),
        ),
    }
}

pub fn systemd_unit_verdict(state: UnitState, is_enabled: bool) -> Check {
    match state {
        UnitState::Active => {
            let detail = if is_enabled {
                "active (running, enabled at login)"
            } else {
                "active (running, disabled at login)"
            };
            Check::new("daemon", "systemd", Status::Ok, detail, None)
        }
        UnitState::Failed => Check::new(
            "daemon",
            "systemd",
            Status::Fail,
            "unit failed (crashed on startup)",
            Some("journalctl --user -u vinoopeneffects-daemon.service -e".into()),
        ),
        UnitState::Inactive => {
            let detail = if is_enabled {
                "inactive (not running, enabled at login)"
            } else {
                "inactive (not running, disabled at login)"
            };
            Check::new(
                "daemon",
                "systemd",
                Status::Info,
                detail,
                Some("systemctl --user start vinoopeneffects-daemon.service".into()),
            )
        }
        UnitState::NotFound => Check::new(
            "daemon",
            "systemd",
            Status::Warn,
            "vinoopeneffects-daemon.service unit not found in systemd search paths",
            Some("copy dist/vinoopeneffects-daemon.service to ~/.local/share/systemd/user/".into()),
        ),
    }
}

pub fn parse_exec_from_service(content: &str) -> Option<PathBuf> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Exec=") {
            let bin = rest.split_whitespace().next()?;
            return Some(PathBuf::from(bin));
        }
    }
    None
}

pub fn probe_activation_files(home: Option<&Path>) -> Vec<ActivationFileInfo> {
    let filename = format!("{DBUS_SERVICE_NAME}.service");
    let mut candidates = Vec::new();
    if let Some(h) = home {
        candidates.push(h.join(".local/share/dbus-1/services").join(&filename));
    }
    candidates.push(PathBuf::from("/usr/share/dbus-1/services").join(&filename));
    candidates.push(PathBuf::from("/usr/local/share/dbus-1/services").join(&filename));

    candidates
        .into_iter()
        .map(|path| {
            let exists = path.exists();
            let (exec_path, exec_exists) = if exists {
                let exec = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|c| parse_exec_from_service(&c));
                let exec_exists = exec.as_ref().map(|p| p.exists()).unwrap_or(false);
                (exec, exec_exists)
            } else {
                (None, false)
            };
            ActivationFileInfo {
                path,
                exists,
                exec_path,
                exec_exists,
            }
        })
        .collect()
}

pub fn probe_systemd_unit() -> (UnitState, bool) {
    use std::process::Command;

    let is_active = Command::new("systemctl")
        .args(["--user", "is-active", SYSTEMD_UNIT])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let is_enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", SYSTEMD_UNIT])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let state = match is_active.as_str() {
        "active" => UnitState::Active,
        "failed" => UnitState::Failed,
        "inactive" => UnitState::Inactive,
        _ => {
            if is_enabled {
                UnitState::Inactive
            } else {
                UnitState::NotFound
            }
        }
    };
    (state, is_enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dbus_verdict_ok_and_fail() {
        let ok = dbus_verdict(Ok("idle"), None, None);
        assert_eq!(ok.status, Status::Ok);
        assert!(ok.detail.contains("idle"));

        let fail = dbus_verdict(Err("name has no owner"), None, None);
        assert_eq!(fail.status, Status::Fail);
        assert!(fail.fix.is_some());
    }

    #[test]
    fn test_dbus_verdict_when_stopped_diagnoses_activation() {
        // 1. Missing activation file
        let no_files = vec![];
        let v1 = dbus_verdict(Err("name has no owner"), Some(&no_files), Some(UnitState::Inactive));
        assert_eq!(v1.status, Status::Fail);
        assert!(v1.detail.contains("no D-Bus activation"));

        // 2. Broken binary path
        let broken = vec![ActivationFileInfo {
            path: PathBuf::from("/usr/share/dbus-1/services/test.service"),
            exists: true,
            exec_path: Some(PathBuf::from("/usr/bin/missing")),
            exec_exists: false,
        }];
        let v2 = dbus_verdict(Err("name has no owner"), Some(&broken), Some(UnitState::Inactive));
        assert_eq!(v2.status, Status::Fail);
        assert!(v2.detail.contains("missing"));

        // 3. Failed unit
        let valid = vec![ActivationFileInfo {
            path: PathBuf::from("/usr/share/dbus-1/services/test.service"),
            exists: true,
            exec_path: Some(PathBuf::from("/usr/bin/daemon")),
            exec_exists: true,
        }];
        let v3 = dbus_verdict(Err("name has no owner"), Some(&valid), Some(UnitState::Failed));
        assert_eq!(v3.status, Status::Fail);
        assert!(v3.detail.contains("daemon failed on startup"));

        // 4. Inactive unit with valid file (not activated yet)
        let v4 = dbus_verdict(Err("name has no owner"), Some(&valid), Some(UnitState::Inactive));
        assert_eq!(v4.status, Status::Fail);
        assert!(v4.fix.as_ref().unwrap().contains("ReloadConfig"));
    }

    #[test]
    fn test_activation_file_verdict_missing() {
        let files = vec![ActivationFileInfo {
            path: PathBuf::from("/usr/share/dbus-1/services/test.service"),
            exists: false,
            exec_path: None,
            exec_exists: false,
        }];
        let v = activation_file_verdict(&files);
        assert_eq!(v.status, Status::Warn);
    }

    #[test]
    fn test_activation_file_verdict_broken_exec() {
        let files = vec![ActivationFileInfo {
            path: PathBuf::from("/usr/share/dbus-1/services/test.service"),
            exists: true,
            exec_path: Some(PathBuf::from("/usr/bin/missing-daemon")),
            exec_exists: false,
        }];
        let v = activation_file_verdict(&files);
        assert_eq!(v.status, Status::Fail);
        assert!(v.detail.contains("missing-daemon"));
    }

    #[test]
    fn test_activation_file_verdict_valid() {
        let files = vec![ActivationFileInfo {
            path: PathBuf::from("/usr/share/dbus-1/services/test.service"),
            exists: true,
            exec_path: Some(PathBuf::from("/usr/bin/valid-daemon")),
            exec_exists: true,
        }];
        let v = activation_file_verdict(&files);
        assert_eq!(v.status, Status::Ok);
    }

    #[test]
    fn test_systemd_unit_states() {
        let active = systemd_unit_verdict(UnitState::Active, true);
        assert_eq!(active.status, Status::Ok);

        let failed = systemd_unit_verdict(UnitState::Failed, true);
        assert_eq!(failed.status, Status::Fail);
        assert!(failed.fix.unwrap().contains("journalctl"));

        let inactive = systemd_unit_verdict(UnitState::Inactive, false);
        assert_eq!(inactive.status, Status::Info);
    }
}
