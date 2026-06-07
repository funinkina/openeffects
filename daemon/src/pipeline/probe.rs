//! The virtual camera is now published by a native PipeWire `Video/Source` node
//! (see [`super::provider`]), so there is no GStreamer output sink to probe for.
//! All that remains here is the stable node name, shared across the daemon (the
//! provider advertises it, `cameras` skips it during enumeration, and the D-Bus
//! state reports it).

pub const PIPEWIRE_NODE_NAME: &str = "openeffects";
