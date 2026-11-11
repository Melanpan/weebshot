//! Experimental vertical stitching when several independently moving regions overlap.
use super::{FrameChoice, Method, Options, Recipe, check_cancel, decode, zoom::Transform};
use anyhow::{Result, bail};
use image::{GrayImage, imageops};
use std::{path::Path, sync::atomic::AtomicBool};

const PATCH_RADIUS: i32 = 9;
const MIN_INLIERS: usize = 12;
const MAX_MODELS: usize = 5;
pub(super) const ERROR: &str = "Could not track consistent layered motion. Mark a shorter In/Out range around one vertical pan.";

#[derive(Clone, Copy)]
struct Match {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
}

struct Features {
    pixels: Vec<i16>,
    width: i32,
    height: i32,
}

impl Features {
    fn new(gray: &GrayImage) -> Self {
        let smooth = imageops::blur(gray, 3.0);
        Self {
            pixels: gray
                .as_raw()
                .iter()
                .zip(smooth.as_raw())
                .map(|(&pixel, &average)| i16::from(pixel) - i16::from(average))
                .collect(),
            width: gray.width() as i32,
            height: gray.height() as i32,
        }
    }

    fn patch(&self, x: i32, y: i32) -> [i16; 49] {
        std::array::from_fn(|i| {
            let px = x + (i % 7) as i32 * 3 - PATCH_RADIUS;
            let py = y + (i / 7) as i32 * 3 - PATCH_RADIUS;
            self.pixels[(py * self.width + px) as usize]
        })
    }
}

fn patch_stats(patch: &[i16; 49]) -> (i32, i32) {
    let (sum, squares) = patch.iter().fold((0, 0), |(sum, squares), &p| {
        let p = i32::from(p);
        (sum + p, squares + p * p)
    });
    (sum, squares * 49 - sum * sum)
}

fn patch_matches(a: &Features, b: &Features, cancel: &AtomicBool) -> Result<Vec<Match>> {
    let mut matches = Vec::new();
    let margin = PATCH_RADIUS + 1;
    let max_dx = a.width / 10;
    let max_dy = a.height * 9 / 20;
    let spacing = (a.width / 28).max(12) as usize;
    for y in (margin..a.height - margin).step_by(spacing) {
        for x in (margin..a.width - margin).step_by(spacing) {
            check_cancel(cancel)?;
            let patch = a.patch(x, y);
            let (mean, variance) = patch_stats(&patch);
            if variance < 60_000 {
                continue;
            }
            let norm = (variance as f32).sqrt();
            let correlation = |cx, cy| {
                let other = b.patch(cx, cy);
                let mut cross = 0;
                let mut sum = 0;
                let mut squares = 0;
                for (&p, q) in patch.iter().zip(other) {
                    let q = i32::from(q);
                    cross += i32::from(p) * q;
                    sum += q;
                    squares += q * q;
                }
                let other_variance = squares * 49 - sum * sum;
                if other_variance < 2_000 {
                    return -1.0;
                }
                (cross * 49 - mean * sum) as f32 / (norm * (other_variance as f32).sqrt())
            };
            let left = (x - max_dx).max(margin);
            let right = (x + max_dx).min(b.width - margin - 1);
            let top = (y - max_dy).max(margin);
            let bottom = (y + max_dy).min(b.height - margin - 1);
            let mut peaks = Vec::new();
            for cy in (top..=bottom).step_by(2) {
                for cx in (left..=right).step_by(2) {
                    let score = correlation(cx, cy);
                    if score > 0.62 {
                        peaks.push((score, cx, cy));
                    }
                }
            }
            peaks.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
            let mut best = (-1.0, x, y);
            for &(_, px, py) in peaks.iter().take(8) {
                for cy in (py - 1).max(top)..=(py + 1).min(bottom) {
                    for cx in (px - 1).max(left)..=(px + 1).min(right) {
                        let score = correlation(cx, cy);
                        if score > best.0 {
                            best = (score, cx, cy);
                        }
                    }
                }
            }
            if best.0 < 0.76
                || peaks.iter().any(|&(s, cx, cy)| {
                    s > best.0 - 0.07 && (cx - best.1).abs().max((cy - best.2).abs()) > 5
                })
            {
                continue;
            }
            matches.push(Match {
                x: x as f32 - a.width as f32 * 0.5,
                y: y as f32 - a.height as f32 * 0.5,
                dx: (best.1 - x) as f32,
                dy: (best.2 - y) as f32,
            });
        }
    }
    Ok(matches)
}

fn hypothesis(a: Match, b: Match) -> Option<Transform> {
    let x = a.x - b.x;
    let y = a.y - b.y;
    let distance = x * x + y * y;
    if distance < 32.0 * 32.0 {
        return None;
    }
    let scale = (x * (a.dx - b.dx) + y * (a.dy - b.dy)) / distance;
    if !(-0.20..=0.24).contains(&scale) {
        return None;
    }
    Some(Transform {
        x: a.dx - scale * a.x,
        y: a.dy - scale * a.y,
        scale: 1.0 + scale,
    })
}

fn agrees(point: Match, model: Transform) -> bool {
    let scale = model.scale - 1.0;
    let x = point.dx - model.x - scale * point.x;
    let y = point.dy - model.y - scale * point.y;
    x * x + y * y < 7.0
}

fn refine(points: &[Match]) -> Transform {
    let sum = points.iter().fold([0.0; 4], |[x, y, dx, dy], p| {
        [x + p.x, y + p.y, dx + p.dx, dy + p.dy]
    });
    let [x, y, dx, dy] = sum.map(|value| value / points.len() as f32);
    let (covariance, variance) = points.iter().fold((0.0, 0.0), |(covariance, variance), p| {
        let px = p.x - x;
        let py = p.y - y;
        (
            covariance + px * (p.dx - dx) + py * (p.dy - dy),
            variance + px * px + py * py,
        )
    });
    let scale = covariance / variance.max(1.0);
    Transform {
        x: dx - scale * x,
        y: dy - scale * y,
        scale: 1.0 + scale,
    }
}

fn groups(mut matches: Vec<Match>, cancel: &AtomicBool) -> Result<Vec<(Transform, usize)>> {
    let mut models = Vec::new();
    for _ in 0..MAX_MODELS {
        let mut best = None;
        let mut best_count = MIN_INLIERS - 1;
        for (i, &a) in matches.iter().enumerate() {
            check_cancel(cancel)?;
            for &b in &matches[i + 1..] {
                let Some(model) = hypothesis(a, b) else {
                    continue;
                };
                let count = matches.iter().filter(|&&p| agrees(p, model)).count();
                if count > best_count {
                    best = Some(model);
                    best_count = count;
                }
            }
        }
        let Some(model) = best else { break };
        let (inliers, other): (Vec<_>, Vec<_>) =
            matches.into_iter().partition(|&p| agrees(p, model));
        models.push((refine(&inliers), best_count));
        matches = other;
    }
    Ok(models)
}

pub(super) fn recipe(
    frames: &[GrayImage],
    mut recipe: Recipe,
    ffmpeg: &Path,
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Recipe> {
    let (width, height) = frames[0].dimensions();
    if width < 160 || height < 100 {
        bail!("Layered motion requires a larger analysis frame.");
    }
    // Long enough to distinguish foreground motion from the background, while
    // retaining overlap. The spacing control adjusts this interval.
    let stride = (recipe.fps * 1.5 * f64::from(options.selection_travel / 0.08))
        .round()
        .clamp(2.0, 4.0) as usize;
    let mut indices: Vec<_> = (0..frames.len()).step_by(stride).collect();
    let last = frames.len() - 1;
    if last - indices[indices.len() - 1] >= 2 {
        indices.push(last);
    }
    if indices.len() < 3 {
        bail!("Mark at least three seconds of a continuous vertical pan for layered motion.");
    }
    let mut poses = vec![Transform::IDENTITY];
    let mut reference = Features::new(&frames[0]);
    let mut speed = None::<f32>;
    for (n, pair) in indices.windows(2).enumerate() {
        check_cancel(cancel)?;
        report(
            0.15 + 0.5 * n as f32 / indices.len() as f32,
            "Tracking independently moving artwork",
        );
        let current = Features::new(&frames[pair[1]]);
        let matches = patch_matches(&reference, &current, cancel)?;
        let models = groups(matches, cancel)?;
        let slowest = models
            .iter()
            .filter(|(m, _)| m.y.abs() > 3.0 && m.x.abs() < m.y.abs() * 0.4)
            .min_by(|(a, _), (b, _)| a.y.abs().total_cmp(&b.y.abs()));
        let Some(&(slow, _)) = slowest else {
            bail!(ERROR)
        };
        let fast = models.iter().find(|(m, _)| {
            m.y.signum() == slow.y.signum()
                && m.y.abs() > slow.y.abs() * 1.45
                && m.x.abs() < m.y.abs() * 0.4
        });
        let Some(&(fast, _)) = fast else { bail!(ERROR) };
        let motion = if options.prefer_background {
            slow
        } else {
            fast
        };
        let velocity = motion.y / (pair[1] - pair[0]) as f32;
        if speed.is_some_and(|old| {
            velocity.signum() != old.signum() || !(0.5..=2.0).contains(&(velocity / old))
        }) {
            bail!(ERROR);
        }
        speed = Some(velocity);
        let inverse = Transform {
            x: -motion.x / motion.scale,
            y: -motion.y / motion.scale,
            scale: 1.0 / motion.scale,
        };
        let pose = poses[n].then(inverse);
        if !(0.5..=2.0).contains(&pose.scale) {
            bail!(ERROR);
        }
        poses.push(pose);
        reference = current;
    }
    let native = recipe.native;
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
            let mut pose = poses[n];
            pose.x *= native.0 as f32 / width as f32;
            pose.y *= native.1 as f32 / height as f32;
            recipe.frames.push(FrameChoice {
                time: recipe.start + indices[n] as f64 / recipe.fps,
                thumbnail: imageops::thumbnail(&rgb, 160, 90),
                sample: indices[n],
                position: pose.corner(native),
                secondary: None,
                scale: pose.scale,
            });
            Ok(())
        },
    )?;
    recipe.confidence = 0.0; // Different layers disagree by design. No global confidence.
    recipe.method = Method::LayeredMotion;
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_consensus_recovers_overlapping_groups() {
        let slow = Transform {
            x: 0.0,
            y: -40.0,
            scale: 0.975,
        };
        let fast = Transform {
            x: 2.0,
            y: -105.0,
            scale: 0.935,
        };
        let mut points = Vec::new();
        for row in 0..5 {
            for column in 0..8 {
                let x = (column - 4) as f32 * 42.0;
                let y = (row - 2) as f32 * 31.0;
                let model = if (row + column) % 2 == 0 { slow } else { fast };
                points.push(Match {
                    x,
                    y,
                    dx: model.x + (model.scale - 1.0) * x,
                    dy: model.y + (model.scale - 1.0) * y,
                });
            }
        }
        points.extend((0..8).map(|i| Match {
            x: 5.0 * i as f32,
            y: 5.0 * i as f32,
            dx: i as f32 * 11.0,
            dy: 110.0,
        }));
        let models = groups(points, &AtomicBool::new(false)).unwrap();
        assert_eq!(models.len(), 2);
        for expected in [slow, fast] {
            assert!(models.iter().any(|(actual, count)| {
                *count == 20
                    && (actual.x - expected.x).abs() < 0.01
                    && (actual.y - expected.y).abs() < 0.01
                    && (actual.scale - expected.scale).abs() < 0.001
            }));
        }
    }
}
