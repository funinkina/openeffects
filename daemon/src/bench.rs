//! `vinoopeneffects-daemon --bench-inference [ITERS] [FRAME.png|jpg]`: times each model
//! wrapper on one frame under whatever execution provider `VINOOPENEFFECTS_EP` selects.
//! Numbers cover the whole wrapper call (resample + `session.run` + decode).

use std::time::{Duration, Instant};

use gstreamer::prelude::*;
use gstreamer_app::AppSink;

use anyhow::{Context, Result};

use oe_core::inference::hand::{CropBox, HandLandmarker, PalmDetector};
use oe_core::inference::models::{SelfieSeg, YuNet};

const WARMUP: usize = 10;
const DEFAULT_ITERS: usize = 200;
const SCORE_THRESH: f32 = 0.5;

pub fn run(args: &[String]) -> Result<()> {
    let iters = args
        .iter()
        .find_map(|a| a.parse::<usize>().ok())
        .unwrap_or(DEFAULT_ITERS);
    let frame_path = args.iter().find(|a| a.parse::<usize>().is_err());
    let (rgba, w, h) = match frame_path {
        Some(path) => load_frame(path)?,
        None => synthetic_frame(1280, 720),
    };
    let stride = w * 4;
    println!(
        "ep={} iters={iters} warmup={WARMUP} frame={w}x{h}{}",
        oe_core::inference::requested_ep(),
        if frame_path.is_some() {
            ""
        } else {
            " (synthetic)"
        },
    );
    println!(
        "{:<20} {:>10} {:>9} {:>9} {:>9} {:>9}",
        "model", "load(ms)", "mean", "p50", "p99", "max"
    );

    let t = Instant::now();
    let mut seg = SelfieSeg::load()?;
    report("selfie_segmentation", t.elapsed(), iters, |_| {
        seg.segment(&rgba, w, h, stride).map(drop)
    })?;

    let t = Instant::now();
    let mut yunet = YuNet::load()?;
    report("yunet", t.elapsed(), iters, |_| {
        yunet.detect(&rgba, w, h, stride, SCORE_THRESH).map(drop)
    })?;

    let t = Instant::now();
    let mut palm = PalmDetector::load()?;
    report("palm_detection", t.elapsed(), iters, |_| {
        palm.detect(&rgba, w, h, stride, SCORE_THRESH).map(drop)
    })?;

    let t = Instant::now();
    let mut hand = HandLandmarker::load()?;
    let crop = CropBox {
        cx: 0.5,
        cy: 0.5,
        side: 0.4,
    };
    report("hand_landmark", t.elapsed(), iters, |_| {
        hand.infer(&rgba, w, h, stride, crop).map(drop)
    })?;
    Ok(())
}

fn report(
    name: &str,
    load: Duration,
    iters: usize,
    mut call: impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    for i in 0..WARMUP {
        call(i).with_context(|| format!("{name} warmup"))?;
    }
    let mut samples = Vec::with_capacity(iters);
    for i in 0..iters {
        let t = Instant::now();
        call(i).with_context(|| format!("{name} iteration {i}"))?;
        samples.push(t.elapsed().as_secs_f64() * 1e3);
    }
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let pct = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    println!(
        "{name:<20} {:>10.1} {mean:>9.2} {:>9.2} {:>9.2} {:>9.2}",
        load.as_secs_f64() * 1e3,
        pct(0.5),
        pct(0.99),
        samples[samples.len() - 1],
    );
    Ok(())
}

fn load_frame(path: &str) -> Result<(Vec<u8>, usize, usize)> {
    let img = image::open(path)
        .with_context(|| format!("open {path}"))?
        .to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    Ok((img.into_raw(), w, h))
}

/// Smooth gradient plus a bright blob; enough texture to keep the model from
/// taking a degenerate path, though no face or hand will be found in it.
fn synthetic_frame(w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let mut buf = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = (y * w + x) * 4;
            let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
            let blob = (-(dx * dx + dy * dy) / 40_000.0).exp();
            buf[p] = (x * 255 / w) as u8;
            buf[p + 1] = (y * 255 / h) as u8;
            buf[p + 2] = (blob * 200.0) as u8 + 30;
        }
    }
    (buf, w, h)
}

/// `vinoopeneffects-daemon --bench-video FILE [--seconds N] [--fps F]`: runs all four
/// models on every frame of a decoded clip for N seconds. `--fps` paces the
/// loop to a camera rate (sleeping between frames) so power draw reflects a
/// realistic duty cycle; without it the loop runs flat out and reports
/// throughput. Frames are decoded up front so decode cost stays out of the
/// measurement. Prints one `key=value` summary line for the harness script.
pub fn run_video(args: &[String]) -> Result<()> {
    let mut path = None;
    let mut seconds = 30.0_f64;
    let mut fps: Option<f64> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => seconds = it.next().context("--seconds needs a value")?.parse()?,
            "--fps" => fps = Some(it.next().context("--fps needs a value")?.parse()?),
            other => path = Some(other.to_string()),
        }
    }
    let path = path.context("usage: --bench-video FILE [--seconds N] [--fps F]")?;
    let (w, h) = (1280usize, 720usize);
    let frames = decode_frames(&path, w, h, 90, 8)?;
    anyhow::ensure!(!frames.is_empty(), "no frames decoded from {path}");
    let stride = w * 4;

    let mut seg = SelfieSeg::load()?;
    let mut yunet = YuNet::load()?;
    let mut palm = PalmDetector::load()?;
    let mut hand = HandLandmarker::load()?;
    let crop = CropBox {
        cx: 0.5,
        cy: 0.5,
        side: 0.4,
    };

    let mut one = |f: &[u8]| -> Result<()> {
        seg.segment(f, w, h, stride)?;
        yunet.detect(f, w, h, stride, SCORE_THRESH)?;
        palm.detect(f, w, h, stride, SCORE_THRESH)?;
        hand.infer(f, w, h, stride, crop)?;
        Ok(())
    };
    for f in frames.iter().cycle().take(WARMUP) {
        one(f)?;
    }

    let budget = fps.map(|f| Duration::from_secs_f64(1.0 / f));
    println!("# measuring");
    let cpu0 = cpu_seconds();
    let start = Instant::now();
    let mut lat = Vec::new();
    let mut deadline = start;
    for f in frames.iter().cycle() {
        if start.elapsed().as_secs_f64() >= seconds {
            break;
        }
        let t = Instant::now();
        one(f)?;
        lat.push(t.elapsed().as_secs_f64() * 1e3);
        if let Some(b) = budget {
            deadline += b;
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu0;
    lat.sort_by(f64::total_cmp);
    let n = lat.len();
    let pct = |p: f64| lat[((n - 1) as f64 * p).round() as usize];
    println!(
        "ep={} frames={n} fps={:.1} ms_mean={:.2} ms_p50={:.2} ms_p99={:.2} cpu_ms_per_frame={:.2} cpu_util_pct={:.1} wall_s={wall:.1}",
        oe_core::inference::requested_ep(),
        n as f64 / wall,
        lat.iter().sum::<f64>() / n as f64,
        pct(0.5),
        pct(0.99),
        cpu * 1e3 / n as f64,
        cpu / wall * 100.0,
    );
    Ok(())
}

fn cpu_seconds() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

/// Decode `path` to tightly packed RGBA at `w`×`h`, keeping every `keep_every`th
/// frame up to `max` frames.
fn decode_frames(
    path: &str,
    w: usize,
    h: usize,
    max: usize,
    keep_every: usize,
) -> Result<Vec<Vec<u8>>> {
    gstreamer::init()?;
    let desc = format!(
        "filesrc location=\"{path}\" ! decodebin ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,width={w},height={h} ! appsink name=sink sync=false"
    );
    let pipeline = gstreamer::parse::launch(&desc)?;
    let sink = pipeline
        .downcast_ref::<gstreamer::Bin>()
        .and_then(|b| b.by_name("sink"))
        .and_then(|e| e.downcast::<AppSink>().ok())
        .context("appsink missing from decode pipeline")?;
    pipeline.set_state(gstreamer::State::Playing)?;
    let mut frames = Vec::new();
    let mut seen = 0usize;
    while let Ok(sample) = sink.pull_sample() {
        seen += 1;
        if !(seen - 1).is_multiple_of(keep_every) {
            continue;
        }
        let buf = sample.buffer().context("sample without buffer")?;
        let map = buf.map_readable()?;
        anyhow::ensure!(
            map.len() == w * h * 4,
            "unexpected frame size {}",
            map.len()
        );
        frames.push(map.to_vec());
        if frames.len() >= max {
            break;
        }
    }
    pipeline.set_state(gstreamer::State::Null)?;
    Ok(frames)
}
