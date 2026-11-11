//! A conservative fallback for two side-by-side planes in a vertical pan.
use super::{
    FrameChoice, MAX_SELECTED, Method, Options, Position, Recipe, check_cancel, decode, motion,
    select,
};
use anyhow::{Result, bail};
use image::{GrayImage, imageops};
use std::{collections::VecDeque, path::Path, sync::atomic::AtomicBool};

struct Track {
    split: u32,
    positions: Vec<[Position; 2]>,
}

fn detect(
    frames: &[GrayImage],
    divider_percent: Option<u32>,
    cancel: &AtomicBool,
) -> Result<Option<Track>> {
    let (width, height) = frames[0].dimensions();
    if width < 96 || height < 48 || frames.len() < 3 {
        return Ok(None);
    }
    let thresholds = motion::Thresholds {
        max_error: 8.0,
        min_support: 0.875,
        tile_error: 14.0,
        ..Default::default()
    };
    let mut best = None;
    let mut best_error = f32::INFINITY;
    let candidates = [width / 2, width / 3, width * 2 / 3];
    let custom = divider_percent.map(|percent| width * percent.clamp(20, 80) / 100);
    let candidates = custom
        .as_ref()
        .map_or(candidates.as_slice(), std::slice::from_ref);
    for &split in candidates {
        let crop = |frame: &GrayImage| {
            [
                imageops::crop_imm(frame, 0, 0, split, height).to_image(),
                imageops::crop_imm(frame, split, 0, width - split, height).to_image(),
            ]
        };
        let mut previous = crop(&frames[0]);
        let mut positions = vec![[Position::default(); 2]];
        let mut error = 0.0;
        for frame in &frames[1..] {
            check_cancel(cancel)?;
            let current = crop(frame);
            let matches = [
                motion::estimate(&previous[0], &current[0], &thresholds),
                motion::estimate(&previous[1], &current[1], &thresholds),
            ];
            let [Some(left), Some(right)] = matches else {
                break;
            };
            if [left, right].iter().any(|m| {
                !m.reliable(&thresholds) || m.dx.abs() > 1 || m.dy.abs() > height as i32 / 3
            }) || left.dy * right.dy < 0
            {
                break;
            }
            let last = positions.last().unwrap();
            let next = [
                Position {
                    x: 0,
                    y: last[0].y + left.dy,
                },
                Position {
                    x: 0,
                    y: last[1].y + right.dy,
                },
            ];
            // Require a consistent direction in each plane.
            if next.iter().zip(last).any(|(n, p)| p.y * (n.y - p.y) < 0) {
                break;
            }
            positions.push(next);
            error += left.error + right.error;
            previous = current;
        }
        if positions.len() == frames.len() {
            let [left, right] = *positions.last().unwrap();
            let distinct = (left.y - right.y).abs() >= (height as i32 / 20).max(3);
            if distinct && left.y * right.y >= 0 && error < best_error {
                best_error = error;
                best = Some(Track { split, positions });
            }
        }
    }
    Ok(best)
}

fn native_match(
    reference: &motion::AlignmentFrame,
    current: &motion::AlignmentFrame,
    guess: i32,
    radius: i32,
) -> Option<motion::Motion> {
    let thresholds = motion::Thresholds {
        max_error: 7.0,
        min_support: 0.875,
        tile_error: 14.0,
        ..Default::default()
    };
    let m = motion::align(reference, current, (0, guess), radius, &thresholds);
    (m.dx == 0 && m.reliable(&thresholds) && reference.edge_agreement(current, m) >= 0.8)
        .then_some(m)
}

pub(super) const LAYER_ALIGNMENT_ERROR: &str =
    "Could not align two side-by-side layers. Check the divider or mark a shorter In/Out range.";
pub(super) const ALIGNMENT_ERROR: &str = "The frames could not be aligned with a single shift per frame or two stable vertical layers. Zoom, overlapping layers, or large pose changes can cause this. Try a shorter In/Out selection.";

fn recipe_divider(options: &Options) -> Option<u32> {
    match options.method {
        Method::TwoVerticalLayers => options.layer_divider_percent,
        _ => None,
    }
}

pub(super) fn recipe(
    frames: &[GrayImage],
    mut recipe: Recipe,
    ffmpeg: &Path,
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Option<Recipe>> {
    let Some(track) = detect(frames, recipe_divider(options), cancel)? else {
        return Ok(None);
    };
    let size = frames[0].dimensions();
    let last = track.positions.last().unwrap();
    let faster = usize::from(last[1].y.abs() > last[0].y.abs());
    let points: Vec<_> = track.positions.iter().map(|p| p[faster]).collect();
    let indices = select(&points, size, options);
    if indices.len() > MAX_SELECTED {
        bail!("Too many frames needed. Select a shorter range.");
    }
    let Recipe {
        native,
        start,
        end,
        fps,
        ..
    } = recipe;
    let sx = native.0 as f64 / size.0 as f64;
    let sy = native.1 as f64 / size.1 as f64;
    let boundary = (track.split as f64 * sx).round() as u32;
    let mut anchors = VecDeque::<(usize, [motion::AlignmentFrame; 2], [Position; 2])>::new();
    let mut choices = Vec::new();
    let mut confidence = 1.0f32;
    decode::selected_frames(
        ffmpeg,
        &recipe.input,
        start,
        end,
        fps,
        &indices,
        native,
        cancel,
        |n, rgb| {
            report(
                0.65 + 0.1 * n as f32 / indices.len() as f32,
                "Aligning vertical layers",
            );
            if rgb.dimensions() != native {
                bail!("Video dimensions change within this scene");
            }
            let gray = motion::grayscale(&rgb);
            let prepared = [
                motion::AlignmentFrame::new(
                    imageops::crop_imm(&gray, 0, 0, boundary, native.1).to_image(),
                ),
                motion::AlignmentFrame::new(
                    imageops::crop_imm(&gray, boundary, 0, native.0 - boundary, native.1)
                        .to_image(),
                ),
            ];
            let mut solved = [Position::default(); 2];
            let mut quality = 1.0f32;
            if let Some((_, _, previous)) = anchors.back() {
                for (layer, position) in solved.iter_mut().enumerate() {
                    let mut found = false;
                    for (anchor, reference, positions) in &anchors {
                        check_cancel(cancel)?;
                        let coarse = track.positions[indices[n]][layer].y
                            - track.positions[indices[*anchor]][layer].y;
                        let guess = (coarse as f64 * sy).round() as i32;
                        if guess.abs() > native.1 as i32 / 3 {
                            continue;
                        }
                        if let Some(m) = native_match(
                            &reference[layer],
                            &prepared[layer],
                            guess,
                            (sy * 4.0).ceil() as i32,
                        ) {
                            let candidate = positions[layer].plus(m);
                            if (candidate.y - previous[layer].y) * last[layer].y < 0 {
                                continue;
                            }
                            *position = candidate;
                            quality = quality.min((1.0 - m.error / 36.0) * m.support);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        let error = if options.method == Method::TwoVerticalLayers {
                            LAYER_ALIGNMENT_ERROR
                        } else {
                            ALIGNMENT_ERROR
                        };
                        bail!(error);
                    }
                }
            }
            confidence = confidence.min(quality);
            choices.push(FrameChoice {
                time: start + indices[n] as f64 / fps,
                thumbnail: imageops::thumbnail(&rgb, 160, 90),
                sample: indices[n],
                position: solved[0],
                secondary: Some(solved[1]),
                scale: 1.0,
            });
            if anchors.len() == 3 {
                anchors.pop_front();
            }
            anchors.push_back((n, prepared, solved));
            Ok(())
        },
    )?;

    let first = &choices[0];
    let last = choices.last().unwrap();
    let left = last.position.y - first.position.y;
    let right = last.secondary.unwrap().y - first.secondary.unwrap().y;
    let travel = left.abs().min(right.abs());
    if travel as f32 / (native.1 as f32) < options.min_travel
        || left.signum() != right.signum()
        || (left - right).abs() < (native.1 / 20) as i32
    {
        return Ok(None);
    }
    recipe.frames = choices;
    recipe.confidence = confidence;
    recipe.split = Some(boundary);
    recipe.method = Method::TwoVerticalLayers;
    Ok(Some(recipe))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layered_tracking_rejects_cut_and_reversal() {
        let source = GrayImage::from_fn(160, 180, |x, y| {
            let bits = (x + y * 160).wrapping_mul(0x9e37_79b9);
            image::Luma([(bits ^ (bits >> 13)) as u8])
        });
        let sample = |n: u32| {
            GrayImage::from_fn(160, 120, |x, y| {
                *source.get_pixel(x, y + if x < 80 { 2 * n } else { 5 * n })
            })
        };
        let cancel = AtomicBool::new(false);
        let mut frames: Vec<_> = (0..7).map(sample).collect();
        assert!(detect(&frames, None, &cancel).unwrap().is_some());
        frames.push(sample(5));
        assert!(detect(&frames, None, &cancel).unwrap().is_none());
        *frames.last_mut().unwrap() = GrayImage::from_pixel(160, 120, image::Luma([255]));
        assert!(detect(&frames, None, &cancel).unwrap().is_none());
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(detect(&frames, None, &cancel).is_err());
    }
}
