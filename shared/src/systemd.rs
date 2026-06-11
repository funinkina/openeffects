//! Thin wrappers over `systemctl --user` for managing the daemon's user unit.
//!
//! Shared by the CLI (`openeffectsctl autostart …`) and the GUI (Startup
//! preferences). This is the only place in the workspace that shells out to an
//! external process, so the unit name and command shape live here.

use std::process::Command;

/// The systemd user unit installed by packaging (`/usr/lib/systemd/user/`).
pub const UNIT: &str = "openeffectsd.service";

fn systemctl(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("systemctl").arg("--user").args(args).output()
}

/// Enable or disable the unit so it starts with the graphical session.
/// Returns the captured stderr on failure so callers can surface it.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let verb = if enabled { "enable" } else { "disable" };
    match systemctl(&[verb, UNIT]) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(err) => Err(err.to_string()),
    }
}

/// Whether the unit is enabled (`systemctl --user is-enabled` exits 0).
pub fn is_enabled() -> bool {
    systemctl(&["is-enabled", UNIT])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Whether the unit is currently active/running.
pub fn is_active() -> bool {
    systemctl(&["is-active", UNIT])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Start the unit now. `Type=dbus` means this blocks until the bus name is up.
pub fn start() -> Result<(), String> {
    match systemctl(&["start", UNIT]) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(err) => Err(err.to_string()),
    }
}
