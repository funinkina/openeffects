use std::path::{Path, PathBuf};

use super::types::{Check, Status};

pub const VIRTUAL_CAMERA_NODE_NAME: &str = "vinoopeneffects";

pub fn pipewire_verdict(running: bool) -> Check {
    if running {
        Check::new(
            "pipeline",
            "pipewire",
            Status::Ok,
            "PipeWire daemon is running",
            None,
        )
    } else {
        Check::new(
            "pipeline",
            "pipewire",
            Status::Fail,
            "PipeWire daemon is not running (pipewire-0 socket missing)",
            Some("systemctl --user start pipewire.service".into()),
        )
    }
}

pub fn virtual_camera_verdict(vcam_present: bool) -> Check {
    if vcam_present {
        Check::new(
            "pipeline",
            "virtual camera",
            Status::Ok,
            format!("virtual camera node '{VIRTUAL_CAMERA_NODE_NAME}' is registered in PipeWire"),
            None,
        )
    } else {
        Check::new(
            "pipeline",
            "virtual camera",
            Status::Info,
            format!(
                "virtual camera node '{VIRTUAL_CAMERA_NODE_NAME}' not active (created on demand when daemon runs)"
            ),
            None,
        )
    }
}

pub fn probe_pipewire(runtime_dir: Option<&Path>) -> bool {
    let socket = match runtime_dir {
        Some(r) => r.join("pipewire-0"),
        None => match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(r) => PathBuf::from(r).join("pipewire-0"),
            None => return false,
        },
    };
    socket.exists()
}

pub fn probe_virtual_camera() -> bool {
    use std::process::Command;

    if let Ok(out) = Command::new("pw-cli")
        .args(["list-objects", "Node"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains(VIRTUAL_CAMERA_NODE_NAME) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipewire_verdict() {
        let ok = pipewire_verdict(true);
        assert_eq!(ok.status, Status::Ok);

        let fail = pipewire_verdict(false);
        assert_eq!(fail.status, Status::Fail);
        assert!(fail.fix.is_some());
    }

    #[test]
    fn test_virtual_camera_verdict() {
        let ok = virtual_camera_verdict(true);
        assert_eq!(ok.status, Status::Ok);

        let info = virtual_camera_verdict(false);
        assert_eq!(info.status, Status::Info);
    }
}
