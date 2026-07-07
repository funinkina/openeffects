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
use std::sync::{Mutex, OnceLock};

use tracing::{info, warn};

/// The camera PipeWire remote fd obtained from the portal, kept alive for the
/// whole process. `None` means not yet granted — unlike a one-shot cache we keep
/// retrying, because the grant may only land after the GUI prompts the user.
static CAMERA_FD: Mutex<Option<OwnedFd>> = Mutex::new(None);

/// Runtime handle so the sync GStreamer pipeline thread can drive the async
/// portal request on the daemon's tokio runtime.
static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Whether we are running inside a Flatpak sandbox.
pub fn in_sandbox() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// Record the tokio runtime handle (call once from `#[tokio::main]`).
pub fn set_runtime_handle(handle: tokio::runtime::Handle) {
    let _ = RT.set(handle);
}

/// Acquire the camera PipeWire remote fd through the portal if we don't have it
/// yet. No-op when not sandboxed or already granted. The first successful call
/// may show a permission dialog — but a windowless daemon can't host one, so in
/// practice the grant is primed by the GUI and this just picks it up.
pub async fn ensure_camera() {
    if !in_sandbox() || CAMERA_FD.lock().unwrap().is_some() {
        return;
    }
    match ashpd::desktop::camera::request().await {
        Ok(Some(fd)) => {
            info!("camera portal granted a PipeWire remote");
            *CAMERA_FD.lock().unwrap() = Some(fd);
        }
        Ok(None) => warn!("camera portal returned no camera (denied or none present)"),
        Err(err) => warn!(%err, "camera portal request failed; falling back to direct capture"),
    }
}

/// Blocking [`ensure_camera`] for the sync pipeline thread, driven on the tokio
/// runtime. Safe to call repeatedly; returns immediately once the fd is held.
pub fn ensure_camera_blocking() {
    if !in_sandbox() || CAMERA_FD.lock().unwrap().is_some() {
        return;
    }
    if let Some(rt) = RT.get() {
        rt.block_on(ensure_camera());
    }
}

/// A fresh duplicate of the portal camera fd for one `pipewiresrc`, or `None`
/// when not sandboxed / no grant. The dup is owned by the caller (hand it to
/// GStreamer, which closes it). Duplicating per source keeps the cached fd valid
/// across pipeline restarts.
pub fn dup_camera_fd() -> Option<OwnedFd> {
    match CAMERA_FD.lock().unwrap().as_ref()?.try_clone() {
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
/// The "Background Apps" menu entry comes from xdg-desktop-portal listing running
/// window-less instances; "running" means the host-side `flatpak run` pid is alive,
/// which is why the flatpak launcher execs this daemon as the sandbox's main
/// process. `RequestBackground` (autostart off — we are already running) stores the
/// background permission so the portal doesn't kill a window-less instance;
/// `SetStatus` attaches the message shown under the menu entry.
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
