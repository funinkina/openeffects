use gst::prelude::*;
use gstreamer as gst;

#[test]
fn videotestsrc_passthrough_delivers_five_frames() -> anyhow::Result<()> {
    gst::init()?;

    let pipeline = gst::parse::launch(
        "videotestsrc num-buffers=5 ! video/x-raw,format=I420,width=64,height=64 ! appsink name=out sync=false",
    )?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow::anyhow!("launch did not return a pipeline"))?;

    let appsink = pipeline
        .by_name("out")
        .ok_or_else(|| anyhow::anyhow!("appsink not found"))?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow::anyhow!("out is not an appsink"))?;

    pipeline.set_state(gst::State::Playing)?;

    let mut frames = 0;
    while frames < 5 {
        let sample = appsink.pull_sample()?;
        let buffer = sample
            .buffer()
            .ok_or_else(|| anyhow::anyhow!("sample without buffer"))?;
        assert!(buffer.size() > 0);
        frames += 1;
    }

    pipeline.set_state(gst::State::Null)?;
    assert_eq!(frames, 5);
    Ok(())
}
