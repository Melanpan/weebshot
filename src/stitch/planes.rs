//! Two-plane reconstruction for parallax pans: a foreground layer, usually a
//! character cel, that moves at a different speed from the background.
//!
//! Both planes are tracked with a sub-pixel block motion field. Each selected
//! frame is segmented by which plane's motion explains its pixels, the planes
//! are stitched separately from their own pixels, and the foreground mosaic is
//! placed over the background as it was at the middle frame.
use super::{
    FrameChoice, MAX_SELECTED, Method, Options, Position, Recipe, bounds, check_cancel, compose,
    decode, motion, psd,
};
use anyhow::{Result, bail};
use image::{
    GrayImage, Luma, RgbImage, RgbaImage,
    imageops::{self, FilterType},
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) const ERROR: &str = "Could not find two independently moving layers. Use Automatic for artwork that moves as one plane, or mark In/Out around one pan.";

/// Long edge of the frames used for tracking and segmentation.
const WORK_EDGE: u32 = 960;
/// Upper bound on decoded tracking frames, which are kept in memory.
const MAX_WORK_FRAMES: f64 = 120.0;
const BLOCK: i32 = 32;

type Vector = (f32, f32);

fn distance(a: Vector, b: Vector) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn add(a: Vector, b: Vector) -> Vector {
    (a.0 + b.0, a.1 + b.1)
}

fn sub(a: Vector, b: Vector) -> Vector {
    (a.0 - b.0, a.1 - b.1)
}

/// Block centres in `b` and their sub-pixel shifts, `a(x + d) ≈ b(x)`,
/// searched around each guess. Blocks crossed only by parallel lines cannot
/// fix their shift along those lines and are skipped.
fn block_field(
    a: &GrayImage,
    b: &GrayImage,
    guesses: &[Vector],
    radius: i32,
) -> Vec<(Vector, Vector)> {
    let (w, h) = (a.width() as i32, a.height() as i32);
    let aa = a.as_raw();
    let bb = b.as_raw();
    let stride = w as usize;
    let inside = |bx: i32, by: i32, dx: i32, dy: i32| {
        bx + dx >= 0 && by + dy >= 0 && bx + dx + BLOCK <= w && by + dy + BLOCK <= h
    };
    let cost = |bx: i32, by: i32, dx: i32, dy: i32| -> f32 {
        let mut total = 0u32;
        for y in (by..by + BLOCK).step_by(2) {
            let ar = (y + dy) as usize * stride;
            let br = y as usize * stride;
            for x in (bx..bx + BLOCK).step_by(2) {
                total += u32::from(aa[ar + (x + dx) as usize].abs_diff(bb[br + x as usize]));
            }
        }
        total as f32
    };
    let mut out = Vec::new();
    for by in (0..=h - BLOCK).step_by(BLOCK as usize) {
        for bx in (0..=w - BLOCK).step_by(BLOCK as usize) {
            let mut texture = 0u32;
            for y in (by..by + BLOCK - 1).step_by(2) {
                let row = y as usize * stride;
                for x in (bx..bx + BLOCK - 1).step_by(2) {
                    let p = bb[row + x as usize];
                    texture += u32::from(p.abs_diff(bb[row + x as usize + 1]))
                        + u32::from(p.abs_diff(bb[row + stride + x as usize]));
                }
            }
            if texture < (BLOCK * BLOCK / 4) as u32 * 3 {
                continue;
            }
            let mut best = (f32::INFINITY, 0, 0);
            for &(gx, gy) in guesses {
                let (cx, cy) = (gx.round() as i32, gy.round() as i32);
                for dy in (-radius..=radius).step_by(2) {
                    for dx in (-radius..=radius).step_by(2) {
                        let (sx, sy) = (cx + dx, cy + dy);
                        if inside(bx, by, sx, sy) {
                            let c = cost(bx, by, sx, sy);
                            if c < best.0 {
                                best = (c, sx, sy);
                            }
                        }
                    }
                }
            }
            if !best.0.is_finite() {
                continue;
            }
            let (_, cx, cy) = best;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if inside(bx, by, cx + dx, cy + dy) {
                        let c = cost(bx, by, cx + dx, cy + dy);
                        if c < best.0 {
                            best = (c, cx + dx, cy + dy);
                        }
                    }
                }
            }
            let (centre, sx, sy) = best;
            if ![(-1, 0), (1, 0), (0, -1), (0, 1)]
                .iter()
                .all(|&(dx, dy)| inside(bx, by, sx + dx, sy + dy))
            {
                continue;
            }
            let (left, right) = (cost(bx, by, sx - 1, sy), cost(bx, by, sx + 1, sy));
            let (up, down) = (cost(bx, by, sx, sy - 1), cost(bx, by, sx, sy + 1));
            let (curve_x, curve_y) = (left + right - 2.0 * centre, up + down - 2.0 * centre);
            if curve_x.min(curve_y) < 0.25 * curve_x.max(curve_y) || curve_x.min(curve_y) < 64.0 {
                continue;
            }
            out.push((
                ((bx + BLOCK / 2) as f32, (by + BLOCK / 2) as f32),
                (
                    sx as f32 + 0.5 * (left - right) / curve_x,
                    sy as f32 + 0.5 * (up - down) / curve_y,
                ),
            ));
        }
    }
    out
}

/// Up to two dominant shifts and how many blocks support each. The second
/// counts only when it is well supported and clearly separate.
fn modes(vectors: &[Vector]) -> Vec<(Vector, usize)> {
    let mut rest = vectors.to_vec();
    let mut out = Vec::new();
    while out.len() < 2 && !rest.is_empty() {
        let support = |c: Vector| rest.iter().filter(|&&v| distance(v, c) <= 1.0).count();
        let seed = *rest.iter().max_by_key(|&&c| support(c)).unwrap();
        let members: Vec<_> = rest
            .iter()
            .copied()
            .filter(|&v| distance(v, seed) <= 1.0)
            .collect();
        let n = members.len() as f32;
        let mean = members
            .iter()
            .fold((0.0, 0.0), |s, v| (s.0 + v.0 / n, s.1 + v.1 / n));
        if let Some(&(first, count)) = out.first()
            && (members.len() < 6.max(vectors.len() * 3 / 25)
                || distance(mean, first) < 1.5
                || members.len() * 6 < count)
        {
            break;
        }
        out.push((mean, members.len()));
        rest.retain(|&v| distance(v, mean) > 1.5);
    }
    out
}

/// How many of `points` lie within 40 pixels of any of `previous`.
fn overlap(points: &[Vector], previous: &[Vector]) -> usize {
    points
        .iter()
        .filter(|&&p| previous.iter().any(|&q| distance(p, q) <= 40.0))
        .count()
}

/// Track both planes between consecutive tracking frames and accumulate
/// their positions.
///
/// Plane identity follows image regions: each motion's blocks are mapped back
/// into the previous frame and matched with the plane whose blocks they
/// overlap. Velocity matching fails while the camera accelerates, and speed
/// order fails when a character moves within the scene during the pan.
///
/// Often only one motion is visible in a pair. Then the hidden plane keeps
/// the nearest observed difference between the two motions, so a mistake
/// costs that small difference rather than a stale velocity.
fn track(
    work: &[GrayImage],
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<[Vec<Vector>; 2]> {
    let small: Vec<_> = work
        .iter()
        .map(|f| {
            let scale = 320.0 / f.width().max(f.height()) as f32;
            imageops::resize(
                f,
                (f.width() as f32 * scale).round() as u32,
                (f.height() as f32 * scale).round() as u32,
                FilterType::Triangle,
            )
        })
        .collect();
    let ratio = work[0].width() as f32 / small[0].width() as f32;
    let pairs = work.len() - 1;
    // Per pair: the velocity of each plane, if visible.
    let mut seen: Vec<[Option<Vector>; 2]> = Vec::with_capacity(pairs);
    // Block centres of each plane in the current frame's coordinates.
    let mut regions: [Vec<Vector>; 2] = [Vec::new(), Vec::new()];
    let mut previous_modes: Vec<Vector> = Vec::new();
    let mut doubles = 0;
    for i in 0..pairs {
        check_cancel(cancel)?;
        report(
            0.2 + 0.25 * i as f32 / work.len() as f32,
            "Tracking foreground and background",
        );
        let mut guesses = previous_modes.clone();
        if let Some(m) = motion::estimate(&small[i], &small[i + 1], &options.thresholds) {
            guesses.push((m.dx as f32 * ratio, m.dy as f32 * ratio));
        }
        if guesses.is_empty() {
            guesses.push((0.0, 0.0));
        }
        let field = block_field(&work[i], &work[i + 1], &guesses, 8);
        let shifts: Vec<Vector> = field.iter().map(|(_, d)| *d).collect();
        let found = modes(&shifts);
        previous_modes = found.iter().map(|(v, _)| *v).collect();
        // Members of each motion: their centres in this pair's first frame,
        // to compare with the planes' regions, and in its second frame.
        let members: Vec<(Vector, Vec<Vector>, Vec<Vector>)> = found
            .iter()
            .map(|&(v, _)| {
                let chosen: Vec<_> = field
                    .iter()
                    .filter(|(_, d)| distance(*d, v) <= 1.0)
                    .collect();
                (
                    v,
                    chosen.iter().map(|(p, d)| add(*p, *d)).collect(),
                    chosen.iter().map(|(p, _)| *p).collect(),
                )
            })
            .collect();
        let score = |m: usize, plane: usize| overlap(&members[m].1, &regions[plane]);
        let mut visible = [None, None];
        match members.len() {
            0 => {}
            1 => {
                let plane = usize::from(score(0, 1) > score(0, 0));
                visible[plane] = Some(0);
            }
            _ => {
                doubles += 1;
                let swap = score(0, 1) + score(1, 0) > score(0, 0) + score(1, 1);
                visible = if swap {
                    [Some(1), Some(0)]
                } else {
                    [Some(0), Some(1)]
                };
            }
        }
        let mut velocities = [None, None];
        for plane in 0..2 {
            if let Some(m) = visible[plane] {
                velocities[plane] = Some(members[m].0);
                regions[plane] = members[m].2.clone();
            }
        }
        // One motion explaining nearly every textured block means both
        // planes moved alike (for example while the camera holds), not that
        // the other plane was hidden.
        if let [(only, count)] = found.as_slice()
            && *count * 100 >= field.len() * 85
        {
            velocities = [Some(*only); 2];
        }
        crate::log::log(
            crate::log::Level::Debug,
            "stitch",
            format!("planes pair {i}: modes {found:?}, planes {velocities:?}"),
        );
        seen.push(velocities);
    }
    // Two distinct motions must be seen often enough to be a layer, not noise.
    if doubles * 10 < pairs * 3 {
        bail!(ERROR);
    }
    let both: Vec<usize> = (0..pairs)
        .filter(|&i| seen[i][0].is_some() && seen[i][1].is_some())
        .collect();
    let mut velocity: [Vec<Vector>; 2] = [Vec::with_capacity(pairs), Vec::with_capacity(pairs)];
    for i in 0..pairs {
        let nearest = both.iter().min_by_key(|&&k| k.abs_diff(i)).unwrap();
        let [n0, n1] = seen[*nearest].map(Option::unwrap);
        let pair = match seen[i] {
            [Some(a), Some(b)] => [a, b],
            [Some(a), None] => [a, add(a, sub(n1, n0))],
            [None, Some(b)] => [add(b, sub(n0, n1)), b],
            [None, None] => [n0, n1],
        };
        velocity[0].push(pair[0]);
        velocity[1].push(pair[1]);
    }
    let accumulate = |v: &[Vector]| {
        let mut positions = vec![(0.0, 0.0)];
        for step in v {
            positions.push(add(*positions.last().unwrap(), *step));
        }
        positions
    };
    Ok([accumulate(&velocity[0]), accumulate(&velocity[1])])
}

/// Mean absolute difference between `frame` and each neighbour warped by its
/// plane displacement, `frame(p) ≈ neighbour(p - d)`, box-filtered.
fn residual(frame: &GrayImage, neighbours: &[(&GrayImage, Vector)]) -> Vec<f32> {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let mut sum = vec![0.0f32; w * h];
    let mut count = vec![0.0f32; w * h];
    for &(other, d) in neighbours {
        let raw = other.as_raw();
        for y in 0..h {
            let sy = y as f32 - d.1;
            if sy < 0.0 || sy > (h - 2) as f32 {
                continue;
            }
            let (iy, fy) = (sy as usize, sy.fract());
            for x in 0..w {
                let sx = x as f32 - d.0;
                if sx < 0.0 || sx > (w - 2) as f32 {
                    continue;
                }
                let (ix, fx) = (sx as usize, sx.fract());
                let i = iy * w + ix;
                let top = f32::from(raw[i]) * (1.0 - fx) + f32::from(raw[i + 1]) * fx;
                let bottom = f32::from(raw[i + w]) * (1.0 - fx) + f32::from(raw[i + w + 1]) * fx;
                let value = top * (1.0 - fy) + bottom * fy;
                sum[y * w + x] += (f32::from(frame.as_raw()[y * w + x]) - value).abs();
                count[y * w + x] += 1.0;
            }
        }
    }
    let sum = box_filter(&sum, w, h, 3);
    let count = box_filter(&count, w, h, 3);
    sum.iter()
        .zip(&count)
        .map(|(s, c)| if *c > 0.5 { s / c } else { f32::NAN })
        .collect()
}

/// Square box sum of radius `r`, with clamped edges.
fn box_filter(values: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |input: &[f32], horizontal: bool| {
        let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
        let mut out = vec![0.0; w * h];
        for o in 0..outer {
            let at = |i: usize| if horizontal { o * w + i } else { i * w + o };
            let mut acc = 0.0;
            for i in 0..=r.min(inner - 1) {
                acc += input[at(i)];
            }
            for i in 0..inner {
                out[at(i)] = acc;
                if i + r + 1 < inner {
                    acc += input[at(i + r + 1)];
                }
                if i >= r {
                    acc -= input[at(i - r)];
                }
            }
        }
        out
    };
    pass(&pass(values, true), false)
}

/// Grow (`grow`) or shrink a mask by a square of radius `r`.
fn morph(mask: &[bool], w: usize, h: usize, r: usize, grow: bool) -> Vec<bool> {
    let counts = box_filter(
        &mask
            .iter()
            .map(|&m| if m { 1.0 } else { 0.0 })
            .collect::<Vec<_>>(),
        w,
        h,
        r,
    );
    let span = |i: usize, n: usize| ((i + r).min(n - 1) - i.saturating_sub(r) + 1) as f32;
    counts
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if grow {
                c > 0.5
            } else {
                // Windows are clipped at the frame edge; compare with the
                // pixels actually inside, so the edge erodes like the interior
                // instead of sealing enclosed-looking regions against it.
                let (x, y) = (i % w, i / w);
                c > span(x, w) * span(y, h) - 0.5
            }
        })
        .collect()
}

/// Turn per-pixel plane votes into a foreground region.
///
/// Votes sit mostly on line art, so the foreground is everything confident
/// background cannot reach: the background grows from its own votes through
/// pixels that are not sealed foreground outline. Seeding from background
/// votes, not from the frame edge, keeps a character that is cut off by the
/// frame edge whole. Small islands are dropped and the result is dilated so
/// outlines stay with the foreground.
fn clean_mask(fg: &[bool], bg: &[bool], w: usize, h: usize) -> Vec<bool> {
    let flood = |seeds: &[bool], passable: &dyn Fn(usize) -> bool| {
        let mut reached = seeds.to_vec();
        let mut stack: Vec<usize> = (0..w * h).filter(|&i| seeds[i]).collect();
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for n in [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then_some(i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then_some(i + w),
            ]
            .into_iter()
            .flatten()
            {
                if !reached[n] && passable(n) {
                    reached[n] = true;
                    stack.push(n);
                }
            }
        }
        reached
    };
    // Close gaps in the outline generously; only used as a barrier.
    let sealed = morph(&morph(fg, w, h, 8, true), w, h, 8, false);
    // Isolated background votes inside a character are noise; seed only from
    // background votes outside the sealed outline.
    let seeds: Vec<bool> = (0..w * h).map(|i| bg[i] && !sealed[i]).collect();
    let background = flood(&seeds, &|i| !sealed[i]);
    let region: Vec<bool> = (0..w * h).map(|i| !background[i] || fg[i]).collect();
    // Remove islands smaller than 0.2% of the frame.
    let minimum = w * h / 500;
    let mut kept = vec![false; w * h];
    let mut seen = vec![false; w * h];
    for seed in 0..w * h {
        if !region[seed] || seen[seed] {
            continue;
        }
        let mut component = vec![seed];
        seen[seed] = true;
        let mut k = 0;
        while k < component.len() {
            let i = component[k];
            k += 1;
            let (x, y) = (i % w, i / w);
            for n in [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then_some(i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then_some(i + w),
            ]
            .into_iter()
            .flatten()
            {
                if region[n] && !seen[n] {
                    seen[n] = true;
                    component.push(n);
                }
            }
        }
        if component.len() >= minimum {
            for i in component {
                kept[i] = true;
            }
        }
    }
    morph(&kept, w, h, 2, true)
}

/// Per-pixel votes for each plane in frame `k`, compared with its selected
/// neighbours. Flat fills agree with both planes and stay undecided.
fn votes(
    work: &[GrayImage],
    selected: &[usize],
    planes: &[Vec<Vector>; 2],
    k: usize,
) -> [Vec<bool>; 2] {
    let frame = &work[selected[k]];
    let neighbours: Vec<usize> = [k.checked_sub(1), (k + 1 < selected.len()).then_some(k + 1)]
        .into_iter()
        .flatten()
        .collect();
    let [ra, rb] = [0, 1].map(|p| {
        let list: Vec<_> = neighbours
            .iter()
            .map(|&n| (&work[selected[n]], sub(planes[p][n], planes[p][k])))
            .collect();
        residual(frame, &list)
    });
    let decide = |mine: f32, other: f32| {
        mine.is_finite() && other.is_finite() && mine + 2.0 + 0.15 * mine.min(other) < other
    };
    [
        ra.iter().zip(&rb).map(|(&a, &b)| decide(a, b)).collect(),
        ra.iter().zip(&rb).map(|(&a, &b)| decide(b, a)).collect(),
    ]
}

/// The plane that owns more of the frame borders is the background.
fn background_plane(votes: &[[Vec<bool>; 2]], w: usize, h: usize) -> usize {
    let band = (w.min(h) / 20).max(2);
    let mut counts = [0usize; 2];
    for frame in votes {
        for (plane, mask) in frame.iter().enumerate() {
            counts[plane] += mask
                .iter()
                .enumerate()
                .filter(|&(i, &m)| {
                    let (x, y) = (i % w, i / w);
                    m && (x < band || y < band || x + band >= w || y + band >= h)
                })
                .count();
        }
    }
    usize::from(counts[1] > counts[0])
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recipe(
    ffmpeg: &Path,
    start: f64,
    end: f64,
    source_fps: f64,
    mut recipe: Recipe,
    options: &Options,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<Recipe> {
    let native = recipe.native;
    let scale = WORK_EDGE as f32 / native.0.max(native.1) as f32;
    let size = (
        ((native.0 as f32 * scale).round() as u32).max(64),
        ((native.1 as f32 * scale).round() as u32).max(64),
    );
    let fps = 8.0f64
        .min(source_fps.max(1.0))
        .min(MAX_WORK_FRAMES / (end - start).max(0.1));
    report(0.15, "Decoding frames for layer tracking");
    let mut work = Vec::new();
    decode::samples(ffmpeg, &recipe.input, start, end, fps, size, cancel, |f| {
        work.push(f);
        Ok(())
    })?;
    if work.len() < 3 {
        bail!("Mark at least half a second of the pan for layered reconstruction.");
    }
    let tracked = track(&work, options, cancel, report)?;
    // Select on the travel of either plane, so both stay overlapped.
    let (w, h) = size;
    let travel = |i: usize, s: usize| {
        tracked
            .iter()
            .map(|p| {
                let d = sub(p[i], p[s]);
                (d.0.abs() / w as f32).max(d.1.abs() / h as f32)
            })
            .fold(0.0, f32::max)
    };
    let mut selected = vec![0];
    for i in 1..work.len() {
        if travel(i, *selected.last().unwrap()) >= options.selection_travel.clamp(0.02, 0.25) {
            selected.push(i);
        }
    }
    let last = work.len() - 1;
    if *selected.last().unwrap() != last && travel(last, *selected.last().unwrap()) > 0.01 {
        selected.push(last);
    }
    if selected.len() < 2 {
        bail!("No useful camera movement detected. Select a range with newly revealed artwork.");
    }
    if selected.len() > MAX_SELECTED {
        bail!("Too many frames needed. Select a shorter range.");
    }
    // Refine both planes directly between consecutive selected frames; the
    // accumulated tracking only predicts where to look.
    let mut planes: [Vec<Vector>; 2] = [vec![(0.0, 0.0)], vec![(0.0, 0.0)]];
    let mut support = Vec::new();
    for pair in selected.windows(2) {
        check_cancel(cancel)?;
        let guess = [0, 1].map(|p| sub(tracked[p][pair[1]], tracked[p][pair[0]]));
        let field: Vec<Vector> = block_field(&work[pair[0]], &work[pair[1]], &guess, 6)
            .into_iter()
            .map(|(_, d)| d)
            .collect();
        let mut explained = 0;
        for p in 0..2 {
            // Only correct the tracked motion slightly: when the planes move
            // alike, a wider window would let one plane adopt the other.
            let mine: Vec<Vector> = field
                .iter()
                .copied()
                .filter(|&v| distance(v, guess[p]) <= 2.0)
                .collect();
            let refined =
                modes(&mine)
                    .first()
                    .filter(|(_, n)| *n >= 6)
                    .map_or(guess[p], |(v, n)| {
                        explained += n;
                        *v
                    });
            let next = add(*planes[p].last().unwrap(), refined);
            planes[p].push(next);
        }
        support.push(explained as f32 / field.len().max(1) as f32);
    }
    report(0.5, "Separating foreground from background");
    let mut all_votes = Vec::new();
    for k in 0..selected.len() {
        check_cancel(cancel)?;
        all_votes.push(votes(&work, &selected, &planes, k));
    }
    let background = background_plane(&all_votes, w as usize, h as usize);
    let foreground = 1 - background;
    let masks: Vec<Arc<GrayImage>> = all_votes
        .iter()
        .map(|v| {
            let mask = clean_mask(&v[foreground], &v[background], w as usize, h as usize);
            Arc::new(GrayImage::from_fn(w, h, |x, y| {
                Luma([if mask[(y * w + x) as usize] { 255 } else { 0 }])
            }))
        })
        .collect();
    if let Some(dir) = std::env::var_os("WEEBSHOT_PLANES_DEBUG") {
        let dir = std::path::PathBuf::from(dir);
        for (k, &i) in selected.iter().enumerate() {
            let tinted = RgbImage::from_fn(w, h, |x, y| {
                let g = work[i].get_pixel(x, y)[0];
                let v = &all_votes[k];
                let j = (y * w + x) as usize;
                if v[foreground][j] {
                    image::Rgb([255, 40, 40])
                } else if v[background][j] {
                    image::Rgb([40, 120, 255])
                } else if masks[k].get_pixel(x, y)[0] > 0 {
                    image::Rgb([g / 2 + 128, g / 2, g / 2])
                } else {
                    image::Rgb([g, g, g])
                }
            });
            let _ = tinted.save(dir.join(format!("mask{k:02}.png")));
        }
        crate::log::log(
            crate::log::Level::Info,
            "stitch",
            format!("planes: background {background}, selected {selected:?}, positions {planes:?}"),
        );
    }
    let to_native = |v: Vector| Position {
        x: (v.0 * native.0 as f32 / w as f32).round() as i32,
        y: (v.1 * native.1 as f32 / h as f32).round() as i32,
    };
    report(0.6, "Preparing layer thumbnails");
    let mut n = 0;
    decode::selected_frames(
        ffmpeg,
        &recipe.input,
        start,
        end,
        fps,
        &selected,
        native,
        cancel,
        |k, rgb| {
            check_cancel(cancel)?;
            recipe.frames.push(FrameChoice {
                time: start + selected[k] as f64 / fps,
                thumbnail: imageops::thumbnail(&rgb, 160, 90),
                sample: selected[k],
                position: to_native(planes[background][k]),
                secondary: Some(to_native(planes[foreground][k])),
                scale: 1.0,
                pose: None,
                layer_mask: Some(masks[k].clone()),
            });
            n += 1;
            Ok(())
        },
    )?;
    if n != selected.len() {
        bail!("Could not decode every selected frame");
    }
    recipe.start = start;
    recipe.end = end;
    recipe.fps = fps;
    recipe.confidence = support.iter().copied().fold(1.0, f32::min);
    recipe.method = Method::LayeredMotion;
    Ok(recipe)
}

fn upsample(mask: &GrayImage, native: (u32, u32), invert: bool) -> GrayImage {
    let mut out = imageops::resize(mask, native.0, native.1, FilterType::Nearest);
    if invert {
        for p in out.pixels_mut() {
            p[0] = 255 - p[0];
        }
    }
    out
}

/// Stitch the background and foreground planes separately, then place the
/// foreground as it was relative to the background at the middle frame.
///
/// The background uses the usual seam compositor. Foreground pixels come from
/// the frame nearest in time to the middle frame that shows them, so an
/// animated face or hand stays whole instead of being cut between poses.
/// The result covers the background's extent; foreground that moved beyond
/// any seen background is cropped rather than left floating.
pub(super) fn compose(
    ffmpeg: &Path,
    recipe: &Recipe,
    selected: &[&FrameChoice],
    retain_layers: bool,
    cancel: &AtomicBool,
    report: &impl Fn(f32, &'static str),
) -> Result<(RgbaImage, Vec<psd::Source>)> {
    let native = recipe.native;
    let back: Vec<Position> = selected.iter().map(|f| f.position).collect();
    let front: Vec<Position> = selected
        .iter()
        .map(|f| f.secondary.unwrap_or(f.position))
        .collect();
    let (back_min, back_size) = bounds(&back, native)?;
    let (front_min, front_size) = bounds(&front, native)?;
    let r = selected.len() / 2;
    // Foreground canvas origin in background canvas coordinates.
    let shift = (
        (back[r].x - back_min.x) - (front[r].x - front_min.x),
        (back[r].y - back_min.y) - (front[r].y - front_min.y),
    );
    if retain_layers {
        let frame = u64::from(native.0) * u64::from(native.1);
        psd::check_budget(
            2 * frame * selected.len() as u64,
            u64::from(back_size.0) * u64::from(back_size.1),
        )?;
    }
    let mut background = compose::Composer::new(back_size);
    if retain_layers {
        background.track_sources();
    }
    // Background spots that every frame labelled foreground are filled from
    // the frame nearest the middle that saw them, so no holes remain.
    let mut fallback = RgbaImage::new(back_size.0, back_size.1);
    let mut fallback_rank = vec![u16::MAX; back_size.0 as usize * back_size.1 as usize];
    let mut foreground = RgbaImage::new(front_size.0, front_size.1);
    let mut rank = vec![u16::MAX; front_size.0 as usize * front_size.1 as usize];
    let mut owners = GrayImage::new(front_size.0, front_size.1);
    let mut layers: [Vec<psd::Source>; 2] = [Vec::new(), Vec::new()];
    let indices: Vec<_> = selected.iter().map(|f| f.sample).collect();
    report(0.0, "Composing background and foreground");
    decode::selected_frames(
        ffmpeg,
        &recipe.input,
        recipe.start,
        recipe.end,
        recipe.fps,
        &indices,
        native,
        cancel,
        |n, rgb: RgbImage| {
            check_cancel(cancel)?;
            report(
                0.8 * n as f32 / selected.len() as f32,
                "Composing background and foreground",
            );
            if rgb.dimensions() != native {
                bail!("Video dimensions change within this scene");
            }
            let Some(mask) = &selected[n].layer_mask else {
                bail!("Layer masks are missing from this stitch");
            };
            let behind = upsample(mask, native, true);
            let p = back[n];
            let offset = ((p.x - back_min.x) as u32, (p.y - back_min.y) as u32);
            background.push(&rgb, Some(&behind), offset, cancel)?;
            let mine = n.abs_diff(r) as u16;
            for (x, y, c) in rgb.enumerate_pixels() {
                let i = ((offset.1 + y) * back_size.0 + offset.0 + x) as usize;
                if mine < fallback_rank[i] {
                    fallback_rank[i] = mine;
                    fallback.put_pixel(
                        offset.0 + x,
                        offset.1 + y,
                        image::Rgba([c[0], c[1], c[2], 255]),
                    );
                }
            }
            if retain_layers {
                layers[0].push(psd::Source::new(
                    &rgb,
                    Some(&behind),
                    0,
                    n,
                    (offset.0 as i32, offset.1 as i32),
                    offset,
                    selected[n].time,
                ));
            }
            drop(behind);
            let front_mask = upsample(mask, native, false);
            let p = front[n];
            let offset = ((p.x - front_min.x) as u32, (p.y - front_min.y) as u32);
            let mine = n.abs_diff(r) as u16;
            for (x, y, m) in front_mask.enumerate_pixels() {
                if m[0] == 0 {
                    continue;
                }
                let (cx, cy) = (offset.0 + x, offset.1 + y);
                let i = (cy * front_size.0 + cx) as usize;
                if mine < rank[i] {
                    rank[i] = mine;
                    let c = rgb.get_pixel(x, y);
                    foreground.put_pixel(cx, cy, image::Rgba([c[0], c[1], c[2], 255]));
                    owners.put_pixel(cx, cy, Luma([n as u8 + 1]));
                }
            }
            if retain_layers {
                layers[1].push(psd::Source::new(
                    &rgb,
                    Some(&front_mask),
                    1,
                    n,
                    (offset.0 as i32 + shift.0, offset.1 as i32 + shift.1),
                    offset,
                    selected[n].time,
                ));
            }
            Ok(())
        },
    )?;
    if retain_layers {
        for layer in &mut layers[0] {
            layer.make_mask(background.owners().unwrap(), cancel)?;
        }
    }
    // Foreground pixels outside the background canvas are cropped, so mask
    // them out of the layers as well.
    let (bw, bh) = (back_size.0 as i32, back_size.1 as i32);
    for (x, y, owner) in owners.enumerate_pixels_mut() {
        let (fx, fy) = (x as i32 + shift.0, y as i32 + shift.1);
        if fx < 0 || fy < 0 || fx >= bw || fy >= bh {
            owner[0] = 0;
        }
    }
    for layer in &mut layers[1] {
        layer.make_mask(&owners, cancel)?;
    }
    let mut output = background.finish();
    for (pixel, spare) in output.pixels_mut().zip(fallback.pixels()) {
        if pixel[3] == 0 && spare[3] == 255 {
            *pixel = *spare;
        }
    }
    for (y, row) in foreground.rows().enumerate() {
        if y % 64 == 0 {
            check_cancel(cancel)?;
        }
        let fy = y as i32 + shift.1;
        if fy < 0 || fy >= bh {
            continue;
        }
        for (x, pixel) in row.enumerate() {
            let fx = x as i32 + shift.0;
            if pixel[3] == 255 && fx >= 0 && fx < bw {
                output.put_pixel(fx as u32, fy as u32, *pixel);
            }
        }
    }
    let [mut back_layers, front_layers] = layers;
    back_layers.extend(front_layers);
    Ok((output, back_layers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_separate_two_motions_and_ignore_outliers() {
        let mut vectors = vec![(0.1, -20.0); 40];
        vectors.extend(vec![(0.0, -12.3); 15]);
        vectors.extend([(5.0, 5.0), (-9.0, 3.0)]);
        let found = modes(&vectors);
        assert_eq!(found.len(), 2);
        assert!(distance(found[0].0, (0.1, -20.0)) < 0.01 && found[0].1 == 40);
        assert!(distance(found[1].0, (0.0, -12.3)) < 0.01 && found[1].1 == 15);
        // A second motion seen in only a few blocks is not a layer.
        let mut single = vec![(0.0, -20.0); 60];
        single.extend(vec![(0.0, -12.0); 3]);
        assert_eq!(modes(&single).len(), 1);
    }

    #[test]
    fn mask_cleanup_fills_enclosed_interiors_and_drops_specks() {
        let (w, h) = (80, 60);
        let mut fg = vec![false; w * h];
        // Textured background around the figure votes for the background.
        let mut bg = vec![false; w * h];
        for y in (0..h).step_by(6) {
            for x in (0..w).step_by(6) {
                if !(8..52).contains(&y) || !(18..52).contains(&x) {
                    bg[y * w + x] = true;
                }
            }
        }
        // An outlined figure with an undecided interior, and a lone speck.
        for y in 10..50 {
            for x in 20..50 {
                if y == 10 || y == 49 || x == 20 || x == 49 {
                    fg[y * w + x] = true;
                }
            }
        }
        fg[5 * w + 70] = true;
        let mask = clean_mask(&fg, &bg, w, h);
        assert!(mask[30 * w + 35], "interior should be foreground");
        assert!(!mask[5 * w + 70], "speck should be removed");
        assert!(!mask[30 * w + 5], "background should stay background");
    }
}
