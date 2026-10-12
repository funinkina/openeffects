//! `openeffectsd --bench-inference [ITERS] [FRAME.png|jpg]`: times each model
//! wrapper on one frame under whatever execution provider `OE_EP` selects.
//! Numbers cover the whole wrapper call (resample + `session.run` + decode).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::engine::{SelfieSeg, YuNet};
use super::hand::{CropBox, HandLandmarker, PalmDetector};

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
        super::requested_ep(),
        if frame_path.is_some() { "" } else { " (synthetic)" },
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
    let crop = CropBox { cx: 0.5, cy: 0.5, side: 0.4 };
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
