//! Alignment for a single plane that translates and changes scale.
use super::{FrameChoice, MAX_SELECTED, Options, Position, Recipe, check_cancel, decode, motion};
use anyhow::{Result, bail};
use image::{GrayImage, RgbImage, imageops};
use std::{collections::VecDeque, path::Path, sync::atomic::AtomicBool};

pub(super) const ALIGNMENT_ERROR: &str = "Could not align a single plane with shift and scale. Try a shorter In/Out range around one continuous shot.";

#[derive(Clone, Copy, Debug)]
pub(super) struct Transform {
    // Maps the newer frame to the older frame, relative to their image centers.
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) scale: f32,
}

impl Transform {
    pub(super) const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
    };

    pub(super) fn then(self, other: Self) -> Self {
        Self {
            x: self.x + self.scale * other.x,
            y: self.y + self.scale * other.y,
            scale: self.scale * other.scale,
        }
    }

    fn relative_to(self, anchor: Self) -> Self {
        Self {
            x: (self.x - anchor.x) / anchor.scale,
            y: (self.y - anchor.y) / anchor.scale,
            scale: self.scale / anchor.scale,
        }
    }

    pub(super) fn corner(self, (w, h): (u32, u32)) -> Position {
        Position {
            x: (self.x + (1.0 - self.scale) * w as f32 * 0.5).round() as i32,
            y: (self.y + (1.0 - self.scale) * h as f32 * 0.5).round() as i32,
        }
    }
}

fn sample(image: &GrayImage, x: f32, y: f32) -> f32 {
    let (ix, iy) = (x as u32, y as u32);
    let tx = x - ix as f32;
    let ty = y - iy as f32;
    let p = |dx, dy| image.get_pixel(ix + dx, iy + dy)[0] as f32;
    let top = p(0, 0) + tx * (p(1, 0) - p(0, 0));
    let bottom = p(0, 1) + tx * (p(1, 1) - p(0, 1));
    top + ty * (bottom - top)
}

fn score(a: &GrayImage, b: &GrayImage, t: Transform) -> f32 {
    if !(0.78..=1.28).contains(&t.scale) {
        return f32::INFINITY;
    }
    let (w, h) = (a.width() as f32, a.height() as f32);
    let mut total = 0.0;
    let mut textured = 0;
    let mut count = 0;
    let mut outliers = 0;
    for row in 0..28 {
        let y = ((row as f32 + 0.5) * h / 28.0).round().clamp(1.0, h - 2.0);
        let ay = (y - h * 0.5) * t.scale + h * 0.5 + t.y;
        if ay < 0.0 || ay >= h - 1.0 {
            continue;
        }
        for col in 0..40 {
            let x = ((col as f32 + 0.5) * w / 40.0).round().clamp(1.0, w - 2.0);
            let ax = (x - w * 0.5) * t.scale + w * 0.5 + t.x;
            if ax < 0.0 || ax >= w - 1.0 {
                continue;
            }
            let pixel = |dx, dy| b.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0];
            let detail = u16::from(pixel(1, 0).abs_diff(pixel(-1, 0)))
                + u16::from(pixel(0, 1).abs_diff(pixel(0, -1)));
            let error = (sample(a, ax, ay) - f32::from(pixel(0, 0))).abs();
            count += 1;
            if detail >= 12 {
                textured += 1;
                outliers += usize::from(error > 16.0);
                total += error.min(48.0);
            }
        }
    }
    if count < 630 || textured < 48 {
        return f32::INFINITY;
    }
    total / textured as f32 + 12.0 * outliers as f32 / textured as f32
}

fn refine(
    a: &GrayImage,
    b: &GrayImage,
    guess: Transform,
    steps: &[(f32, f32, i32)],
    cancel: &AtomicBool,
) -> Result<(Transform, f32)> {
    let mut best = (guess, score(a, b, guess));
    for &(shift, scale, radius) in steps {
        check_cancel(cancel)?;
        let seed = best.0;
        for ds in -radius..=radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let candidate = Transform {
                        x: seed.x + dx as f32 * shift,
                        y: seed.y + dy as f32 * shift,
                        scale: seed.scale + ds as f32 * scale,
                    };
                    let error = score(a, b, candidate);
                    if error < best.1 {
                        best = (candidate, error);
                    }
                }
            }
        }
    }
    Ok(best)
}

pub(super) fn recipe(
    frames: &[GrayImage],
    mut recipe: Recipe,
    ffmpeg: &Path,
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Recipe> {
    let native = recipe.native;
    if native.0.min(native.1) < 24 {
        bail!("Video dimensions are too small for shift and scale alignment.");
    }
    let size = frames[0].dimensions();
    let mut poses = vec![Transform::IDENTITY];
    for (i, pair) in frames.windows(2).enumerate() {
        check_cancel(cancel)?;
        let initial = motion::estimate(&pair[0], &pair[1], &options.thresholds);
        let guess = initial.map_or(Transform::IDENTITY, |m| Transform {
            x: m.dx as f32,
            y: m.dy as f32,
            scale: 1.0,
        });
        let (local, error) = refine(
            &pair[0],
            &pair[1],
            guess,
            &[(2.5, 0.025, 2), (1.0, 0.01, 2), (0.25, 0.0025, 2)],
            cancel,
        )?;
        if error > 13.0 {
            bail!(ALIGNMENT_ERROR);
        }
        let pose = poses[i].then(local);
        if !(0.5..=2.0).contains(&pose.scale) {
            bail!("Scale change exceeds the supported range. Mark a shorter In/Out range.");
        }
        poses.push(pose);
        report(
            0.15 + 0.4 * poses.len() as f32 / frames.len() as f32,
            "Estimating shift and scale",
        );
    }
    // Select on movement of either corner so that changes in scale count as travel.
    let mut indices = vec![0];
    for (i, pose) in poses.iter().enumerate().skip(1) {
        let relative = pose.relative_to(poses[*indices.last().unwrap()]);
        let travel = (relative.x.abs() / size.0 as f32).max(relative.y.abs() / size.1 as f32)
            + (relative.scale - 1.0).abs() * 0.5;
        if travel >= options.selection_travel.clamp(0.02, 0.25) {
            indices.push(i);
        }
    }
    let last = poses.len() - 1;
    if *indices.last().unwrap() != last {
        indices.push(last);
    }
    if indices.len() > MAX_SELECTED {
        bail!("Too many frames needed. Select a shorter range.");
    }
    let mut anchors = VecDeque::<(usize, GrayImage, Transform)>::new();
    let mut confidence = 1.0f32;
    let mut solved = Vec::new();
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
                0.55 + 0.2 * n as f32 / indices.len() as f32,
                "Refining shift and scale at source resolution",
            );
            let gray = motion::grayscale(&rgb);
            let i = indices[n];
            let mut result = None;
            for (j, reference, anchor) in &anchors {
                let mut guess = poses[i].relative_to(poses[*j]);
                guess.x *= native.0 as f32 / size.0 as f32;
                guess.y *= native.1 as f32 / size.1 as f32;
                let step = (native.0 as f32 / size.0 as f32).max(1.0);
                let (local, error) = refine(
                    reference,
                    &gray,
                    guess,
                    &[(step, 0.006, 2), (step * 0.4, 0.0025, 2), (0.5, 0.001, 2)],
                    cancel,
                )?;
                if error <= 14.0 {
                    result = Some((anchor.then(local), error));
                    break;
                }
            }
            let (pose, error) = if n == 0 {
                (Transform::IDENTITY, 0.0)
            } else {
                let Some(result) = result else {
                    bail!(ALIGNMENT_ERROR)
                };
                result
            };
            if !(0.5..=2.0).contains(&pose.scale) {
                bail!("Scale change exceeds the supported range. Mark a shorter In/Out range.");
            }
            confidence = confidence.min(1.0 - error / 40.0);
            recipe.frames.push(FrameChoice {
                time: recipe.start + i as f64 / recipe.fps,
                thumbnail: imageops::thumbnail(&rgb, 160, 90),
                sample: i,
                position: pose.corner(native),
                secondary: None,
                scale: pose.scale,
            });
            solved.push(pose);
            if anchors.len() == 3 {
                anchors.pop_front();
            }
            anchors.push_back((i, gray, pose));
            Ok(())
        },
    )?;
    if !solved.iter().any(|p| (p.scale - 1.0).abs() >= 0.02) {
        bail!("No useful scale change detected. Try Automatic or Single translation.");
    }
    recipe.confidence = confidence;
    recipe.method = super::Method::ShiftAndScale;
    Ok(recipe)
}

pub(super) fn frame_size(native: (u32, u32), scale: f32) -> (u32, u32) {
    (
        (native.0 as f32 * scale).round().max(1.0) as u32,
        (native.1 as f32 * scale).round().max(1.0) as u32,
    )
}

pub(super) fn bounds(
    positions: &[Position],
    sizes: &[(u32, u32)],
) -> Result<(Position, (u32, u32))> {
    let mut min = positions[0];
    let mut max = min;
    for (position, &(width, height)) in positions.iter().zip(sizes) {
        min.x = min.x.min(position.x);
        min.y = min.y.min(position.y);
        max.x = max.x.max(position.x + width as i32);
        max.y = max.y.max(position.y + height as i32);
    }
    let (width, height) = ((max.x - min.x) as u32, (max.y - min.y) as u32);
    if width > super::MAX_EDGE
        || height > super::MAX_EDGE
        || u64::from(width) * u64::from(height) > super::MAX_PIXELS
    {
        bail!("Stitch exceeds the 32 megapixel / 16384 pixel edge limit. Select a shorter range.");
    }
    Ok((min, (width, height)))
}

/// Resample a frame with a bounded allocation and a cancellation point per block of rows.
pub(super) fn resize(
    input: &RgbImage,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> Result<RgbImage> {
    fn axis(out: u32, input_size: u32, output_size: u32) -> (u32, u32, f32) {
        let coordinate = ((out as f32 + 0.5) * input_size as f32 / output_size as f32 - 0.5)
            .clamp(0.0, (input_size - 1) as f32);
        let start = coordinate as u32;
        (
            start,
            (start + 1).min(input_size - 1),
            coordinate - start as f32,
        )
    }
    let columns: Vec<_> = (0..width).map(|x| axis(x, input.width(), width)).collect();
    let mut resized = RgbImage::new(width, height);
    for (y, row) in resized.rows_mut().enumerate() {
        if y % 32 == 0 {
            check_cancel(cancel)?;
        }
        let (top, bottom, v) = axis(y as u32, input.height(), height);
        for (output, &(left, right, u)) in row.zip(&columns) {
            let a = input.get_pixel(left, top);
            let b = input.get_pixel(right, top);
            let c = input.get_pixel(left, bottom);
            let d = input.get_pixel(right, bottom);
            for channel in 0..3 {
                let top = f32::from(a[channel]) * (1.0 - u) + f32::from(b[channel]) * u;
                let bottom = f32::from(c[channel]) * (1.0 - u) + f32::from(d[channel]) * u;
                output[channel] = (top * (1.0 - v) + bottom * v).round() as u8;
            }
        }
    }
    Ok(resized)
}
