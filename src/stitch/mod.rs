//! Local camera-pan reconstruction, independent of egui and libmpv.
mod compose;
mod decode;
mod forced;
mod layers;
mod motion;
mod planes;
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
/// Edge agreement at which the oldest overlapping reference is accepted
/// without comparing newer ones.
const STRONG_EDGE_AGREEMENT: f32 = 0.7;
/// Further pans Automatic tries after the one nearest the playhead fails.
const MAX_ALTERNATIVES: usize = 3;
/// Seconds searched on each side of the playhead before widening.
const INITIAL_SEARCH: f64 = 6.0;

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
            Self::ShiftAndScale => "Shift, scale and rotation",
            Self::LayeredMotion => "Layered (two planes)",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub method: Method,
    pub layer_divider_percent: Option<u32>,
    /// Unused since layered stitching keeps both planes; kept so saved
    /// settings and MCP calls that still send it remain valid.
    pub prefer_background: bool,
    pub sampling_fps: f64,
    pub search_seconds: f64,
    pub allow_pauses: bool,
    pub selection_travel: f32,
    /// When every checked method refuses, build an unverified translation-only
    /// stitch for manual repair instead of failing.
    pub force: bool,
    // Advanced matching thresholds.
    pub pause_seconds: f64,
    pub min_travel: f32,
    pub thresholds: motion::Thresholds,
    pub native_thresholds: motion::Thresholds,
    pub direction_tolerance: f32,
    pub min_edge_agreement: f32,
    /// Engine tuning the advanced thresholds above were saved with. They are
    /// hidden from the UI, so settings files keep whatever defaults existed
    /// when first written; values from an older tuning are replaced by the
    /// current defaults. Missing in older files, hence zero.
    #[serde(default)]
    pub tuning: u32,
}

/// Bump when advanced threshold defaults change so saved copies are refreshed.
pub const TUNING: u32 = 1;

impl Options {
    /// These options with stale advanced thresholds replaced by current defaults.
    /// Visible settings are always honoured.
    pub fn current(&self) -> Self {
        if self.tuning >= TUNING {
            return self.clone();
        }
        let defaults = Self::default();
        Self {
            pause_seconds: defaults.pause_seconds,
            min_travel: defaults.min_travel,
            thresholds: defaults.thresholds,
            native_thresholds: defaults.native_thresholds,
            direction_tolerance: defaults.direction_tolerance,
            min_edge_agreement: defaults.min_edge_agreement,
            tuning: TUNING,
            ..self.clone()
        }
    }
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
            force: false,
            thresholds: Default::default(),
            native_thresholds: motion::Thresholds {
                max_error: 8.0,
                min_support: 0.8,
                ..Default::default()
            },
            direction_tolerance: 0.65,
            // Measured on broadcast anime: pose changes and parallax score
            // below 0.2, while coherent pans range from about 0.45 upward
            // as characters animate between references.
            min_edge_agreement: 0.4,
            tuning: TUNING,
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
    /// Full similarity pose for Shift, scale and rotation; `None` elsewhere.
    pose: Option<zoom::Transform>,
    /// Foreground mask at tracking resolution for the layered method.
    layer_mask: Option<Arc<GrayImage>>,
}

impl FrameChoice {
    /// Canvas size of this frame after any scaling or rotation.
    fn extent(&self, native: (u32, u32)) -> (u32, u32) {
        match self.pose {
            Some(pose) => pose.placement(native).1,
            None => zoom::frame_size(native, self.scale),
        }
    }
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
    /// The marked In/Out range had to be shortened to a coherent pan.
    trimmed: bool,
    split: Option<u32>,
    method: Method,
    /// Built by the unchecked fallback; alignment was not verified.
    forced: bool,
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
    pub range_trimmed: bool,
    /// The checked methods refused and "Force a result" built this anyway.
    pub forced: bool,
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
    // Walk outward from the nearest moving pair, not from the playhead itself:
    // a playhead on a hold or an unmatched pair (the first frame of a clip,
    // the frame after a cut) would otherwise end the walk before it starts.
    let (anchor, reference) = nearby
        .into_iter()
        .filter_map(|i| Some((i, motions[i]?)))
        .find(|(_, m)| m.reliable(&options.thresholds) && m.moving())
        .context("No coherent camera pan near the playhead. Try marking In and Out, then Stitch Selection.")?;
    let compatible = |m: motion::Motion| {
        let dot = m.dx * reference.dx + m.dy * reference.dy;
        let cross = (m.dx * reference.dy - m.dy * reference.dx).abs();
        !m.moving() || (dot > 0 && cross as f32 <= dot as f32 * options.direction_tolerance + 1.0)
    };
    let walk = |back: bool| {
        let mut frame = anchor;
        let mut last = anchor;
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

/// Every coherent pan in the window that travels far enough to stitch, nearest
/// the seed first.
fn other_regions(
    motions: &[Option<motion::Motion>],
    seed: usize,
    options: &Options,
    fps: f64,
    size: (u32, u32),
) -> Vec<(usize, usize)> {
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for (i, m) in motions.iter().enumerate() {
        if regions.last().is_some_and(|&(_, hi)| i < hi)
            || !m.is_some_and(|m| m.reliable(&options.thresholds) && m.moving())
        {
            continue;
        }
        let Ok((lo, hi)) = region(motions, i, options, fps) else {
            continue;
        };
        let (dx, dy) = motions[lo..hi]
            .iter()
            .flatten()
            .fold((0, 0), |(x, y), m| (x + m.dx, y + m.dy));
        if (dx.abs() as f32 / size.0 as f32).max(dy.abs() as f32 / size.1 as f32)
            >= options.min_travel
        {
            regions.push((lo, hi));
        }
    }
    regions.sort_by_key(|&(lo, hi)| {
        if seed < lo {
            lo - seed
        } else {
            seed.saturating_sub(hi)
        }
    });
    regions
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

/// A marked range is authoritative about where to look, not a promise that
/// every pair matches. Keep the run of reliable pairs that reveals the most.
fn best_segment(
    motions: &[Option<motion::Motion>],
    size: (u32, u32),
    thresholds: &motion::Thresholds,
) -> Option<(usize, usize)> {
    let mut best: Option<((usize, usize), f32)> = None;
    let mut start = 0;
    for end in 0..=motions.len() {
        let reliable = motions
            .get(end)
            .is_some_and(|m| m.is_some_and(|m| m.reliable(thresholds)));
        if reliable {
            continue;
        }
        // A lone matching pair is more often two shots that happen to agree
        // on flat fills than a pan; require at least two consecutive pairs.
        if end >= start + 2 {
            let (dx, dy) = motions[start..end]
                .iter()
                .flatten()
                .fold((0, 0), |(x, y), m| (x + m.dx, y + m.dy));
            let travel = (dx.abs() as f32 / size.0 as f32).max(dy.abs() as f32 / size.1 as f32);
            if best.is_none_or(|(_, t)| travel > t) {
                best = Some(((start, end), travel));
            }
        }
        start = end + 1;
    }
    best.map(|(span, _)| span)
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
    // Automatic first follows the motion nearest the playhead. When that is
    // not a stitchable pan (a small movement before the real one, a pan that
    // starts a second later), try the other pans in the window, nearest first.
    let tried = std::cell::Cell::new(None);
    let mut skip: Vec<(f64, f64)> = Vec::new();
    let mut first_error = None;
    for attempt in 0..=MAX_ALTERNATIVES {
        tried.set(None);
        let alternatives = (attempt > 0).then_some(skip.as_slice());
        match solve(ffmpeg, id, job, alternatives, &tried, cancel, &report) {
            Ok(mut output) => {
                output.elapsed = started.elapsed().as_secs_f32();
                return Ok(output);
            }
            Err(error) if cancel.load(Ordering::Relaxed) => return Err(error),
            Err(error) => {
                crate::log::log(
                    crate::log::Level::Debug,
                    "stitch",
                    format!("attempt {attempt} on {:?}: {error:#}", tried.get()),
                );
                first_error.get_or_insert(error);
            }
        }
        // A marked range has no alternatives, and an attempt that found no
        // further pan ends the search.
        match tried.get() {
            Some(span) if job.range.is_none() => skip.push(span),
            _ if attempt > 0 || job.range.is_some() => break,
            _ => {}
        }
    }
    let error = first_error.unwrap();
    if !job.options.force {
        return Err(error);
    }
    crate::log::log(
        crate::log::Level::Info,
        "stitch",
        format!("forcing a result after: {error:#}"),
    );
    // A forced attempt that also fails reports why the checked one refused.
    let forced =
        forced::recipe(ffmpeg, job, &job.options.current(), cancel, &report).and_then(|recipe| {
            let included = vec![true; recipe.frames.len()];
            recompose(
                ffmpeg,
                id,
                Arc::new(recipe),
                included,
                cancel,
                |fraction, stage| report(0.7 + 0.3 * fraction, stage),
            )
        });
    match forced {
        Ok(mut output) => {
            output.elapsed = started.elapsed().as_secs_f32();
            Ok(output)
        }
        Err(second) if cancel.load(Ordering::Relaxed) => Err(second),
        Err(second) => {
            crate::log::log(
                crate::log::Level::Debug,
                "stitch",
                format!("forced stitch failed: {second:#}"),
            );
            Err(anyhow::anyhow!(
                "{error:#} Force a result could not help either: {second}."
            ))
        }
    }
}

/// One attempt. `skip` is `None` for the pan nearest the playhead; otherwise
/// it lists the time spans already attempted and the nearest remaining pan is
/// used. `tried` receives the span this attempt settled on.
fn solve(
    ffmpeg: &std::path::Path,
    id: u64,
    job: &Job,
    skip: Option<&[(f64, f64)]>,
    tried: &std::cell::Cell<Option<(f64, f64)>>,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Output> {
    let started = Instant::now();
    check_cancel(cancel)?;
    if !job.duration.is_finite() || job.duration <= 0.0 || !job.playhead.is_finite() {
        bail!("Video duration is unavailable");
    }
    let options = &job.options.current();
    let search = options.search_seconds.clamp(1.0, 30.0);
    let window = |reach: f64| {
        job.range.unwrap_or((
            (job.playhead - reach).max(0.0),
            (job.playhead + reach).min(job.duration),
        ))
    };
    // Most pans are short. Discover within a few seconds first and widen to the
    // configured search limit only when the pan reaches that smaller window's
    // edge; this avoids decoding up to 30 seconds of unrelated shots.
    let mut reach = if options.method == Method::TwoVerticalLayers {
        search
    } else {
        search.min(INITIAL_SEARCH)
    };
    let (mut start, mut end) = window(reach);
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end > job.duration + 0.01
        || end <= start
        || end - start > 60.0
    {
        bail!("Choose a valid In/Out range of at most 60 seconds");
    }
    let initial_fps = options
        .sampling_fps
        .clamp(2.0, 16.0)
        .min(job.source_fps.max(2.0));
    report(0.0, "Analysing camera pan");
    let (first, aspect) = decode::frame(ffmpeg, &job.input, start, cancel)?;
    let native = first.dimensions();
    drop(first);
    let scale = 320.0 / native.0.max(native.1) as f64;
    let size = (
        (native.0 as f64 * scale).round().max(24.0) as u32,
        (native.1 as f64 * scale).round().max(24.0) as u32,
    );
    let mut fps = initial_fps;
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
        // Shift, scale and rotation reuses translation discovery below to pick
        // the sampling rate and the pan's extent.
        if matches!(options.method, Method::TwoVerticalLayers) {
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
        } else if let Some(skip) = skip {
            let span =
                |(lo, hi): (usize, usize)| (start + lo as f64 / fps, start + hi as f64 / fps);
            other_regions(&discovery, seed, options, fps, size)
                .into_iter()
                .find(|&r| {
                    let (a, b) = span(r);
                    !skip.iter().any(|&(lo, hi)| a < hi && lo < b)
                })
                .context("No coherent camera pan near the playhead. Try marking In and Out, then Stitch Selection.")
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
        let at_edge = candidate.as_ref().is_ok_and(|&(lo, hi)| {
            (lo == 0 && start > 0.001) || (hi + 1 == frames.len() && end < job.duration - 0.001)
        });
        if job.range.is_none() && at_edge && reach < search {
            reach = search;
            (start, end) = window(reach);
            report(0.0, "Extending the search");
            continue;
        }
        // An explicit range that still contains a cut or unmatched frames
        // keeps its longest coherent run; the preview reports the shortening.
        let candidate = match candidate {
            Ok(_) if job.range.is_some() && bad => {
                Ok(best_segment(&motions, size, &options.thresholds)
                    .unwrap_or((0, frames.len() - 1)))
            }
            other => other,
        };
        if let Ok((lo, hi)) = candidate
            && job.range.is_none()
            && hi > lo
        {
            tried.set(Some((start + lo as f64 / fps, start + hi as f64 / fps)));
        }
        break (frames, motions, Some(candidate));
    };
    let analysed = frames.len();
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
        trimmed: false,
        split: None,
        method: Method::Translation,
        forced: false,
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
    // Automatic's last resort: a single plane that also rolls or zooms. Only the
    // discovered pan (or the marked range) is analysed so cuts outside it cannot
    // break the chain of pairwise transforms.
    let span = candidate
        .as_ref()
        .and_then(|c| c.as_ref().ok())
        .copied()
        .filter(|(lo, hi)| hi > lo);
    // Fast rolls and zooms change too much between coarse samples, and the
    // translation span ends where the roll begins. Re-sample the span plus one
    // coarse sample on each side, up to the source rate, and let the matcher
    // extend into those margins while pairs still align.
    let similarity = |(lo, hi): (usize, usize)| -> Result<Output> {
        let first = lo.saturating_sub(1);
        let last = (hi + 1).min(frames.len() - 1);
        let t0 = start + first as f64 / fps;
        let t1 = (start + (last as f64 + 0.5) / fps).min(end);
        let dense = (fps * 3.0)
            .min(job.source_fps)
            .min(24.0)
            .min((MAX_SAMPLES - 2) as f64 / (t1 - t0));
        // Whether the analysed slice spans the whole marked range.
        let covers = first == 0 && last == frames.len() - 1;
        if dense <= fps + 0.1 {
            let mut recipe = zoom::recipe(
                &frames[first..=last],
                (lo - first, hi - first),
                first,
                base_recipe(),
                ffmpeg,
                options,
                cancel,
                &report,
            )?;
            recipe.trimmed = job.range.is_some() && (!covers || recipe.trimmed);
            return compose_recipe(recipe);
        }
        report(0.15, "Retrying with closer frames");
        let mut fine = Vec::new();
        decode::samples(ffmpeg, &job.input, t0, t1, dense, size, cancel, |f| {
            fine.push(f);
            Ok(())
        })?;
        if fine.len() < 2 {
            bail!(zoom::ALIGNMENT_ERROR);
        }
        let to_fine = |coarse: usize| (coarse - first) as f64 / fps * dense;
        let core_hi = (to_fine(hi).floor() as usize).min(fine.len() - 1);
        let core = ((to_fine(lo).ceil() as usize).min(core_hi), core_hi);
        let recipe = Recipe {
            start: t0,
            end: t1,
            fps: dense,
            ..base_recipe()
        };
        let mut recipe = zoom::recipe(&fine, core, 0, recipe, ffmpeg, options, cancel, &report)?;
        recipe.trimmed = job.range.is_some() && (!covers || recipe.trimmed);
        compose_recipe(recipe)
    };
    let fallback = || -> Result<Option<Output>> {
        if let Some(output) = layered()? {
            return Ok(Some(output));
        }
        let Some(span) = span.filter(|_| options.method == Method::Automatic) else {
            return Ok(None);
        };
        report(0.7, "Checking for camera roll and zoom");
        match similarity(span) {
            Ok(output) => Ok(Some(output)),
            Err(error) if cancel.load(Ordering::Relaxed) => Err(error),
            Err(error) => {
                crate::log::log(
                    crate::log::Level::Debug,
                    "stitch",
                    format!("shift, scale and rotation fallback: {error:#}"),
                );
                Ok(None)
            }
        }
    };
    if options.method == Method::ShiftAndScale {
        return similarity(span.unwrap_or((0, frames.len() - 1)));
    }
    if options.method == Method::LayeredMotion {
        // The translation span finds the pan; layer tracking samples it again
        // at a higher resolution. Include one coarse sample on each side.
        let (lo, hi) = span.unwrap_or((0, frames.len() - 1));
        let t0 = start + lo.saturating_sub(1) as f64 / fps;
        let t1 = (start + (hi + 1) as f64 / fps).min(end);
        return compose_recipe(planes::recipe(
            ffmpeg,
            t0,
            t1,
            job.source_fps,
            base_recipe(),
            options,
            cancel,
            &report,
        )?);
    }
    let Some(candidate) = candidate else {
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
        Err(error) => return fallback()?.ok_or(error),
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
                return fallback()?
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
    // Why unmatched frames failed, for the error message: no reference
    // matched at all, or one matched in position but its outlines differ.
    let (mut unmatched, mut outlines_differ) = (0usize, 0usize);
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
            // The oldest reference limits drift, so a strong match there wins
            // at once. Otherwise keep the best-agreeing acceptable reference:
            // an old one is more likely to differ by animation.
            let mut weaker: Option<(Position, f32, f32)> = None;
            let mut positioned = false;
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
                let reliable = m.reliable(&options.native_thresholds);
                positioned |= reliable;
                let edge_agreement = if reliable {
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
                if reliable && edge_agreement >= options.min_edge_agreement {
                    let found = (
                        native_positions[*anchor].plus(m),
                        (1.0 - m.error / 36.0) * m.support,
                        edge_agreement,
                    );
                    if edge_agreement >= STRONG_EDGE_AGREEMENT.max(options.min_edge_agreement) {
                        (pos, quality, _) = found;
                        matched = true;
                        break;
                    }
                    if weaker.is_none_or(|w| edge_agreement > w.2) {
                        weaker = Some(found);
                    }
                }
            }
            if !matched && let Some(found) = weaker {
                (pos, quality, _) = found;
                matched = true;
            }
            if !matched {
                // Pose changes can invalidate an early frame while leaving a
                // perfectly stitchable pan later. Start a new candidate rather
                // than anchoring every subsequent match to that first pose.
                group += 1;
                anchors.clear();
                if positioned {
                    outlines_differ += 1;
                } else {
                    unmatched += 1;
                }
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
        let total = original_count - 1;
        let cause = if outlines_differ > 0 && outlines_differ >= unmatched {
            format!(
                "{outlines_differ} of {total} frames line up by position but their outlines differ, which usually means a character moving or layers sliding over each other. A shorter range or Force a result may help."
            )
        } else if unmatched > 0 {
            format!(
                "{unmatched} of {total} frames found no match at full resolution, which usually means zoom, rotation or heavy animation. Force a result may help."
            )
        } else {
            "The frames that match reveal too little new area.".to_string()
        };
        return fallback()?.with_context(|| format!("{error} What failed: {cause}"));
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
                pose: None,
                layer_mask: None,
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
        trimmed: job.range.is_some() && (lo > 0 || hi + 1 < analysed),
        split: None,
        method: Method::Translation,
        forced: false,
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
    if recipe.method == Method::LayeredMotion {
        return planes::compose(ffmpeg, recipe, selected, retain_layers, cancel, report);
    }
    let native_positions: Vec<_> = selected.iter().map(|frame| frame.position).collect();
    let indices: Vec<_> = selected.iter().map(|frame| frame.sample).collect();
    let native = recipe.native;
    let scaled = recipe.method == Method::ShiftAndScale;
    let frame_sizes: Vec<_> = selected.iter().map(|f| f.extent(native)).collect();
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
                let mut mask = None;
                let frame = if recipe.split.is_some() {
                    transformed = imageops::crop_imm(&rgb, *x, 0, *width, native.1).to_image();
                    &transformed
                } else if let Some(pose) = selected[n].pose.filter(|p| p.rotated()) {
                    let (warped, coverage) = zoom::warp(&rgb, pose, cancel)?;
                    transformed = warped;
                    mask = Some(coverage);
                    &transformed
                } else if scaled && frame_sizes[n] != native {
                    let (width, height) = frame_sizes[n];
                    transformed = zoom::resize(&rgb, width, height, cancel)?;
                    &transformed
                } else {
                    &rgb
                };
                composer.push(frame, mask.as_ref(), offset, cancel)?;
                if retain_layers {
                    let origin = if recipe.split.is_some() {
                        (*x as i32, p.y - top)
                    } else {
                        (offset.0 as i32, offset.1 as i32)
                    };
                    layers.push(psd::Source::new(
                        frame,
                        mask.as_ref(),
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
    let scaled = recipe.method == Method::ShiftAndScale;
    let frame_sizes: Vec<_> = selected.iter().map(|f| f.extent(native)).collect();
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
    let trimmed = recipe.trimmed && recipe.omitted == 0;
    let omitted = recipe.omitted;
    let excluded = recipe.frames.len() - selected.len();
    let warning = format!(
        "{}{}{}{}{}",
        if recipe.forced {
            "Forced result: these frames did not pass the alignment checks, so they are placed by their best-guess shift only. Expect seams and doubled artwork; save as PSD to move and mask the frames by hand. "
        } else {
            ""
        },
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
            "Search limit reached; the pan may continue. ".to_string()
        } else if trimmed {
            format!(
                "Part of the marked range contains a cut, a pose change or frames that could not be matched; the result covers {:.2}–{:.2}s. ",
                output_range.0, output_range.1
            )
        } else {
            String::new()
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
            "The background and the foreground layer were stitched separately; the foreground is shown as it was at the middle frame. Scenes with more than two depths, and parts of a character that only appear in other poses, can show breaks. Inspect outlines before saving."
        } else if selected.iter().any(|f| f.pose.is_some_and(|p| p.rotated())) {
            "Rotation and scale correction resample frames into the first frame's orientation. Inspect the preview for moving layers, seams and transparent edges."
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
        range_trimmed: trimmed,
        forced: recipe.forced,
        range: output_range,
        elapsed: started.elapsed().as_secs_f32(),
        recipe,
        included,
    })
}
