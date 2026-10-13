use std::path::Path;

use super::types::{Check, Status};

pub struct DistroInfo {
    pub id: String,
    pub id_like: String,
}

pub fn detect_distro(os_release: &str) -> DistroInfo {
    let mut id = String::new();
    let mut id_like = String::new();
    for line in os_release.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("ID=") {
            id = val.trim_matches('"').to_ascii_lowercase();
        } else if let Some(val) = line.strip_prefix("ID_LIKE=") {
            id_like = val.trim_matches('"').to_ascii_lowercase();
        }
    }
    DistroInfo { id, id_like }
}

pub fn gtk4_sink_package_remedy(distro: Option<&DistroInfo>) -> (&'static str, &'static str) {
    let (id, like) = match distro {
        Some(d) => (d.id.as_str(), d.id_like.as_str()),
        None => ("", ""),
    };

    if id == "fedora"
        || id == "rhel"
        || id == "centos"
        || like.contains("fedora")
        || like.contains("rhel")
    {
        (
            "gstreamer1-plugin-gtk4",
            "sudo dnf install gstreamer1-plugin-gtk4",
        )
    } else if id == "ubuntu"
        || id == "debian"
        || id == "pop"
        || like.contains("debian")
        || like.contains("ubuntu")
    {
        ("gstreamer1.0-gtk4", "sudo apt install gstreamer1.0-gtk4")
    } else if id == "arch" || id == "manjaro" || like.contains("arch") {
        ("gst-plugin-gtk4", "sudo pacman -S gst-plugin-gtk4")
    } else if id == "opensuse" || id == "suse" || like.contains("suse") {
        (
            "gstreamer-plugin-gtk4",
            "sudo zypper install gstreamer-plugin-gtk4",
        )
    } else {
        (
            "gstreamer1-plugin-gtk4",
            "install gstreamer1-plugin-gtk4 package",
        )
    }
}

pub fn gtk4_sink_verdict(sink_found: bool, distro: Option<&DistroInfo>) -> Check {
    if sink_found {
        Check::new(
            "gui",
            "sink",
            Status::Ok,
            "gtk4paintablesink element available for live preview",
            None,
        )
    } else {
        let (pkg, cmd) = gtk4_sink_package_remedy(distro);
        Check::new(
            "gui",
            "sink",
            Status::Fail,
            format!(
                "gtk4paintablesink element missing (needs {pkg}); live preview cannot render frames"
            ),
            Some(cmd.to_string()),
        )
    }
}

pub fn gtk_versions_verdict(gtk_ver: Option<&str>, adw_ver: Option<&str>) -> Check {
    match (gtk_ver, adw_ver) {
        (Some(gtk), Some(adw)) => Check::new(
            "gui",
            "libraries",
            Status::Ok,
            format!("GTK {gtk}, Libadwaita {adw}"),
            None,
        ),
        (Some(gtk), None) => Check::new("gui", "libraries", Status::Ok, format!("GTK {gtk}"), None),
        _ => Check::new(
            "gui",
            "libraries",
            Status::Info,
            "GTK / Libadwaita versions not queried",
            None,
        ),
    }
}

pub fn probe_gtk4_sink() -> bool {
    use std::process::Command;

    if Command::new("gst-inspect-1.0")
        .arg("gtk4paintablesink")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
    {
        return true;
    }

    let plugin_paths = [
        "/usr/lib64/gstreamer-1.0/libgstgtk4.so",
        "/usr/lib/gstreamer-1.0/libgstgtk4.so",
        "/usr/lib/x86_64-linux-gnu/gstreamer-1.0/libgstgtk4.so",
        "/usr/lib/aarch64-linux-gnu/gstreamer-1.0/libgstgtk4.so",
        "/usr/local/lib/gstreamer-1.0/libgstgtk4.so",
    ];
    plugin_paths.iter().any(|p| Path::new(p).exists())
}

pub fn probe_distro() -> Option<DistroInfo> {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .map(|c| detect_distro(&c))
}

pub fn probe_gtk_versions() -> (Option<String>, Option<String>) {
    use std::process::Command;

    let gtk = Command::new("pkg-config")
        .args(["--modversion", "gtk4"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let adw = Command::new("pkg-config")
        .args(["--modversion", "libadwaita-1"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    (gtk, adw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distro_detection() {
        let fedora = "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=41\n";
        let info = detect_distro(fedora);
        assert_eq!(info.id, "fedora");

        let ubuntu = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
        let info = detect_distro(ubuntu);
        assert_eq!(info.id, "ubuntu");
        assert_eq!(info.id_like, "debian");
    }

    #[test]
    fn test_gtk4_sink_verdict_fedora_names_gstreamer1_plugin_gtk4() {
        let distro = DistroInfo {
            id: "fedora".into(),
            id_like: "".into(),
        };
        let c = gtk4_sink_verdict(false, Some(&distro));
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("gstreamer1-plugin-gtk4"));
        assert_eq!(
            c.fix,
            Some("sudo dnf install gstreamer1-plugin-gtk4".into())
        );
    }

    #[test]
    fn test_gtk4_sink_verdict_ubuntu_names_pkg() {
        let distro = DistroInfo {
            id: "ubuntu".into(),
            id_like: "debian".into(),
        };
        let c = gtk4_sink_verdict(false, Some(&distro));
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("gstreamer1.0-gtk4"));
        assert_eq!(c.fix, Some("sudo apt install gstreamer1.0-gtk4".into()));
    }

    #[test]
    fn test_gtk4_sink_verdict_ok() {
        let c = gtk4_sink_verdict(true, None);
        assert_eq!(c.status, Status::Ok);
    }
}
