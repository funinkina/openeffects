//! Global live-preview side pane. An `AdwOverlaySplitView` wraps the whole
//! window content with a collapsible sidebar on the right, so it is present on
//! every page and never covers the page controls or the narrow view-switcher.
//! A header toggle button reveals it. Revealing starts a GStreamer pipeline
//! that consumes the daemon's virtual-camera node
//! (`pipewiresrc target-object=openeffects`) and renders it into a
//! `gtk::Picture` via `gtk4paintablesink`; hiding it tears the pipeline down so
//! the real camera is released (the daemon's on-demand model keeps the camera
//! open only while a consumer is linked).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gst::prelude::*;
use gstreamer as gst;
use gtk::glib;

use crate::constants::VCAM_NODE_NAME;

pub struct Preview {
    pub split: adw::OverlaySplitView,
    /// Header button that reveals/hides the pane; bound to `show-sidebar`.
    pub toggle: gtk::ToggleButton,
    picture: gtk::Picture,
    status: gtk::Label,
    /// Built lazily on first reveal and reused; rebuilt if a previous attempt
    /// failed (e.g. the daemon node was not up yet).
    pipeline: RefCell<Option<gst::Pipeline>>,
}

/// Wrap `content` in a split view with the preview as the right sidebar and
/// return the shared `Preview`. The caller puts `preview.split` into the window
/// and `preview.toggle` into the header bar.
pub fn build(content: &impl IsA<gtk::Widget>) -> Rc<Preview> {
    // ── Pane body: the live preview surface ─────────────────────────────────
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .hexpand(true)
        .vexpand(true)
        .build();

    let status = gtk::Label::builder()
        .label("Connecting…")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    status.add_css_class("dim-label");

    // Picture and status share an overlay so the hint shows until frames flow.
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&status);

    let pane_header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .title_widget(&adw::WindowTitle::new("Preview", ""))
        .build();

    let pane = adw::ToolbarView::new();
    pane.add_top_bar(&pane_header);
    pane.set_content(Some(&overlay));

    // ── Split view: page content + collapsible left sidebar ─────────────────
    let split = adw::OverlaySplitView::builder()
        .content(content)
        .sidebar(&pane)
        .sidebar_position(gtk::PackType::Start)
        .show_sidebar(false)
        .min_sidebar_width(280.0)
        .max_sidebar_width(420.0)
        .sidebar_width_fraction(0.4)
        .build();

    // ── Header toggle that reveals the pane ─────────────────────────────────
    let toggle = gtk::ToggleButton::builder()
        .icon_name("camera-photo-symbolic")
        .tooltip_text("Toggle live preview")
        .build();
    toggle
        .bind_property("active", &split, "show-sidebar")
        .bidirectional()
        .sync_create()
        .build();

    let preview = Rc::new(Preview {
        split,
        toggle,
        picture,
        status,
        pipeline: RefCell::new(None),
    });

    // Start/stop the pipeline strictly on reveal/hide.
    {
        let preview = preview.clone();
        preview
            .split
            .clone()
            .connect_show_sidebar_notify(move |split| {
                if split.shows_sidebar() {
                    preview.start();
                } else {
                    preview.stop();
                }
            });
    }

    preview
}

impl Preview {
    fn start(&self) {
        self.status.set_label("Connecting…");
        self.status.set_visible(true);

        if self.pipeline.borrow().is_none() {
            match self.build_pipeline() {
                Ok(pipeline) => *self.pipeline.borrow_mut() = Some(pipeline),
                Err(err) => {
                    self.status
                        .set_label(&format!("Preview unavailable: {err}"));
                    return;
                }
            }
        }

        if let Some(pipeline) = self.pipeline.borrow().as_ref()
            && let Err(err) = pipeline.set_state(gst::State::Playing)
        {
            self.status
                .set_label(&format!("Could not start preview: {err}"));
        }
    }

    fn stop(&self) {
        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            let _ = pipeline.set_state(gst::State::Null);
        }
    }

    /// `pipewiresrc target-object=openeffects ! videoconvert ! gtk4paintablesink`,
    /// wiring the sink's paintable into the `Picture`.
    fn build_pipeline(&self) -> Result<gst::Pipeline, glib::BoolError> {
        let src = gst::ElementFactory::make("pipewiresrc")
            .property("target-object", VCAM_NODE_NAME)
            .build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let sink = gst::ElementFactory::make("gtk4paintablesink").build()?;

        let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
        self.picture.set_paintable(Some(&paintable));

        // Hide the hint once the first frame invalidates the paintable.
        let status = self.status.downgrade();
        paintable.connect_invalidate_contents(move |_| {
            if let Some(status) = status.upgrade() {
                status.set_visible(false);
            }
        });

        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&src, &convert, &sink])?;
        gst::Element::link_many([&src, &convert, &sink])?;

        Ok(pipeline)
    }
}
