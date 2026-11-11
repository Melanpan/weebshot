//! Local camera-pan reconstruction, independent of egui and libmpv.
mod compose;
mod decode;
mod foreground;
mod layers;
mod motion;
mod png;
mod psd;
#[cfg(test)]
mod tests;
mod zoom;

use anyhow::{Context, Result, bail};
use image::{
    GrayImage, RgbImage, RgbaImage,
    imageops::{self, FilterType},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

const MAX_SAMPLES: usize = 1000;
const MAX_SELECTED: usize = 96;
const MAX_PIXELS: u64 = 32_000_000;
const MAX_EDGE: u32 = 16384;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    Automatic,
    Translation,
    TwoVerticalLayers,
    ShiftAndScale,
    LayeredMotion,
}

impl Method {
    pub const ALL: [Self; 5] = [
        Self::Automatic,
        Self::Translation,
        Self::TwoVerticalLayers,
        Self::ShiftAndScale,
        Self::LayeredMotion,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Translation => "Single translation",
            Self::TwoVerticalLayers => "Two vertical layers",
            Self::ShiftAndScale => "Shift and scale",
            Self::LayeredMotion => "Layered motion (experimental)",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub method: Method,
    pub layer_divider_percent: Option<u32>,
    pub prefer_background: bool,
    pub sampling_fps: f64,
    pub search_seconds: f64,
    pub allow_pauses: bool,
    pub selection_travel: f32,
    // Advanced matching thresholds.
    pub pause_seconds: f64,
    pub min_travel: f32,
    pub thresholds: motion::Thresholds,
    pub native_thresholds: motion::Thresholds,
    pub direction_tolerance: f32,
    pub min_edge_agreement: f32,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            method: Method::default(),
            layer_divider_percent: None,
            prefer_background: false,
            sampling_fps: 8.0,
            search_seconds: 15.0,
            allow_pauses: true,
            pause_seconds: 0.75,
            min_travel: 0.025,
            selection_travel: 0.08,
            thresholds: Default::default(),
            native_thresholds: motion::Thresholds {
                max_error: 8.0,
                min_support: 0.8,
                ..Default::default()
            },
            direction_tolerance: 0.65,
            min_edge_agreement: 0.7,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub input: PathBuf,
    pub playhead: f64,
    pub duration: f64,
    /// Explicit In/Out always wins over boundary discovery.
    pub range: Option<(f64, f64)>,
    pub source_fps: f64,
    pub options: Options,
}

/// Scratch PNG lives as long as a preview or an outstanding save needs it.
/// No native-resolution image crosses the worker/UI boundary.
#[derive(Debug)]
pub struct Artifact(pub PathBuf);
impl Drop for Artifact {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub struct FrameChoice {
    pub time: f64,
    pub thumbnail: RgbImage,
    sample: usize,
    position: Position,
    secondary: Option<Position>,
    scale: f32,
}

/// Retain only small thumbnails and solved positions, never native frame caches.
/// Frame edits can reuse alignment and decode just the included source frames.
pub struct Recipe {
    pub frames: Vec<FrameChoice>,
    input: PathBuf,
    start: f64,
    end: f64,
    fps: f64,
    native: (u32, u32),
    aspect: (u32, u32),
    confidence: f32,
    omitted: usize,
    truncated: bool,
    split: Option<u32>,
    method: Method,
}

pub struct Output {
    pub artifact: Arc<Artifact>,
    pub preview: RgbaImage,
    pub dimensions: (u32, u32),
    pub pixel_aspect: f32,
    pub frames: usize,
    pub direction: &'static str,
    pub confidence: f32,
    pub warning: String,
    pub method: Method,
    pub omitted_frames: usize,
    pub search_limited: bool,
    pub range: (f64, f64),
    pub elapsed: f32,
    pub recipe: Arc<Recipe>,
    pub included: Vec<bool>,
}

pub fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Position {
    x: i32,
    y: i32,
}
impl Position {
    fn plus(self, m: motion::Motion) -> Self {
        Self {
            x: self.x + m.dx,
            y: self.y + m.dy,
        }
    }
    fn delta(self, other: Self) -> (i32, i32) {
        (self.x - other.x, self.y - other.y)
    }
}

/// Extend from a seed, stopping before scene cuts, reversals, or long holds.
/// Motion on either side is interpreted in increasing timestamp order.
fn region(
    motions: &[Option<motion::Motion>],
    seed: usize,
    options: &Options,
    fps: f64,
) -> Result<(usize, usize)> {
    let pause_limit = if options.allow_pauses {
        (options.pause_seconds * fps).ceil() as usize
    } else {
        0
    };
    let radius = (fps * options.pause_seconds.max(0.5)).ceil() as usize;
    let mut nearby: Vec<_> =
        (seed.saturating_sub(radius)..(seed + radius + 1).min(motions.len())).collect();
    nearby.sort_by_key(|&i| i.abs_diff(seed));
    let reference=nearby.into_iter().filter_map(|i|motions[i])
        .find(|m|m.reliable(&options.thresholds) && m.moving())
        .context("No coherent camera pan near the playhead. Try marking In and Out, then Stitch Selection.")?;
    let compatible = |m: motion::Motion| {
        let dot = m.dx * reference.dx + m.dy * reference.dy;
        let cross = (m.dx * reference.dy - m.dy * reference.dx).abs();
        !m.moving() || (dot > 0 && cross as f32 <= dot as f32 * options.direction_tolerance + 1.0)
    };
    let walk = |back: bool| {
        let mut frame = seed;
        let mut last = seed;
        let mut pauses = 0;
        loop {
            let pair = if back {
                if frame == 0 {
                    break;
                }
                frame - 1
            } else {
                if frame >= motions.len() {
                    break;
                }
                frame
            };
            let Some(m) =
                motions[pair].filter(|m| m.reliable(&options.thresholds) && compatible(*m))
            else {
                break;
            };
            frame = if back { frame - 1 } else { frame + 1 };
            if m.moving() {
                pauses = 0;
                last = frame;
            } else {
                pauses += 1;
                if pauses > pause_limit {
                    break;
                }
                last = frame;
            }
        }
        last
    };
    Ok((walk(true), walk(false)))
}

fn positions(
    frames: &[GrayImage],
    motions: &[Option<motion::Motion>],
    lo: usize,
    hi: usize,
    cancel: &AtomicBool,
    options: &Options,
) -> Result<Vec<Position>> {
    let mut positions = vec![Position::default()];
    for i in lo + 1..=hi {
        check_cancel(cancel)?;
        let m=motions[i-1].filter(|m|m.reliable(&options.thresholds)).context("Scene cut, insufficient overlap, or excessive animation in selection. Narrow the In/Out range.")?;
        let mut pos = positions[i - lo - 1].plus(m);
        // Recheck a nonadjacent overlapping frame. Match the oldest nearby reference
        // directly instead of integrating all intervening rounded displacements.
        for j in (lo..i.saturating_sub(1))
            .rev()
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let delta = pos.delta(positions[j - lo]);
            if delta.0.abs() < (frames[i].width() / 4) as i32
                && delta.1.abs() < (frames[i].height() / 4) as i32
            {
                let direct = motion::refine(&frames[j], &frames[i], delta, 3, &options.thresholds);
                if direct.reliable(&options.thresholds) {
                    pos = positions[j - lo].plus(direct);
                    break;
                }
            }
        }
        positions.push(pos);
    }
    Ok(positions)
}

fn select(positions: &[Position], size: (u32, u32), options: &Options) -> Vec<usize> {
    let mut selected = vec![0];
    for i in 1..positions.len() {
        let last = *selected.last().unwrap();
        let (dx, dy) = positions[i].delta(positions[last]);
        let travel = (dx.abs() as f32 / size.0 as f32).max(dy.abs() as f32 / size.1 as f32);
        if travel >= options.selection_travel.clamp(0.02, 0.25) {
            selected.push(i);
        }
    }
    let last = positions.len() - 1;
    if positions[last] != positions[*selected.last().unwrap()] {
        selected.push(last);
    }
    selected
}

/// Pick one connected, consistently aligned sequence. Rank by revealed area,
/// rather than frame count, so a long stationary tail cannot beat a real pan.
fn best_component(
    positions: &[Position],
    groups: &[usize],
    size: (u32, u32),
    min_travel: f32,
) -> Option<usize> {
    (0..=*groups.last()?)
        .filter_map(|group| {
            let points: Vec<_> = positions
                .iter()
                .zip(groups)
                .filter(|(_, g)| **g == group)
                .map(|(p, _)| *p)
                .collect();
            if points.len() < 2 {
                return None;
            }
            let minx = points.iter().map(|p| p.x).min()?;
            let maxx = points.iter().map(|p| p.x).max()?;
            let miny = points.iter().map(|p| p.y).min()?;
            let maxy = points.iter().map(|p| p.y).max()?;
            let dx = (maxx - minx) as f32 / size.0 as f32;
            let dy = (maxy - miny) as f32 / size.1 as f32;
            if dx.max(dy) < min_travel {
                return None;
            }
            Some((group, dx + dy))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(group, _)| group)
}

fn bounds(positions: &[Position], size: (u32, u32)) -> Result<(Position, (u32, u32))> {
    let min = Position {
        x: positions.iter().map(|p| p.x).min().unwrap_or(0),
        y: positions.iter().map(|p| p.y).min().unwrap_or(0),
    };
    let max = Position {
        x: positions.iter().map(|p| p.x).max().unwrap_or(0),
        y: positions.iter().map(|p| p.y).max().unwrap_or(0),
    };
    let w = (max.x as i64 - min.x as i64 + size.0 as i64) as u64;
    let h = (max.y as i64 - min.y as i64 + size.1 as i64) as u64;
    if w > MAX_EDGE as u64 || h > MAX_EDGE as u64 || w * h > MAX_PIXELS {
        bail!("Stitch exceeds the 32 megapixel / 16384 pixel edge limit. Select a shorter range.");
    }
    Ok((min, (w as u32, h as u32)))
}

pub fn run(
    ffmpeg: &std::path::Path,
    id: u64,
    job: &Job,
    cancel: &AtomicBool,
    report: impl Fn(f32, &'static str),
) -> Result<Output> {
    let started = Instant::now();
    check_cancel(cancel)?;
    if !job.duration.is_finite() || job.duration <= 0.0 || !job.playhead.is_finite() {
        bail!("Video duration is unavailable");
    }
    let options = &job.options;
    let search = options.search_seconds.clamp(1.0, 30.0);
    let (start, end) = job.range.unwrap_or((
        (job.playhead - search).max(0.0),
        (job.playhead + search).min(job.duration),
    ));
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end > job.duration + 0.01
        || end <= start
        || end - start > 60.0
    {
        bail!("Choose a valid In/Out range of at most 60 seconds");
    }
    let experimental = options.method == Method::LayeredMotion;
    if experimental && job.range.is_none() {
        bail!("Mark In and Out around one vertical pan to use Layered motion.");
    }
    let initial_fps = options
        .sampling_fps
        .clamp(2.0, 16.0)
        .min(job.source_fps.max(2.0));
    report(0.0, "Analysing camera pan");
    let (first, aspect) = decode::frame(ffmpeg, &job.input, start, cancel)?;
    let native = first.dimensions();
    drop(first);
    let scale = if experimental { 640.0 } else { 320.0 } / native.0.max(native.1) as f64;
    let size = (
        (native.0 as f64 * scale).round().max(24.0) as u32,
        (native.1 as f64 * scale).round().max(24.0) as u32,
    );
    let mut fps = if experimental { 2.0 } else { initial_fps };
    let (frames, motions, candidate) = loop {
        let mut frames = Vec::new();
        decode::samples(ffmpeg, &job.input, start, end, fps, size, cancel, |f| {
            if frames.len() >= MAX_SAMPLES {
                bail!("Too many analysis frames. Reduce the selected range or sampling rate.");
            }
            frames.push(f);
            report(
                0.15 * (frames.len() as f32 / ((end - start) * fps) as f32).min(1.0),
                "Decoding analysis frames",
            );
            Ok(())
        })?;
        if frames.len() < 2 {
            bail!("The range is too short to stitch");
        }
        if matches!(
            options.method,
            Method::TwoVerticalLayers | Method::ShiftAndScale | Method::LayeredMotion
        ) {
            break (frames, Vec::new(), None);
        }
        let mut motions = Vec::new();
        for i in 1..frames.len() {
            check_cancel(cancel)?;
            motions.push(motion::estimate(
                &frames[i - 1],
                &frames[i],
                &options.thresholds,
            ));
            report(
                0.15 + 0.25 * i as f32 / frames.len() as f32,
                "Detecting motion and scene boundaries",
            );
        }
        // A slow pan can move less than half an analysis pixel per sample. Use
        // a longer baseline for discovery across reliable adjacent holds. Position
        // solving still uses original pairs and nonadjacent reference checks.
        let mut discovery = motions.clone();
        let baseline = (fps * 0.5).ceil().max(2.0) as usize;
        for i in 0..motions.len() {
            check_cancel(cancel)?;
            let j = (i + baseline).min(frames.len() - 1);
            if j - i == baseline
                && motions[i..j]
                    .iter()
                    .all(|m| m.is_some_and(|m| m.reliable(&options.thresholds) && !m.moving()))
            {
                let m = motion::refine(&frames[i], &frames[j], (0, 0), 4, &options.thresholds);
                if m.reliable(&options.thresholds) && m.moving() {
                    discovery[i] = Some(m);
                }
            }
        }
        let seed = (((job.playhead - start) * fps).round() as usize).min(frames.len() - 1);
        let candidate = if job.range.is_some() {
            Ok((0, frames.len() - 1))
        } else {
            region(&discovery, seed, options, fps)
        };
        let bad = candidate.as_ref().is_err()
            || candidate.as_ref().is_ok_and(|&(lo, hi)| {
                motions[lo..hi]
                    .iter()
                    .any(|m| m.is_none_or(|m| !m.reliable(&options.thresholds)))
            });
        // A second decode at twice the temporal density recovers fast pans, up to
        // the source frame rate and a strict bounded analysis cache.
        let next = (fps * 2.0).min(job.source_fps).min(32.0);
        if bad && next > fps + 0.1 && (end - start) * next < MAX_SAMPLES as f64 {
            fps = next;
            report(0.0, "Retrying with closer frames");
            continue;
        }
        break (frames, motions, Some(candidate));
    };
    let base_recipe = || Recipe {
        frames: Vec::new(),
        input: job.input.clone(),
        start,
        end,
        fps,
        native,
        aspect,
        confidence: 1.0,
        omitted: 0,
        truncated: job.range.is_none() && (start > 0.001 || end < job.duration - 0.001),
        split: None,
        method: Method::Translation,
    };
    let compose_recipe = |recipe: Recipe| -> Result<Output> {
        let recipe = Arc::new(recipe);
        let included = vec![true; recipe.frames.len()];
        let mut output = recompose(ffmpeg, id, recipe, included, cancel, |fraction, stage| {
            report(0.75 + 0.25 * fraction, stage)
        })?;
        output.elapsed = started.elapsed().as_secs_f32();
        Ok(output)
    };
    let layered = || -> Result<Option<Output>> {
        if options.method == Method::Translation {
            return Ok(None);
        }
        report(0.65, "Checking for independently moving layers");
        layers::recipe(&frames, base_recipe(), ffmpeg, options, cancel, &report)?
            .map(compose_recipe)
            .transpose()
    };
    let Some(candidate) = candidate else {
        if experimental {
            return compose_recipe(foreground::recipe(
                &frames,
                base_recipe(),
                ffmpeg,
                options,
                cancel,
                &report,
            )?);
        }
        if options.method == Method::ShiftAndScale {
            return compose_recipe(zoom::recipe(
                &frames,
                base_recipe(),
                ffmpeg,
                options,
                cancel,
                &report,
            )?);
        }
        return layered()?.context(layers::LAYER_ALIGNMENT_ERROR);
    };
    let analysis = candidate.and_then(|(lo, hi)| {
        if hi <= lo {
            bail!("No camera movement detected. Try marking In and Out around the pan.");
        }
        let points = positions(&frames, &motions, lo, hi, cancel, options)?;
        let (dx, dy) = points.last().unwrap().delta(points[0]);
        if (dx.abs() as f32 / size.0 as f32).max(dy.abs() as f32 / size.1 as f32)
            < options.min_travel
        {
            bail!(
                "No useful camera movement detected. Select a range with newly revealed artwork."
            );
        }
        Ok((lo, hi, points))
    });
    let (lo, hi, positions) = match analysis {
        Ok(analysis) => analysis,
        Err(error) => return layered()?.ok_or(error),
    };
    // Manual selections must also be coherent: never silently stitch a reversal.
    let mut reference = None;
    for m in &motions[lo..hi] {
        let m = m
            .filter(|m| m.reliable(&options.thresholds))
            .context("Unreliable alignment or scene cut. Select a shorter range.")?;
        if m.moving() {
            let r = reference.get_or_insert(m);
            if m.dx * r.dx + m.dy * r.dy < 0 {
                return layered()?
                    .context("Pan changes direction. Select one continuous direction.");
            }
        }
    }
    let selected = select(&positions, size, options);
    if selected.len() < 2 {
        bail!("Not enough new image content");
    }
    if selected.len() > MAX_SELECTED {
        bail!("Too many frames needed. Select a shorter range.");
    }
    let sx = native.0 as f64 / size.0 as f64;
    let sy = native.1 as f64 / size.1 as f64;
    let mut native_positions: Vec<Position> = Vec::new();
    // Keep at most three native grayscale reference frames. The oldest one still
    // overlapping by at least 60% anchors a direct high precision match.
    let mut anchors: std::collections::VecDeque<(usize, motion::AlignmentFrame)> =
        std::collections::VecDeque::new();
    let mut groups = Vec::new();
    let mut group = 0usize;
    let mut qualities = Vec::new();
    let mut thumbnails = Vec::new();
    let indices: Vec<_> = selected.iter().map(|i| lo + i).collect();
    report(0.4, "Aligning selected frames at source resolution");
    decode::selected_frames(
        ffmpeg,
        &job.input,
        start,
        end,
        fps,
        &indices,
        native,
        cancel,
        |n, rgb| {
            let i = selected[n];
            check_cancel(cancel)?;
            report(
                0.4 + 0.3 * n as f32 / selected.len() as f32,
                "Aligning selected frames at source resolution",
            );
            if rgb.dimensions() != native {
                bail!("Video dimensions change within this scene");
            }
            thumbnails.push(imageops::thumbnail(&rgb, 160, 90));
            let gray = motion::grayscale(&rgb);
            drop(rgb);
            let prepared = motion::AlignmentFrame::new(gray);
            let mut pos = Position::default();
            let mut quality = 1.0f32;
            let mut matched = n == 0;
            for (anchor, a) in &anchors {
                check_cancel(cancel)?;
                // Compare analysis positions in the same coordinate system. A
                // refined native anchor must not be subtracted from a drifting
                // coarse global position to form the next search centre.
                let coarse = positions[i].delta(positions[selected[*anchor]]);
                let delta = (
                    (coarse.0 as f64 * sx).round() as i32,
                    (coarse.1 as f64 * sy).round() as i32,
                );
                if delta.0.abs() > native.0 as i32 * 2 / 5
                    || delta.1.abs() > native.1 as i32 * 2 / 5
                {
                    continue;
                }
                let m = motion::align(
                    a,
                    &prepared,
                    delta,
                    (sx.max(sy) * 10.0).ceil().clamp(8.0, 96.0) as i32,
                    &options.native_thresholds,
                );
                let edge_agreement = if m.reliable(&options.native_thresholds) {
                    a.edge_agreement(&prepared, m)
                } else {
                    0.0
                };
                crate::log::log(
                    crate::log::Level::Debug,
                    "stitch",
                    format!(
                        "frame {n}, reference {anchor}, estimate {delta:?}, native {m:?}, edges {edge_agreement:.3}"
                    ),
                );
                if m.reliable(&options.native_thresholds)
                    && edge_agreement >= options.min_edge_agreement
                {
                    pos = native_positions[*anchor].plus(m);
                    quality = (1.0 - m.error / 36.0) * m.support;
                    matched = true;
                    break;
                }
            }
            if !matched {
                // Pose changes can invalidate an early frame while leaving a
                // perfectly stitchable pan later. Start a new candidate rather
                // than anchoring every subsequent match to that first pose.
                group += 1;
                anchors.clear();
            }
            groups.push(group);
            qualities.push(quality);
            native_positions.push(pos);
            if anchors.len() == 3 {
                anchors.pop_front();
            }
            anchors.push_back((n, prepared));
            Ok(())
        },
    )?;
    drop(anchors);
    let original_count = selected.len();
    let best = best_component(&native_positions, &groups, native, options.min_travel);
    let Some(best) = best else {
        let error = if options.method == Method::Translation {
            "The frames could not be aligned with a single shift per frame. Try Automatic or mark a shorter In/Out range."
        } else {
            layers::ALIGNMENT_ERROR
        };
        return layered()?.context(error);
    };
    let keep: Vec<_> = (0..selected.len()).filter(|&n| groups[n] == best).collect();
    drop(frames);
    let confidence = keep.iter().map(|&n| qualities[n]).fold(1.0f32, f32::min);
    let recipe = Arc::new(Recipe {
        frames: keep
            .iter()
            .map(|&n| FrameChoice {
                time: start + (lo + selected[n]) as f64 / fps,
                thumbnail: thumbnails[n].clone(),
                sample: lo + selected[n],
                position: native_positions[n],
                secondary: None,
                scale: 1.0,
            })
            .collect(),
        input: job.input.clone(),
        start,
        end,
        fps,
        native,
        aspect,
        confidence,
        omitted: original_count - keep.len(),
        truncated: job.range.is_none()
            && ((lo == 0 && start > 0.001) || (hi == motions.len() && end < job.duration - 0.001)),
        split: None,
        method: Method::Translation,
    });
    let included = vec![true; recipe.frames.len()];
    let mut out = recompose(ffmpeg, id, recipe, included, cancel, |fraction, stage| {
        report(0.7 + 0.3 * fraction, stage)
    })?;
    out.elapsed = started.elapsed().as_secs_f32();
    Ok(out)
}

fn selected_frames<'a>(recipe: &'a Recipe, included: &[bool]) -> Result<Vec<&'a FrameChoice>> {
    if included.len() != recipe.frames.len() {
        bail!("Frame selection no longer matches this stitch");
    }
    let selected: Vec<_> = recipe
        .frames
        .iter()
        .zip(included)
        .filter(|(_, enabled)| **enabled)
        .map(|(frame, _)| frame)
        .collect();
    if selected.len() < 2 {
        bail!("Include at least two frames to rebuild the stitch");
    }
    Ok(selected)
}

fn compose_frames(
    ffmpeg: &std::path::Path,
    recipe: &Recipe,
    selected: &[&FrameChoice],
    retain_layers: bool,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<(RgbaImage, Vec<psd::Source>)> {
    let native_positions: Vec<_> = selected.iter().map(|frame| frame.position).collect();
    let indices: Vec<_> = selected.iter().map(|frame| frame.sample).collect();
    let native = recipe.native;
    let scaled = matches!(recipe.method, Method::LayeredMotion | Method::ShiftAndScale);
    let frame_sizes: Vec<_> = selected
        .iter()
        .map(|frame| zoom::frame_size(native, frame.scale))
        .collect();
    report(0.0, "Composing selected frames");
    let mut tracks = vec![(0, native.0, native_positions)];
    if let Some(split) = recipe.split {
        tracks[0].1 = split;
        let other = selected.iter().map(|f| f.secondary.unwrap()).collect();
        tracks.push((split, native.0 - split, other));
    }
    let mut planes = Vec::new();
    let mut top = i32::MIN;
    let mut bottom = i32::MAX;
    let mut pixels = 0u64;
    for (x, width, positions) in &tracks {
        let (min, size) = if scaled {
            zoom::bounds(positions, &frame_sizes)?
        } else {
            bounds(positions, (*width, native.1))?
        };
        top = top.max(min.y);
        bottom = bottom.min(min.y + size.1 as i32);
        pixels += size.0 as u64 * size.1 as u64;
        planes.push((*x, *width, min, size));
    }
    if recipe.split.is_some() {
        if pixels > MAX_PIXELS {
            bail!("Layered stitch exceeds the 32 megapixel canvas limit. Select a shorter range.");
        }
        if bottom - top <= native.1 as i32 {
            bail!(
                "The selected frames reveal no new area shared by both layers. Include more frames."
            );
        }
    }
    if retain_layers {
        let total: u64 = frame_sizes
            .iter()
            .map(|&(w, h)| u64::from(w) * u64::from(h))
            .sum();
        psd::check_budget(total, pixels)?;
    }
    let mut composers: Vec<_> = planes
        .iter()
        .map(|p| {
            let mut composer = compose::Composer::new(p.3);
            if retain_layers {
                composer.track_sources();
            }
            composer
        })
        .collect();
    let mut layers = Vec::new();
    decode::selected_frames(
        ffmpeg,
        &recipe.input,
        recipe.start,
        recipe.end,
        recipe.fps,
        &indices,
        native,
        cancel,
        |n, rgb| {
            check_cancel(cancel)?;
            report(
                0.8 * n as f32 / selected.len() as f32,
                if scaled {
                    "Composing scale-corrected frames"
                } else {
                    "Composing original pixels"
                },
            );
            if rgb.dimensions() != native {
                bail!("Video dimensions change within this scene");
            }
            for (plane, ((composer, (x, width, min, _)), (_, _, positions))) in
                composers.iter_mut().zip(&planes).zip(&tracks).enumerate()
            {
                let p = positions[n];
                let offset = ((p.x - min.x) as u32, (p.y - min.y) as u32);
                let transformed;
                let frame = if recipe.split.is_some() {
                    transformed = imageops::crop_imm(&rgb, *x, 0, *width, native.1).to_image();
                    &transformed
                } else if scaled && frame_sizes[n] != native {
                    let (width, height) = frame_sizes[n];
                    transformed = zoom::resize(&rgb, width, height, cancel)?;
                    &transformed
                } else {
                    &rgb
                };
                composer.push(frame, offset, cancel)?;
                if retain_layers {
                    let origin = if recipe.split.is_some() {
                        (*x as i32, p.y - top)
                    } else {
                        (offset.0 as i32, offset.1 as i32)
                    };
                    layers.push(psd::Source::new(
                        frame,
                        plane,
                        n,
                        origin,
                        offset,
                        selected[n].time,
                    ));
                }
            }
            Ok(())
        },
    )?;
    for layer in &mut layers {
        layer.make_mask(composers[layer.plane].owners().unwrap(), cancel)?;
    }
    let mut canvases = composers.into_iter().map(compose::Composer::finish);
    let canvas = if recipe.split.is_some() {
        let mut output = RgbaImage::new(native.0, (bottom - top) as u32);
        for (plane, (x, width, min, _)) in canvases.zip(planes) {
            let source_y = (top - min.y) as usize;
            let bytes_per_row = width as usize * 4;
            let stride = native.0 as usize * 4;
            for row in 0..output.height() as usize {
                if row % 32 == 0 {
                    check_cancel(cancel)?;
                }
                let start = row * stride + x as usize * 4;
                let source = (source_y + row) * bytes_per_row;
                output.as_mut()[start..start + bytes_per_row]
                    .copy_from_slice(&plane.as_raw()[source..source + bytes_per_row]);
            }
        }
        output
    } else {
        canvases.next().unwrap()
    };
    Ok((canvas, layers))
}

pub fn save_psd(
    ffmpeg: &std::path::Path,
    id: u64,
    recipe: &Recipe,
    included: &[bool],
    destination: &std::path::Path,
    cancel: &AtomicBool,
    report: impl Fn(f32, &'static str),
) -> Result<()> {
    let selected = selected_frames(recipe, included)?;
    let (canvas, layers) = compose_frames(ffmpeg, recipe, &selected, true, cancel, &report)?;
    report(0.85, "Writing Photoshop layers");
    let bytes = psd::encode(canvas, layers, recipe.aspect, cancel)?;
    crate::export::save_image_data(&bytes, destination, &recipe.input, id, cancel)
}

pub fn recompose(
    ffmpeg: &std::path::Path,
    id: u64,
    recipe: Arc<Recipe>,
    included: Vec<bool>,
    cancel: &AtomicBool,
    report: impl Fn(f32, &'static str),
) -> Result<Output> {
    let started = Instant::now();
    check_cancel(cancel)?;
    let selected = selected_frames(&recipe, &included)?;
    let native_positions: Vec<_> = selected.iter().map(|f| f.position).collect();
    let output_range = (selected[0].time, selected[selected.len() - 1].time);
    let (mut dx, mut dy) = native_positions.last().unwrap().delta(native_positions[0]);
    let native = recipe.native;
    let experimental = recipe.method == Method::LayeredMotion;
    let scaled = experimental || recipe.method == Method::ShiftAndScale;
    let frame_sizes: Vec<_> = selected
        .iter()
        .map(|frame| zoom::frame_size(native, frame.scale))
        .collect();
    if scaled {
        let first = frame_sizes[0];
        let last = frame_sizes.last().unwrap();
        dx += (last.0 as i32 - first.0 as i32) / 2;
        dy += (last.1 as i32 - first.1 as i32) / 2;
    }
    let aspect = recipe.aspect;
    let (canvas, _) = compose_frames(ffmpeg, &recipe, &selected, false, cancel, &report)?;
    let dimensions = canvas.dimensions();
    check_cancel(cancel)?;
    report(0.85, "Preparing PNG preview");
    let preview = imageops::resize(
        &canvas,
        ((dimensions.0 as f64 * (2048.0 / dimensions.0.max(dimensions.1) as f64).min(1.0)).round()
            as u32)
            .max(1),
        ((dimensions.1 as f64 * (2048.0 / dimensions.0.max(dimensions.1) as f64).min(1.0)).round()
            as u32)
            .max(1),
        FilterType::Triangle,
    );
    let artifact = Arc::new(Artifact(
        std::env::temp_dir().join(format!("weebshot-stitch-{}-{id}.png", std::process::id())),
    ));
    png::save(&canvas, &artifact.0, aspect, cancel).context("Could not write stitch preview")?;
    check_cancel(cancel)?;
    let direction = if scaled && dx.abs().max(dy.abs()) < native.0.min(native.1) as i32 / 50 {
        "Zoom"
    } else if dx.abs() > dy.abs() * 3 {
        if dx > 0 { "Right" } else { "Left" }
    } else if dy.abs() > dx.abs() * 3 {
        if dy > 0 { "Down" } else { "Up" }
    } else {
        "Diagonal"
    };
    let truncated = recipe.truncated;
    let omitted = recipe.omitted;
    let excluded = recipe.frames.len() - selected.len();
    let warning = format!(
        "{}{}{}{}",
        if omitted > 0 {
            format!(
                "Omitted {omitted} selected {} with incompatible animation; output covers {:.2}–{:.2}s. ",
                if omitted == 1 { "frame" } else { "frames" },
                output_range.0,
                output_range.1
            )
        } else {
            String::new()
        },
        if truncated {
            "Search limit reached; the pan may continue. "
        } else {
            ""
        },
        if excluded > 0 {
            format!(
                "{excluded} {} excluded by your selection. ",
                if excluded == 1 { "frame" } else { "frames" }
            )
        } else {
            String::new()
        },
        if recipe.split.is_some() {
            "Two vertical layers were aligned separately and cropped to their shared height. Inspect the divider and any animation before saving."
        } else if experimental {
            "Experimental layered motion aligns the prioritized artwork. Other layers can show visible seams, and edges may be transparent. Inspect the preview before saving."
        } else if scaled {
            "Scale correction resamples frames. Inspect the preview for moving layers, seams and transparent edges."
        } else {
            "Local animation, baked-in subtitles, and parallax may remain. Inspect the preview before saving."
        }
    );
    report(1.0, "Ready to preview");
    Ok(Output {
        artifact,
        preview,
        dimensions,
        pixel_aspect: aspect.0 as f32 / aspect.1 as f32,
        frames: selected.len(),
        direction,
        confidence: recipe.confidence,
        warning,
        method: recipe.method,
        omitted_frames: omitted,
        search_limited: truncated,
        range: output_range,
        elapsed: started.elapsed().as_secs_f32(),
        recipe,
        included,
    })
}
