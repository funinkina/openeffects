//! Pure effect and inference logic: RGBA buffers in, buffers or values out. No
//! GStreamer, so it builds and tests anywhere; the daemon is thin glue over it.

pub mod effects;
pub mod gesture;
pub mod inference;
