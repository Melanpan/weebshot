//! Alignment for a single plane that translates, changes scale and rotates
//! (camera roll), i.e. a similarity transform per frame.
use super::{FrameChoice, MAX_SELECTED, Options, Position, Recipe, check_cancel, decode, motion};
use anyhow::{Result, bail};
use image::{GrayImage, Luma, RgbImage, imageops};
use std::{collections::VecDeque, path::Path, sync::atomic::AtomicBool};

pub(super) const ALIGNMENT_ERROR: &str = "Could not align a single plane with shift, scale and rotation. Try a shorter In/Out range around one continuous shot.";

/// Rotations below this angle (about 0.4°) are treated as an unrotated frame.
/// At 1920 pixels this still moves a frame edge by less than seven pixels.
const MIN_ANGLE: f32 = 0.007;
/// Relative roll between two compared frames (about 20°). Larger turns leave
/// too little reliable overlap for the bilinear score.
const MAX_PAIR_ANGLE: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Transform {
    // Maps the newer frame to the older frame, relative to their image centers:
    // older = scale * rotate(angle) * newer + (x, y). Positive angles turn
    // clockwise on screen because y points down.
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) scale: f32,
    pub(super) angle: f32,
}

impl Transform {
    pub(super) const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        angle: 0.0,
    };

    /// Apply the linear part (scale and rotation) to a centered vector.
    fn linear(self, x: f32, y: f32) -> (f32, f32) {
        let (sin, cos) = self.angle.sin_cos();
        (
            self.scale * (cos * x - sin * y),
            self.scale * (sin * x + cos * y),
        )
    }

    /// Map a centered point of the newer frame into the older frame.
    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = self.linear(x, y);
        (x + self.x, y + self.y)
    }

    pub(super) fn then(self, other: Self) -> Self {
        let (x, y) = self.apply(other.x, other.y);
        Self {
            x,
            y,
            scale: self.scale * other.scale,
            angle: self.angle + other.angle,
        }
    }

    fn inverse(self) -> Self {
        let inverse = Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0 / self.scale,
            angle: -self.angle,
        };
        let (x, y) = inverse.linear(-self.x, -self.y);
        Self { x, y, ..inverse }
    }

    fn relative_to(self, anchor: Self) -> Self {
        anchor.inverse().then(self)
    }

    pub(super) fn rotated(self) -> bool {
        self.angle.abs() >= MIN_ANGLE
    }

    /// Integer canvas placement and size of a frame warped by this transform,
    /// in the reference frame's pixel grid. Unrotated frames keep the simple
    /// corner-and-resize placement used before rotation support.
    pub(super) fn placement(self, native: (u32, u32)) -> (Position, (u32, u32)) {
        if !self.rotated() {
            return (self.corner(native), frame_size(native, self.scale));
        }
        let (cx, cy) = (native.0 as f32 * 0.5, native.1 as f32 * 0.5);
        let corners = [(-cx, -cy), (cx, -cy), (-cx, cy), (cx, cy)].map(|(x, y)| {
            let (x, y) = self.apply(x, y);
            (x + cx, y + cy)
        });
        let x0 = corners
            .iter()
            .map(|c| c.0)
            .fold(f32::INFINITY, f32::min)
            .floor();
        let y0 = corners
            .iter()
            .map(|c| c.1)
            .fold(f32::INFINITY, f32::min)
            .floor();
        let x1 = corners
            .iter()
            .map(|c| c.0)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil();
        let y1 = corners
            .iter()
            .map(|c| c.1)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil();
        (
            Position {
                x: x0 as i32,
                y: y0 as i32,
            },
            ((x1 - x0).max(1.0) as u32, (y1 - y0).max(1.0) as u32),
        )
    }

    pub(super) fn corner(self, (w, h): (u32, u32)) -> Position {
        Position {
            x: (self.x + (1.0 - self.scale) * w as f32 * 0.5).round() as i32,
            y: (self.y + (1.0 - self.scale) * h as f32 * 0.5).round() as i32,
        }
    }
}

fn sample(image: &GrayImage, x: f32, y: f32) -> f32 {
    let (ix, iy) = (x as usize, y as usize);
    let tx = x - ix as f32;
    let ty = y - iy as f32;
    let stride = image.width() as usize;
    let raw = image.as_raw();
    let i = iy * stride + ix;
    let p = |offset: usize| f32::from(raw[i + offset]);
    let top = p(0) + tx * (p(1) - p(0));
    let bottom = p(stride) + tx * (p(stride + 1) - p(stride));
    top + ty * (bottom - top)
}

/// The newer frame's sample grid, centered. Its values and texture do not
/// depend on the candidate transform, so they are read once per refinement.
struct Grid {
    size: (f32, f32),
    /// Every grid point, for the overlap count.
    points: Vec<(f32, f32)>,
    /// Textured points and their intensity, which carry the residual.
    textured: Vec<(f32, f32, f32)>,
    minimum: usize,
}

impl Grid {
    fn new(b: &GrayImage) -> Self {
        let (w, h) = (b.width() as f32, b.height() as f32);
        // Keep roughly the same sample spacing at source resolution as on the
        // 320-pixel analysis frames. Thin line art on flat anime fills would
        // otherwise fall between a fixed 40-column grid at 1080p.
        let density = (b.width().max(b.height()) / 320).clamp(1, 4) as usize;
        let (rows, cols) = (28 * density, 40 * density);
        let mut points = Vec::with_capacity(rows * cols);
        let mut textured = Vec::new();
        for row in 0..rows {
            let y = ((row as f32 + 0.5) * h / rows as f32)
                .round()
                .clamp(1.0, h - 2.0);
            for col in 0..cols {
                let x = ((col as f32 + 0.5) * w / cols as f32)
                    .round()
                    .clamp(1.0, w - 2.0);
                let pixel = |dx, dy| b.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0];
                let detail = u16::from(pixel(1, 0).abs_diff(pixel(-1, 0)))
                    + u16::from(pixel(0, 1).abs_diff(pixel(0, -1)));
                let centered = (x - w * 0.5, y - h * 0.5);
                points.push(centered);
                if detail >= 12 {
                    textured.push((centered.0, centered.1, f32::from(pixel(0, 0))));
                }
            }
        }
        Self {
            size: (w, h),
            points,
            textured,
            minimum: 630 * density * density,
        }
    }
}

fn score(a: &GrayImage, b: &Grid, t: Transform) -> f32 {
    if !(0.78..=1.28).contains(&t.scale) || t.angle.abs() > MAX_PAIR_ANGLE {
        return f32::INFINITY;
    }
    let (sin, cos) = t.angle.sin_cos();
    let (sin, cos) = (t.scale * sin, t.scale * cos);
    let (w, h) = b.size;
    let (ox, oy) = (w * 0.5 + t.x, h * 0.5 + t.y);
    let map = |xc: f32, yc: f32| {
        let ax = cos * xc - sin * yc + ox;
        let ay = sin * xc + cos * yc + oy;
        (ax >= 0.0 && ax < w - 1.0 && ay >= 0.0 && ay < h - 1.0).then_some((ax, ay))
    };
    let count = b
        .points
        .iter()
        .filter(|&&(x, y)| map(x, y).is_some())
        .count();
    if count < b.minimum {
        return f32::INFINITY;
    }
    let mut total = 0.0;
    let mut textured = 0;
    let mut outliers = 0;
    for &(xc, yc, value) in &b.textured {
        let Some((ax, ay)) = map(xc, yc) else {
            continue;
        };
        let error = (sample(a, ax, ay) - value).abs();
        textured += 1;
        outliers += usize::from(error > 16.0);
        total += error.min(48.0);
    }
    if textured < 48 {
        return f32::INFINITY;
    }
    total / textured as f32 + 12.0 * outliers as f32 / textured as f32
}

/// Fraction of the frame revealed by a relative transform. A roll of `angle`
/// moves the frame edge by about half the width times the angle.
fn travel(relative: Transform, size: (u32, u32)) -> f32 {
    (relative.x.abs() / size.0 as f32).max(relative.y.abs() / size.1 as f32)
        + (relative.scale - 1.0).abs() * 0.5
        + relative.angle.abs() * 0.5
}

const ANALYSIS_STEPS: [Step; 4] = [
    (2.5, 0.025, 0.02, 2),
    (1.0, 0.01, 0.006, 2),
    (0.25, 0.0025, 0.0015, 2),
    (0.125, 0.001, 0.0005, 1),
];

/// One refinement level: translation step, scale step, angle step and radius.
/// Scale and angle are searched as one coupled pair at radius one so that a
/// level costs `(2r + 1)² × 9` evaluations rather than a full 4-D grid.
type Step = (f32, f32, f32, i32);

fn refine(
    a: &GrayImage,
    b: &GrayImage,
    guess: Transform,
    steps: &[Step],
    cancel: &AtomicBool,
) -> Result<(Transform, f32)> {
    if a.dimensions() != b.dimensions() {
        return Ok((guess, f32::INFINITY));
    }
    let b = &Grid::new(b);
    let mut best = (guess, score(a, b, guess));
    for &(shift, scale, angle, radius) in steps {
        // Iterate each level until it stops improving, so a translation guess
        // can walk to a rotated optimum further than one grid radius away.
        for _ in 0..4 {
            check_cancel(cancel)?;
            let seed = best.0;
            for ds in -1..=1 {
                for da in -1..=1 {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            let candidate = Transform {
                                x: seed.x + dx as f32 * shift,
                                y: seed.y + dy as f32 * shift,
                                scale: seed.scale + ds as f32 * scale,
                                angle: seed.angle + da as f32 * angle,
                            };
                            let error = score(a, b, candidate);
                            if error < best.1 {
                                best = (candidate, error);
                            }
                        }
                    }
                }
            }
            if best.0 == seed {
                break;
            }
        }
    }
    Ok(best)
}

/// `frames[core.0..=core.1]` must align as one plane; frames outside the core
/// are margins that extend it until a pair no longer matches, as happens at a
/// cut. `first` is the sample index of `frames[0]` on the recipe's FPS grid.
#[allow(clippy::too_many_arguments)]
pub(super) fn recipe(
    frames: &[GrayImage],
    core: (usize, usize),
    first: usize,
    mut recipe: Recipe,
    ffmpeg: &Path,
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Recipe> {
    let native = recipe.native;
    if native.0.min(native.1) < 24 {
        bail!("Video dimensions are too small for shift, scale and rotation alignment.");
    }
    if frames.len() < 2 || core.0 > core.1 || core.1 >= frames.len() {
        bail!(ALIGNMENT_ERROR);
    }
    let size = frames[0].dimensions();
    let mut locals = Vec::with_capacity(frames.len() - 1);
    // Start each pair from the previous pair's scale and roll: camera moves are smooth.
    let mut previous = Transform::IDENTITY;
    for i in 1..frames.len() {
        check_cancel(cancel)?;
        let initial = motion::estimate(&frames[i - 1], &frames[i], &options.thresholds);
        let guess = initial.map_or(previous, |m| Transform {
            x: m.dx as f32,
            y: m.dy as f32,
            ..previous
        });
        let (local, error) = refine(&frames[i - 1], &frames[i], guess, &ANALYSIS_STEPS, cancel)?;
        crate::log::log(
            crate::log::Level::Debug,
            "stitch",
            format!("similarity pair {i}: {local:?}, error {error:.2}"),
        );
        let aligned = error <= 13.0;
        locals.push(aligned.then_some(local));
        previous = if aligned { local } else { Transform::IDENTITY };
        report(
            0.15 + 0.3 * i as f32 / frames.len() as f32,
            "Estimating shift, scale and rotation",
        );
    }
    if locals[core.0..core.1].iter().any(Option::is_none) {
        bail!(ALIGNMENT_ERROR);
    }
    let (mut lo, mut hi) = core;
    while lo > 0 && locals[lo - 1].is_some() {
        lo -= 1;
    }
    while hi < locals.len() && locals[hi].is_some() {
        hi += 1;
    }
    if hi == lo {
        bail!(ALIGNMENT_ERROR);
    }
    // Report whether the matcher stopped before either end of its frames.
    recipe.trimmed = lo > 0 || hi + 1 < frames.len();
    let first = first + lo;
    let locals = &locals[lo..hi];
    let frames = &frames[lo..=hi];
    let mut poses = vec![Transform::IDENTITY];
    // Slow pans move less than a pixel between analysis samples. Chaining every
    // rounded pairwise estimate drifts, so each frame is re-matched directly
    // against a keyframe that is only advanced once the overlap shrinks.
    let mut key = 0;
    for i in 1..frames.len() {
        check_cancel(cancel)?;
        let mut pose = poses[i - 1].then(locals[i - 1].unwrap());
        if key + 1 < i {
            let (direct, error) = refine(
                &frames[key],
                &frames[i],
                pose.relative_to(poses[key]),
                &ANALYSIS_STEPS[1..],
                cancel,
            )?;
            if error <= 13.0 {
                pose = poses[key].then(direct);
            } else {
                key = i - 1;
            }
        }
        if !(0.5..=2.0).contains(&pose.scale) {
            bail!("Scale change exceeds the supported range. Mark a shorter In/Out range.");
        }
        if travel(pose.relative_to(poses[key]), size) >= 0.2 {
            key = i;
        }
        poses.push(pose);
        report(
            0.45 + 0.1 * poses.len() as f32 / frames.len() as f32,
            "Estimating shift, scale and rotation",
        );
    }
    // Select on movement of either corner so that changes in scale count as travel.
    let mut indices = vec![0];
    for (i, pose) in poses.iter().enumerate().skip(1) {
        if travel(pose.relative_to(poses[*indices.last().unwrap()]), size)
            >= options.selection_travel.clamp(0.02, 0.25)
        {
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
    let samples: Vec<_> = indices.iter().map(|i| first + i).collect();
    let mut anchors = VecDeque::<(usize, GrayImage, Transform)>::new();
    let mut confidence = 1.0f32;
    let mut solved = Vec::new();
    decode::selected_frames(
        ffmpeg,
        &recipe.input,
        recipe.start,
        recipe.end,
        recipe.fps,
        &samples,
        native,
        cancel,
        |n, rgb| {
            check_cancel(cancel)?;
            report(
                0.55 + 0.2 * n as f32 / indices.len() as f32,
                "Refining shift, scale and rotation at source resolution",
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
                    &[
                        (step, 0.006, 0.004, 2),
                        (step * 0.4, 0.0025, 0.0015, 2),
                        (0.5, 0.001, 0.0004, 2),
                    ],
                    cancel,
                )?;
                crate::log::log(
                    crate::log::Level::Debug,
                    "stitch",
                    format!(
                        "similarity frame {n} vs sample {j}: guess {guess:?}, native {local:?}, error {error:.2}"
                    ),
                );
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
                time: recipe.start + (first + i) as f64 / recipe.fps,
                thumbnail: imageops::thumbnail(&rgb, 160, 90),
                sample: first + i,
                position: pose.placement(native).0,
                secondary: None,
                scale: pose.scale,
                pose: Some(pose),
                layer_mask: None,
            });
            solved.push(pose);
            if anchors.len() == 3 {
                anchors.pop_front();
            }
            anchors.push_back((i, gray, pose));
            Ok(())
        },
    )?;
    if !solved
        .iter()
        .any(|p| (p.scale - 1.0).abs() >= 0.02 || p.rotated())
    {
        bail!("No useful scale change or rotation detected. Try Automatic or Single translation.");
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

/// Warp a native frame into its rotated, scaled placement. The mask marks pixels
/// that sample inside the source frame; corners outside it stay uncovered.
pub(super) fn warp(
    input: &RgbImage,
    pose: Transform,
    cancel: &AtomicBool,
) -> Result<(RgbImage, GrayImage)> {
    let native = input.dimensions();
    let (origin, (width, height)) = pose.placement(native);
    let (cx, cy) = (native.0 as f32 * 0.5, native.1 as f32 * 0.5);
    let inverse = pose.inverse();
    let (max_x, max_y) = ((native.0 - 1) as f32, (native.1 - 1) as f32);
    let mut warped = RgbImage::new(width, height);
    let mut mask = GrayImage::new(width, height);
    for y in 0..height {
        if y % 32 == 0 {
            check_cancel(cancel)?;
        }
        for x in 0..width {
            let (sx, sy) = inverse.apply(
                (origin.x + x as i32) as f32 - cx,
                (origin.y + y as i32) as f32 - cy,
            );
            let (sx, sy) = (sx + cx, sy + cy);
            // Half a pixel of tolerance keeps exact frame edges covered.
            if !(-0.5..=max_x + 0.5).contains(&sx) || !(-0.5..=max_y + 0.5).contains(&sy) {
                continue;
            }
            let (sx, sy) = (sx.clamp(0.0, max_x), sy.clamp(0.0, max_y));
            let (ix, iy) = (sx as u32, sy as u32);
            let (jx, jy) = ((ix + 1).min(native.0 - 1), (iy + 1).min(native.1 - 1));
            let (u, v) = (sx - ix as f32, sy - iy as f32);
            let a = input.get_pixel(ix, iy);
            let b = input.get_pixel(jx, iy);
            let c = input.get_pixel(ix, jy);
            let d = input.get_pixel(jx, jy);
            let output = warped.get_pixel_mut(x, y);
            for channel in 0..3 {
                let top = f32::from(a[channel]) * (1.0 - u) + f32::from(b[channel]) * u;
                let bottom = f32::from(c[channel]) * (1.0 - u) + f32::from(d[channel]) * u;
                output[channel] = (top * (1.0 - v) + bottom * v).round() as u8;
            }
            mask.put_pixel(x, y, Luma([255]));
        }
    }
    Ok((warped, mask))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Transform, b: Transform) -> bool {
        (a.x - b.x).abs() < 1e-3
            && (a.y - b.y).abs() < 1e-3
            && (a.scale - b.scale).abs() < 1e-5
            && (a.angle - b.angle).abs() < 1e-5
    }

    #[test]
    fn similarity_composition_round_trips() {
        let a = Transform {
            x: 12.0,
            y: -40.0,
            scale: 0.98,
            angle: 0.2,
        };
        let b = Transform {
            x: -3.0,
            y: 7.5,
            scale: 1.03,
            angle: -0.05,
        };
        assert!(close(a.then(a.inverse()), Transform::IDENTITY));
        assert!(close(a.then(b).relative_to(a), b));
        let (x, y) = a.then(b).apply(5.0, -9.0);
        let (bx, by) = b.apply(5.0, -9.0);
        let (ax, ay) = a.apply(bx, by);
        assert!((x - ax).abs() < 1e-3 && (y - ay).abs() < 1e-3);
        // Without rotation the composition matches the original scale-only form.
        let s = Transform { angle: 0.0, ..a };
        let t = Transform { angle: 0.0, ..b };
        let st = s.then(t);
        assert!((st.x - (s.x + s.scale * t.x)).abs() < 1e-4);
        assert!((st.scale - s.scale * t.scale).abs() < 1e-6);
    }

    #[test]
    fn rotated_placement_bounds_the_warped_frame() {
        let pose = Transform {
            x: 10.0,
            y: -20.0,
            scale: 1.0,
            angle: 0.3,
        };
        let (origin, size) = pose.placement((320, 180));
        let rgb = RgbImage::from_pixel(320, 180, image::Rgb([200, 100, 50]));
        let (warped, mask) = warp(&rgb, pose, &AtomicBool::new(false)).unwrap();
        assert_eq!(warped.dimensions(), size);
        // The rotated frame keeps its area and touches every side of its box.
        let covered = mask.pixels().filter(|p| p[0] == 255).count() as f32;
        assert!((covered / (320.0 * 180.0) - 1.0).abs() < 0.02, "{covered}");
        assert!(mask.get_pixel(0, 0)[0] == 0, "corner should be uncovered");
        let (cx, cy) = (size.0 as i32 / 2 + origin.x, size.1 as i32 / 2 + origin.y);
        assert!(
            (cx - 170).abs() <= 1 && (cy - 70).abs() <= 1,
            "{origin:?} {size:?}"
        );
        // Unrotated poses keep the original corner-and-resize placement.
        let flat = Transform { angle: 0.0, ..pose };
        assert_eq!(
            flat.placement((320, 180)),
            (flat.corner((320, 180)), (320, 180))
        );
    }
}
