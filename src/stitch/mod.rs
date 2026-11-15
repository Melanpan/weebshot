//! Local camera-pan reconstruction, independent of egui and libmpv.
mod compose;
mod decode;
mod forced;
mod layers;
mod motion;
mod planes;
mod png;
mod psd;
mod solve;
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
/// A stitch whose canvas is less than this much larger than one frame adds
/// little to a screenshot. Measured on local footage, two-frame results add
/// 3 to 11 percent and real pans from about 11 percent upward.
pub const SMALL_GAIN: f32 = 0.10;
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
    /// Even out lighting that differs between frames (vignettes, glows), which
    /// otherwise shows as a brightness step at every seam.
    pub even_seams: bool,
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
            even_seams: true,
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
    /// Shift each frame's broad colour to agree with the frames before it.
    even_seams: bool,
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
    /// Canvas area beyond a single source frame, as a fraction of that frame.
    pub gain: f32,
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

/// Every coherent pan in the window that travels far enough to stitch: nearest
/// the seed first, or by decreasing travel without one.
fn other_regions(
    motions: &[Option<motion::Motion>],
    seed: Option<usize>,
    options: &Options,
    fps: f64,
    size: (u32, u32),
) -> Vec<(usize, usize)> {
    let mut regions: Vec<((usize, usize), f32)> = Vec::new();
    for (i, m) in motions.iter().enumerate() {
        if regions.last().is_some_and(|&((_, hi), _)| i < hi)
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
        let travel = (dx.abs() as f32 / size.0 as f32).max(dy.abs() as f32 / size.1 as f32);
        if travel >= options.min_travel {
            regions.push(((lo, hi), travel));
        }
    }
    match seed {
        Some(seed) => regions.sort_by_key(|&((lo, hi), _)| {
            if seed < lo {
                lo - seed
            } else {
                seed.saturating_sub(hi)
            }
        }),
        None => regions.sort_by(|a, b| b.1.total_cmp(&a.1)),
    }
    regions.into_iter().map(|(span, _)| span).collect()
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
    // A marked range is first taken whole; when that fails (two movements in
    // different directions, a stretch that cannot be aligned), the pans inside
    // it are tried, largest first, and the result reports the shortened range.
    let tried = std::cell::Cell::new(None);
    let analyses = std::cell::RefCell::new(solve::Analyses::default());
    let mut skip: Vec<(f64, f64)> = Vec::new();
    let mut first_error = None;
    let mut small: Option<Output> = None;
    for attempt in 0..=MAX_ALTERNATIVES {
        tried.set(None);
        let alternatives = (attempt > 0).then_some(skip.as_slice());
        match solve::solve(
            ffmpeg,
            id,
            job,
            alternatives,
            &tried,
            &analyses,
            cancel,
            &report,
        ) {
            // A result that adds little to a single frame is set aside while
            // the other pans in the window are tried: a small movement near the
            // playhead should not hide the real pan a second later. A marked
            // range is the user's choice and is returned as it is.
            Ok(mut output)
                if job.range.is_none() && output.gain < SMALL_GAIN && tried.get().is_some() =>
            {
                crate::log::log(
                    crate::log::Level::Debug,
                    "stitch",
                    format!(
                        "attempt {attempt} on {:?} adds only {:.0}%; looking further",
                        tried.get(),
                        output.gain * 100.0
                    ),
                );
                if small
                    .as_ref()
                    .is_none_or(|kept: &Output| output.gain > kept.gain)
                {
                    // Later attempts write the same preview path; park this one.
                    drop(small.take());
                    let parked = Arc::get_mut(&mut output.artifact).and_then(|artifact| {
                        let path = artifact.0.with_extension("small.png");
                        std::fs::rename(&artifact.0, &path).ok()?;
                        artifact.0 = path;
                        Some(())
                    });
                    if parked.is_none() {
                        output.elapsed = started.elapsed().as_secs_f32();
                        return Ok(output);
                    }
                    small = Some(output);
                }
            }
            Ok(mut output) => {
                drop(small);
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
        // An attempt that found no further pan ends the search. The first
        // attempt on a marked range covers all of it and records nothing, so
        // the pans inside it are tried next.
        match tried.get() {
            Some(span) => skip.push(span),
            None if attempt > 0 => break,
            None => {}
        }
    }
    if let Some(mut output) = small {
        output.elapsed = started.elapsed().as_secs_f32();
        return Ok(output);
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
                let evened = if recipe.even_seams {
                    composer.matched(frame, mask.as_ref(), offset, cancel)?
                } else {
                    None
                };
                let frame = evened.as_ref().unwrap_or(frame);
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
    let gain = (u64::from(dimensions.0) * u64::from(dimensions.1)) as f32
        / (u64::from(native.0) * u64::from(native.1)) as f32
        - 1.0;
    let warning = format!(
        "{}{}{}{}{}{}",
        if gain < SMALL_GAIN {
            format!(
                "Small result: the stitch is only {:.0}% larger than a single frame. ",
                gain.max(0.0) * 100.0
            )
        } else {
            String::new()
        },
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
        gain,
        search_limited: truncated,
        range_trimmed: trimmed,
        forced: recipe.forced,
        range: output_range,
        elapsed: started.elapsed().as_secs_f32(),
        recipe,
        included,
    })
}
