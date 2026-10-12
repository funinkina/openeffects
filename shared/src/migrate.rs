//! One-shot migration of user state left behind by the pre-fork `openeffects`.
//!
//! All filesystem work happens in [`migrate_in`], which takes explicit roots so
//! tests never touch `$HOME`. Nothing here may block startup: every failure is
//! recorded in the [`MigrationReport`] and the caller carries on.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

const OLD_APP_DIR: &str = "openeffects";
const NEW_APP_DIR: &str = "vinoopeneffects";
const OLD_UNIT: &str = "openeffectsd.service";
const NEW_UNIT: &str = "vinoopeneffects-daemon.service";
const OLD_DROPIN_DIR: &str = "openeffectsd.service.d";
const NEW_DROPIN_DIR: &str = "vinoopeneffects-daemon.service.d";

/// Environment variable renames applied to moved drop-in `*.conf` files.
const ENV_RENAMES: &[(&str, &str)] = &[
    ("OE_EP", "VINOOPENEFFECTS_EP"),
    ("OE_MODELS_DEST", "VINOOPENEFFECTS_MODELS_DEST"),
];

/// What a migration run did. Every entry is a human-readable line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Items that were moved or rewritten (log at info).
    pub moved: Vec<String>,
    /// Items skipped on purpose, e.g. destination already exists (log at warn).
    pub warnings: Vec<String>,
    /// I/O failures (log at warn); migration continued past them.
    pub errors: Vec<String>,
}

impl MigrationReport {
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.warnings.is_empty() && self.errors.is_empty()
    }

    /// Emit every line through `tracing`: moves at info, the rest at warn.
    pub fn log(&self) {
        for line in &self.moved {
            tracing::info!("migrate: {line}");
        }
        for line in self.warnings.iter().chain(&self.errors) {
            tracing::warn!("migrate: {line}");
        }
    }
}

/// Migrate legacy `openeffects` state under the real XDG config and data dirs.
///
/// Call first thing at startup in the daemon and GUI, before any config or
/// model lookup, so an upgrade picks up existing state without user action.
pub fn migrate_legacy_paths() -> MigrationReport {
    match (dirs::config_dir(), dirs::data_dir()) {
        (Some(config), Some(data)) => migrate_in(&config, &data),
        _ => MigrationReport {
            warnings: vec!["no XDG config/data directory; skipping legacy migration".into()],
            ..Default::default()
        },
    }
}

/// Migrate legacy state under explicit `config_dir` / `data_dir` roots.
pub fn migrate_in(config_dir: &Path, data_dir: &Path) -> MigrationReport {
    let mut r = MigrationReport::default();

    move_item(
        &config_dir.join(OLD_APP_DIR),
        &config_dir.join(NEW_APP_DIR),
        &mut r,
    );
    let data_moved = move_item(
        &data_dir.join(OLD_APP_DIR),
        &data_dir.join(NEW_APP_DIR),
        &mut r,
    );
    if data_moved {
        // state.toml stores absolute background paths under the data dir.
        rewrite_state_paths(
            &config_dir.join(NEW_APP_DIR).join("state.toml"),
            &data_dir.join(OLD_APP_DIR),
            &data_dir.join(NEW_APP_DIR),
            &mut r,
        );
    }

    let user_units = config_dir.join("systemd").join("user");
    migrate_dropins(&user_units, &mut r);
    migrate_wants(&user_units, &mut r);

    let stale_unit = user_units.join(OLD_UNIT);
    if fs::symlink_metadata(&stale_unit).is_ok() {
        r.warnings.push(format!(
            "stale user unit {} remains; remove it or replace it with {NEW_UNIT}",
            stale_unit.display()
        ));
    }
    r
}

/// Rename `src` to `dst` (same parent, so atomic). Returns true if moved.
fn move_item(src: &Path, dst: &Path, r: &mut MigrationReport) -> bool {
    match fs::symlink_metadata(src) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
        Err(e) => {
            r.errors
                .push(format!("cannot inspect {}: {e}", src.display()));
            return false;
        }
    }
    if fs::symlink_metadata(dst).is_ok() {
        r.warnings.push(format!(
            "both {} and {} exist; leaving {} untouched",
            src.display(),
            dst.display(),
            src.display()
        ));
        return false;
    }
    match fs::rename(src, dst) {
        Ok(()) => {
            r.moved
                .push(format!("moved {} -> {}", src.display(), dst.display()));
            true
        }
        // Lost a race with the daemon/GUI starting concurrently.
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => {
            r.errors.push(format!(
                "failed to move {} -> {}: {e}",
                src.display(),
                dst.display()
            ));
            false
        }
    }
}

fn migrate_dropins(user_units: &Path, r: &mut MigrationReport) {
    let src = user_units.join(OLD_DROPIN_DIR);
    let dst = user_units.join(NEW_DROPIN_DIR);
    if !move_item(&src, &dst, r) {
        return;
    }
    let entries = match fs::read_dir(&dst) {
        Ok(e) => e,
        Err(e) => {
            r.errors.push(format!("cannot read {}: {e}", dst.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "conf") && path.is_file() {
            rewrite_conf(&path, r);
        }
    }
}

/// Point absolute paths in `state.toml` that name the old data dir at the new one.
fn rewrite_state_paths(state: &Path, old_dir: &Path, new_dir: &Path, r: &mut MigrationReport) {
    let text = match fs::read_to_string(state) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            r.errors
                .push(format!("cannot read {}: {e}", state.display()));
            return;
        }
    };
    let (old, new) = (
        format!("{}/", old_dir.display()),
        format!("{}/", new_dir.display()),
    );
    if !text.contains(&old) {
        return;
    }
    match write_replacing(state, &text.replace(&old, &new)) {
        Ok(()) => r.moved.push(format!(
            "updated data paths in {} ({} -> {})",
            state.display(),
            old_dir.display(),
            new_dir.display()
        )),
        Err(e) => r
            .errors
            .push(format!("failed to rewrite {}: {e}", state.display())),
    }
}

/// Write `content` over `path` via a temp file, keeping the original mode.
fn write_replacing(path: &Path, content: &str) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".migrate-tmp");
    let tmp = PathBuf::from(tmp);
    let res = fs::write(&tmp, content)
        .and_then(|()| fs::set_permissions(&tmp, fs::metadata(path)?.permissions()))
        .and_then(|()| fs::rename(&tmp, path));
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

fn rewrite_conf(path: &Path, r: &mut MigrationReport) {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            r.errors
                .push(format!("cannot read {}: {e}", path.display()));
            return;
        }
    };
    let new = ENV_RENAMES.iter().fold(text.clone(), |acc, (old, new)| {
        replace_token(&acc, old, new)
    });
    if new == text {
        return;
    }
    match write_replacing(path, &new) {
        Ok(()) => r
            .moved
            .push(format!("updated variable names in {}", path.display())),
        Err(e) => r
            .errors
            .push(format!("failed to rewrite {}: {e}", path.display())),
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Replace `old` with `new` only where it is a whole identifier token.
fn replace_token(text: &str, old: &str, new: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in text.match_indices(old) {
        let end = i + old.len();
        let before_ok = i == 0 || !is_ident(bytes[i - 1]);
        let after_ok = end == bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            out.push_str(&text[last..i]);
            out.push_str(new);
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn migrate_wants(user_units: &Path, r: &mut MigrationReport) {
    let entries = match fs::read_dir(user_units) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let dir: PathBuf = entry.path();
        if dir.extension().is_none_or(|x| x != "wants") {
            continue;
        }
        let old_link = dir.join(OLD_UNIT);
        let target = match fs::read_link(&old_link) {
            Ok(t) => t,
            // Absent, or not a symlink: nothing to do.
            Err(_) => continue,
        };
        let new_link = dir.join(NEW_UNIT);
        if fs::symlink_metadata(&new_link).is_ok() {
            r.warnings.push(format!(
                "both {} and {} exist; leaving the old link",
                old_link.display(),
                new_link.display()
            ));
            continue;
        }
        let new_target = if target.file_name().is_some_and(|n| n == OLD_UNIT) {
            target.with_file_name(NEW_UNIT)
        } else {
            target
        };
        match std::os::unix::fs::symlink(&new_target, &new_link) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                r.errors
                    .push(format!("failed to create {}: {e}", new_link.display()));
                continue;
            }
        }
        match fs::remove_file(&old_link) {
            Ok(()) => r.moved.push(format!(
                "re-linked {} -> {} (was {})",
                new_link.display(),
                new_target.display(),
                old_link.display()
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => r
                .errors
                .push(format!("failed to remove {}: {e}", old_link.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn roots() -> (TempDir, PathBuf, PathBuf) {
        let t = TempDir::new().unwrap();
        let c = t.path().join("config");
        let d = t.path().join("data");
        fs::create_dir_all(&c).unwrap();
        fs::create_dir_all(&d).unwrap();
        (t, c, d)
    }

    #[test]
    fn moves_config_dir_with_contents() {
        let (_t, c, d) = roots();
        fs::create_dir_all(c.join("openeffects")).unwrap();
        fs::write(c.join("openeffects/state.toml"), "a = 1\n").unwrap();
        fs::write(c.join("openeffects/gui.toml"), "b = 2\n").unwrap();
        let r = migrate_in(&c, &d);
        assert_eq!(r.moved.len(), 1, "{r:?}");
        assert!(r.errors.is_empty() && r.warnings.is_empty());
        assert!(!c.join("openeffects").exists());
        assert_eq!(
            fs::read_to_string(c.join("vinoopeneffects/state.toml")).unwrap(),
            "a = 1\n"
        );
        assert!(c.join("vinoopeneffects/gui.toml").is_file());
    }

    #[test]
    fn moves_data_dir_with_nested_models() {
        let (_t, c, d) = roots();
        fs::create_dir_all(d.join("openeffects/models")).unwrap();
        fs::write(d.join("openeffects/models/yunet.onnx"), b"onnx").unwrap();
        let r = migrate_in(&c, &d);
        assert_eq!(r.moved.len(), 1, "{r:?}");
        assert_eq!(
            fs::read(d.join("vinoopeneffects/models/yunet.onnx")).unwrap(),
            b"onnx"
        );
        assert!(!d.join("openeffects").exists());
    }

    #[test]
    fn both_present_moves_nothing() {
        let (_t, c, d) = roots();
        fs::create_dir_all(c.join("openeffects")).unwrap();
        fs::create_dir_all(c.join("vinoopeneffects")).unwrap();
        fs::write(c.join("openeffects/state.toml"), "old").unwrap();
        fs::write(c.join("vinoopeneffects/state.toml"), "new").unwrap();
        let r = migrate_in(&c, &d);
        assert!(r.moved.is_empty());
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(
            fs::read_to_string(c.join("openeffects/state.toml")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(c.join("vinoopeneffects/state.toml")).unwrap(),
            "new"
        );
    }

    #[test]
    fn state_background_paths_follow_the_data_dir() {
        let (_t, config, data) = roots();
        fs::create_dir_all(config.join("openeffects")).unwrap();
        fs::create_dir_all(data.join("openeffects/backgrounds")).unwrap();
        let old_bg = format!("{}/openeffects/backgrounds/a.jpg", data.display());
        fs::write(
            config.join("openeffects/state.toml"),
            format!("[bg_replace]\nbackground = \"{old_bg}\"\n"),
        )
        .unwrap();
        let r = migrate_in(&config, &data);
        assert!(r.errors.is_empty(), "{r:?}");
        let text = fs::read_to_string(config.join("vinoopeneffects/state.toml")).unwrap();
        assert!(text.contains(&format!(
            "{}/vinoopeneffects/backgrounds/a.jpg",
            data.display()
        )));
        assert!(!text.contains("/openeffects/"));
    }

    #[test]
    fn nothing_present_is_empty_report() {
        let (_t, c, d) = roots();
        assert!(migrate_in(&c, &d).is_empty());
    }

    #[test]
    fn missing_roots_are_not_an_error() {
        let t = TempDir::new().unwrap();
        let r = migrate_in(&t.path().join("nope-c"), &t.path().join("nope-d"));
        assert!(r.is_empty(), "{r:?}");
    }

    #[test]
    fn second_run_is_noop() {
        let (_t, c, d) = roots();
        fs::create_dir_all(c.join("openeffects")).unwrap();
        fs::write(c.join("openeffects/state.toml"), "x").unwrap();
        fs::create_dir_all(d.join("openeffects")).unwrap();
        let ud = c.join("systemd/user/openeffectsd.service.d");
        fs::create_dir_all(&ud).unwrap();
        fs::write(ud.join("npu.conf"), "[Service]\nEnvironment=OE_EP=npu\n").unwrap();
        let wants = c.join("systemd/user/default.target.wants");
        fs::create_dir_all(&wants).unwrap();
        std::os::unix::fs::symlink(
            "/usr/lib/systemd/user/openeffectsd.service",
            wants.join("openeffectsd.service"),
        )
        .unwrap();

        let first = migrate_in(&c, &d);
        assert!(first.errors.is_empty(), "{first:?}");
        assert!(!first.moved.is_empty());
        let second = migrate_in(&c, &d);
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn dropin_moved_and_variables_renamed() {
        let (_t, c, d) = roots();
        let ud = c.join("systemd/user/openeffectsd.service.d");
        fs::create_dir_all(&ud).unwrap();
        fs::write(
            ud.join("npu.conf"),
            "[Service]\nEnvironment=OE_EP=npu\nEnvironment=\"OE_MODELS_DEST=/x\"\nEnvironment=FOO_OE_EP=keep\nEnvironment=OE_EPX=keep\n",
        )
        .unwrap();
        fs::write(ud.join("notes.txt"), "OE_EP stays\n").unwrap();
        let r = migrate_in(&c, &d);
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(!ud.exists());
        let new = c.join("systemd/user/vinoopeneffects-daemon.service.d");
        assert_eq!(
            fs::read_to_string(new.join("npu.conf")).unwrap(),
            "[Service]\nEnvironment=VINOOPENEFFECTS_EP=npu\nEnvironment=\"VINOOPENEFFECTS_MODELS_DEST=/x\"\nEnvironment=FOO_OE_EP=keep\nEnvironment=OE_EPX=keep\n"
        );
        assert_eq!(
            fs::read_to_string(new.join("notes.txt")).unwrap(),
            "OE_EP stays\n"
        );
    }

    #[test]
    fn wants_symlink_recreated() {
        let (_t, c, d) = roots();
        let wants = c.join("systemd/user/default.target.wants");
        fs::create_dir_all(&wants).unwrap();
        std::os::unix::fs::symlink(
            "/usr/lib/systemd/user/openeffectsd.service",
            wants.join("openeffectsd.service"),
        )
        .unwrap();
        let r = migrate_in(&c, &d);
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(fs::symlink_metadata(wants.join("openeffectsd.service")).is_err());
        assert_eq!(
            fs::read_link(wants.join("vinoopeneffects-daemon.service")).unwrap(),
            PathBuf::from("/usr/lib/systemd/user/vinoopeneffects-daemon.service")
        );
    }

    #[test]
    fn stale_user_unit_only_warns() {
        let (_t, c, d) = roots();
        let units = c.join("systemd/user");
        fs::create_dir_all(&units).unwrap();
        fs::write(units.join("openeffectsd.service"), "[Unit]\n").unwrap();
        let r = migrate_in(&c, &d);
        assert!(r.moved.is_empty());
        assert_eq!(r.warnings.len(), 1);
        assert!(units.join("openeffectsd.service").is_file());
    }

    #[test]
    fn racing_source_removal_is_not_an_error() {
        let (_t, c, _d) = roots();
        let mut r = MigrationReport::default();
        let moved = move_item(&c.join("openeffects"), &c.join("vinoopeneffects"), &mut r);
        assert!(!moved && r.is_empty());
        let mut r = MigrationReport::default();
        rewrite_conf(&c.join("gone.conf"), &mut r);
        // A vanished conf is reported, but never panics.
        assert_eq!(r.errors.len(), 1);
    }
}
