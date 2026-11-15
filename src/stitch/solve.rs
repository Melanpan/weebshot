//! One attempt at a stitch, in stages: discover the window, sampling rate and
//! candidate pan; solve it as a translation; fall back to two vertical layers
//! or to shift, scale and rotation; compose. `run` in the parent module
//! decides which pans to attempt and what to do when all of them fail.
use super::*;

/// Downscaled frames of one window at one rate, with the motion between
/// consecutive frames. `discovery` also finds pans too slow to register
/// between adjacent samples.
struct Analysis {
    frames: Vec<GrayImage>,
    motions: Vec<Option<motion::Motion>>,
    discovery: Vec<Option<motion::Motion>>,
}

type Size = (u32, u32);

/// Analyses already decoded for this job. Every attempt after the first asks
/// for the same windows and rates, so only the first pays for decoding.
#[derive(Default)]
pub(super) struct Analyses {
    /// Start time, native size and pixel aspect of the first decoded frame.
    first: Option<(u64, Size, Size)>,
    samples: Vec<([u64; 3], Arc<Analysis>)>,
}

impl Analyses {
    /// Frames kept across all cached analyses (about 58 kB each at 320×180).
    const MAX_FRAMES: usize = 2 * MAX_SAMPLES;

    /// Native size and pixel aspect of the frame at `start`.
    fn first(
        &mut self,
        ffmpeg: &std::path::Path,
        job: &Job,
        start: f64,
        cancel: &AtomicBool,
        report: &impl Fn(f32, &'static str),
    ) -> Result<(Size, Size)> {
        if let Some((key, native, aspect)) = self.first
            && key == start.to_bits()
        {
            return Ok((native, aspect));
        }
        report(0.0, "Analysing camera pan");
        let (first, aspect) = decode::frame(ffmpeg, &job.input, start, cancel)?;
        self.first = Some((start.to_bits(), first.dimensions(), aspect));
        Ok((first.dimensions(), aspect))
    }

    #[allow(clippy::too_many_arguments)]
    fn samples(
        &mut self,
        ffmpeg: &std::path::Path,
        job: &Job,
        options: &Options,
        (start, end): (f64, f64),
        fps: f64,
        size: (u32, u32),
        cancel: &AtomicBool,
        report: &impl Fn(f32, &'static str),
    ) -> Result<Arc<Analysis>> {
        let key = [start.to_bits(), end.to_bits(), fps.to_bits()];
        if let Some((_, analysis)) = self.samples.iter().find(|(k, _)| *k == key) {
            return Ok(analysis.clone());
        }
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
        let mut motions = Vec::new();
        let mut discovery = Vec::new();
        if options.method != Method::TwoVerticalLayers {
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
            discovery = motions.clone();
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
        }
        let analysis = Arc::new(Analysis {
            frames,
            motions,
            discovery,
        });
        self.samples.push((key, analysis.clone()));
        // Drop the oldest analyses beyond the frame budget.
        let total = |samples: &[([u64; 3], Arc<Analysis>)]| -> usize {
            samples.iter().map(|(_, a)| a.frames.len()).sum()
        };
        while self.samples.len() > 1 && total(&self.samples) > Self::MAX_FRAMES {
            self.samples.remove(0);
        }
        Ok(analysis)
    }
}

/// What discovery settled on for one attempt.
struct Discovery {
    analysis: Arc<Analysis>,
    start: f64,
    end: f64,
    fps: f64,
    /// Sample range of the pan to solve; `None` for Two vertical layers,
    /// which analyses the whole window.
    candidate: Option<Result<(usize, usize)>>,
}

/// Choose the search window and sampling rate, and the pan to attempt in it.
/// The rate doubles while the candidate has unmatched pairs, and the window
/// widens when the pan reaches its edge.
#[allow(clippy::too_many_arguments)]
fn discover(
    ffmpeg: &std::path::Path,
    job: &Job,
    options: &Options,
    skip: Option<&[(f64, f64)]>,
    tried: &std::cell::Cell<Option<(f64, f64)>>,
    analyses: &std::cell::RefCell<Analyses>,
    size: Size,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Discovery> {
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
    let mut reach = initial_reach(options);
    let (mut start, mut end) = window(reach);
    let mut fps = options
        .sampling_fps
        .clamp(2.0, 16.0)
        .min(job.source_fps.max(2.0));
    loop {
        let analysis = analyses.borrow_mut().samples(
            ffmpeg,
            job,
            options,
            (start, end),
            fps,
            size,
            cancel,
            report,
        )?;
        let (frames, motions, discovery) =
            (&analysis.frames, &analysis.motions, &analysis.discovery);
        // Shift, scale and rotation reuses translation discovery below to pick
        // the sampling rate and the pan's extent.
        if matches!(options.method, Method::TwoVerticalLayers) {
            return Ok(Discovery {
                analysis,
                start,
                end,
                fps,
                candidate: None,
            });
        }
        let seed = (((job.playhead - start) * fps).round() as usize).min(frames.len() - 1);
        // A region includes the holds around its pan, so two pans separated
        // by a short hold share still frames. Identify a region by the time
        // span of its moving pairs only.
        let span = |(mut lo, mut hi): (usize, usize)| {
            let moving = |i: usize| discovery[i].is_some_and(|m| m.moving());
            while lo < hi && !moving(lo) {
                lo += 1;
            }
            while hi > lo && !moving(hi - 1) {
                hi -= 1;
            }
            (start + lo as f64 / fps, start + hi as f64 / fps)
        };
        let candidate = if let Some(skip) = skip {
            // Inside a marked range the playhead means nothing: take the pan
            // that reveals the most first.
            other_regions(discovery, job.range.is_none().then_some(seed), options, fps, size)
                .into_iter()
                .find(|&r| {
                    let (a, b) = span(r);
                    !skip.iter().any(|&(lo, hi)| a < hi && lo < b)
                })
                .context("No coherent camera pan near the playhead. Try marking In and Out, then Stitch Selection.")
        } else if job.range.is_some() {
            Ok((0, frames.len() - 1))
        } else {
            region(discovery, seed, options, fps)
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
                Ok(best_segment(motions, size, &options.thresholds)
                    .unwrap_or((0, frames.len() - 1)))
            }
            other => other,
        };
        if let Ok((lo, hi)) = candidate
            && (job.range.is_none() || skip.is_some())
            && hi > lo
        {
            tried.set(Some(span((lo, hi))));
        }
        return Ok(Discovery {
            analysis,
            start,
            end,
            fps,
            candidate: Some(candidate),
        });
    }
}

fn initial_reach(options: &Options) -> f64 {
    let search = options.search_seconds.clamp(1.0, 30.0);
    if options.method == Method::TwoVerticalLayers {
        search
    } else {
        search.min(INITIAL_SEARCH)
    }
}

/// Selected frames aligned at source resolution. Frames that match no earlier
/// reference start a new group; the caller keeps the best group.
struct Aligned {
    positions: Vec<Position>,
    groups: Vec<usize>,
    qualities: Vec<f32>,
    thumbnails: Vec<RgbImage>,
    /// Frames that matched no reference at all.
    unmatched: usize,
    /// Frames that matched in position but whose outlines differ.
    outlines_differ: usize,
}

/// State shared by the stages of one attempt.
struct Attempt<'a, R: Fn(f32, &'static str)> {
    ffmpeg: &'a std::path::Path,
    id: u64,
    job: &'a Job,
    options: &'a Options,
    cancel: &'a AtomicBool,
    report: &'a R,
    started: Instant,
    analysis: Arc<Analysis>,
    start: f64,
    end: f64,
    fps: f64,
    native: Size,
    aspect: Size,
    /// Analysis frame size.
    size: Size,
}

impl<R: Fn(f32, &'static str)> Attempt<'_, R> {
    fn base_recipe(&self) -> Recipe {
        Recipe {
            frames: Vec::new(),
            input: self.job.input.clone(),
            start: self.start,
            end: self.end,
            fps: self.fps,
            native: self.native,
            aspect: self.aspect,
            confidence: 1.0,
            omitted: 0,
            truncated: self.job.range.is_none()
                && (self.start > 0.001 || self.end < self.job.duration - 0.001),
            trimmed: false,
            split: None,
            method: Method::Translation,
            forced: false,
            even_seams: self.options.even_seams,
        }
    }

    /// Compose a solved recipe; `from` is where composition starts on the
    /// progress scale.
    fn compose(&self, recipe: Recipe, from: f32) -> Result<Output> {
        let recipe = Arc::new(recipe);
        let included = vec![true; recipe.frames.len()];
        let report = self.report;
        let mut output = recompose(
            self.ffmpeg,
            self.id,
            recipe,
            included,
            self.cancel,
            |fraction, stage| report(from + (1.0 - from) * fraction, stage),
        )?;
        output.elapsed = self.started.elapsed().as_secs_f32();
        Ok(output)
    }

    /// Two vertical layers over the whole window; `None` when the method is
    /// Single translation or the layers do not separate.
    fn layered(&self) -> Result<Option<Output>> {
        if self.options.method == Method::Translation {
            return Ok(None);
        }
        let report = self.report;
        report(0.65, "Checking for independently moving layers");
        layers::recipe(
            &self.analysis.frames,
            self.base_recipe(),
            self.ffmpeg,
            self.options,
            self.cancel,
            &report,
        )?
        .map(|recipe| self.compose(recipe, 0.75))
        .transpose()
    }

    /// Shift, scale and rotation over a sample span. Fast rolls and zooms
    /// change too much between coarse samples, and the translation span ends
    /// where the roll begins. Re-sample the span plus one coarse sample on
    /// each side, up to the source rate, and let the matcher extend into
    /// those margins while pairs still align.
    fn similarity(&self, (lo, hi): (usize, usize)) -> Result<Output> {
        let Self {
            ffmpeg,
            job,
            options,
            cancel,
            report,
            start,
            end,
            fps,
            size,
            ..
        } = *self;
        let frames = &self.analysis.frames;
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
                self.base_recipe(),
                ffmpeg,
                options,
                cancel,
                &report,
            )?;
            recipe.trimmed = job.range.is_some() && (!covers || recipe.trimmed);
            return self.compose(recipe, 0.75);
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
            ..self.base_recipe()
        };
        let mut recipe = zoom::recipe(&fine, core, 0, recipe, ffmpeg, options, cancel, &report)?;
        recipe.trimmed = job.range.is_some() && (!covers || recipe.trimmed);
        self.compose(recipe, 0.75)
    }

    /// What Automatic tries when translation fails: two vertical layers, then
    /// a single plane that also rolls or zooms. Only the discovered pan (or
    /// the marked range) is analysed for the latter so cuts outside it cannot
    /// break the chain of pairwise transforms.
    fn fallback(&self, span: Option<(usize, usize)>) -> Result<Option<Output>> {
        if let Some(output) = self.layered()? {
            return Ok(Some(output));
        }
        let Some(span) = span.filter(|_| self.options.method == Method::Automatic) else {
            return Ok(None);
        };
        (self.report)(0.7, "Checking for camera roll and zoom");
        match self.similarity(span) {
            Ok(output) => Ok(Some(output)),
            Err(error) if self.cancel.load(Ordering::Relaxed) => Err(error),
            Err(error) => {
                crate::log::log(
                    crate::log::Level::Debug,
                    "stitch",
                    format!("shift, scale and rotation fallback: {error:#}"),
                );
                Ok(None)
            }
        }
    }

    /// Layered (two planes). The translation span finds the pan; layer
    /// tracking samples it again at a higher resolution. Include one coarse
    /// sample on each side.
    fn planes(&self, span: Option<(usize, usize)>) -> Result<Output> {
        let (lo, hi) = span.unwrap_or((0, self.analysis.frames.len() - 1));
        let t0 = self.start + lo.saturating_sub(1) as f64 / self.fps;
        let t1 = (self.start + (hi + 1) as f64 / self.fps).min(self.end);
        let report = self.report;
        self.compose(
            planes::recipe(
                self.ffmpeg,
                t0,
                t1,
                self.job.source_fps,
                self.base_recipe(),
                self.options,
                self.cancel,
                &report,
            )?,
            0.75,
        )
    }

    /// One shift per frame: solve positions over the candidate, select frames
    /// by travel, align them at source resolution and keep the best group.
    fn translation(&self, candidate: Result<(usize, usize)>) -> Result<Output> {
        let Self {
            job,
            options,
            cancel,
            start,
            end,
            fps,
            native,
            aspect,
            size,
            ..
        } = *self;
        let (frames, motions) = (&self.analysis.frames, &self.analysis.motions);
        let analysed = frames.len();
        let span = candidate.as_ref().ok().copied().filter(|(lo, hi)| hi > lo);
        let analysis = candidate.and_then(|(lo, hi)| {
            if hi <= lo {
                bail!("No camera movement detected. Try marking In and Out around the pan.");
            }
            let points = positions(frames, motions, lo, hi, cancel, options)?;
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
            Err(error) => return self.fallback(span)?.ok_or(error),
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
                    return self
                        .fallback(span)?
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
        let Aligned {
            positions: native_positions,
            groups,
            qualities,
            thumbnails,
            unmatched,
            outlines_differ,
        } = self.align(lo, &positions, &selected)?;
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
            return self
                .fallback(span)?
                .with_context(|| format!("{error} What failed: {cause}"));
        };
        let keep: Vec<_> = (0..selected.len()).filter(|&n| groups[n] == best).collect();
        let confidence = keep.iter().map(|&n| qualities[n]).fold(1.0f32, f32::min);
        let recipe = Recipe {
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
                && ((lo == 0 && start > 0.001)
                    || (hi == motions.len() && end < job.duration - 0.001)),
            trimmed: job.range.is_some() && (lo > 0 || hi + 1 < analysed),
            split: None,
            method: Method::Translation,
            forced: false,
            even_seams: options.even_seams,
        };
        self.compose(recipe, 0.7)
    }

    /// Decode the selected frames at source resolution and place each against
    /// up to three earlier frames.
    fn align(&self, lo: usize, positions: &[Position], selected: &[usize]) -> Result<Aligned> {
        let Self {
            ffmpeg,
            job,
            options,
            cancel,
            report,
            start,
            end,
            fps,
            native,
            size,
            ..
        } = *self;
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
        Ok(Aligned {
            positions: native_positions,
            groups,
            qualities,
            thumbnails,
            unmatched,
            outlines_differ,
        })
    }
}

/// One attempt. `skip` is `None` for the pan nearest the playhead; otherwise
/// it lists the time spans already attempted and the nearest remaining pan is
/// used. `tried` receives the span this attempt settled on.
#[allow(clippy::too_many_arguments)]
pub(super) fn solve(
    ffmpeg: &std::path::Path,
    id: u64,
    job: &Job,
    skip: Option<&[(f64, f64)]>,
    tried: &std::cell::Cell<Option<(f64, f64)>>,
    analyses: &std::cell::RefCell<Analyses>,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Output> {
    let started = Instant::now();
    check_cancel(cancel)?;
    if !job.duration.is_finite() || job.duration <= 0.0 || !job.playhead.is_finite() {
        bail!("Video duration is unavailable");
    }
    let options = &job.options.current();
    let reach = initial_reach(options);
    let (start, end) = job.range.unwrap_or((
        (job.playhead - reach).max(0.0),
        (job.playhead + reach).min(job.duration),
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
    let (native, aspect) = analyses
        .borrow_mut()
        .first(ffmpeg, job, start, cancel, report)?;
    let scale = 320.0 / native.0.max(native.1) as f64;
    let size = (
        (native.0 as f64 * scale).round().max(24.0) as u32,
        (native.1 as f64 * scale).round().max(24.0) as u32,
    );
    let Discovery {
        analysis,
        start,
        end,
        fps,
        candidate,
    } = discover(
        ffmpeg, job, options, skip, tried, analyses, size, cancel, report,
    )?;
    let attempt = Attempt {
        ffmpeg,
        id,
        job,
        options,
        cancel,
        report,
        started,
        analysis,
        start,
        end,
        fps,
        native,
        aspect,
        size,
    };
    let span = candidate
        .as_ref()
        .and_then(|c| c.as_ref().ok())
        .copied()
        .filter(|(lo, hi)| hi > lo);
    match (options.method, candidate) {
        (Method::ShiftAndScale, _) => {
            attempt.similarity(span.unwrap_or((0, attempt.analysis.frames.len() - 1)))
        }
        (Method::LayeredMotion, _) => attempt.planes(span),
        (_, None) => attempt.layered()?.context(layers::LAYER_ALIGNMENT_ERROR),
        (_, Some(candidate)) => attempt.translation(candidate),
    }
}
