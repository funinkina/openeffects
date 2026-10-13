use super::types::{CameraClass, CameraNode, Check, Status};

pub const COLOR_FORMATS: &[&str] = &[
    "MJPG", "JPEG", "YUYV", "NV12", "NV21", "RGB3", "BGR3", "RGB4", "BGR4", "UYVY", "YV12",
    "YU12",
];

pub const IR_FORMATS: &[&str] = &["GREY", "Y16", "Y10", "Y8", "Z16", "INZI"];

pub fn classify_formats(formats: &[String], is_meta: bool) -> CameraClass {
    if is_meta || formats.is_empty() {
        return CameraClass::Metadata;
    }
    if formats.iter().any(|f| COLOR_FORMATS.contains(&f.as_str())) {
        return CameraClass::Rgb;
    }
    if formats.iter().any(|f| IR_FORMATS.contains(&f.as_str())) {
        return CameraClass::Ir;
    }
    CameraClass::Unknown
}

pub fn camera_list_verdict(cameras: &[CameraNode]) -> Check {
    if cameras.is_empty() {
        return Check::new(
            "camera",
            "devices",
            Status::Fail,
            "no cameras detected under /dev/video*",
            Some("connect a camera or verify camera permissions".into()),
        );
    }

    let summary: Vec<String> = cameras
        .iter()
        .map(|c| {
            let class_tag = match c.class {
                CameraClass::Rgb => "RGB",
                CameraClass::Ir => "IR",
                CameraClass::Metadata => "meta",
                CameraClass::Unknown => "unknown",
            };
            format!("{} [{class_tag}]", c.path)
        })
        .collect();

    Check::new(
        "camera",
        "devices",
        Status::Ok,
        format!("{} device(s): {}", cameras.len(), summary.join(", ")),
        None,
    )
}

pub fn selected_camera_verdict(selected: &str, cameras: &[CameraNode]) -> Check {
    let rgb_alt = cameras.iter().find(|c| c.class == CameraClass::Rgb);

    if selected.is_empty() {
        return match rgb_alt {
            Some(rgb) => Check::new(
                "camera",
                "selected",
                Status::Ok,
                format!("auto-select (picks RGB camera {})", rgb.path),
                None,
            ),
            None => {
                if cameras.is_empty() {
                    Check::new(
                        "camera",
                        "selected",
                        Status::Fail,
                        "no camera available to auto-select",
                        Some("connect a USB or integrated webcam".into()),
                    )
                } else {
                    Check::new(
                        "camera",
                        "selected",
                        Status::Warn,
                        "auto-select found no RGB cameras (only IR/metadata nodes present)",
                        None,
                    )
                }
            }
        };
    }

    let found = cameras
        .iter()
        .find(|c| c.path == selected || c.name == selected);

    match found {
        Some(node) => match node.class {
            CameraClass::Rgb => Check::new(
                "camera",
                "selected",
                Status::Ok,
                format!("{} ({}) is RGB", node.path, node.name),
                None,
            ),
            CameraClass::Ir => {
                let fix = rgb_alt.map(|alt| format!("vinoopeneffects-ctl camera {}", alt.path));
                let detail = match rgb_alt {
                    Some(alt) => format!(
                        "{} is an IR camera (greyscale); prefer RGB alternative ({})",
                        node.path, alt.path
                    ),
                    None => format!(
                        "{} is an IR camera (greyscale); no RGB alternative detected",
                        node.path
                    ),
                };
                Check::new("camera", "selected", Status::Warn, detail, fix)
            }
            CameraClass::Metadata => {
                let fix = rgb_alt.map(|alt| format!("vinoopeneffects-ctl camera {}", alt.path));
                Check::new(
                    "camera",
                    "selected",
                    Status::Fail,
                    format!("{} is a metadata node, not a video capture device", node.path),
                    fix,
                )
            }
            CameraClass::Unknown => Check::new(
                "camera",
                "selected",
                Status::Warn,
                format!("{} has unrecognized formats ({})", node.path, node.formats.join(", ")),
                None,
            ),
        },
        None => Check::new(
            "camera",
            "selected",
            Status::Warn,
            format!("configured camera '{selected}' was not found in enumerated devices"),
            Some("vinoopeneffects-ctl camera list".into()),
        ),
    }
}

#[cfg(target_os = "linux")]
pub fn probe_cameras() -> Vec<CameraNode> {
    use std::fs;

    let mut nodes = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/video4linux") else {
        return nodes;
    };

    let mut dev_names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("video"))
        .collect();

    dev_names.sort_by_key(|n| n.trim_start_matches("video").parse::<u32>().unwrap_or(u32::MAX));

    for dev in dev_names {
        let path = format!("/dev/{dev}");
        let sys_name = fs::read_to_string(format!("/sys/class/video4linux/{dev}/name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "Unknown".into());

        let (is_meta, formats) = query_v4l2_caps_and_formats(&path);
        let class = classify_formats(&formats, is_meta);

        nodes.push(CameraNode {
            path,
            name: sys_name,
            class,
            formats,
        });
    }

    nodes
}

#[cfg(target_os = "linux")]
fn query_v4l2_caps_and_formats(device_path: &str) -> (bool, Vec<String>) {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    #[repr(C)]
    struct v4l2_capability {
        driver: [u8; 16],
        card: [u8; 32],
        bus_info: [u8; 32],
        version: u32,
        capabilities: u32,
        device_caps: u32,
        reserved: [u32; 3],
    }

    #[repr(C)]
    struct v4l2_fmtdesc {
        index: u32,
        type_: u32,
        flags: u32,
        description: [u8; 32],
        pixelformat: u32,
        mbus_code: u32,
        reserved: [u32; 3],
    }

    const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;
    const VIDIOC_ENUM_FMT: libc::c_ulong = 0xc0405602;
    const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
    const V4L2_CAP_META_CAPTURE: u32 = 0x00800000;
    const V4L2_CAP_DEVICE_CAPS: u32 = 0x80000000;

    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(device_path)
    else {
        return (false, Vec::new());
    };
    let fd = file.as_raw_fd();

    let mut cap: v4l2_capability = unsafe { std::mem::zeroed() };
    let mut is_meta = false;
    if unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP, &mut cap) } == 0 {
        let effective = if cap.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        is_meta = (effective & V4L2_CAP_META_CAPTURE) != 0;
    }

    let mut formats = Vec::new();
    let mut idx = 0u32;
    loop {
        let mut fmt: v4l2_fmtdesc = unsafe { std::mem::zeroed() };
        fmt.index = idx;
        fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        if unsafe { libc::ioctl(fd, VIDIOC_ENUM_FMT, &mut fmt) } != 0 {
            break;
        }
        let fourcc: String = fmt
            .pixelformat
            .to_le_bytes()
            .iter()
            .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
            .collect();
        let trimmed = fourcc.trim().to_string();
        if !trimmed.is_empty() && !formats.contains(&trimmed) {
            formats.push(trimmed);
        }
        idx += 1;
        if idx > 128 {
            break;
        }
    }

    (is_meta, formats)
}

#[cfg(not(target_os = "linux"))]
pub fn probe_cameras() -> Vec<CameraNode> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_classification() {
        assert_eq!(classify_formats(&["YUYV".into(), "MJPG".into()], false), CameraClass::Rgb);
        assert_eq!(classify_formats(&["GREY".into()], false), CameraClass::Ir);
        assert_eq!(classify_formats(&["Y16".into()], false), CameraClass::Ir);
        assert_eq!(classify_formats(&[], true), CameraClass::Metadata);
        assert_eq!(classify_formats(&[], false), CameraClass::Metadata);
        assert_eq!(classify_formats(&["FOOO".into()], false), CameraClass::Unknown);
    }

    #[test]
    fn test_selected_camera_ir_warns_with_rgb_alt() {
        let cameras = vec![
            CameraNode {
                path: "/dev/video0".into(),
                name: "Integrated_Webcam_FHD".into(),
                class: CameraClass::Rgb,
                formats: vec!["YUYV".into()],
            },
            CameraNode {
                path: "/dev/video2".into(),
                name: "Integrated_Webcam_FHD".into(),
                class: CameraClass::Ir,
                formats: vec!["GREY".into()],
            },
        ];

        let check = selected_camera_verdict("/dev/video2", &cameras);
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("IR camera"));
        assert!(check.detail.contains("/dev/video0"));
        assert_eq!(check.fix, Some("vinoopeneffects-ctl camera /dev/video0".into()));
    }

    #[test]
    fn test_selected_camera_rgb_ok() {
        let cameras = vec![CameraNode {
            path: "/dev/video0".into(),
            name: "Cam".into(),
            class: CameraClass::Rgb,
            formats: vec!["MJPG".into()],
        }];
        let check = selected_camera_verdict("/dev/video0", &cameras);
        assert_eq!(check.status, Status::Ok);
    }
}
