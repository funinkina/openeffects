use anyhow::{anyhow, Context, Result};
use gst::prelude::*;
use gstreamer as gst;
use tracing::warn;

use super::probe::PIPEWIRE_NODE_NAME;

#[derive(Debug, Clone)]
pub struct CameraInfo {
    /// Stable identifier we persist into `app.camera.selected`.
    /// - PipeWire: `node.name` (e.g. `v4l2_input.pci-...`)
    /// - V4L2:     `/dev/videoN`
    pub id: String,
    pub name: String,
    pub path: String,
    pub api: String,
}

pub fn enumerate() -> Vec<CameraInfo> {
    let monitor = gst::DeviceMonitor::new();
    let _ = monitor.add_filter(Some("Video/Source"), None);
    if let Err(err) = monitor.start() {
        warn!(%err, "DeviceMonitor start failed");
        return Vec::new();
    }
    let devices = monitor.devices();
    monitor.stop();

    let mut cams: Vec<CameraInfo> = Vec::new();
    for device in devices.into_iter() {
        if let Some(info) = device_to_info(&device) {
            cams.push(info);
        }
    }
    dedup(cams)
}

pub fn autodetect() -> Option<CameraInfo> {
    enumerate().into_iter().next()
}

/// Build a GStreamer source element for the given camera. We always construct
/// the element ourselves (rather than calling `gst::Device::create_element`)
/// so we control which property is set: `target-object` with the stable
/// `node.name`, not a serial that gets invalidated when nodes reincarnate.
pub fn build_source_for(info: &CameraInfo) -> Result<gst::Element> {
    match info.api.as_str() {
        "pipewire" => {
            if gst::ElementFactory::find("pipewiresrc").is_none() {
                return Err(anyhow!("pipewiresrc plugin missing"));
            }
            gst::ElementFactory::make("pipewiresrc")
                .property("target-object", info.id.as_str())
                .build()
                .context("create pipewiresrc")
        }
        "v4l2" => {
            if gst::ElementFactory::find("v4l2src").is_none() {
                return Err(anyhow!("v4l2src plugin missing"));
            }
            let device_path = if info.path.is_empty() {
                info.id.as_str()
            } else {
                info.path.as_str()
            };
            gst::ElementFactory::make("v4l2src")
                .property("device", device_path)
                .build()
                .context("create v4l2src")
        }
        other => Err(anyhow!("unknown camera api: {other}")),
    }
}

fn device_to_info(device: &gst::Device) -> Option<CameraInfo> {
    let props = device.properties()?;

    let node_name = props.get::<String>("node.name").ok();
    if node_name.as_deref() == Some(PIPEWIRE_NODE_NAME) {
        return None; // never enumerate our own virtual output
    }

    let device_api = props.get::<String>("device.api").ok().unwrap_or_default();
    let v4l2_path = props
        .get::<String>("api.v4l2.path")
        .ok()
        .or_else(|| props.get::<String>("device.path").ok())
        .unwrap_or_default();

    let display = device.display_name().to_string();
    let description = props
        .get::<String>("node.description")
        .ok()
        .or_else(|| props.get::<String>("node.nick").ok())
        .unwrap_or(display);

    let (api, id, path) = if device_api == "v4l2" && !v4l2_path.is_empty() {
        // Any v4l2-backed camera — whether surfaced by v4l2deviceprovider or by
        // pipewiredeviceprovider — is reachable via v4l2src on /dev/videoN.
        // That works without any PipeWire camera-access grant.
        (String::from("v4l2"), v4l2_path.clone(), v4l2_path)
    } else if let Some(name) = node_name.clone() {
        // PipeWire node without a v4l2 backing path (true virtual sources only).
        (String::from("pipewire"), name, v4l2_path)
    } else if !v4l2_path.is_empty() {
        (String::from("v4l2"), v4l2_path.clone(), v4l2_path)
    } else {
        return None;
    };

    Some(CameraInfo {
        id,
        name: description,
        path,
        api,
    })
}

/// Collapse duplicates surfaced by both `pipewiredeviceprovider` and
/// `v4l2deviceprovider`. Two entries are dups if they share a non-empty
/// `path`. Prefer the v4l2 entry: PipeWire camera nodes typically require
/// xdg-desktop-portal / WirePlumber-granted access for non-confined clients
/// (`pipewiresrc` returns `target not found` without it), whereas `v4l2src`
/// on `/dev/videoN` works against any unclaimed device.
fn dedup(mut cams: Vec<CameraInfo>) -> Vec<CameraInfo> {
    cams.sort_by(|a, b| {
        let rank = |c: &CameraInfo| match c.api.as_str() {
            "v4l2" => 0,
            "pipewire" => 1,
            _ => 2,
        };
        rank(a).cmp(&rank(b)).then(a.id.cmp(&b.id))
    });
    let mut seen_paths: Vec<String> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    cams.retain(|c| {
        if seen_ids.iter().any(|i| i == &c.id) {
            return false;
        }
        if !c.path.is_empty() && seen_paths.iter().any(|p| p == &c.path) {
            return false;
        }
        seen_ids.push(c.id.clone());
        if !c.path.is_empty() {
            seen_paths.push(c.path.clone());
        }
        true
    });
    cams
}
