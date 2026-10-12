//! The live preview: the large framed video that leads the main window. It
//! consumes the daemon's virtual-camera node
//! (`pipewiresrc target-object=vinoopeneffects`) and renders it into a
//! `gtk::Picture` via `gtk4paintablesink`. The pipeline runs only while the
//! preview is mapped, so hiding or closing the window releases the real
//! camera (the daemon's on-demand model keeps the camera open only while a
//! consumer is linked).
//!
//! Floating over the video: the physical-camera picker (top left), a "live"
//! pill once frames flow (top right), a hold-to-compare button (bottom left)
//! and the virtual camera's node name (bottom right).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gst::prelude::*;
use gstreamer as gst;
use gtk::glib;

use crate::constants::VCAM_NODE_NAME;
use crate::dbus_client::{CmdTx, GuiCommand};

/// How long to wait for the first frame before saying something is wrong.
/// Covers the camera opening and the models loading on first use; a missing
/// PipeWire target would otherwise sit silent for pipewiresrc's own 30 s.
const NO_FRAMES_TIMEOUT_SECS: u32 = 10;

pub struct Preview {
    /// The framed preview; place this in the window.
    pub frame: gtk::Overlay,
    /// Physical-camera picker. Its model is filled by the state-sync layer.
    pub camera_dropdown: gtk::DropDown,
    pub camera_handler: glib::SignalHandlerId,
    /// Press-and-hold shows the camera without effects; wired by the window.
    pub compare: gtk::Button,
    /// "Virtual camera live" text; shortened at narrow widths.
    pub live_label: gtk::Label,
    /// Virtual camera node-name pill.
    pub node_pill: gtk::Label,
    picture: gtk::Picture,
    status: gtk::Label,
    live_pill: gtk::Box,
    /// Centred "can't reach the virtual camera" notice with its actions.
    problem: gtk::Box,
    problem_text: gtk::Label,
    retry_btn: gtk::Button,
    restart_btn: gtk::Button,
    /// Set once the sink has produced a frame for the current attempt.
    has_frames: Rc<Cell<bool>>,
    /// Bumped on every start/stop so a stale no-frames timer does nothing.
    attempt: Cell<u32>,
    bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
    /// Built lazily on first map and reused; rebuilt if a previous attempt
    /// failed (e.g. the daemon node was not up yet).
    pipeline: RefCell<Option<gst::Pipeline>>,
}

/// Build the preview. `camera_ids` holds the camera ids in the same order as
/// the picker's model, so a selection resolves back to an id.
pub fn build(cmd_tx: &CmdTx, camera_ids: Rc<RefCell<Vec<String>>>) -> Rc<Preview> {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .hexpand(true)
        .vexpand(true)
        .build();

    let status = gtk::Label::builder()
        .label("Connecting…")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["oe-preview-status"])
        .build();

    // centre: shown when the virtual camera can't be reached
    let problem_text = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["oe-preview-status"])
        .build();
    let retry_btn = gtk::Button::builder()
        .label("Try again")
        .css_classes(["oe-pill-button"])
        .build();
    let restart_btn = gtk::Button::builder()
        .label("Restart camera services")
        .tooltip_text(
            "Restarts WirePlumber and the vinoOpenEffects daemon. \
             Other apps may briefly lose camera or audio routing.",
        )
        .css_classes(["oe-pill-button"])
        .build();
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .halign(gtk::Align::Center)
        .build();
    actions.append(&retry_btn);
    actions.append(&restart_btn);
    let problem = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    problem.append(&problem_text);
    problem.append(&actions);

    // top left: camera picker
    let camera_dropdown = gtk::DropDown::builder()
        .model(&gtk::StringList::new(&["No cameras found"]))
        .tooltip_text("Camera")
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .margin_start(14)
        .margin_top(14)
        .css_classes(["oe-pill-dropdown"])
        .build();
    let camera_handler = {
        let cmd_tx = cmd_tx.clone();
        camera_dropdown.connect_selected_notify(move |dd| {
            let ids = camera_ids.borrow();
            if let Some(id) = ids.get(dd.selected() as usize) {
                let _ = cmd_tx.send(GuiCommand::SelectCamera { id: id.clone() });
            }
        })
    };

    // top right: live indicator, shown once frames flow
    let live_label = gtk::Label::new(Some("Virtual camera live"));
    let live_pill = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .margin_end(14)
        .margin_top(14)
        .visible(false)
        .css_classes(["oe-pill"])
        .build();
    live_pill.append(
        &gtk::Box::builder()
            .valign(gtk::Align::Center)
            .css_classes(["oe-live-dot"])
            .build(),
    );
    live_pill.append(&live_label);

    // bottom left: hold to compare
    let compare_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    compare_content.append(&gtk::Image::from_icon_name(
        "object-flip-horizontal-symbolic",
    ));
    compare_content.append(&gtk::Label::new(Some("Hold to compare")));
    let compare = gtk::Button::builder()
        .child(&compare_content)
        .tooltip_text("Hold to see your camera without effects")
        .halign(gtk::Align::Start)
        .valign(gtk::Align::End)
        .margin_start(14)
        .margin_bottom(14)
        .css_classes(["oe-pill-button"])
        .build();

    // bottom right: virtual camera node name
    let node_pill = gtk::Label::builder()
        .label(VCAM_NODE_NAME)
        .tooltip_text("Pick this camera in Zoom, OBS or your browser")
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .margin_end(14)
        .margin_bottom(14)
        .css_classes(["oe-pill", "monospace"])
        .build();

    let frame = gtk::Overlay::builder()
        .child(&picture)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["oe-preview-frame"])
        .build();
    frame.add_overlay(&status);
    frame.add_overlay(&problem);
    frame.add_overlay(&camera_dropdown);
    frame.add_overlay(&live_pill);
    frame.add_overlay(&compare);
    frame.add_overlay(&node_pill);

    let preview = Rc::new(Preview {
        frame,
        camera_dropdown,
        camera_handler,
        compare,
        live_label,
        node_pill,
        picture,
        status,
        live_pill,
        problem,
        problem_text,
        retry_btn,
        restart_btn,
        has_frames: Rc::new(Cell::new(false)),
        attempt: Cell::new(0),
        bus_watch: RefCell::new(None),
        pipeline: RefCell::new(None),
    });

    {
        let weak = Rc::downgrade(&preview);
        preview.retry_btn.connect_clicked(move |_| {
            if let Some(preview) = weak.upgrade() {
                preview.restart_pipeline();
            }
        });
        let weak = Rc::downgrade(&preview);
        preview.restart_btn.connect_clicked(move |_| {
            if let Some(preview) = weak.upgrade() {
                preview.restart_services();
            }
        });
    }

    // Run the pipeline only while the preview is on screen.
    {
        let preview_map = Rc::downgrade(&preview);
        preview.frame.connect_map(move |_| {
            if let Some(preview) = preview_map.upgrade() {
                preview.start();
            }
        });
        let preview_unmap = Rc::downgrade(&preview);
        preview.frame.connect_unmap(move |_| {
            if let Some(preview) = preview_unmap.upgrade() {
                preview.stop();
            }
        });
    }

    preview
}

impl Preview {
    fn start(self: &Rc<Self>) {
        self.status.set_label("Connecting…");
        self.status.set_visible(true);
        self.live_pill.set_visible(false);
        self.problem.set_visible(false);
        self.has_frames.set(false);

        let attempt = self.attempt.get().wrapping_add(1);
        self.attempt.set(attempt);
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local_once(NO_FRAMES_TIMEOUT_SECS, move || {
            if let Some(preview) = weak.upgrade()
                && preview.attempt.get() == attempt
                && !preview.has_frames.get()
            {
                preview.show_problem("No picture from the virtual camera yet.");
            }
        });

        if self.pipeline.borrow().is_none() {
            match self.build_pipeline() {
                Ok(pipeline) => {
                    self.watch_errors(&pipeline);
                    *self.pipeline.borrow_mut() = Some(pipeline);
                }
                Err(err) => {
                    self.show_problem(&format!("Preview unavailable: {err}"));
                    return;
                }
            }
        }

        if let Some(pipeline) = self.pipeline.borrow().as_ref()
            && let Err(err) = pipeline.set_state(gst::State::Playing)
        {
            self.show_problem(&format!("Could not start preview: {err}"));
        }
    }

    /// Surface a pipeline error (typically PipeWire's "target not found")
    /// right away instead of leaving the preview stuck on "Connecting…".
    fn watch_errors(self: &Rc<Self>, pipeline: &gst::Pipeline) {
        let Some(bus) = pipeline.bus() else { return };
        let weak = Rc::downgrade(self);
        let guard = bus.add_watch_local(move |_, msg| {
            if let gst::MessageView::Error(err) = msg.view()
                && let Some(preview) = weak.upgrade()
            {
                preview.on_error(&err.error().to_string());
            }
            glib::ControlFlow::Continue
        });
        if let Ok(guard) = guard {
            *self.bus_watch.borrow_mut() = Some(guard);
        }
    }

    fn on_error(&self, message: &str) {
        self.drop_pipeline();
        self.show_problem(&if message.contains("target not found") {
            "PipeWire can't find the virtual camera. The camera service may be stuck.".to_string()
        } else {
            format!("Preview error: {message}")
        });
    }

    fn show_problem(&self, text: &str) {
        self.status.set_visible(false);
        self.live_pill.set_visible(false);
        self.problem_text.set_label(text);
        self.retry_btn.set_sensitive(true);
        self.restart_btn.set_sensitive(true);
        self.problem.set_visible(true);
    }

    fn drop_pipeline(&self) {
        self.bus_watch.borrow_mut().take();
        if let Some(pipeline) = self.pipeline.borrow_mut().take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
    }

    fn restart_pipeline(self: &Rc<Self>) {
        self.drop_pipeline();
        self.start();
    }

    /// Restart the services the preview depends on: WirePlumber (which links
    /// consumers to the virtual camera) and our daemon. Then reconnect.
    fn restart_services(self: &Rc<Self>) {
        self.drop_pipeline();
        self.attempt.set(self.attempt.get().wrapping_add(1));
        self.problem_text.set_label("Restarting camera services…");
        self.retry_btn.set_sensitive(false);
        self.restart_btn.set_sensitive(false);

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(|| {
                std::process::Command::new("systemctl")
                    .args(["--user", "restart", "wireplumber", "vinoopeneffects-daemon"])
                    .output()
            })
            .await;
            // Give the node a moment to re-register before reconnecting.
            glib::timeout_future_seconds(2).await;
            let Some(preview) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(out)) if out.status.success() => preview.restart_pipeline(),
                Ok(Ok(out)) => preview.show_problem(&format!(
                    "Restart failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                _ => preview.show_problem("Could not run systemctl to restart the services."),
            }
        });
    }

    fn stop(&self) {
        self.attempt.set(self.attempt.get().wrapping_add(1));
        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        self.live_pill.set_visible(false);
    }

    /// `pipewiresrc target-object=vinoopeneffects ! videoconvert ! gtk4paintablesink`,
    /// wiring the sink's paintable into the `Picture`.
    fn build_pipeline(&self) -> Result<gst::Pipeline, glib::BoolError> {
        let src = gst::ElementFactory::make("pipewiresrc")
            .property("target-object", VCAM_NODE_NAME)
            .build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let sink = gst::ElementFactory::make("gtk4paintablesink").build()?;

        let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
        self.picture.set_paintable(Some(&paintable));

        // Swap the hint for the live pill once the first frame arrives.
        let status = self.status.downgrade();
        let live_pill = self.live_pill.downgrade();
        let problem = self.problem.downgrade();
        let has_frames = self.has_frames.clone();
        paintable.connect_invalidate_contents(move |_| {
            has_frames.set(true);
            if let Some(problem) = problem.upgrade() {
                problem.set_visible(false);
            }
            if let Some(status) = status.upgrade() {
                status.set_visible(false);
            }
            if let Some(live_pill) = live_pill.upgrade() {
                live_pill.set_visible(true);
            }
        });

        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&src, &convert, &sink])?;
        gst::Element::link_many([&src, &convert, &sink])?;

        Ok(pipeline)
    }
}
