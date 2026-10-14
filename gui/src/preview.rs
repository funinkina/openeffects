//! Global live-preview side pane. An `AdwOverlaySplitView` wraps the whole
//! window content with a collapsible sidebar on the right, so it is present on
//! every page and never covers the page controls or the narrow view-switcher.
//! A header toggle button reveals it. Revealing starts a GStreamer pipeline
//! that consumes the daemon's virtual-camera node
//! (`pipewiresrc target-object=openeffects`) and renders it into a
//! `gtk::Picture` via an RGBA `appsink` -> `gdk::MemoryTexture` (only needs
//! gst-plugins-base; `gtk4paintablesink` isn't packaged on Ubuntu 24.04 or
//! older Debian); hiding it tears the pipeline down so
//! the real camera is released (the daemon's on-demand model keeps the camera
//! open only while a consumer is linked).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
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
    // Pane body: the live preview surface
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

    // split view: page content + collapsible left sidebar
    let split = adw::OverlaySplitView::builder()
        .content(content)
        .sidebar(&pane)
        .sidebar_position(gtk::PackType::Start)
        .show_sidebar(false)
        .min_sidebar_width(280.0)
        .max_sidebar_width(420.0)
        .sidebar_width_fraction(0.4)
        .build();

    // header toggle that reveals the pane
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

    /// `pipewiresrc target-object=openeffects ! videoconvert ! appsink(RGBA)`.
    /// The streaming thread hands each sample to the main loop over a 1-slot
    /// channel (frames the UI can't keep up with are dropped), where it is
    /// wrapped zero-copy in a `MemoryTexture` and set on the `Picture`.
    fn build_pipeline(&self) -> Result<gst::Pipeline, glib::BoolError> {
        let src = gst::ElementFactory::make("pipewiresrc")
            .property("target-object", VCAM_NODE_NAME)
            .build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let sink = gst_app::AppSink::builder()
            .caps(
                &gst::Caps::builder("video/x-raw")
                    .field("format", "RGBA")
                    .build(),
            )
            .max_buffers(1)
            .drop(true)
            .build();

        let (tx, rx) = async_channel::bounded::<gst::Sample>(1);
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let _ = tx.try_send(sample);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let picture = self.picture.downgrade();
        let status = self.status.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(sample) = rx.recv().await {
                let (Some(picture), Some(texture)) = (picture.upgrade(), sample_texture(&sample))
                else {
                    continue;
                };
                picture.set_paintable(Some(&texture));
                // Hide the hint once frames flow.
                if let Some(status) = status.upgrade() {
                    status.set_visible(false);
                }
            }
        });

        let sink = sink.upcast::<gst::Element>();
        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&src, &convert, &sink])?;
        gst::Element::link_many([&src, &convert, &sink])?;

        Ok(pipeline)
    }
}

/// Wrap an RGBA sample's buffer in a texture without copying.
fn sample_texture(sample: &gst::Sample) -> Option<gtk::gdk::MemoryTexture> {
    let s = sample.caps()?.structure(0)?;
    let (w, h) = (s.get::<i32>("width").ok()?, s.get::<i32>("height").ok()?);
    let buf = sample.buffer_owned()?.into_mapped_buffer_readable().ok()?;
    let stride = buf.len() / h.max(1) as usize;
    Some(gtk::gdk::MemoryTexture::new(
        w,
        h,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(buf),
        stride,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_sample_becomes_texture() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(
            "videotestsrc num-buffers=1 ! video/x-raw,width=321,height=181 ! videoconvert \
             ! appsink name=sink caps=video/x-raw,format=RGBA",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let sink = pipeline
            .by_name("sink")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let sample = sink.pull_sample().unwrap();
        let tex = sample_texture(&sample).unwrap();
        pipeline.set_state(gst::State::Null).unwrap();
        assert_eq!((tex.width(), tex.height()), (321, 181));
    }
}
