//! Source-pixel composition with a minimum-disagreement seam through overlaps.
//! Seam search is bounded to a small grid; final pixels are never resized or
//! averaged between frames. Optionally each frame's broad colour is shifted to
//! agree with the canvas before it is placed (`Composer::matched`).
use super::check_cancel;
use anyhow::{Result, bail};
use image::{GrayImage, RgbImage, RgbaImage};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, PartialEq)]
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
    /// The seam `matched` shaped its frame around, for the `push` of that
    /// frame: where the frame lands and the path.
    planned: Option<(Rect, Vec<u32>)>,
}
impl Composer {
    pub fn new(size: (u32, u32)) -> Self {
        Self {
            canvas: RgbaImage::new(size.0, size.1),
            covered: None,
            previous: None,
            owners: None,
            source: 0,
            planned: None,
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

    /// `mask` limits a warped frame to the pixels it actually covers; zero
    /// entries are neither drawn nor used to choose the seam.
    pub fn push(
        &mut self,
        frame: &RgbImage,
        mask: Option<&GrayImage>,
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
        let planned = self.planned.take();
        if self.previous == Some(rect) {
            return Ok(());
        }
        let (vertical, positive) = self.direction(rect);
        let overlap = self.covered.and_then(|r| r.intersection(rect));
        if self.covered.is_some() && overlap.is_none() {
            bail!("Selected frames do not overlap. Include an intermediate frame.");
        }
        let seam = match (overlap, planned) {
            (Some(_), Some((planned, path))) if planned == rect => Some(path),
            (Some(r), _) => Some(self.seam(frame, mask, offset, r, vertical, cancel)?),
            (None, _) => None,
        };
        for y in 0..frame.height() {
            if y % 32 == 0 {
                check_cancel(cancel)?;
            }
            for x in 0..frame.width() {
                if mask.is_some_and(|m| m.get_pixel(x, y)[0] == 0) {
                    continue;
                }
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

    /// Whether a frame at `rect` continues along y, and towards larger
    /// coordinates, judged from the frame before it.
    fn direction(&self, rect: Rect) -> (bool, bool) {
        let movement = self.previous.map(|p| {
            (
                i64::from(rect.x0) - i64::from(p.x0),
                i64::from(rect.y0) - i64::from(p.y0),
            )
        });
        let vertical = movement.is_none_or(|(dx, dy)| dy.abs() >= dx.abs());
        let positive = movement.is_none_or(|(dx, dy)| if vertical { dy > 0 } else { dx > 0 });
        (vertical, positive)
    }

    /// `frame` with its broad colour shifted to agree with what the canvas
    /// already holds, or `None` when nothing needs to change. Call it right
    /// before the `push` of the same frame.
    ///
    /// Shots often carry lighting fixed to the screen instead of the artwork
    /// (a vignette, a glow, a haze gradient), so the same artwork differs by
    /// a few levels between frames and every seam shows as a step. The
    /// difference is measured per cell where the frame overlaps the canvas,
    /// ignoring cells that mostly disagree (animation), smoothed, carried
    /// into the newly revealed area with a falloff, and added back. Only
    /// that smooth field changes: lines and texture keep their source pixels.
    ///
    /// Lighting that itself changes during the shot (a coloured beam fading
    /// out) differs by far more. Carrying that much across a frame would
    /// repaint it in an earlier moment's light, so only `BROAD` levels are
    /// carried; the rest is added in full at the seam, where it closes the
    /// step, and released over a short distance behind it.
    pub fn matched(
        &mut self,
        frame: &RgbImage,
        mask: Option<&GrayImage>,
        offset: (u32, u32),
        cancel: &AtomicBool,
    ) -> Result<Option<RgbImage>> {
        const CELL: u32 = 16;
        // Within a cell, lighting moves every pixel by about the same amount;
        // pixels further than this from the cell's typical difference are a
        // different drawing or an edge that moved.
        const SPREAD: i32 = 14;
        // Beyond this a cell is a different drawing, however even it is.
        const LIMIT: i32 = 96;
        // Differences up to this are taken as lighting on their own. Larger
        // ones (a coloured light fading in or out) must be reached from such
        // cells in steps of at most `STEP`, which a soft light is and the
        // inside of a repainted shape, ringed by cells of mixed pixels, is not.
        const SOFT: f32 = 32.0;
        const STEP: f32 = 32.0;
        // The most that is carried across the frame.
        const BROAD: f32 = 32.0;
        let (w, h) = frame.dimensions();
        self.planned = None;
        if offset.0 + w > self.canvas.width() || offset.1 + h > self.canvas.height() {
            return Ok(None);
        }
        let rect = Rect {
            x0: offset.0,
            y0: offset.1,
            x1: offset.0 + w,
            y1: offset.1 + h,
        };
        let Some(overlap) = self.covered.and_then(|r| r.intersection(rect)) else {
            return Ok(None);
        };
        if self.previous == Some(rect) {
            return Ok(None);
        }
        let (gw, gh) = (w.div_ceil(CELL) as usize, h.div_ceil(CELL) as usize);
        let mut field = vec![[0.0f32; 3]; gw * gh];
        let mut known = vec![false; gw * gh];
        let mut samples: Vec<[i32; 3]> = Vec::with_capacity((CELL * CELL / 4) as usize);
        let mut channel: Vec<i32> = Vec::with_capacity(samples.capacity());
        for gy in 0..gh {
            check_cancel(cancel)?;
            for gx in 0..gw {
                samples.clear();
                for y in (gy as u32 * CELL..((gy as u32 + 1) * CELL).min(h)).step_by(2) {
                    for x in (gx as u32 * CELL..((gx as u32 + 1) * CELL).min(w)).step_by(2) {
                        let old = self.canvas.get_pixel(offset.0 + x, offset.1 + y);
                        if old[3] == 0 || mask.is_some_and(|m| m.get_pixel(x, y)[0] == 0) {
                            continue;
                        }
                        let new = frame.get_pixel(x, y);
                        samples.push([0, 1, 2].map(|c| i32::from(old[c]) - i32::from(new[c])));
                    }
                }
                // A cell on the canvas edge says nothing reliable.
                if samples.len() < (CELL * CELL / 8) as usize {
                    continue;
                }
                let typical = [0, 1, 2].map(|c| {
                    channel.clear();
                    channel.extend(samples.iter().map(|d| d[c]));
                    let middle = channel.len() / 2;
                    *channel.select_nth_unstable(middle).1
                });
                if typical.iter().any(|v| v.abs() > LIMIT) {
                    continue;
                }
                let (mut sum, mut agree) = ([0i32; 3], 0usize);
                for d in &samples {
                    if (0..3).all(|c| (d[c] - typical[c]).abs() < SPREAD) {
                        agree += 1;
                        for c in 0..3 {
                            sum[c] += d[c];
                        }
                    }
                }
                // Nor does one that mostly disagrees with itself.
                if agree * 4 >= samples.len() * 3 {
                    let i = gy * gw + gx;
                    known[i] = true;
                    field[i] = sum.map(|v| v as f32 / agree as f32);
                }
            }
        }
        // Keep large differences only where they join the small ones smoothly.
        let mut reached: Vec<bool> = (0..gw * gh)
            .map(|i| known[i] && field[i].iter().all(|v| v.abs() <= SOFT))
            .collect();
        let mut stack: Vec<usize> = (0..gw * gh).filter(|&i| reached[i]).collect();
        while let Some(i) = stack.pop() {
            let (x, y) = (i % gw, i / gw);
            let around = [
                (x > 0).then(|| i - 1),
                (x + 1 < gw).then_some(i + 1),
                (y > 0).then(|| i - gw),
                (y + 1 < gh).then_some(i + gw),
            ];
            for n in around.into_iter().flatten() {
                if known[n]
                    && !reached[n]
                    && (0..3).all(|c| (field[n][c] - field[i][c]).abs() <= STEP)
                {
                    reached[n] = true;
                    stack.push(n);
                }
            }
        }
        for i in 0..gw * gh {
            if !reached[i] {
                known[i] = false;
                field[i] = [0.0; 3];
            }
        }
        if !known.iter().any(|&k| k) {
            return Ok(None);
        }
        // What is carried across the frame comes only from the cells that
        // differ a little; the rest is treated there as not measured.
        let modest: Vec<bool> = (0..gw * gh)
            .map(|i| known[i] && field[i].iter().all(|v| v.abs() <= BROAD))
            .collect();
        let whole = spread(field.clone(), &known, gw, gh);
        let broad = if modest.iter().any(|&m| m) {
            spread(field, &modest, gw, gh)
        } else {
            vec![[0.0; 3]; gw * gh]
        };
        let differs = |a: &[f32; 3], b: &[f32; 3]| (0..3).any(|c| (a[c] - b[c]).abs() >= 2.0);
        let local = whole.iter().zip(&broad).any(|(a, b)| differs(a, b));
        if !local && broad.iter().all(|v| v.iter().all(|c| c.abs() < 0.5)) {
            return Ok(None);
        }
        let cell = CELL as f32;
        let sample = |field: &[[f32; 3]], x: u32, y: u32| {
            let fy = ((y as f32 + 0.5) / cell - 0.5).clamp(0.0, (gh - 1) as f32);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(gh - 1);
            let fx = ((x as f32 + 0.5) / cell - 0.5).clamp(0.0, (gw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(gw - 1);
            [0, 1, 2].map(|c| {
                let top = field[y0 * gw + x0][c] * (1.0 - tx) + field[y0 * gw + x1][c] * tx;
                let bottom = field[y1 * gw + x0][c] * (1.0 - tx) + field[y1 * gw + x1][c] * tx;
                top * (1.0 - ty) + bottom * ty
            })
        };
        let shifted = |field: &[[f32; 3]]| -> Result<RgbImage> {
            let mut out = frame.clone();
            for y in 0..h {
                if y % 64 == 0 {
                    check_cancel(cancel)?;
                }
                for x in 0..w {
                    let shift = sample(field, x, y);
                    let pixel = out.get_pixel_mut(x, y);
                    for c in 0..3 {
                        pixel[c] = (f32::from(pixel[c]) + shift[c]).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            Ok(out)
        };
        if !local {
            return Ok(Some(shifted(&broad)?));
        }
        // With the whole difference added the frame agrees with the canvas
        // as well as it can, so that is what the seam is chosen on: light
        // that changed is no reason to cut elsewhere.
        let mut out = shifted(&whole)?;
        let (vertical, positive) = self.direction(rect);
        let path = self.seam(&out, mask, offset, overlap, vertical, cancel)?;
        let release = (if vertical { h } else { w } / 8).max(CELL) as f32;
        for y in 0..h {
            if y % 64 == 0 {
                check_cancel(cancel)?;
            }
            for x in 0..w {
                let (cx, cy) = (offset.0 + x, offset.1 + y);
                let (cross, along) = if vertical {
                    (cx.clamp(overlap.x0, overlap.x1 - 1) - overlap.x0, cy)
                } else {
                    (cy.clamp(overlap.y0, overlap.y1 - 1) - overlap.y0, cx)
                };
                let boundary = path[cross as usize];
                let behind = if positive {
                    along as f32 - boundary as f32
                } else {
                    boundary as f32 - along as f32
                };
                let t = (1.0 - behind / release).clamp(0.0, 1.0);
                if t >= 1.0 {
                    continue;
                }
                let kept = t * t * (3.0 - 2.0 * t);
                let (far, near) = (sample(&broad, x, y), sample(&whole, x, y));
                let source = frame.get_pixel(x, y);
                let pixel = out.get_pixel_mut(x, y);
                for c in 0..3 {
                    let shift = far[c] + (near[c] - far[c]) * kept;
                    pixel[c] = (f32::from(source[c]) + shift).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.planned = Some((rect, path));
        Ok(Some(out))
    }

    fn seam(
        &self,
        frame: &RgbImage,
        mask: Option<&GrayImage>,
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
                        let (fx, fy) = (x - offset.0, y - offset.1);
                        if a[3] == 0 || mask.is_some_and(|m| m.get_pixel(fx, fy)[0] == 0) {
                            continue;
                        }
                        let b = frame.get_pixel(fx, fy);
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

/// A cell field made whole and smooth: each unmeasured cell takes its value
/// from the measured ones nearest it, fading out with distance.
fn spread(mut field: Vec<[f32; 3]>, known: &[bool], gw: usize, gh: usize) -> Vec<[f32; 3]> {
    // Distance in cells from the measured area, and each unmeasured cell's
    // value from its already-filled neighbours.
    let mut distance = vec![u32::MAX; gw * gh];
    let mut queue: std::collections::VecDeque<usize> = (0..gw * gh)
        .filter(|&i| known[i])
        .inspect(|&i| distance[i] = 0)
        .collect();
    let neighbours = |i: usize| {
        let (x, y) = (i % gw, i / gw);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < gw).then_some(i + 1),
            (y > 0).then(|| i - gw),
            (y + 1 < gh).then_some(i + gw),
        ]
        .into_iter()
        .flatten()
    };
    while let Some(i) = queue.pop_front() {
        for n in neighbours(i) {
            if distance[n] == u32::MAX {
                distance[n] = distance[i] + 1;
                let (mut sum, mut count) = ([0.0f32; 3], 0.0f32);
                for m in neighbours(n).filter(|&m| distance[m] <= distance[i]) {
                    for c in 0..3 {
                        sum[c] += field[m][c];
                    }
                    count += 1.0;
                }
                field[n] = sum.map(|v| v / count);
                queue.push_back(n);
            }
        }
    }
    // Let go of the correction over a third of the frame beyond the
    // measured area, so a new strip joins smoothly but lighting is not
    // dragged across the whole stitch.
    let reach = (gw.max(gh) / 3).max(2) as f32;
    for (value, &d) in field.iter_mut().zip(&distance) {
        let t = (1.0 - d as f32 / reach).clamp(0.0, 1.0);
        let falloff = t * t * (3.0 - 2.0 * t);
        *value = value.map(|v| v * falloff);
    }
    // Smooth cell-to-cell noise and the fill pattern.
    for _ in 0..3 {
        let previous = field.clone();
        for i in 0..gw * gh {
            let (mut sum, mut count) = (previous[i], 1.0f32);
            for n in neighbours(i) {
                for c in 0..3 {
                    sum[c] += previous[n][c];
                }
                count += 1.0;
            }
            field[i] = sum.map(|v| v / count);
        }
    }
    field
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
