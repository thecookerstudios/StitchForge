//! Auto-trace: turn a raster logo (PNG/JPG, a photo of a mockup, or the
//! picture inside a PDF) into clean, layered vector art that is ready to
//! stitch, without tracing by hand.
//!
//! The steps:
//! 1. find the background (transparent pixels, the colour around the edges,
//!    and on a mockup photo the garment itself) and crop to the logo;
//! 2. resample to a fixed physical resolution (about 0.1 mm per pixel at
//!    the size it will sew), so cleanup thresholds mean the same at any
//!    image size;
//! 3. pick the logo's colours from its flat areas only, so anti-aliased
//!    edges and JPEG noise don't add in-between colours;
//! 4. give every pixel a colour, then clean up: majority filter for ragged
//!    edges, specks below the smallest stitchable size merged into what
//!    surrounds them;
//! 5. stack the colours the way a digitizer would: big fills at the bottom,
//!    each running under the colours on top of it (no gaps), outlines last;
//! 6. trace each colour's outline with sub-pixel accuracy and fit smooth
//!    Bézier curves with real corners.
//!
//! The result is plain SVG (in mm), so it goes through the same import as
//! any other vector art and stays editable.

use crate::geom::{Pt, VPath, fit_path};

/// How much detail to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detail {
    /// Fewer colours, smoother curves, bigger specks removed.
    Clean,
    #[default]
    Balanced,
    /// Keeps small features and close colours.
    Fine,
}

impl Detail {
    pub const ALL: [Detail; 3] = [Detail::Clean, Detail::Balanced, Detail::Fine];
    pub fn label(&self) -> &'static str {
        match self {
            Detail::Clean => "Clean",
            Detail::Balanced => "Balanced",
            Detail::Fine => "Fine detail",
        }
    }
    /// Colours closer than this (ΔE) become one thread.
    fn merge_de(&self) -> f32 {
        match self {
            Detail::Clean => 20.0,
            Detail::Balanced => 14.0,
            Detail::Fine => 9.0,
        }
    }
    /// Smallest piece kept (mm²).
    fn speck_mm2(&self) -> f64 {
        match self {
            Detail::Clean => 2.5,
            Detail::Balanced => 1.2,
            Detail::Fine => 0.6,
        }
    }
    /// Curve-fit tolerance (mm): well inside what a needle can place.
    fn fit_tol(&self) -> f64 {
        match self {
            Detail::Clean => 0.15,
            Detail::Balanced => 0.10,
            Detail::Fine => 0.06,
        }
    }
    /// Edge smoothing radius (mm).
    fn smooth_mm(&self) -> f64 {
        match self {
            Detail::Clean => 0.2,
            Detail::Balanced => 0.13,
            Detail::Fine => 0.08,
        }
    }
}

/// What kind of picture it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// Decide from the picture.
    #[default]
    Auto,
    /// Flat artwork (logo file, screenshot, PDF picture).
    Logo,
    /// A photo or render of the logo on a garment.
    Mockup,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Auto, Source::Logo, Source::Mockup];
    pub fn label(&self) -> &'static str {
        match self {
            Source::Auto => "Auto",
            Source::Logo => "Logo artwork",
            Source::Mockup => "Mockup / photo",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TraceOptions {
    /// Width the traced logo will sew at (mm).
    pub width_mm: f64,
    /// Most thread colours to use (0 = decide automatically, up to 12).
    pub max_colors: usize,
    pub detail: Detail,
    pub source: Source,
    /// Remove the background (and on mockups, the garment).
    pub remove_background: bool,
    /// Crop in source pixels (x, y, w, h); None = crop to the logo.
    pub crop: Option<[u32; 4]>,
    /// Palette colours the user switched off (treated as background).
    pub drop: Vec<usize>,
    /// Background-coloured palette colours the user wants stitched anyway.
    pub keep: Vec<usize>,
    /// Lower colours run under the ones on top (no gaps between colours).
    pub overlap: bool,
    /// Fine control (None = what the Detail level picks):
    /// edge smoothing radius (mm)
    pub smooth_mm: Option<f64>,
    /// smallest piece kept (mm²)
    pub speck_mm2: Option<f64>,
    /// thinnest line kept (mm)
    pub min_line_mm: Option<f64>,
    /// colours closer than this ΔE become one thread
    pub merge_de: Option<f32>,
    /// how closely curves follow the picture (mm; smaller = closer)
    pub fit_tol_mm: Option<f64>,
    /// snap round shapes to true circles
    pub snap_circles: bool,
    /// snap straight-edged shapes to sharp corners
    pub snap_corners: bool,
    /// Exact thread colours to use instead of finding them (empty = automatic).
    /// Each picture pixel takes the nearest of these.
    pub fixed_palette: Vec<[u8; 3]>,
}

impl TraceOptions {
    pub fn eff_smooth(&self) -> f64 {
        self.smooth_mm.unwrap_or(self.detail.smooth_mm()).clamp(0.0, 1.0)
    }
    pub fn eff_speck(&self) -> f64 {
        self.speck_mm2.unwrap_or(self.detail.speck_mm2()).clamp(0.02, 50.0)
    }
    pub fn eff_min_line(&self) -> f64 {
        self.min_line_mm
            .unwrap_or(match self.detail {
                Detail::Clean => 0.6,
                Detail::Balanced => 0.45,
                Detail::Fine => 0.3,
            })
            .clamp(0.02, 3.0)
    }
    pub fn eff_merge(&self) -> f32 {
        self.merge_de.unwrap_or(self.detail.merge_de()).clamp(2.0, 60.0)
    }
    pub fn eff_fit(&self) -> f64 {
        self.fit_tol_mm.unwrap_or(self.detail.fit_tol()).clamp(0.01, 0.5)
    }
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            width_mm: 101.6,
            max_colors: 0,
            detail: Detail::Balanced,
            source: Source::Auto,
            remove_background: true,
            crop: None,
            drop: Vec::new(),
            keep: Vec::new(),
            overlap: true,
            smooth_mm: None,
            speck_mm2: None,
            min_line_mm: None,
            merge_de: None,
            fit_tol_mm: None,
            snap_circles: true,
            snap_corners: true,
            fixed_palette: Vec::new(),
        }
    }
}

/// A decoded picture, RGBA, row by row.
#[derive(Clone)]
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub px: Vec<[u8; 4]>,
}

impl Raster {
    pub fn decode(bytes: &[u8]) -> Result<Raster, String> {
        let img = image::load_from_memory(bytes).map_err(|e| format!("can't read the picture: {e}"))?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        if w < 8 || h < 8 {
            return Err("the picture is too small to trace".into());
        }
        Ok(Raster {
            w,
            h,
            px: rgba.pixels().map(|p| p.0).collect(),
        })
    }
    fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_fn(self.w, self.h, |x, y| image::Rgba(self.px[(y * self.w + x) as usize]))
    }
    fn crop(&self, c: [u32; 4]) -> Raster {
        let x0 = c[0].min(self.w - 1);
        let y0 = c[1].min(self.h - 1);
        let w = c[2].clamp(1, self.w - x0);
        let h = c[3].clamp(1, self.h - y0);
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                px.push(self.px[(y * self.w + x) as usize]);
            }
        }
        Raster { w, h, px }
    }
    fn resized(&self, w: u32, h: u32) -> Raster {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let filter = if w > self.w {
            image::imageops::FilterType::CatmullRom
        } else {
            image::imageops::FilterType::Triangle
        };
        let r = image::imageops::resize(&self.to_image(), w, h, filter);
        Raster {
            w,
            h,
            px: r.pixels().map(|p| p.0).collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PaletteColor {
    pub rgb: [u8; 3],
    /// Share of the logo's pixels (0–1).
    pub share: f64,
    /// Switched off by the user.
    pub dropped: bool,
    /// Same colour as the removed background: switch it off to let the
    /// fabric show through instead of stitching it.
    pub like_background: bool,
}

#[derive(Clone, Debug)]
pub struct TraceLayer {
    pub color: usize,
    pub rgb: [u8; 3],
    pub name: String,
    pub paths: Vec<VPath>,
    /// A thin ring around other colours: sewn last, on top.
    pub outline: bool,
}

pub struct Trace {
    /// Working grid (pixels) and its scale.
    pub w: usize,
    pub h: usize,
    pub mm_per_px: f64,
    /// Palette index per pixel (NONE = background).
    pub labels: Vec<u16>,
    pub palette: Vec<PaletteColor>,
    /// Bottom to top.
    pub layers: Vec<TraceLayer>,
    /// Crop used, in source pixels (x, y, w, h).
    pub crop: [u32; 4],
    /// Background colour that was removed (if any).
    pub background: Option<[u8; 3]>,
    pub mockup: bool,
    pub notes: Vec<String>,
}

const NONE: u16 = u16::MAX;

// ---------------------------------------------------------------- colour

fn lab(c: [u8; 3]) -> [f32; 3] {
    let l = crate::thread::lab_of(c);
    [l[0] as f32, l[1] as f32, l[2] as f32]
}

fn de(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Lab of every pixel, None where transparent.
fn lab_grid(r: &Raster) -> Vec<Option<[f32; 3]>> {
    // many logos use few colours: cache conversions
    let mut cache: std::collections::HashMap<[u8; 3], [f32; 3]> = Default::default();
    r.px.iter()
        .map(|p| {
            if p[3] < 128 {
                None
            } else {
                // over white, so half-transparent edges look as printed
                let a = p[3] as f32 / 255.0;
                let c = [0, 1, 2].map(|i| (p[i] as f32 * a + 255.0 * (1.0 - a)).round() as u8);
                Some(*cache.entry(c).or_insert_with(|| lab(c)))
            }
        })
        .collect()
}

// ---------------------------------------------------------------- background

/// Most common colour among the given pixels (binned Lab, mean of the bin).
fn dominant(labs: &[Option<[f32; 3]>], idx: impl Iterator<Item = usize>) -> Option<[f32; 3]> {
    let mut bins: std::collections::BTreeMap<[i32; 3], (usize, [f64; 3])> = Default::default();
    for i in idx {
        if let Some(c) = labs[i] {
            let k = [(c[0] / 5.0) as i32, (c[1] / 5.0) as i32, (c[2] / 5.0) as i32];
            let e = bins.entry(k).or_insert((0, [0.0; 3]));
            e.0 += 1;
            for j in 0..3 {
                e.1[j] += c[j] as f64;
            }
        }
    }
    bins.values()
        .max_by_key(|(n, _)| *n)
        .map(|(n, s)| [(s[0] / *n as f64) as f32, (s[1] / *n as f64) as f32, (s[2] / *n as f64) as f32])
}

/// Grow a region from `seeds` through pixels that change gently from their
/// neighbour (shading, wrinkles) and stay near the reference colour.
fn flood(
    labs: &[Option<[f32; 3]>],
    w: usize,
    h: usize,
    seeds: &[usize],
    reference: [f32; 3],
    step: f32,
    cap: f32,
    allowed: &dyn Fn(usize) -> bool,
) -> Vec<bool> {
    let mut on = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for &s in seeds {
        if !on[s] && allowed(s) {
            if let Some(c) = labs[s] {
                if de(c, reference) < cap {
                    on[s] = true;
                    stack.push(s);
                }
            } else {
                on[s] = true;
                stack.push(s);
            }
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let here = labs[i];
        let mut visit = |j: usize| {
            if on[j] || !allowed(j) {
                return;
            }
            let ok = match (here, labs[j]) {
                (_, None) => true,
                (None, Some(c)) => de(c, reference) < cap,
                (Some(a), Some(c)) => de(a, c) < step && de(c, reference) < cap,
            };
            if ok {
                on[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    on
}

/// Spread of colours in a region (mean ΔE from its mean): flat art is near
/// 0–2, photographed fabric is well above.
fn texture(labs: &[Option<[f32; 3]>], mask: &[bool]) -> f32 {
    let mut n = 0.0f64;
    let mut s = [0.0f64; 3];
    for (i, m) in mask.iter().enumerate() {
        if *m {
            if let Some(c) = labs[i] {
                n += 1.0;
                for j in 0..3 {
                    s[j] += c[j] as f64;
                }
            }
        }
    }
    if n < 1.0 {
        return 0.0;
    }
    let mean = [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32];
    let mut t = 0.0f64;
    for (i, m) in mask.iter().enumerate() {
        if *m {
            if let Some(c) = labs[i] {
                t += de(c, mean) as f64;
            }
        }
    }
    (t / n) as f32
}

struct Background {
    mask: Vec<bool>,
    color: Option<[f32; 3]>,
    mockup: bool,
    /// Every colour that was taken as background (surroundings, garment).
    refs: Vec<[f32; 3]>,
}

fn find_background(labs: &[Option<[f32; 3]>], w: usize, h: usize, source: Source, remove: bool) -> Background {
    let n = w * h;
    let mut mask: Vec<bool> = labs.iter().map(|c| c.is_none()).collect();
    if !remove {
        return Background { mask, color: None, mockup: false, refs: Vec::new() };
    }
    let border: Vec<usize> = (0..w)
        .flat_map(|x| [x, (h - 1) * w + x])
        .chain((0..h).flat_map(|y| [y * w, y * w + w - 1]))
        .collect();
    let mut color = None;
    let mut refs = Vec::new();
    if let Some(bg) = dominant(labs, border.iter().copied()) {
        let seeds: Vec<usize> = border
            .iter()
            .copied()
            .filter(|&i| labs[i].map(|c| de(c, bg) < 18.0).unwrap_or(true))
            .collect();
        // only treat the edge colour as background if it really surrounds
        // the picture (most of the border)
        if seeds.len() * 2 > border.len() {
            let f = flood(labs, w, h, &seeds, bg, 6.0, 26.0, &|_| true);
            for i in 0..n {
                mask[i] |= f[i];
            }
            color = Some(bg);
            refs.push(bg);
        }
    }
    // Mockups: what's left may be the garment with the logo on it. Peel the
    // garment off when it is big, textured (photographed) and surrounds the rest.
    let mut mockup = false;
    if source != Source::Logo {
        for _ in 0..2 {
            let fg_count = mask.iter().filter(|m| !**m).count();
            if fg_count < n / 50 {
                break;
            }
            // pixels of the foreground touching the background (or the frame)
            let edge: Vec<usize> = (0..n)
                .filter(|&i| {
                    if mask[i] {
                        return false;
                    }
                    let (x, y) = (i % w, i / w);
                    x == 0 || y == 0 || x + 1 == w || y + 1 == h || mask[i - 1] || mask[i + 1] || mask[i - w] || mask[i + w]
                })
                .collect();
            let Some(g) = dominant(labs, edge.iter().copied()) else { break };
            if std::env::var("SF_DEBUG_TRACE").is_ok() {
                eprintln!("peel: fg {fg_count} edge {} g {g:?}", edge.len());
            }
            let seeds: Vec<usize> = edge.iter().copied().filter(|&i| labs[i].map(|c| de(c, g) < 15.0).unwrap_or(false)).collect();
            if seeds.len() * 3 < edge.len() {
                break;
            }
            let free = |i: usize| !mask[i];
            // fabric shading drifts far from the average colour, but only
            // gradually: small steps, wide overall range
            let f = flood(labs, w, h, &seeds, g, 5.5, 40.0, &free);
            let got = f.iter().filter(|m| **m).count();
            let rest = fg_count - got;
            let textured = texture(labs, &f) > 2.6;
            if std::env::var("SF_DEBUG_TRACE").is_ok() {
                eprintln!("  seeds {} got {got} rest {rest} texture {}", seeds.len(), texture(labs, &f));
            }
            let wanted = match source {
                Source::Mockup => got * 10 > fg_count * 3,
                _ => textured && got * 10 > fg_count * 4,
            };
            if !wanted || rest < n / 400 {
                break;
            }
            for i in 0..n {
                mask[i] |= f[i];
            }
            mockup = true;
            refs.push(g);
            if color.is_none() {
                color = Some(g);
            }
        }
    }
    Background { mask, color, mockup, refs }
}

/// Background at full size from the colours found on the small copy:
/// everything reachable from the edges through those colours (with their
/// shading), plus transparent pixels.
fn background_from(labs: &[Option<[f32; 3]>], w: usize, h: usize, refs: &[[f32; 3]]) -> Vec<bool> {
    let n = w * h;
    let mut mask: Vec<bool> = labs.iter().map(|c| c.is_none()).collect();
    if refs.is_empty() {
        return mask;
    }
    let near = |c: [f32; 3]| refs.iter().map(|r| de(c, *r)).fold(f32::MAX, f32::min);
    let border: Vec<usize> = (0..w)
        .flat_map(|x| [x, (h - 1) * w + x])
        .chain((0..h).flat_map(|y| [y * w, y * w + w - 1]))
        .filter(|&i| labs[i].map(|c| near(c) < 18.0).unwrap_or(true))
        .collect();
    let mut stack = border.clone();
    for &i in &border {
        mask[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let here = labs[i];
        for j in [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ]
        .into_iter()
        .flatten()
        {
            if mask[j] {
                continue;
            }
            let ok = match (here, labs[j]) {
                (_, None) => true,
                (None, Some(c)) => near(c) < 18.0,
                (Some(a), Some(c)) => de(a, c) < 5.5 && near(c) < 40.0,
            };
            if ok {
                mask[j] = true;
                stack.push(j);
            }
        }
    }
    let _ = n;
    mask
}

// ---------------------------------------------------------------- regions

/// Connected pieces (4-neighbour) of pixels where `same(i, j)` holds.
fn components(w: usize, h: usize, same: impl Fn(usize, usize) -> bool) -> (Vec<u32>, Vec<usize>) {
    let n = w * h;
    let mut comp = vec![u32::MAX; n];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for s in 0..n {
        if comp[s] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        comp[s] = id;
        stack.push(s);
        let mut size = 0;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % w, i / w);
            let mut go = |j: usize| {
                if comp[j] == u32::MAX && same(i, j) {
                    comp[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                go(i - 1);
            }
            if x + 1 < w {
                go(i + 1);
            }
            if y > 0 {
                go(i - w);
            }
            if y + 1 < h {
                go(i + w);
            }
        }
        sizes.push(size);
    }
    (comp, sizes)
}

/// Fill the holes of a mask (pieces of its outside that don't reach the frame).
fn holes(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<bool>) {
    let (comp, sizes) = components(w, h, |i, j| !mask[i] && !mask[j]);
    let mut is_hole = vec![true; sizes.len()];
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        if mask[i] || x == 0 || y == 0 || x + 1 == w || y + 1 == h {
            if !mask[i] {
                is_hole[comp[i] as usize] = false;
            }
            if mask[i] {
                // the mask's own pieces aren't holes
                is_hole[comp[i] as usize] = false;
            }
        }
    }
    (comp, is_hole)
}

// ---------------------------------------------------------------- palette

fn kmeans(samples: &[[f32; 3]], k: usize) -> Vec<([f32; 3], usize)> {
    if samples.is_empty() {
        return Vec::new();
    }
    // deterministic farthest-point start from the most common colour
    let first = dominant(&samples.iter().map(|c| Some(*c)).collect::<Vec<_>>(), 0..samples.len()).unwrap();
    let mut centers = vec![first];
    let mut dist: Vec<f32> = samples.iter().map(|c| de(*c, first)).collect();
    while centers.len() < k {
        let (i, d) = dist
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, d)| (i, *d))
            .unwrap();
        if d < 3.0 {
            break;
        }
        centers.push(samples[i]);
        for (j, c) in samples.iter().enumerate() {
            dist[j] = dist[j].min(de(*c, samples[i]));
        }
    }
    let mut counts = vec![0usize; centers.len()];
    for _ in 0..12 {
        let mut sum = vec![[0.0f64; 3]; centers.len()];
        counts.iter_mut().for_each(|c| *c = 0);
        for c in samples {
            let j = nearest(&centers, *c);
            counts[j] += 1;
            for t in 0..3 {
                sum[j][t] += c[t] as f64;
            }
        }
        for j in 0..centers.len() {
            if counts[j] > 0 {
                centers[j] = [0, 1, 2].map(|t| (sum[j][t] / counts[j] as f64) as f32);
            }
        }
    }
    centers.into_iter().zip(counts).filter(|(_, n)| *n > 0).collect()
}

fn nearest(centers: &[[f32; 3]], c: [f32; 3]) -> usize {
    let mut best = (0, f32::MAX);
    for (j, k) in centers.iter().enumerate() {
        let d = de(*k, c);
        if d < best.1 {
            best = (j, d);
        }
    }
    best.0
}

/// Merge the closest colours until all are at least `min_de` apart and
/// there are at most `max` of them.
fn merge_colors(mut cs: Vec<([f32; 3], usize)>, min_de: f32, max: usize) -> Vec<([f32; 3], usize)> {
    loop {
        if cs.len() <= 1 {
            return cs;
        }
        let mut best = (0, 1, f32::MAX);
        for i in 0..cs.len() {
            for j in i + 1..cs.len() {
                let d = de(cs[i].0, cs[j].0);
                if d < best.2 {
                    best = (i, j, d);
                }
            }
        }
        if best.2 >= min_de && cs.len() <= max {
            return cs;
        }
        let (i, j) = (best.0, best.1);
        let (a, na) = cs[i];
        let (b, nb) = cs[j];
        let n = (na + nb).max(1);
        // keep the bigger colour's hue: logos have a "true" colour and
        // lighter/darker edge pixels around it
        let wa = if na >= nb * 3 { 0.85 } else if nb >= na * 3 { 0.15 } else { na as f32 / n as f32 };
        cs[i] = ([0, 1, 2].map(|t| a[t] * wa + b[t] * (1.0 - wa)), na + nb);
        cs.remove(j);
    }
}

// ---------------------------------------------------------------- contours

/// Sub-pixel outline of a mask: marching squares on a lightly blurred copy,
/// returning closed loops in pixel coordinates.
pub(crate) fn contours(mask: &[bool], w: usize, h: usize, sigma: f64) -> Vec<Vec<Pt>> {
    // padded, blurred field: the blur evens out pixel steps and noisy
    // colour edges so the outline comes out smooth
    let rad = (sigma * 3.0).ceil().max(1.0) as usize;
    let pad = rad + 2;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut f = vec![0.0f32; pw * ph];
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                f[(y + pad) * pw + x + pad] = 1.0;
            }
        }
    }
    let kern: Vec<f32> = {
        let k: Vec<f32> = (0..=2 * rad)
            .map(|i| {
                let d = i as f64 - rad as f64;
                (-(d * d) / (2.0 * sigma.max(0.3).powi(2))).exp() as f32
            })
            .collect();
        let s: f32 = k.iter().sum();
        k.into_iter().map(|v| v / s).collect()
    };
    let mut t = vec![0.0f32; pw * ph];
    for y in 0..ph {
        for x in rad..pw - rad {
            let mut acc = 0.0;
            for (k, kv) in kern.iter().enumerate() {
                acc += kv * f[y * pw + x + k - rad];
            }
            t[y * pw + x] = acc;
        }
    }
    for y in rad..ph - rad {
        for x in 0..pw {
            let mut acc = 0.0;
            for (k, kv) in kern.iter().enumerate() {
                acc += kv * t[(y + k - rad) * pw + x];
            }
            f[y * pw + x] = acc;
        }
    }
    let v = |x: usize, y: usize| f[y * pw + x];
    let edge_pt = |id: usize| -> Pt {
        let cell = id / 2;
        let (x, y) = (cell % pw, cell / pw);
        let (a, b, q) = if id % 2 == 0 {
            (v(x, y), v(x + 1, y), true)
        } else {
            (v(x, y), v(x, y + 1), false)
        };
        let t = ((0.5 - a) / (b - a)).clamp(0.0, 1.0) as f64;
        let (fx, fy) = if q { (x as f64 + t, y as f64) } else { (x as f64, y as f64 + t) };
        // back to mask pixel centres
        Pt::new(fx - pad as f64 + 0.5, fy - pad as f64 + 0.5)
    };
    let mut segs: Vec<[usize; 2]> = Vec::new();
    for y in 0..ph - 1 {
        for x in 0..pw - 1 {
            let (a, b, c, d) = (v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1));
            let case = ((a > 0.5) as u8) << 3 | ((b > 0.5) as u8) << 2 | ((c > 0.5) as u8) << 1 | (d > 0.5) as u8;
            let top = (y * pw + x) * 2;
            let bottom = ((y + 1) * pw + x) * 2;
            let left = (y * pw + x) * 2 + 1;
            let right = (y * pw + x + 1) * 2 + 1;
            let centre = (a + b + c + d) / 4.0 > 0.5;
            let mut add = |p: usize, q: usize| segs.push([p, q]);
            match case {
                1 | 14 => add(left, bottom),
                2 | 13 => add(bottom, right),
                3 | 12 => add(left, right),
                4 | 11 => add(top, right),
                6 | 9 => add(top, bottom),
                7 | 8 => add(left, top),
                5 => {
                    if centre {
                        add(left, top);
                        add(bottom, right);
                    } else {
                        add(top, right);
                        add(left, bottom);
                    }
                }
                10 => {
                    if centre {
                        add(top, right);
                        add(left, bottom);
                    } else {
                        add(left, top);
                        add(bottom, right);
                    }
                }
                _ => {}
            }
        }
    }
    // link segments sharing an edge point into loops
    let mut at: std::collections::HashMap<usize, [usize; 2]> = Default::default();
    for (i, s) in segs.iter().enumerate() {
        for &e in s {
            let slot = at.entry(e).or_insert([usize::MAX; 2]);
            if slot[0] == usize::MAX {
                slot[0] = i;
            } else {
                slot[1] = i;
            }
        }
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    for s0 in 0..segs.len() {
        if used[s0] {
            continue;
        }
        used[s0] = true;
        let start = segs[s0][0];
        let mut ring = vec![edge_pt(start)];
        let mut e = segs[s0][1];
        let mut guard = 0;
        while e != start && guard < segs.len() + 2 {
            guard += 1;
            ring.push(edge_pt(e));
            let pair = at[&e];
            let next = if pair[0] != usize::MAX && !used[pair[0]] {
                pair[0]
            } else if pair[1] != usize::MAX && !used[pair[1]] {
                pair[1]
            } else {
                break;
            };
            used[next] = true;
            e = if segs[next][0] == e { segs[next][1] } else { segs[next][0] };
        }
        if ring.len() >= 4 {
            loops.push(ring);
        }
    }
    loops
}

/// Group paths whose boxes come within `gap` mm of each other (holes stay
/// with the shape around them), ordered top to bottom, left to right.
fn cluster_paths(paths: Vec<VPath>, gap: f64) -> Vec<Vec<VPath>> {
    let boxes: Vec<crate::geom::BBox> = paths
        .iter()
        .map(|p| crate::geom::BBox::of(p.flatten().iter()))
        .collect();
    let n = paths.len();
    // gaps count relative to the pieces' size: letter and word spacing
    // keep a line of text together, a separate emblem splits off
    let mut hs: Vec<f64> = boxes.iter().map(|b| b.height().min(b.width()).max(0.0)).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let gap = gap.max(hs.get(hs.len() / 2).copied().unwrap_or(0.0) * 1.1);
    if std::env::var("SF_DEBUG_TRACE").is_ok() {
        eprintln!("cluster gap {gap:.2}");
        for b in &boxes {
            eprintln!("  {:.1}..{:.1} × {:.1}..{:.1}", b.min.x, b.max.x, b.min.y, b.max.y);
        }
    }
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut i = i;
        while p[i] != r {
            let nx = p[i];
            p[i] = r;
            i = nx;
        }
        r
    }
    for i in 0..n {
        for j in i + 1..n {
            if boxes[i].overlaps(&boxes[j], gap) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<(f64, f64, Vec<VPath>)> = Vec::new();
    let mut paths: Vec<Option<VPath>> = paths.into_iter().map(Some).collect();
    for (_, idx) in groups {
        let mut b = crate::geom::BBox::empty();
        for &i in &idx {
            b.add(boxes[i].min);
            b.add(boxes[i].max);
        }
        out.push((b.min.y, b.min.x, idx.into_iter().map(|i| paths[i].take().unwrap()).collect()));
    }
    out.sort_by(|a, b| (a.0 / 5.0).floor().partial_cmp(&(b.0 / 5.0).floor()).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
    out.into_iter().map(|g| g.2).collect()
}

/// A loop made only of straight edges (bars, stars, block letters): rebuilt
/// as an exact polygon with sharp corners, each edge the best-fit line
/// through its points.
fn as_polygon(pts: &[Pt], tol: f64) -> Option<VPath> {
    let n = pts.len();
    if n < 8 {
        return None;
    }
    // start the loop at the point farthest from the centre (surely a corner
    // if the shape is a polygon)
    let c = Pt::new(pts.iter().map(|p| p.x).sum::<f64>() / n as f64, pts.iter().map(|p| p.y).sum::<f64>() / n as f64);
    let start = (0..n).max_by(|&a, &b| pts[a].dist(c).partial_cmp(&pts[b].dist(c)).unwrap()).unwrap();
    let mut ring: Vec<Pt> = pts[start..].iter().chain(pts[..start].iter()).copied().collect();
    ring.push(ring[0]);
    let lim = (tol * 1.5).max(0.12);
    let simple = crate::geom::simplify(&ring, lim);
    let mut verts: Vec<Pt> = simple[..simple.len() - 1].to_vec();
    let dbg = std::env::var("SF_DEBUG_POLY").is_ok();
    if dbg { eprintln!("poly: {} verts {:?}", verts.len(), verts.iter().map(|v| (v.x as i32, v.y as i32)).collect::<Vec<_>>()); }
    if verts.len() < 3 || verts.len() > 60 {
        return None;
    }
    // index of each vertex along the ring
    let mut idx = Vec::new();
    let mut k = 0;
    for v in &verts {
        while k < ring.len() && ring[k].dist(*v) > 1e-9 {
            k += 1;
        }
        idx.push(k.min(ring.len() - 1));
    }
    // a rounded corner shows up as two vertices close together: merge them
    loop {
        let m = verts.len();
        let Some(e) = (0..m).find(|&e| verts[e].dist(verts[(e + 1) % m]) < 0.8) else { break };
        let f = (e + 1) % m;
        verts[e] = Pt::new((verts[e].x + verts[f].x) / 2.0, (verts[e].y + verts[f].y) / 2.0);
        idx[e] = if f == 0 { idx[e] } else { (idx[e] + idx[f]) / 2 };
        verts.remove(f);
        idx.remove(f);
        if f == 0 {
            // the loop's start moved: keep indices increasing
            if let Some(first) = idx.first().copied() {
                if first > *idx.last().unwrap_or(&0) {
                    return None;
                }
            }
        }
        if verts.len() < 3 {
            return None;
        }
    }
    // vertices where the outline hardly turns aren't corners: drop them,
    // smallest turn first (a curve then fails the straight-edge test below)
    loop {
        let m = verts.len();
        if m <= 3 {
            break;
        }
        let turn = |e: usize| -> f64 {
            let (a, b, c) = (verts[(e + m - 1) % m], verts[e], verts[(e + 1) % m]);
            let (u, v) = (b.sub(a), c.sub(b));
            let (lu, lv) = (u.len(), v.len());
            if lu < 1e-9 || lv < 1e-9 {
                return 0.0;
            }
            ((u.x * v.x + u.y * v.y) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees()
        };
        let (e, t) = (0..m).map(|e| (e, turn(e))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        if t >= 22.0 {
            break;
        }
        if e == 0 {
            // keep the loop's start vertex in place; drop it by rotating
            verts.rotate_left(1);
            idx.rotate_left(1);
            let last = verts.len() - 1;
            verts.remove(last);
            idx.remove(last);
            // indices must increase along the ring again
            let first = idx[0];
            for v in idx.iter_mut() {
                if *v < first {
                    *v += ring.len() - 1;
                }
            }
            let shift = first;
            ring.rotate_left(shift);
            for v in idx.iter_mut() {
                *v -= shift;
            }
            let start = ring[0];
            ring.pop();
            ring.push(start);
            let _ = start;
        } else {
            verts.remove(e);
            idx.remove(e);
        }
    }
    // every edge: long enough, its middle points on one line
    let m = verts.len();
    let mut lines: Vec<(Pt, Pt)> = Vec::new(); // point on line, direction
    for e in 0..m {
        let (i0, i1) = (idx[e], if e + 1 < m { idx[e + 1] } else { ring.len() - 1 });
        let (a, b) = (verts[e], verts[(e + 1) % m]);
        if a.dist(b) < 0.8 {
            if dbg { eprintln!("  edge {e} short {:.2}", a.dist(b)); }
            return None;
        }
        let inner: Vec<Pt> = ring[i0..=i1]
            .iter()
            .copied()
            .filter(|q| q.dist(a) > 0.7 && q.dist(b) > 0.7)
            .collect();
        // too short to tell a straight edge from a wobble
        if inner.len() < 3 {
            return None;
        }
        // total least squares line
        let k = inner.len() as f64;
        let mx = inner.iter().map(|q| q.x).sum::<f64>() / k;
        let my = inner.iter().map(|q| q.y).sum::<f64>() / k;
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for q in &inner {
            sxx += (q.x - mx).powi(2);
            syy += (q.y - my).powi(2);
            sxy += (q.x - mx) * (q.y - my);
        }
        let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let dir = Pt::new(ang.cos(), ang.sin());
        let worst = inner
            .iter()
            .map(|q| ((q.x - mx) * dir.y - (q.y - my) * dir.x).abs())
            .fold(0.0, f64::max);
        if worst > lim {
            if dbg { eprintln!("  edge {e} worst {worst:.3} > {lim:.3}"); }
            return None;
        }
        lines.push((Pt::new(mx, my), dir));
    }
    // corners must be real corners (curves split into short edges have
    // small turns at every vertex)
    let mut out = Vec::new();
    for e in 0..m {
        let (p1, d1) = lines[(e + m - 1) % m];
        let (p2, d2) = lines[e];
        let cross = d1.x * d2.y - d1.y * d2.x;
        let turn = cross.abs().asin().to_degrees();
        let turn = if d1.x * d2.x + d1.y * d2.y < 0.0 { 180.0 - turn } else { turn };
        if turn < 22.0 {
            if dbg { eprintln!("  vertex {e} turn {turn:.1}"); }
            return None;
        }
        let t = ((p2.x - p1.x) * d2.y - (p2.y - p1.y) * d2.x) / cross;
        let x = Pt::new(p1.x + t * d1.x, p1.y + t * d1.y);
        // the corner must stay near where the outline turned
        if x.dist(verts[e]) > 1.2 {
            return None;
        }
        out.push(x);
    }
    verts.clear();
    Some(VPath {
        nodes: out.into_iter().map(crate::geom::Node::corner).collect(),
        closed: true,
    })
}

/// A loop that is a circle to within needle accuracy: (centre, radius).
fn as_circle(pts: &[Pt]) -> Option<(Pt, f64)> {
    if pts.len() < 12 {
        return None;
    }
    // algebraic (Kåsa) fit: x² + y² + Dx + Ey + F = 0
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.x).sum::<f64>() / n, pts.iter().map(|p| p.y).sum::<f64>() / n);
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let (u, v) = (p.x - mx, p.y - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-12 {
        return None;
    }
    let (b1, b2) = (0.5 * (suuu + suvv), 0.5 * (svvv + svuu));
    let uc = (b1 * svv - b2 * suv) / det;
    let vc = (suu * b2 - suv * b1) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    let c = Pt::new(uc + mx, vc + my);
    if !(r > 1.0) {
        return None;
    }
    // robust: a few JPEG spikes shouldn't stop a clear circle
    let mut dev: Vec<f64> = pts.iter().map(|p| (p.dist(c) - r).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = dev[dev.len() * 95 / 100];
    let rms = (dev.iter().map(|d| d * d).sum::<f64>() / n).sqrt();
    // the whole way round, not an arc
    let mut ang: Vec<f64> = pts.iter().map(|p| (p.y - c.y).atan2(p.x - c.x)).collect();
    ang.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let gap = ang.windows(2).map(|w| w[1] - w[0]).fold(ang[0] + std::f64::consts::TAU - ang[ang.len() - 1], f64::max);
    if std::env::var("SF_DEBUG_TRACE").is_ok() && r > 5.0 {
        eprintln!("circle? r {r:.2} p95 {p95:.3} rms {rms:.3} gap {gap:.2}");
    }
    (p95 < (0.015 * r).max(0.2) && rms < (0.008 * r).max(0.1) && gap < 0.6).then_some((c, r))
}

/// Curve segments that are straight to within `tol` become straight lines,
/// and corners that the edge smoothing rounded off are made sharp again.
pub(crate) fn straighten(v: &mut VPath, tol: f64) {
    let n = v.nodes.len();
    let segs = if v.closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let j = (i + 1) % n;
        let (a, b) = (v.nodes[i].p, v.nodes[j].p);
        let (h1, h2) = (v.nodes[i].cout, v.nodes[j].cin);
        let d = b.sub(a);
        let len = d.len();
        if len < 1e-9 {
            continue;
        }
        // how far the curve itself strays from the chord, and whether it
        // stays between the ends
        let mut worst: f64 = 0.0;
        let mut worst_mid: f64 = 0.0;
        let mut inside = true;
        for k in 1..10 {
            let t = k as f64 / 10.0;
            let u = 1.0 - t;
            let q = Pt::new(
                u * u * u * a.x + 3.0 * u * u * t * h1.x + 3.0 * u * t * t * h2.x + t * t * t * b.x,
                u * u * u * a.y + 3.0 * u * u * t * h1.y + 3.0 * u * t * t * h2.y + t * t * t * b.y,
            );
            let w = q.sub(a);
            let along = (w.x * d.x + w.y * d.y) / (len * len);
            inside &= (-0.02..=1.02).contains(&along);
            let off = ((w.x * d.y - w.y * d.x) / len).abs();
            worst = worst.max(off);
            if (0.25..=0.75).contains(&t) {
                worst_mid = worst_mid.max(off);
            }
        }
        // the middle must be straight; the ends may bend as much as the
        // edge smoothing rounds a corner
        if inside && worst_mid < (tol * 1.3).max(0.12) && worst < (tol * 2.5).max(0.3) && len > 1.0 {
            v.nodes[i].cout = a;
            v.nodes[j].cin = b;
        }
    }
    // a short rounded bit between two straight edges → one sharp corner
    let is_line = |v: &VPath, i: usize| {
        let j = (i + 1) % v.nodes.len();
        v.nodes[i].cout == v.nodes[i].p && v.nodes[j].cin == v.nodes[j].p
    };
    let mut guard = 0;
    let mut i = 0;
    while v.closed && v.nodes.len() > 4 && i < v.nodes.len() && guard < 1000 {
        guard += 1;
        let m = v.nodes.len();
        let (p0, p1, p2, p3) = ((i + m - 1) % m, i, (i + 1) % m, (i + 2) % m);
        let short = v.nodes[p1].p.dist(v.nodes[p2].p);
        if short < 1.5 && is_line(v, p0) && is_line(v, p2) && !is_line(v, p1) || (short < 0.6 && is_line(v, p0) && is_line(v, p2)) {
            let (a, b) = (v.nodes[p0].p, v.nodes[p1].p);
            let (c, d) = (v.nodes[p2].p, v.nodes[p3].p);
            let (r, sdir) = (b.sub(a), d.sub(c));
            let den = r.x * sdir.y - r.y * sdir.x;
            let (lr, ls) = (r.len(), sdir.len());
            if lr > 1e-9 && ls > 1e-9 && den.abs() > 0.3 * lr * ls {
                let t = ((c.x - a.x) * sdir.y - (c.y - a.y) * sdir.x) / den;
                let x = Pt::new(a.x + t * r.x, a.y + t * r.y);
                let mid = Pt::new((b.x + c.x) / 2.0, (b.y + c.y) / 2.0);
                // only rounding as small as the edge smoothing makes; real
                // rounded corners (a rounded rectangle) stay round
                if x.dist(mid) < 0.35 {
                    v.nodes[p1] = crate::geom::Node::corner(x);
                    v.nodes.remove(p2);
                    if p2 < p1 {
                        i = i.saturating_sub(1);
                    }
                    continue;
                }
            }
        }
        i += 1;
    }
    // anchors in the middle of a straight run add nothing
    let straight_in = |v: &VPath, i: usize| v.nodes[i].cin == v.nodes[i].p;
    let straight_out = |v: &VPath, i: usize| v.nodes[i].cout == v.nodes[i].p;
    let mut i = 0;
    while v.nodes.len() > 3 && i < v.nodes.len() {
        let m = v.nodes.len();
        if !v.closed && (i == 0 || i == m - 1) {
            i += 1;
            continue;
        }
        let (pi, ni) = ((i + m - 1) % m, (i + 1) % m);
        if straight_in(v, i) && straight_out(v, i) && straight_out(v, pi) && straight_in(v, ni) {
            let (a, b, q) = (v.nodes[pi].p, v.nodes[ni].p, v.nodes[i].p);
            let d = b.sub(a);
            let len = d.len();
            let w = q.sub(a);
            let t = (w.x * d.x + w.y * d.y) / (len * len).max(1e-12);
            if len > 1e-9 && (0.0..=1.0).contains(&t) && ((w.x * d.y - w.y * d.x) / len).abs() < tol * 0.5 {
                v.nodes.remove(i);
                continue;
            }
        }
        i += 1;
    }
}

fn ring_area(r: &[Pt]) -> f64 {
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    a / 2.0
}

// ---------------------------------------------------------------- main

/// Trace a picture into stacked colour layers.
pub fn trace(src: &Raster, o: &TraceOptions) -> Result<Trace, String> {
    // 1. background and crop, worked out on a small copy
    let area = o.crop.unwrap_or([0, 0, src.w, src.h]);
    let region = src.crop(area);
    let s = (700.0 / region.w.max(region.h) as f64).min(1.0);
    let (aw, ah) = (((region.w as f64 * s).round() as u32).max(8), ((region.h as f64 * s).round() as u32).max(8));
    let small = region.resized(aw, ah);
    let small_labs = lab_grid(&small);
    let first = find_background(&small_labs, aw as usize, ah as usize, o.source, o.remove_background);
    let crop = match o.crop {
        Some(c) => c,
        None => {
            let bg = &first;
            // drop crumbs when finding the logo's extent
            let (comp, sizes) = components(aw as usize, ah as usize, |i, j| bg.mask[i] == bg.mask[j]);
            let min = (aw as usize * ah as usize / 4000).max(4);
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for i in 0..(aw * ah) as usize {
                if !bg.mask[i] && sizes[comp[i] as usize] >= min {
                    let (x, y) = (i % aw as usize, i / aw as usize);
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
            if x0 == usize::MAX {
                return Err("couldn't find anything but background in the picture; try turning off background removal".into());
            }
            let pad = 3usize;
            let to_src = |v: usize, k: u32| ((v as f64 / s).round() as u32).min(k);
            let x0s = to_src(x0.saturating_sub(pad), src.w);
            let y0s = to_src(y0.saturating_sub(pad), src.h);
            let x1s = to_src((x1 + pad + 1).min(aw as usize), src.w);
            let y1s = to_src((y1 + pad + 1).min(ah as usize), src.h);
            [x0s, y0s, (x1s - x0s).max(1), (y1s - y0s).max(1)]
        }
    };
    let img = src.crop(crop);

    // 2. fixed physical resolution
    let detail = o.detail;
    let target_mmpp = match detail {
        Detail::Clean => 0.12,
        Detail::Balanced => 0.10,
        Detail::Fine => 0.07,
    };
    let width_mm = o.width_mm.clamp(5.0, 600.0);
    let mut ww = (width_mm / target_mmpp).round().clamp(200.0, 2600.0);
    let aspect = img.h as f64 / img.w as f64;
    if ww * ww * aspect > 5.0e6 {
        ww = (5.0e6 / aspect).sqrt();
    }
    let (w, h) = (ww as usize, ((ww * aspect).round() as usize).max(8));
    let mm_per_px = width_mm / w as f64;
    // the source's own pixel size at this width: smoothing must hide its steps
    let src_px_mm = width_mm / img.w as f64;
    let img = img.resized(w as u32, h as u32);
    let labs = lab_grid(&img);
    let n = w * h;
    let bg = Background {
        mask: background_from(&labs, w, h, &first.refs),
        color: first.color,
        mockup: first.mockup,
        refs: first.refs.clone(),
    };

    // 3. palette from flat areas
    let flat: Vec<usize> = (0..n)
        .filter(|&i| {
            if bg.mask[i] {
                return false;
            }
            let Some(c) = labs[i] else { return false };
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                return false;
            }
            [i - 1, i + 1, i - w, i + w, i - w - 1, i + w + 1].iter().all(|&j| labs[j].map(|d| de(c, d) < 4.5).unwrap_or(false))
        })
        .collect();
    let fg: Vec<usize> = (0..n).filter(|&i| !bg.mask[i]).collect();
    if fg.is_empty() {
        return Err("nothing left after removing the background".into());
    }
    let pool: &[usize] = if flat.len() >= fg.len() / 8 && flat.len() > 200 { &flat } else { &fg };
    let stride = (pool.len() / 40000).max(1);
    let samples: Vec<[f32; 3]> = pool.iter().step_by(stride).filter_map(|&i| labs[i]).collect();
    let max = if o.max_colors == 0 { 12 } else { o.max_colors.clamp(1, 15) };
    let mut cs = if o.fixed_palette.is_empty() {
        let mut cs = kmeans(&samples, (max * 2).max(16));
        // tiny clusters are noise or edge colours
        let total: usize = cs.iter().map(|c| c.1).sum();
        let min_share = match detail {
            Detail::Clean => 0.006,
            Detail::Balanced => 0.003,
            Detail::Fine => 0.0012,
        };
        cs.retain(|c| c.1 as f64 >= total as f64 * min_share);
        merge_colors(cs, o.eff_merge(), max)
    } else {
        // the user gave the thread colours: use them exactly
        o.fixed_palette.iter().map(|c| (lab(*c), 1usize)).collect()
    };
    cs.sort_by(|a, b| b.1.cmp(&a.1));
    let centers: Vec<[f32; 3]> = cs.iter().map(|c| c.0).collect();
    if centers.is_empty() {
        return Err("couldn't find any colours to trace".into());
    }

    // 4. every pixel gets a colour, then clean up
    let mut labels: Vec<u16> = (0..n)
        .map(|i| if bg.mask[i] { NONE } else { labs[i].map(|c| nearest(&centers, c) as u16).unwrap_or(NONE) })
        .collect();
    // a pixel can also be "background" when it is nearer the removed colour
    // than any thread colour (anti-aliased edges against the background)
    // (or the garment showing through, e.g. inside letters)
    if !bg.refs.is_empty() {
        for i in 0..n {
            if labels[i] != NONE {
                if let Some(c) = labs[i] {
                    let near_bg = bg.refs.iter().map(|r| de(c, *r)).fold(f32::MAX, f32::min);
                    let near_thread = de(c, centers[labels[i] as usize]);
                    if near_bg < near_thread * 0.8 && !centers.iter().any(|k| bg.refs.iter().any(|r| de(*k, *r) < 12.0)) {
                        labels[i] = NONE;
                    }
                }
            }
        }
    }
    for &d in &o.drop {
        if d < centers.len() {
            for l in labels.iter_mut() {
                if *l == d as u16 {
                    *l = NONE;
                }
            }
        }
    }
    for _ in 0..2 {
        majority(&mut labels, w, h);
    }
    let speck_px = (o.eff_speck() / (mm_per_px * mm_per_px)).max(4.0) as usize;
    // thinner than this can't be sewn as an area (mm, then px)
    let min_thick = o.eff_min_line();
    let thin_px = min_thick / mm_per_px;
    despeckle(&mut labels, w, h, speck_px, thin_px);

    // Colours that only live along edges between other colours (JPEG mush,
    // anti-aliasing) are thinner than anything a needle can sew: fold them
    // into their neighbours, thinnest first.
    let mut gone: Vec<bool> = (0..centers.len()).map(|j| o.drop.contains(&j)).collect();
    loop {
        let (cnt, per) = count_perimeter(&labels, w, h, centers.len());
        let fg_now: usize = cnt.iter().sum();
        let blends = blend_colors(&labels, w, h, &centers, &gone);
        let worst = (0..centers.len())
            .filter(|&j| !gone[j] && cnt[j] > 0)
            .map(|j| (j, 2.0 * cnt[j] as f64 / per[j].max(1) as f64 * mm_per_px))
            .filter(|&(j, t)| {
                (t < min_thick && cnt[j] * 7 < fg_now)
                    // a band whose colour is a mix of the two colours it
                    // separates (blur, photo softness)
                    || (blends[j] && t < 1.6 && cnt[j] * 5 < fg_now)
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let Some((j, _)) = worst else { break };
        gone[j] = true;
        let others: Vec<usize> = (0..centers.len()).filter(|&c| !gone[c]).collect();
        if others.is_empty() {
            break;
        }
        for i in 0..n {
            if labels[i] == j as u16 {
                labels[i] = labs[i]
                    .map(|c| {
                        others
                            .iter()
                            .copied()
                            .min_by(|&a, &b| de(c, centers[a]).partial_cmp(&de(c, centers[b])).unwrap())
                            .unwrap() as u16
                    })
                    .unwrap_or(NONE);
            }
        }
        majority(&mut labels, w, h);
        despeckle(&mut labels, w, h, speck_px, thin_px);
    }

    // Areas in the background's colour: where they are background showing
    // through (touching it, or the counters of letters and the inside of
    // thin outlines) they aren't stitched; on a broad fill (white text on a
    // navy banner) they are. The user can switch either way.
    let like_bg: Vec<bool> = centers.iter().map(|c| bg.refs.iter().any(|r| de(*c, *r) < 12.0)).collect();
    if like_bg.iter().any(|b| *b) {
        let (cnt, per) = count_perimeter(&labels, w, h, centers.len());
        let thick: Vec<f64> = (0..centers.len()).map(|j| 2.0 * cnt[j] as f64 / per[j].max(1) as f64 * mm_per_px).collect();
        let (comp, sizes) = components(w, h, |i, j| labels[i] == labels[j]);
        let mut votes: std::collections::BTreeMap<u32, std::collections::BTreeMap<u16, usize>> = Default::default();
        for i in 0..n {
            let l = labels[i];
            if l == NONE || !like_bg[l as usize] || o.keep.contains(&(l as usize)) {
                continue;
            }
            let (x, y) = (i % w, i / w);
            for j in [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ]
            .into_iter()
            .flatten()
            {
                if comp[j] != comp[i] {
                    *votes.entry(comp[i]).or_default().entry(labels[j]).or_default() += 1;
                }
            }
        }
        let mut out: std::collections::HashSet<u32> = Default::default();
        for (c, v) in votes {
            let touches_bg = v.get(&NONE).copied().unwrap_or(0) > 0;
            let (top, _) = v.iter().max_by_key(|(_, n)| **n).map(|(l, n)| (*l, *n)).unwrap();
            let in_thin = top != NONE && thick[top as usize] < 3.5;
            if touches_bg || in_thin {
                out.insert(c);
            }
        }
        let _ = sizes;
        for i in 0..n {
            if out.contains(&comp[i]) {
                labels[i] = NONE;
            }
        }
    }
    // thin slivers along colour boundaries: one more smoothing pass
    majority(&mut labels, w, h);
    despeckle(&mut labels, w, h, speck_px, thin_px);

    // pixel counts, and true colours: the mean of flat (not edge) pixels
    let k = centers.len();
    let mut count = vec![0usize; k];
    let mut sum = vec![[0u64; 3]; k];
    let mut flat_n = vec![0u64; k];
    let mut is_flat = vec![false; n];
    for &i in &flat {
        is_flat[i] = true;
    }
    for i in 0..n {
        let l = labels[i];
        if l != NONE {
            count[l as usize] += 1;
            if is_flat[i] {
                flat_n[l as usize] += 1;
                for t in 0..3 {
                    sum[l as usize][t] += img.px[i][t] as u64;
                }
            }
        }
    }
    let fg_px: usize = count.iter().sum();
    let palette: Vec<PaletteColor> = (0..k)
        .map(|j| {
            let rgb = if flat_n[j] > 20 {
                [0, 1, 2].map(|t| (sum[j][t] / flat_n[j]) as u8)
            } else {
                let c = cs[j].0;
                crate::thread::rgb_of_lab([c[0] as f64, c[1] as f64, c[2] as f64])
            };
            PaletteColor {
                rgb,
                share: count[j] as f64 / fg_px.max(1) as f64,
                dropped: o.drop.contains(&j),
                like_background: like_bg[j],
            }
        })
        .collect();

    // 5. stacking order: fills by enclosed area (largest under), outlines on top
    struct Info {
        color: usize,
        filled: usize,
        outline: bool,
    }
    let mut infos = Vec::new();
    for j in 0..k {
        if count[j] == 0 {
            continue;
        }
        let mask: Vec<bool> = labels.iter().map(|l| *l == j as u16).collect();
        let (comp, is_hole) = holes(&mask, w, h);
        let filled = (0..n).filter(|&i| mask[i] || is_hole[comp[i] as usize]).count();
        let mut perim = 0usize;
        for i in 0..n {
            if mask[i] {
                let (x, y) = (i % w, i / w);
                perim += (x == 0 || !mask[i - 1]) as usize
                    + (x + 1 == w || !mask[i + 1]) as usize
                    + (y == 0 || !mask[i - w]) as usize
                    + (y + 1 == h || !mask[i + w]) as usize;
            }
        }
        let thick_mm = 2.0 * count[j] as f64 / perim.max(1) as f64 * mm_per_px;
        // a thin band enclosing far more than itself: an outline/border
        let outline = thick_mm < 3.5 && filled > count[j] * 3;
        infos.push(Info { color: j, filled, outline });
    }
    infos.sort_by(|a, b| a.outline.cmp(&b.outline).then(b.filled.cmp(&a.filled)));
    let order: Vec<usize> = infos.iter().map(|i| i.color).collect();
    let rank: Vec<usize> = {
        let mut r = vec![usize::MAX; k];
        for (p, &c) in order.iter().enumerate() {
            r[c] = p;
        }
        r
    };

    // 6. outlines → curves
    let under_px = (0.6 / mm_per_px).round().max(1.0) as usize;
    let min_ring_mm2 = o.eff_speck() * 0.6;
    let mut layers = Vec::new();
    let mut used_names: std::collections::HashMap<String, usize> = Default::default();
    let chart = crate::thread::generic_chart();
    for (p, info) in infos.iter().enumerate() {
        let j = info.color;
        let mut mask: Vec<bool> = labels.iter().map(|l| *l == j as u16).collect();
        let above = |l: u16| l != NONE && rank[l as usize] > p;
        if o.overlap && !info.outline {
            // fill holes that the colours on top cover anyway
            let (comp, is_hole) = holes(&mask, w, h);
            let mut hole_px: std::collections::HashMap<u32, (usize, usize)> = Default::default();
            for i in 0..n {
                if !mask[i] && is_hole[comp[i] as usize] {
                    let e = hole_px.entry(comp[i]).or_insert((0, 0));
                    e.0 += 1;
                    if above(labels[i]) {
                        e.1 += 1;
                    }
                }
            }
            for i in 0..n {
                if !mask[i] {
                    if let Some((all, cov)) = hole_px.get(&comp[i]) {
                        if *cov * 100 >= *all * 85 {
                            mask[i] = true;
                        }
                    }
                }
            }
            // and reach a little way under neighbouring colours on top
            let mut dist = vec![u16::MAX; n];
            let mut q = std::collections::VecDeque::new();
            for i in 0..n {
                if mask[i] {
                    dist[i] = 0;
                    q.push_back(i);
                }
            }
            while let Some(i) = q.pop_front() {
                let d = dist[i];
                if d as usize >= under_px {
                    continue;
                }
                let (x, y) = (i % w, i / w);
                let mut go = |jx: usize| {
                    if dist[jx] == u16::MAX && above(labels[jx]) {
                        dist[jx] = d + 1;
                        mask[jx] = true;
                        q.push_back(jx);
                    }
                };
                if x > 0 {
                    go(i - 1);
                }
                if x + 1 < w {
                    go(i + 1);
                }
                if y > 0 {
                    go(i - w);
                }
                if y + 1 < h {
                    go(i + w);
                }
            }
        }
        let mut paths = Vec::new();
        // photos are never as exact as artwork: smooth them more
        let photo = if bg.mockup { 2.0 } else { 1.0 };
        let sigma_mm = if o.smooth_mm.is_some() {
            o.eff_smooth() * photo
        } else {
            detail.smooth_mm().max(0.45 * src_px_mm) * photo
        };
        let tol = if o.fit_tol_mm.is_some() {
            o.eff_fit()
        } else {
            (detail.fit_tol().max(0.3 * src_px_mm) * photo).min(0.3)
        };
        for ring in contours(&mask, w, h, sigma_mm / mm_per_px) {
            let mm: Vec<Pt> = ring.iter().map(|q| Pt::new(q.x * mm_per_px, q.y * mm_per_px)).collect();
            if ring_area(&mm).abs() < min_ring_mm2 {
                continue;
            }
            let circle = if o.snap_circles { as_circle(&mm) } else { None };
            let poly = if o.snap_corners && circle.is_none() { as_polygon(&mm, tol) } else { None };
            let v = match circle {
                Some((c, r)) => crate::vector::ellipse(c.x, c.y, r, r),
                None if let Some(poly) = poly => poly,
                None => {
                    let mut v = fit_path(&mm, true, tol);
                    straighten(&mut v, tol);
                    v
                }
            };
            if v.nodes.len() >= 2 {
                paths.push(v);
            }
        }
        if paths.is_empty() {
            continue;
        }
        let rgb = palette[j].rgb;
        let base = chart[crate::thread::nearest(&chart, rgb).0].name.clone();
        let n_used = used_names.entry(base.clone()).or_insert(0);
        *n_used += 1;
        let name = if *n_used > 1 { format!("{base} {}", n_used) } else { base };
        let name = if info.outline { format!("{name} outline") } else { name };
        // pieces of one colour that sit apart (a star and a line of text)
        // become separate layers, so each can have its own settings
        let groups = cluster_paths(paths, 2.5);
        let many = groups.len() > 1;
        for (gi, g) in groups.into_iter().enumerate() {
            layers.push(TraceLayer {
                color: j,
                rgb,
                name: if many { format!("{name} ({})", gi + 1) } else { name.clone() },
                paths: g,
                outline: info.outline,
            });
        }
    }
    if layers.is_empty() {
        return Err("nothing big enough to stitch was found".into());
    }
    let mut notes = Vec::new();
    if bg.mockup {
        notes.push("Mockup photo: the garment was removed; check the crop".into());
    }
    if palette.iter().filter(|c| !c.dropped).count() >= 10 {
        notes.push("Many colours: try fewer colours or Clean for a simpler logo".into());
    }
    let background = bg.color.map(|c| crate::thread::rgb_of_lab([c[0] as f64, c[1] as f64, c[2] as f64]));
    Ok(Trace {
        w,
        h,
        mm_per_px,
        labels,
        palette,
        layers,
        crop,
        background,
        mockup: bg.mockup,
        notes,
    })
}

/// Colours that look like a mix of the two colours they mostly border.
fn blend_colors(labels: &[u16], w: usize, h: usize, centers: &[[f32; 3]], gone: &[bool]) -> Vec<bool> {
    let k = centers.len();
    let mut nb = vec![vec![0usize; k + 1]; k];
    for i in 0..w * h {
        let l = labels[i];
        if l == NONE {
            continue;
        }
        let (x, y) = (i % w, i / w);
        for j in [(x + 1 < w).then(|| i + 1), (y + 1 < h).then(|| i + w)].into_iter().flatten() {
            let m = labels[j];
            if m != l {
                let mi = if m == NONE { k } else { m as usize };
                nb[l as usize][mi] += 1;
                if m != NONE {
                    nb[m as usize][l as usize] += 1;
                }
            }
        }
    }
    (0..k)
        .map(|j| {
            if gone[j] {
                return false;
            }
            let mut v: Vec<(usize, usize)> = nb[j].iter().copied().enumerate().filter(|(m, n)| *n > 0 && *m != j).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            let total: usize = v.iter().map(|x| x.1).sum();
            if v.len() < 2 || v[0].0 == k || v[1].0 == k || (v[0].1 + v[1].1) * 10 < total * 7 {
                return false;
            }
            let (a, b, c) = (centers[v[0].0], centers[v[1].0], centers[j]);
            // distance from c to the segment a–b in Lab
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let len2 = ab.iter().map(|x| x * x).sum::<f32>().max(1e-6);
            let t = ((c[0] - a[0]) * ab[0] + (c[1] - a[1]) * ab[1] + (c[2] - a[2]) * ab[2]) / len2;
            let p = [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]];
            (0.1..0.9).contains(&t) && de(c, p) < 12.0
        })
        .collect()
}

/// Pixel count and boundary length (pixel edges) of each colour.
fn count_perimeter(labels: &[u16], w: usize, h: usize, k: usize) -> (Vec<usize>, Vec<usize>) {
    let mut cnt = vec![0usize; k];
    let mut per = vec![0usize; k];
    for i in 0..w * h {
        let l = labels[i];
        if l == NONE {
            continue;
        }
        cnt[l as usize] += 1;
        let (x, y) = (i % w, i / w);
        per[l as usize] += (x == 0 || labels[i - 1] != l) as usize
            + (x + 1 == w || labels[i + 1] != l) as usize
            + (y == 0 || labels[i - w] != l) as usize
            + (y + 1 == h || labels[i + w] != l) as usize;
    }
    (cnt, per)
}

/// Replace pixels that disagree with most of their 8 neighbours.
fn majority(labels: &mut [u16], w: usize, h: usize) {
    let src = labels.to_vec();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w - 1 {
            let i = y * w + x;
            let me = src[i];
            let mut cands: [(u16, u8); 9] = [(NONE, 0); 9];
            let mut m = 0;
            let mut same = 0;
            for dy in [-1i64, 0, 1] {
                for dx in [-1i64, 0, 1] {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let l = src[(i as i64 + dy * w as i64 + dx) as usize];
                    if l == me {
                        same += 1;
                        continue;
                    }
                    match cands[..m].iter_mut().find(|c| c.0 == l) {
                        Some(c) => c.1 += 1,
                        None => {
                            cands[m] = (l, 1);
                            m += 1;
                        }
                    }
                }
            }
            if same <= 2 {
                if let Some(best) = cands[..m].iter().max_by_key(|c| c.1) {
                    if best.1 >= 5 {
                        labels[i] = best.0;
                    }
                }
            }
        }
    }
}

/// Merge pieces smaller than `min_px` (of any colour, background included)
/// into the colour that surrounds them most.
fn despeckle(labels: &mut [u16], w: usize, h: usize, min_px: usize, min_thick_px: f64) {
    for _ in 0..3 {
        let (comp, sizes) = components(w, h, |i, j| labels[i] == labels[j]);
        // boundary length of each piece, for its typical thickness
        let mut per = vec![0usize; sizes.len()];
        for i in 0..w * h {
            let c = comp[i];
            let (x, y) = (i % w, i / w);
            per[c as usize] += (x == 0 || comp[i - 1] != c) as usize
                + (x + 1 == w || comp[i + 1] != c) as usize
                + (y == 0 || comp[i - w] != c) as usize
                + (y + 1 == h || comp[i + w] != c) as usize;
        }
        let small = |i: usize| {
            let c = comp[i] as usize;
            sizes[c] < min_px
                || (labels[i] != NONE && (2.0 * sizes[c] as f64 / per[c].max(1) as f64) < min_thick_px)
        };
        let mut changed = false;
        let mut votes: std::collections::BTreeMap<u32, std::collections::BTreeMap<u16, usize>> = Default::default();
        for i in 0..w * h {
            let c = comp[i];
            if !small(i) {
                continue;
            }
            let (x, y) = (i % w, i / w);
            for j in [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ]
            .into_iter()
            .flatten()
            {
                if comp[j] != c {
                    *votes.entry(c).or_default().entry(labels[j]).or_default() += 1;
                }
            }
        }
        let winner: std::collections::HashMap<u32, u16> = votes
            .into_iter()
            .filter_map(|(c, v)| v.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| (c, l)))
            .collect();
        for i in 0..w * h {
            if let Some(l) = winner.get(&comp[i]) {
                if labels[i] != *l {
                    labels[i] = *l;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

impl Trace {
    /// The traced logo as SVG (mm), bottom colour first.
    pub fn to_svg(&self) -> String {
        let (wm, hm) = (self.w as f64 * self.mm_per_px, self.h as f64 * self.mm_per_px);
        let mut s = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{wm:.2}mm' height='{hm:.2}mm' viewBox='0 0 {wm:.3} {hm:.3}'>\n"
        );
        let mut ids = std::collections::HashSet::new();
        for l in &self.layers {
            let d: String = l.paths.iter().map(|v| crate::vector::path_d(v, &|p| p)).collect::<Vec<_>>().join(" ");
            let mut id: String = l
                .name
                .chars()
                .map(|c| if c.is_alphanumeric() || " -()".contains(c) { c } else { '-' })
                .collect();
            while !ids.insert(id.clone()) {
                id.push(' ');
            }
            s.push_str(&format!(
                "  <path id='{id}' fill='#{:02x}{:02x}{:02x}' fill-rule='evenodd' d='{d}'/>\n",
                l.rgb[0], l.rgb[1], l.rgb[2]
            ));
        }
        s.push_str("</svg>\n");
        s
    }

    /// The cleaned colour map as an RGBA picture (background transparent),
    /// for showing what will be traced.
    pub fn preview_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w * self.h * 4);
        for &l in &self.labels {
            if l == NONE || self.palette[l as usize].dropped {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let c = self.palette[l as usize].rgb;
                out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        out
    }

    pub fn height_mm(&self) -> f64 {
        self.h as f64 * self.mm_per_px
    }
}

/// Read and trace a picture file in one go (defaults), giving SVG.
pub fn trace_bytes_to_svg(bytes: &[u8], o: &TraceOptions) -> Result<(String, Vec<String>), String> {
    let r = Raster::decode(bytes)?;
    let t = trace(&r, o)?;
    Ok((t.to_svg(), t.notes))
}

pub fn is_raster(path: &str) -> bool {
    let p = path.to_lowercase();
    [".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp", ".tif", ".tiff"].iter().any(|e| p.ends_with(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A navy disc with a white ring and red bar, anti-aliased, on white,
    /// with mild noise: like a small saved-from-the-web logo.
    fn logo(noise: bool) -> Raster {
        let (w, h) = (240u32, 160u32);
        let mut px = Vec::new();
        let mut seed = 7u32;
        for y in 0..h {
            for x in 0..w {
                // 4×4 supersampling for anti-aliased edges
                let mut acc = [0f32; 3];
                for sy in 0..4 {
                    for sx in 0..4 {
                        let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
                        let d = ((fx - 80.0).powi(2) + (fy - 80.0).powi(2)).sqrt();
                        let c = if d < 60.0 && !(d > 40.0 && d < 46.0) {
                            [20.0, 36.0, 90.0]
                        } else if (150.0..225.0).contains(&fx) && (60.0..100.0).contains(&fy) {
                            [200.0, 24.0, 40.0]
                        } else {
                            [255.0, 255.0, 255.0]
                        };
                        for t in 0..3 {
                            acc[t] += c[t] / 16.0;
                        }
                    }
                }
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let nz = if noise { ((seed >> 16) % 9) as f32 - 4.0 } else { 0.0 };
                px.push([
                    (acc[0] + nz).clamp(0.0, 255.0) as u8,
                    (acc[1] + nz).clamp(0.0, 255.0) as u8,
                    (acc[2] + nz).clamp(0.0, 255.0) as u8,
                    255,
                ]);
            }
        }
        Raster { w, h, px }
    }

    #[test]
    fn traces_a_simple_logo() {
        for noise in [false, true] {
            let t = trace(&logo(noise), &TraceOptions { width_mm: 60.0, ..Default::default() }).unwrap();
            // navy and red; the white background is gone
            let mut names: Vec<&str> = t.layers.iter().map(|l| l.name.as_str()).collect();
            names.sort();
            // the white ring sits on a broad navy fill, so it is stitched
            // (on a dark shirt it must be white) and flagged as background-coloured
            assert_eq!(names, vec!["Navy", "Red", "White outline"], "noise={noise}: {names:?}");
            let white = t.layers.iter().find(|l| l.name.starts_with("White")).unwrap();
            assert!(t.palette[white.color].like_background);
            // cropped to the logo: 205 × 120 px of 240 × 160 → aspect kept
            let ratio = t.height_mm() / 60.0;
            assert!((ratio - 120.0 / 205.0).abs() < 0.04, "ratio {ratio}");
            // the navy disc runs under the white ring: one solid circle,
            // snapped to a true circle (4 anchors), not hundreds of points
            let navy = t.layers.iter().find(|l| l.name == "Navy").unwrap();
            assert_eq!(navy.paths.len(), 1, "noise={noise}");
            assert_eq!(navy.paths[0].nodes.len(), 4);
            // the ring itself: two circles
            assert_eq!(white.paths.len(), 2);
            assert!(white.paths.iter().all(|p| p.nodes.len() <= 8));
            let red = t.layers.iter().find(|l| l.name == "Red").unwrap();
            assert_eq!(red.paths[0].nodes.iter().filter(|n| !n.is_smooth()).count(), 4);
            // a straight-sided bar: straight segments, no curves
            assert!(red.paths[0].nodes.iter().all(|n| n.cin == n.p && n.cout == n.p));
        }
    }

    #[test]
    fn separate_pieces_become_separate_layers() {
        let v = |x: f64| crate::vector::rect(x, 0.0, 5.0, 5.0, 0.0);
        // two letters 1 mm apart stay together; a shape 10 mm away is its own
        let g = cluster_paths(vec![v(0.0), v(6.0), v(21.0)], 2.5);
        assert_eq!(g.iter().map(|g| g.len()).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn svg_round_trip_imports() {
        let t = trace(&logo(false), &TraceOptions { width_mm: 80.0, ..Default::default() }).unwrap();
        let (opt, _) = crate::import::font_options(None).unwrap();
        let art = crate::import::load_svg_str(&t.to_svg(), &opt).unwrap();
        assert_eq!(art.shapes.len(), 3);
        let b = crate::geom::BBox::of(art.shapes.iter().flat_map(|s| s.points()));
        assert!((b.width() - 80.0).abs() < 3.0, "width {}", b.width());
    }

    #[test]
    fn dropping_a_colour() {
        let base = trace(&logo(false), &TraceOptions { width_mm: 60.0, ..Default::default() }).unwrap();
        let red = base.layers.iter().find(|l| l.name == "Red").unwrap().color;
        let t = trace(
            &logo(false),
            &TraceOptions { width_mm: 60.0, drop: vec![red], crop: Some(base.crop), ..Default::default() },
        )
        .unwrap();
        assert_eq!(t.layers.len(), 2);
    }
}
