//! Source-pixel composition with a minimum-disagreement seam through overlaps.
//! Seam search is bounded to a small grid; final pixels are never resized/blended.
use super::check_cancel;
use anyhow::{Result, bail};
use image::{RgbImage, RgbaImage};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy)]
struct Rect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}
impl Rect {
    fn intersection(self, b: Self) -> Option<Self> {
        let r = Self {
            x0: self.x0.max(b.x0),
            y0: self.y0.max(b.y0),
            x1: self.x1.min(b.x1),
            y1: self.y1.min(b.y1),
        };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }
    fn union(self, b: Self) -> Self {
        Self {
            x0: self.x0.min(b.x0),
            y0: self.y0.min(b.y0),
            x1: self.x1.max(b.x1),
            y1: self.y1.max(b.y1),
        }
    }
}

pub(super) struct Composer {
    canvas: RgbaImage,
    covered: Option<Rect>,
    previous: Option<Rect>,
    owners: Option<image::GrayImage>,
    source: u8,
}
impl Composer {
    pub fn new(size: (u32, u32)) -> Self {
        Self {
            canvas: RgbaImage::new(size.0, size.1),
            covered: None,
            previous: None,
            owners: None,
            source: 0,
        }
    }
    pub fn track_sources(&mut self) {
        self.owners = Some(image::GrayImage::new(
            self.canvas.width(),
            self.canvas.height(),
        ));
    }

    pub fn owners(&self) -> Option<&image::GrayImage> {
        self.owners.as_ref()
    }

    pub fn finish(self) -> RgbaImage {
        self.canvas
    }

    pub fn push(
        &mut self,
        frame: &RgbImage,
        offset: (u32, u32),
        cancel: &AtomicBool,
    ) -> Result<()> {
        check_cancel(cancel)?;
        if self.owners.is_some() {
            self.source += 1;
        }
        let Some(x1) = offset.0.checked_add(frame.width()) else {
            bail!("Invalid frame position");
        };
        let Some(y1) = offset.1.checked_add(frame.height()) else {
            bail!("Invalid frame position");
        };
        if x1 > self.canvas.width() || y1 > self.canvas.height() {
            bail!("Frame lies outside the stitch canvas");
        }
        let rect = Rect {
            x0: offset.0,
            y0: offset.1,
            x1,
            y1,
        };
        let movement = self
            .previous
            .map(|p| (offset.0 as i64 - p.x0 as i64, offset.1 as i64 - p.y0 as i64));
        if self.previous.is_some_and(|p| {
            p.x0 == rect.x0 && p.y0 == rect.y0 && p.x1 == rect.x1 && p.y1 == rect.y1
        }) {
            return Ok(());
        }
        let vertical = movement.is_none_or(|(dx, dy)| dy.abs() >= dx.abs());
        let positive = movement.is_none_or(|(dx, dy)| if vertical { dy > 0 } else { dx > 0 });
        let overlap = self.covered.and_then(|r| r.intersection(rect));
        if self.covered.is_some() && overlap.is_none() {
            bail!("Selected frames do not overlap. Include an intermediate frame.");
        }
        let seam = overlap
            .map(|r| self.seam(frame, offset, r, vertical, cancel))
            .transpose()?;
        for y in 0..frame.height() {
            if y % 32 == 0 {
                check_cancel(cancel)?;
            }
            for x in 0..frame.width() {
                let (cx, cy) = (offset.0 + x, offset.1 + y);
                let old = self.canvas.get_pixel(cx, cy);
                let replace = if old[3] == 0 {
                    true
                } else if let (Some(r), Some(path)) = (overlap, seam.as_ref()) {
                    let (cross, along) = if vertical {
                        (cx - r.x0, cy)
                    } else {
                        (cy - r.y0, cx)
                    };
                    let boundary = path[cross as usize];
                    if positive {
                        along >= boundary
                    } else {
                        along <= boundary
                    }
                } else {
                    false
                };
                if replace {
                    if let Some(owners) = &mut self.owners {
                        owners.put_pixel(cx, cy, image::Luma([self.source]));
                    }
                    let p = frame.get_pixel(x, y);
                    self.canvas
                        .put_pixel(cx, cy, image::Rgba([p[0], p[1], p[2], 255]));
                }
            }
        }
        self.covered = Some(self.covered.map_or(rect, |r| r.union(rect)));
        self.previous = Some(rect);
        Ok(())
    }

    fn seam(
        &self,
        frame: &RgbImage,
        offset: (u32, u32),
        r: Rect,
        vertical: bool,
        cancel: &AtomicBool,
    ) -> Result<Vec<u32>> {
        let (cross, along) = if vertical {
            (r.x1 - r.x0, r.y1 - r.y0)
        } else {
            (r.y1 - r.y0, r.x1 - r.x0)
        };
        let step = frame.width().max(frame.height()).div_ceil(512).max(1);
        let cols = cross.div_ceil(step) as usize;
        let rows = along.div_ceil(step) as usize;
        let mut costs = vec![0.0f32; cols * rows];
        let mut parents = vec![0i8; cols * rows];
        let mut previous = vec![0.0f32; rows];
        let mut current = vec![0.0f32; rows];
        for col in 0..cols {
            check_cancel(cancel)?;
            for row in 0..rows {
                let mut error = 0.0;
                let mut count = 0;
                // Pool several samples per cell so codec noise or a one-pixel
                // edge cannot decide where to cut an animated limb.
                for dv in [step / 4, step * 3 / 4] {
                    for du in [step / 4, step * 3 / 4] {
                        let u = (col as u32 * step + du).min(cross - 1);
                        let v = (row as u32 * step + dv).min(along - 1);
                        let (x, y) = if vertical {
                            (r.x0 + u, r.y0 + v)
                        } else {
                            (r.x0 + v, r.y0 + u)
                        };
                        let a = self.canvas.get_pixel(x, y);
                        if a[3] == 0 {
                            continue;
                        }
                        let b = frame.get_pixel(x - offset.0, y - offset.1);
                        for c in 0..3 {
                            let d = (a[c].abs_diff(b[c]) as f32 - 2.0).max(0.0);
                            error += d + d * d / 32.0;
                            count += 1;
                        }
                    }
                }
                let cost = error / count.max(1) as f32;
                costs[col * rows + row] = cost;
            }
        }
        protect_motion(&mut costs, cols, rows, cancel)?;
        for col in 0..cols {
            check_cancel(cancel)?;
            for row in 0..rows {
                let cost = costs[col * rows + row];
                let centre = (row as f32 / (rows.max(2) - 1) as f32 - 0.5).abs() * 0.02;
                if col == 0 {
                    current[row] = cost + centre;
                    continue;
                }
                let mut best = row;
                let mut value = previous[row];
                for candidate in [row.saturating_sub(1), (row + 1).min(rows - 1)] {
                    let candidate_value = previous[candidate] + 0.05;
                    if candidate_value < value {
                        best = candidate;
                        value = candidate_value;
                    }
                }
                current[row] = cost + centre + value;
                parents[col * rows + row] = (best as i32 - row as i32) as i8;
            }
            std::mem::swap(&mut previous, &mut current);
        }
        let mut row = (0..rows)
            .min_by(|&a, &b| previous[a].total_cmp(&previous[b]))
            .unwrap();
        let mut grid = vec![0u32; cols];
        for col in (0..cols).rev() {
            grid[col] = (row as u32 * step + step / 2).min(along - 1);
            row = (row as isize + parents[col * rows + row] as isize) as usize;
        }
        let start = if vertical { r.y0 } else { r.x0 };
        Ok((0..cross)
            .map(|u| {
                let shifted = u.saturating_sub(step / 2);
                let left = (shifted / step) as usize;
                let right = (left + 1).min(cols - 1);
                let t = shifted % step;
                start + (grid[left] * (step - t) + grid[right] * t) / step
            })
            .collect())
    }
}

/// A seam must not cross the unchanged intersection of two different arm poses.
/// Group broad disagreements into motion regions and protect their whole extent.
/// Opening removes thin codec/rounding edges; dilation joins parts of one moving
/// object before its bounding box is protected. Work is bounded to the seam grid.
fn protect_motion(costs: &mut [f32], cols: usize, rows: usize, cancel: &AtomicBool) -> Result<()> {
    let active: Vec<_> = costs.iter().map(|&c| c > 16.0).collect();
    let mut mask = vec![false; costs.len()];
    for x in 1..cols.saturating_sub(1) {
        check_cancel(cancel)?;
        for y in 1..rows.saturating_sub(1) {
            let i = x * rows + y;
            mask[i] =
                active[i] && active[i - 1] && active[i + 1] && active[i - rows] && active[i + rows];
        }
    }
    for _ in 0..2 {
        let previous = mask.clone();
        for x in 0..cols {
            for y in 0..rows {
                let i = x * rows + y;
                mask[i] = previous[i]
                    || (x > 0 && previous[i - rows])
                    || (x + 1 < cols && previous[i + rows])
                    || (y > 0 && previous[i - 1])
                    || (y + 1 < rows && previous[i + 1]);
            }
        }
    }
    let mut stack = Vec::new();
    let minimum = (costs.len() / 1000).max(8);
    for seed in 0..mask.len() {
        if !mask[seed] {
            continue;
        }
        check_cancel(cancel)?;
        mask[seed] = false;
        stack.push(seed);
        let (mut x0, mut x1, mut y0, mut y1) = (cols, 0, rows, 0);
        let mut count = 0;
        while let Some(i) = stack.pop() {
            let (x, y) = (i / rows, i % rows);
            count += 1;
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            let neighbours = [
                (x > 0).then(|| i - rows),
                (x + 1 < cols).then_some(i + rows),
                (y > 0).then(|| i - 1),
                (y + 1 < rows).then_some(i + 1),
            ];
            for n in neighbours.into_iter().flatten() {
                if mask[n] {
                    mask[n] = false;
                    stack.push(n);
                }
            }
        }
        if count >= minimum {
            for x in x0.saturating_sub(1)..=(x1 + 1).min(cols - 1) {
                for y in y0.saturating_sub(1)..=(y1 + 1).min(rows - 1) {
                    costs[x * rows + y] += 10000.0;
                }
            }
        }
    }
    Ok(())
}
