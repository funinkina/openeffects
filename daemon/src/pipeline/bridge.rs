//! Userspace latest-frame slot.
//!
//! Bridges the two streams of the on-demand virtual camera: the GStreamer
//! capture+effects pipeline writes the most recent processed frame here (via its
//! appsink callback), and the native PipeWire provide node reads it (in its
//! `process` callback). Only the newest frame is kept — for a live camera you
//! never want to queue stale frames, so an older frame is simply overwritten.
//!
//! `Bridge` is `Send + Sync` (a `Mutex` over a byte buffer) so it can be shared
//! across the GStreamer streaming thread and the PipeWire loop thread via `Arc`.

use std::sync::Mutex;

/// Latest processed frame in the fixed pipeline format (I420, see
/// [`super::FRAME_SIZE`]). `None` until the first frame arrives, or after
/// [`Bridge::clear`].
#[derive(Default)]
pub struct Bridge {
    latest: Mutex<Option<Vec<u8>>>,
}

impl Bridge {
    pub fn new() -> Self {
        Self {
            latest: Mutex::new(None),
        }
    }

    /// Store the newest processed frame, dropping any previous one.
    pub fn store(&self, frame: Vec<u8>) {
        *self.latest.lock().unwrap() = Some(frame);
    }

    /// Copy the latest frame's bytes into `dst`, returning the number of bytes
    /// written, or `None` if no frame is available yet.
    pub fn copy_latest(&self, dst: &mut [u8]) -> Option<usize> {
        let guard = self.latest.lock().unwrap();
        let frame = guard.as_ref()?;
        let n = frame.len().min(dst.len());
        dst[..n].copy_from_slice(&frame[..n]);
        Some(n)
    }

    /// Drop the held frame so a stale image is never served after the capture
    /// pipeline stops and before it produces its first new frame on reconnect.
    pub fn clear(&self) {
        *self.latest.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::Bridge;

    #[test]
    fn store_then_copy_roundtrips() {
        let bridge = Bridge::new();
        let mut dst = [0u8; 4];
        assert_eq!(bridge.copy_latest(&mut dst), None);

        bridge.store(vec![1, 2, 3, 4]);
        assert_eq!(bridge.copy_latest(&mut dst), Some(4));
        assert_eq!(dst, [1, 2, 3, 4]);

        bridge.clear();
        assert_eq!(bridge.copy_latest(&mut dst), None);
    }

    #[test]
    fn store_overwrites_previous_frame() {
        let bridge = Bridge::new();
        bridge.store(vec![9, 9]);
        bridge.store(vec![7, 7]);
        let mut dst = [0u8; 2];
        bridge.copy_latest(&mut dst);
        assert_eq!(dst, [7, 7]);
    }
}
