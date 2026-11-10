//! Translation-only matching. Coordinates describe the camera origin in the artwork.
//! A trimmed grid residual lets local animation disagree without steering the match.
use image::{
    GrayImage, RgbImage,
    imageops::{self, FilterType},
};

/// Advanced matching thresholds are persisted with stitch settings but kept out
/// of the normal UI. Values refer to 8-bit grayscale analysis pixels.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Thresholds {
    pub max_error: f32,
    pub min_support: f32,
    pub tile_error: f32,
    pub min_detail: f32,
    pub distance_penalty: f32,
}
impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_error: 18.0,
            min_support: 0.65,
            tile_error: 24.0,
            min_detail: 1.8,
            distance_penalty: 0.2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub dx: i32,
    pub dy: i32,
    pub error: f32,
    pub support: f32,
}

impl Motion {
    pub fn reliable(self, thresholds: &Thresholds) -> bool {
        self.error < thresholds.max_error && self.support >= thresholds.min_support
    }
    pub fn moving(self) -> bool {
        self.dx.abs() + self.dy.abs() > 0
    }
}

/// Score only common pixels, spread over sixteen tiles. Discard the four worst
/// tiles, but separately require broad agreement so a single moving object cannot win.
fn residual(a: &GrayImage, b: &GrayImage, dx: i32, dy: i32, thresholds: &Thresholds) -> (f32, f32) {
    let (w, h) = a.dimensions();
    if b.dimensions() != (w, h) || dx.abs() > w as i32 * 2 / 3 || dy.abs() > h as i32 * 2 / 3 {
        return (255.0, 0.0);
    }
    let x0 = 0.max(-dx);
    let y0 = 0.max(-dy);
    let x1 = (w as i32).min(w as i32 - dx);
    let y1 = (h as i32).min(h as i32 - dy);
    if x1 - x0 < 8 || y1 - y0 < 8 {
        return (255.0, 0.0);
    }
    // Coordinates depend on overlap, not on the tile. Compute the 24 sample
    // columns/rows once per candidate instead of dividing for every pixel pair.
    let xs: [i32; 24] = std::array::from_fn(|i| x0 + (x1 - x0 - 1) * i as i32 / 24);
    let ys: [i32; 24] = std::array::from_fn(|i| y0 + (y1 - y0 - 1) * i as i32 / 24);
    let aa = a.as_raw();
    let bb = b.as_raw();
    let stride = w as usize;
    let mut tiles = [0.0f32; 16];
    for ty in 0..4 {
        for tx in 0..4 {
            let mut sum = 0.0;
            for &y in &ys[ty * 6..ty * 6 + 6] {
                let ar = (y + dy) as usize * stride;
                let br = y as usize * stride;
                for &x in &xs[tx * 6..tx * 6 + 6] {
                    sum += (aa[ar + (x + dx) as usize] as f32 - bb[br + x as usize] as f32).abs();
                }
            }
            tiles[ty * 4 + tx] = sum / 36.0;
        }
    }
    let support = tiles.iter().filter(|&&v| v < thresholds.tile_error).count() as f32 / 16.0;
    tiles.sort_by(f32::total_cmp);
    (tiles[..12].iter().sum::<f32>() / 12.0, support)
}

pub fn detail(a: &GrayImage) -> f32 {
    let mut total = 0u64;
    let mut n = 0;
    for y in (1..a.height()).step_by(3) {
        for x in (1..a.width()).step_by(3) {
            let p = a.get_pixel(x, y)[0];
            total += p.abs_diff(a.get_pixel(x - 1, y)[0]) as u64;
            total += p.abs_diff(a.get_pixel(x, y - 1)[0]) as u64;
            n += 2;
        }
    }
    total as f32 / n.max(1) as f32
}

pub fn refine(
    a: &GrayImage,
    b: &GrayImage,
    guess: (i32, i32),
    radius: i32,
    thresholds: &Thresholds,
) -> Motion {
    let mut best = Motion {
        dx: guess.0,
        dy: guess.1,
        error: 255.0,
        support: 0.0,
    };
    for dy in guess.1 - radius..=guess.1 + radius {
        for dx in guess.0 - radius..=guess.0 + radius {
            let (error, support) = residual(a, b, dx, dy, thresholds);
            if error < best.error {
                best = Motion {
                    dx,
                    dy,
                    error,
                    support,
                };
            }
        }
    }
    best
}

pub fn estimate(a: &GrayImage, b: &GrayImage, thresholds: &Thresholds) -> Option<Motion> {
    if a.dimensions() != b.dimensions()
        || a.width().min(a.height()) < 24
        || detail(a).min(detail(b)) < thresholds.min_detail
    {
        return None;
    }
    // Exhaustive coarse search, then keep several hypotheses through the pyramid.
    // This is less fragile than a single hill climb on line art or repeated textures.
    let mut pyramid = vec![(a.clone(), b.clone())];
    while pyramid.last()?.0.width().max(pyramid.last()?.0.height()) > 64 {
        let (a, b) = pyramid.last()?;
        let (w, h) = (a.width() / 2, a.height() / 2);
        if w < 24 || h < 24 {
            break;
        }
        pyramid.push((
            imageops::resize(a, w, h, FilterType::Triangle),
            imageops::resize(b, w, h, FilterType::Triangle),
        ));
    }
    let (a, b) = pyramid.last()?;
    let mut candidates = Vec::new();
    for dy in -(a.height() as i32 / 3)..=a.height() as i32 / 3 {
        for dx in -(a.width() as i32 / 3)..=a.width() as i32 / 3 {
            let (error, support) = residual(a, b, dx, dy, thresholds);
            candidates.push(Motion {
                dx,
                dy,
                error,
                support,
            });
        }
    }
    // Near-equal matches on repeated line patterns should favour the smaller
    // translation. A distant periodic alias must improve the residual enough to
    // justify jumping there; otherwise slow subpixel pans can drift sideways.
    let coarse_scale = pyramid[0].0.width() as f32 / a.width() as f32;
    let cost = |m: &Motion, scale: f32| {
        m.error + thresholds.distance_penalty * scale * (m.dx.abs() + m.dy.abs()) as f32
    };
    candidates.sort_by(|a, b| cost(a, coarse_scale).total_cmp(&cost(b, coarse_scale)));
    let mut best: Vec<Motion> = Vec::new();
    for c in candidates {
        if best
            .iter()
            .all(|b| (b.dx - c.dx).abs() + (b.dy - c.dy).abs() > 2)
        {
            best.push(c);
        }
        if best.len() == 5 {
            break;
        }
    }
    for level in (0..pyramid.len() - 1).rev() {
        let (a, b) = &pyramid[level];
        let (pa, _) = &pyramid[level + 1];
        let sx = a.width() as f32 / pa.width() as f32;
        let sy = a.height() as f32 / pa.height() as f32;
        best = best
            .iter()
            .map(|m| {
                refine(
                    a,
                    b,
                    (
                        (m.dx as f32 * sx).round() as i32,
                        (m.dy as f32 * sy).round() as i32,
                    ),
                    2,
                    thresholds,
                )
            })
            .collect();
    }
    best.into_iter()
        .min_by(|a, b| cost(a, 1.0).total_cmp(&cost(b, 1.0)))
}

/// The image crate's exact integer Rec.709 conversion, over contiguous RGB
/// bytes. Avoid generic per-pixel coordinate and colour-trait work on native frames.
pub fn grayscale(rgb: &RgbImage) -> GrayImage {
    let bytes = rgb
        .as_raw()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((2126 * p[0] as u32 + 7152 * p[1] as u32 + 722 * p[2] as u32) / 10000) as u8)
        .collect();
    GrayImage::from_raw(rgb.width(), rgb.height(), bytes).unwrap()
}

/// A bounded pyramid cached with each of the three native reference frames.
/// Construct each reduced level directly from the original, matching the previous
/// filter exactly, and reuse it across reference checks.
pub struct AlignmentFrame {
    levels: Vec<GrayImage>,
}
impl AlignmentFrame {
    pub fn new(gray: GrayImage) -> Self {
        let factor = (gray.width().max(gray.height()) / 480).max(1);
        let mut levels = vec![gray];
        let mut step = 2;
        while step <= factor {
            let original = &levels[0];
            levels.push(imageops::resize(
                original,
                original.width() / step,
                original.height() / step,
                FilterType::Triangle,
            ));
            step *= 2;
        }
        Self { levels }
    }
}

/// Search cached coarse images, then refine against the original native pixels.
pub fn align(
    a: &AlignmentFrame,
    b: &AlignmentFrame,
    guess: (i32, i32),
    radius: i32,
    thresholds: &Thresholds,
) -> Motion {
    debug_assert_eq!(a.levels.len(), b.levels.len());
    let top = a.levels.len() - 1;
    let mut guess = guess;
    for level in (0..=top).rev() {
        let step = 1i32 << level;
        let center = if level == top {
            (
                (guess.0 as f32 / step as f32).round() as i32,
                (guess.1 as f32 / step as f32).round() as i32,
            )
        } else {
            (guess.0 * 2, guess.1 * 2)
        };
        let m = refine(
            &a.levels[level],
            &b.levels[level],
            center,
            if level == top {
                (radius / step).max(3)
            } else {
                3
            },
            thresholds,
        );
        if level == 0 {
            return m;
        }
        guess = (m.dx, m.dy);
    }
    unreachable!()
}

impl AlignmentFrame {
    /// Flat backgrounds can agree while a character changes pose. Check actual
    /// line/texture geometry as well as the trimmed photometric residual.
    pub fn edge_agreement(&self, other: &Self, m: Motion) -> f32 {
        let a = &self.levels[0];
        let b = &other.levels[0];
        let (w, h) = (a.width() as i32, a.height() as i32);
        let (x0, y0) = (2.max(2 - m.dx), 2.max(2 - m.dy));
        let (x1, y1) = ((w - 2).min(w - m.dx - 2), (h - 2).min(h - m.dy - 2));
        let aa = a.as_raw();
        let bb = b.as_raw();
        let stride = w as usize;
        let mut total = 0usize;
        let mut matched = 0usize;
        for y in (y0..y1).step_by(3) {
            for x in (x0..x1).step_by(3) {
                let j = y as usize * stride + x as usize;
                let bx = bb[j + 1] as i32 - bb[j - 1] as i32;
                let by = bb[j + stride] as i32 - bb[j - stride] as i32;
                let strength = bx.abs() + by.abs();
                if strength < 16 {
                    continue;
                }
                total += 1;
                let i = (y + m.dy) as usize * stride + (x + m.dx) as usize;
                let agrees = (-1isize..=1).any(|oy| {
                    (-1isize..=1).any(|ox| {
                        let k = (i as isize + oy * stride as isize + ox) as usize;
                        let ax = aa[k + 1] as i32 - aa[k - 1] as i32;
                        let ay = aa[k + stride] as i32 - aa[k - stride] as i32;
                        aa[k].abs_diff(bb[j]) <= 24
                            && ax * bx + ay * by > 0
                            && ax.abs() + ay.abs() >= strength / 3
                    })
                });
                if agrees {
                    matched += 1;
                }
            }
        }
        if total < 16 {
            1.0
        } else {
            matched as f32 / total as f32
        }
    }
}
