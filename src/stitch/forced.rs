//! Best-effort stitch for footage the checked methods refuse.
//!
//! Every gate that protects a normal result is skipped here: each pair's best
//! translation is taken however poorly it matches, and frames are placed even
//! where the artwork disagrees. The result is a starting point for manual
//! repair in the layered PSD, never a silent replacement for a checked stitch.
use super::{
    FrameChoice, INITIAL_SEARCH, Job, MAX_SAMPLES, MAX_SELECTED, Method, Options, Position, Recipe,
    check_cancel, decode, motion, select,
};
use anyhow::{Result, bail};
use image::imageops;
use std::{path::Path, sync::atomic::AtomicBool};

/// A pair whose best translation still differs this much (mean absolute
/// grayscale difference over the best twelve of sixteen tiles) or agrees on
/// this few tiles is a scene cut, not a poorly matching pan. Forcing never
/// joins two shots.
const CUT_ERROR: f32 = 30.0;
const CUT_SUPPORT: f32 = 0.2;

pub(super) fn recipe(
    ffmpeg: &Path,
    job: &Job,
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Recipe> {
    let reach = options.search_seconds.clamp(1.0, 30.0).min(INITIAL_SEARCH);
    let (start, end) = job.range.unwrap_or((
        (job.playhead - reach).max(0.0),
        (job.playhead + reach).min(job.duration),
    ));
    if end <= start {
        bail!("The range is too short to stitch");
    }
    // Twice the usual density: unchecked pairs are safer over short steps.
    let fps = (options.sampling_fps.clamp(2.0, 16.0) * 2.0)
        .min(job.source_fps.max(2.0))
        .min((MAX_SAMPLES - 1) as f64 / (end - start));
    report(0.0, "Forcing a result: decoding frames");
    let (first, aspect) = decode::frame(ffmpeg, &job.input, start, cancel)?;
    let native = first.dimensions();
    drop(first);
    let scale = 320.0 / native.0.max(native.1) as f64;
    let size = (
        (native.0 as f64 * scale).round().max(24.0) as u32,
        (native.1 as f64 * scale).round().max(24.0) as u32,
    );
    let mut frames = Vec::new();
    decode::samples(ffmpeg, &job.input, start, end, fps, size, cancel, |f| {
        if frames.len() < MAX_SAMPLES {
            frames.push(f);
        }
        Ok(())
    })?;
    if frames.len() < 2 {
        bail!("The range is too short to stitch");
    }
    report(0.2, "Forcing a result: estimating motion");
    // No detail floor: a flat pair simply reports no movement.
    let loose = motion::Thresholds {
        min_detail: 0.0,
        ..options.thresholds
    };
    let mut motions = Vec::with_capacity(frames.len() - 1);
    for i in 1..frames.len() {
        check_cancel(cancel)?;
        motions.push(
            motion::estimate(&frames[i - 1], &frames[i], &loose).unwrap_or(motion::Motion {
                dx: 0,
                dy: 0,
                error: 0.0,
                support: 1.0,
            }),
        );
    }
    let cut = |m: &motion::Motion| m.error >= CUT_ERROR || m.support < CUT_SUPPORT;
    // Shots are runs of frames between cuts. Without a marked range keep the
    // shot under the playhead; within a marked range keep the one that moves most.
    let mut shots = Vec::new();
    let mut from = 0;
    for (i, m) in motions.iter().enumerate() {
        if cut(m) {
            shots.push((from, i));
            from = i + 1;
        }
    }
    shots.push((from, frames.len() - 1));
    let travel = |&(lo, hi): &(usize, usize)| -> i32 {
        motions[lo..hi]
            .iter()
            .map(|m| m.dx.abs() + m.dy.abs())
            .sum()
    };
    let seed = (((job.playhead - start) * fps).round().max(0.0) as usize).min(frames.len() - 1);
    // Prefer the shot under the playhead when nothing is marked, then whichever
    // shot moves most; a shot too still to yield two frames is passed over.
    shots.sort_by_key(|shot| {
        let under_playhead = job.range.is_none() && (shot.0..=shot.1).contains(&seed);
        (!under_playhead, std::cmp::Reverse(travel(shot)))
    });
    let chosen = shots.iter().find_map(|&(lo, hi)| {
        let mut positions = vec![Position::default()];
        for m in &motions[lo..hi] {
            positions.push(positions.last().unwrap().plus(*m));
        }
        let selected = select(&positions, size, options);
        (selected.len() >= 2).then_some((lo, positions, selected))
    });
    crate::log::log(
        crate::log::Level::Debug,
        "stitch",
        format!(
            "forced: {} samples at {fps:.2} fps, shots {shots:?}, using {:?}",
            frames.len(),
            chosen.as_ref().map(|c| (c.0, c.0 + c.1.len() - 1))
        ),
    );
    let Some((lo, positions, selected)) = chosen else {
        bail!("no camera movement could be measured in this range, so there is nothing to place");
    };
    if selected.len() > MAX_SELECTED {
        bail!("Too many frames needed. Select a shorter range.");
    }
    drop(frames);
    let sx = native.0 as f64 / size.0 as f64;
    let sy = native.1 as f64 / size.1 as f64;
    let indices: Vec<_> = selected.iter().map(|i| lo + i).collect();
    let mut previous: Option<motion::AlignmentFrame> = None;
    let mut placed: Vec<Position> = Vec::new();
    let mut choices = Vec::new();
    let mut confidence = 1.0f32;
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
            check_cancel(cancel)?;
            report(
                0.4 + 0.3 * n as f32 / selected.len() as f32,
                "Forcing a result: placing frames",
            );
            if rgb.dimensions() != native {
                bail!("Video dimensions change within this scene");
            }
            let thumbnail = imageops::thumbnail(&rgb, 160, 90);
            let prepared = motion::AlignmentFrame::new(motion::grayscale(&rgb));
            drop(rgb);
            let mut position = Position::default();
            if let Some(reference) = &previous {
                let coarse = positions[selected[n]].delta(positions[selected[n - 1]]);
                let guess = (
                    (coarse.0 as f64 * sx).round() as i32,
                    (coarse.1 as f64 * sy).round() as i32,
                );
                // The search is bounded around the coarse estimate, so the best
                // native match is used whether or not it passes any threshold.
                let m = motion::align(
                    reference,
                    &prepared,
                    guess,
                    (sx.max(sy) * 10.0).ceil().clamp(8.0, 96.0) as i32,
                    &options.native_thresholds,
                );
                confidence = confidence.min(((1.0 - m.error / 36.0) * m.support).clamp(0.0, 1.0));
                position = placed[n - 1].plus(m);
            }
            placed.push(position);
            choices.push(FrameChoice {
                time: start + (lo + selected[n]) as f64 / fps,
                thumbnail,
                sample: lo + selected[n],
                position,
                secondary: None,
                scale: 1.0,
                pose: None,
                layer_mask: None,
            });
            previous = Some(prepared);
            Ok(())
        },
    )?;
    Ok(Recipe {
        frames: choices,
        input: job.input.clone(),
        start,
        end,
        fps,
        native,
        aspect,
        confidence,
        omitted: 0,
        truncated: false,
        trimmed: job.range.is_some() && shots.len() > 1,
        split: None,
        method: Method::Translation,
        forced: true,
        even_seams: options.even_seams,
    })
}
