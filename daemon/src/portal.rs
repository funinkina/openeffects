//! xdg-desktop-portal integration, active only inside a Flatpak sandbox.
//!
//! Two portals:
//!   * **Camera** — `AccessCamera` + `OpenPipeWireRemote` yields a PipeWire
//!     remote fd that exposes only the camera nodes the user granted. The
//!     capture pipeline feeds that fd to `pipewiresrc`, which is what makes the
//!     shell's "app is using your camera" indicator light up. (The virtual
//!     *output* node still needs the full `--socket=pipewire`; the portal can't
//!     create nodes.)
//!   * **Background** — `SetStatus` gives the headless daemon a description in
//!     GNOME Shell's "Background Apps" menu, so it shows up sensibly after the
//!     GUI window is closed.
//!
//! Outside the sandbox every function here is a no-op and the pipeline falls
//! back to talking to PipeWire / V4L2 directly, exactly as before.

use std::os::fd::OwnedFd;
use std::sync::OnceLock;

use tracing::{info, warn};

/// The camera PipeWire remote fd obtained from the portal, kept alive for the
/// whole process. `Some(None)` means we asked and got nothing (no camera or
/// access denied); `None` means we never asked (not sandboxed).
static CAMERA_FD: OnceLock<Option<OwnedFd>> = OnceLock::new();

/// Whether we are running inside a Flatpak sandbox.
pub fn in_sandbox() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// Request camera access through the portal and cache the resulting PipeWire
/// remote fd. Safe to call when not sandboxed (does nothing). Call once, early,
/// before the first capture — the first call may show a permission dialog.
pub async fn init_camera() {
    if !in_sandbox() {
        return;
    }
    match ashpd::desktop::camera::request().await {
        Ok(Some(fd)) => {
            info!("camera portal granted a PipeWire remote");
            let _ = CAMERA_FD.set(Some(fd));
        }
        Ok(None) => {
            warn!("camera portal returned no camera (denied or none present)");
            let _ = CAMERA_FD.set(None);
        }
        Err(err) => {
            warn!(%err, "camera portal request failed; falling back to direct capture");
            let _ = CAMERA_FD.set(None);
        }
    }
}

/// A fresh duplicate of the portal camera fd for one `pipewiresrc`, or `None`
/// when not sandboxed / no grant. The dup is owned by the caller (hand it to
/// GStreamer, which closes it). Duplicating per source keeps the cached fd valid
/// across pipeline restarts.
pub fn dup_camera_fd() -> Option<OwnedFd> {
    match CAMERA_FD.get()?.as_ref()?.try_clone() {
        Ok(fd) => Some(fd),
        Err(err) => {
            warn!(%err, "failed to dup camera portal fd");
            None
        }
    }
}

/// Register the daemon as a background app and set its status message. No-op when
/// not sandboxed; best-effort otherwise.
///
/// `SetStatus` alone only updates the message of an app the shell already tracks;
/// it does not make the app appear in GNOME's "Background Apps" menu. The app must
/// first hold the background permission via `RequestBackground` (autostart off — we
/// are already running, just asking to keep running window-less). Only then does
/// `SetStatus` light up the menu entry.
pub async fn set_background_status(message: &str) {
    if !in_sandbox() {
        return;
    }

    match ashpd::desktop::background::Background::request()
        .reason("Apply webcam effects to the virtual camera")
        .auto_start(false)
        .send()
        .await
        .and_then(|r| r.response())
    {
        Ok(_) => {}
        Err(err) => warn!(%err, "Background portal RequestBackground failed"),
    }

    let proxy = match ashpd::desktop::background::BackgroundProxy::new().await {
        Ok(p) => p,
        Err(err) => {
            warn!(%err, "could not connect to the Background portal");
            return;
        }
    };
    if let Err(err) = proxy.set_status(message).await {
        warn!(%err, "Background portal SetStatus failed");
    }
}
