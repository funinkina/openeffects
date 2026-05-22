use std::{env, fs};

use gstreamer::ElementFactory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSink {
    PipeWire { node_name: String },
    V4l2Loopback { device: String },
    None,
}

impl OutputSink {
    #[allow(dead_code)]
    pub fn label(&self) -> String {
        match self {
            Self::PipeWire { node_name } => format!("pipewire:{node_name}"),
            Self::V4l2Loopback { device } => format!("v4l2:{device}"),
            Self::None => "none".into(),
        }
    }
}

pub fn probe_output_sink() -> OutputSink {
    if env::var("OPENEFFECTS_FORCE_V4L2").is_ok() {
        return try_v4l2loopback();
    }

    if ElementFactory::find("pipewiresink").is_some() {
        return OutputSink::PipeWire {
            node_name: "openeffects-virtual-camera".into(),
        };
    }

    try_v4l2loopback()
}

fn try_v4l2loopback() -> OutputSink {
    if !fs::metadata("/sys/module/v4l2loopback").is_ok() {
        return OutputSink::None;
    }

    for idx in 0..64 {
        let device = format!("/dev/video{idx}");
        if fs::metadata(&device).is_ok() {
            return OutputSink::V4l2Loopback { device };
        }
    }

    OutputSink::None
}

#[cfg(test)]
mod tests {
    use super::OutputSink;

    #[test]
    fn sink_labels_are_stable() {
        assert_eq!(
            OutputSink::PipeWire {
                node_name: "openeffects-virtual-camera".into()
            }
            .label(),
            "pipewire:openeffects-virtual-camera"
        );
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
