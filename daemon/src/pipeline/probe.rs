use std::fs;

use gstreamer::ElementFactory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSink {
    V4l2Loopback { device: String },
    None,
}

impl OutputSink {
    #[allow(dead_code)]
    pub fn label(&self) -> String {
        match self {
            Self::V4l2Loopback { device } => format!("v4l2:{device}"),
            Self::None => "none".into(),
        }
    }
}

pub fn probe_output_sink() -> OutputSink {
    if ElementFactory::find("v4l2sink").is_none() {
        return OutputSink::None;
    }
    try_v4l2loopback()
}

fn try_v4l2loopback() -> OutputSink {
    if fs::metadata("/sys/module/v4l2loopback").is_err() {
        return OutputSink::None;
    }

    for idx in 0..64 {
        let device = format!("/dev/video{idx}");
        if fs::metadata(&device).is_ok() && is_v4l2loopback_device(idx) {
            return OutputSink::V4l2Loopback { device };
        }
    }

    OutputSink::None
}


fn is_v4l2loopback_device(idx: u32) -> bool {
    let sysfs = format!("/sys/class/video4linux/video{idx}/device/driver/module");
    fs::read_link(&sysfs)
        .ok()
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "v4l2loopback")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::OutputSink;

    #[test]
    fn sink_labels_are_stable() {
        assert_eq!(
            OutputSink::V4l2Loopback {
                device: "/dev/video9".into()
            }
            .label(),
            "v4l2:/dev/video9"
        );
        assert_eq!(OutputSink::None.label(), "none");
    }
}
