//! Global live-preview bottom sheet. An `AdwBottomSheet` wraps the whole
//! window content, so its always-visible bottom bar is present on every page.
//! Expanding the sheet starts a GStreamer pipeline that consumes the daemon's
//! virtual-camera node (`pipewiresrc target-object=openeffects`) and renders it
//! into a `gtk::Picture` via `gtk4paintablesink`; collapsing it tears the
//! pipeline down so the real camera is released (the daemon's on-demand model
//! keeps the camera open only while a consumer is linked).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gst::prelude::*;
use gstreamer as gst;
use gtk::glib;

use crate::constants::VCAM_NODE_NAME;

pub struct Preview {
    pub sheet: adw::BottomSheet,
    picture: gtk::Picture,
    status: gtk::Label,
    /// Built lazily on first expand and reused; rebuilt if a previous attempt
    /// failed (e.g. the daemon node was not up yet).
    pipeline: RefCell<Option<gst::Pipeline>>,
}

/// Wrap `content` in a bottom sheet and return the shared `Preview`. The caller
/// puts `preview.sheet` into the window.
pub fn build(content: &impl IsA<gtk::Widget>) -> Rc<Preview> {
    // ── Sheet body: the live preview surface ────────────────────────────────
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .hexpand(true)
        .vexpand(true)
        .build();

    let status = gtk::Label::builder()
        .label("Expand to preview the processed camera feed")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    status.add_css_class("dim-label");

    // Picture and status share an overlay so the hint shows until frames flow.
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&status);

    let sheet_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sheet_body.set_height_request(280);
    sheet_body.add_css_class("view");
    sheet_body.append(&overlay);

    // ── Always-visible bottom bar (the collapsed handle) ────────────────────
    let bar_label = gtk::Label::new(Some("Preview"));
    let bar_icon = gtk::Image::from_icon_name("camera-photo-symbolic");
    let bar_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar_box.set_halign(gtk::Align::Center);
    bar_box.set_margin_top(8);
    bar_box.set_margin_bottom(8);
    bar_box.append(&bar_icon);
    bar_box.append(&bar_label);

    let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bottom_bar.append(&bar_box);
    bottom_bar.set_hexpand(true);
    bar_box.set_hexpand(true);

    let sheet = adw::BottomSheet::builder()
        .content(content)
        .sheet(&sheet_body)
        .bottom_bar(&bottom_bar)
        .reveal_bottom_bar(true)
        .build();

    let preview = Rc::new(Preview {
        sheet,
        picture,
        status,
        pipeline: RefCell::new(None),
    });

    // Tapping the bottom bar toggles the sheet open.
    {
        let preview = preview.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            preview.sheet.set_open(!preview.sheet.is_open());
        });
        bottom_bar.add_controller(gesture);
    }

    // Start/stop the pipeline strictly on expand/collapse.
    {
        let preview = preview.clone();
        preview.sheet.clone().connect_open_notify(move |sheet| {
            if sheet.is_open() {
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
        if self.pipeline.borrow().is_none() {
            match self.build_pipeline() {
                Ok(pipeline) => *self.pipeline.borrow_mut() = Some(pipeline),
                Err(err) => {
                    self.status
                        .set_label(&format!("Preview unavailable: {err}"));
                    self.status.set_visible(true);
                    return;
                }
            }
        }

        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            if let Err(err) = pipeline.set_state(gst::State::Playing) {
                self.status
                    .set_label(&format!("Could not start preview: {err}"));
                self.status.set_visible(true);
            }
        }
    }

    fn stop(&self) {
        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        self.status
            .set_label("Expand to preview the processed camera feed");
        self.status.set_visible(true);
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

        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&src, &convert, &sink])?;
        gst::Element::link_many([&src, &convert, &sink])?;

        // Hide the hint once buffers reach the sink.
        let status = self.status.downgrade();
        self.picture.connect_paintable_notify(move |picture| {
            if picture.paintable().is_some() {
                if let Some(status) = status.upgrade() {
                    status.set_visible(false);
                }
            }
        });

        Ok(pipeline)
    }
}
