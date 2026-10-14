//! Geometry primitives: points, rings, scanline slicing.
//! All design coordinates are millimetres, x right, y down (SVG convention).

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub fn new(x: f64, y: f64) -> Self {
        Pt { x, y }
    }
    pub fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
    pub fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
    pub fn mul(self, k: f64) -> Pt {
        Pt::new(self.x * k, self.y * k)
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dist(self, o: Pt) -> f64 {
        self.sub(o).len()
    }
    pub fn lerp(self, o: Pt, t: f64) -> Pt {
        Pt::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
    /// Rotate around the origin by `a` radians.
    pub fn rot(self, a: f64) -> Pt {
        let (s, c) = a.sin_cos();
        Pt::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    pub fn norm(self) -> Pt {
        let l = self.len();
        if l < 1e-12 {
            Pt::new(0.0, 0.0)
        } else {
            self.mul(1.0 / l)
        }
    }
}

/// A closed ring (last point is NOT repeated).
pub type Ring = Vec<Pt>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct BBox {
    pub min: Pt,
    pub max: Pt,
}

impl BBox {
    pub fn empty() -> Self {
        BBox {
            min: Pt::new(f64::MAX, f64::MAX),
            max: Pt::new(f64::MIN, f64::MIN),
        }
    }
    pub fn add(&mut self, p: Pt) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }
    pub fn of<'a>(pts: impl IntoIterator<Item = &'a Pt>) -> Self {
        let mut b = BBox::empty();
        for p in pts {
            b.add(*p);
        }
        b
    }
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }
    pub fn center(&self) -> Pt {
        self.min.lerp(self.max, 0.5)
    }
    pub fn overlaps(&self, o: &BBox, margin: f64) -> bool {
        self.min.x - margin < o.max.x
            && o.min.x - margin < self.max.x
            && self.min.y - margin < o.max.y
            && o.min.y - margin < self.max.y
    }
    pub fn is_valid(&self) -> bool {
        self.min.x <= self.max.x && self.min.y <= self.max.y
    }
}

pub fn rings_perimeter(rings: &[Ring]) -> f64 {
    rings
        .iter()
        .map(|r| {
            (0..r.len())
                .map(|i| r[i].dist(r[(i + 1) % r.len()]))
                .sum::<f64>()
        })
        .sum()
}

/// One horizontal span of the shape on a scanline (in the rotated frame).
#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub row: i64,
    pub y: f64,
    pub x0: f64,
    pub x1: f64,
}

impl Seg {
    pub fn len(&self) -> f64 {
        self.x1 - self.x0
    }
}

fn point_in_ring(r: &Ring, p: Pt) -> bool {
    let mut inside = false;
    let n = r.len();
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
            inside = !inside;
        }
    }
    inside
}

fn signed_area(r: &Ring) -> f64 {
    (0..r.len())
        .map(|i| {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.0
}

/// Split a multi-ring shape into separate islands (an outline plus its
/// holes), e.g. one per letter, so each gets its own stitch direction.
/// Outlines that overlap each other stay in one island.
pub fn split_islands(rings: &[Ring], rule: FillRule) -> Vec<Vec<Ring>> {
    let n = rings.len();
    if n <= 1 {
        return vec![rings.to_vec()];
    }
    let area: Vec<f64> = rings.iter().map(signed_area).collect();
    let bbs: Vec<BBox> = rings.iter().map(|r| BBox::of(r.iter())).collect();
    let contains = |j: usize, i: usize| -> bool {
        j != i
            && area[j].abs() > area[i].abs()
            && bbs[j].overlaps(&bbs[i], 0.0)
            && point_in_ring(&rings[j], rings[i][0])
    };
    // Smallest ring containing each ring.
    let parent: Vec<Option<usize>> = (0..n)
        .map(|i| {
            (0..n)
                .filter(|&j| contains(j, i))
                .min_by(|&a, &b| area[a].abs().partial_cmp(&area[b].abs()).unwrap())
        })
        .collect();
    let depth = |mut i: usize| -> usize {
        let mut d = 0;
        while let Some(p) = parent[i] {
            d += 1;
            i = p;
        }
        d
    };
    let is_hole: Vec<bool> = (0..n)
        .map(|i| match (rule, parent[i]) {
            (_, None) => false,
            (FillRule::EvenOdd, _) => depth(i) % 2 == 1,
            // Non-zero: a hole winds the other way from the ring around it.
            (FillRule::NonZero, Some(p)) => area[i].signum() != area[p].signum(),
        })
        .collect();
    // Union outlines that overlap or nest without being holes.
    let mut group: Vec<usize> = (0..n).collect();
    fn find(g: &mut Vec<usize>, i: usize) -> usize {
        if g[i] != i {
            let r = find(g, g[i]);
            g[i] = r;
        }
        g[i]
    }
    for i in 0..n {
        if is_hole[i] {
            continue;
        }
        for j in 0..n {
            if i == j || is_hole[j] || !bbs[i].overlaps(&bbs[j], 0.0) {
                continue;
            }
            let touch = rings[i]
                .iter()
                .step_by(4)
                .any(|&p| point_in_ring(&rings[j], p))
                || rings[j]
                    .iter()
                    .step_by(4)
                    .any(|&p| point_in_ring(&rings[i], p));
            if touch {
                let (a, b) = (find(&mut group, i), find(&mut group, j));
                group[a] = b;
            }
        }
    }
    // Holes join the island of the outline around them.
    let mut owner: Vec<usize> = (0..n).collect();
    for i in 0..n {
        owner[i] = if is_hole[i] { parent[i].unwrap() } else { i };
    }
    let mut islands: Vec<(usize, Vec<Ring>)> = Vec::new();
    for i in 0..n {
        let root = find(&mut group, owner[i]);
        match islands.iter_mut().find(|(r, _)| *r == root) {
            Some((_, v)) => v.push(rings[i].clone()),
            None => islands.push((root, vec![rings[i].clone()])),
        }
    }
    // Keep reading order: left to right, top to bottom.
    islands.sort_by(|a, b| {
        let (ba, bb) = (
            BBox::of(a.1.iter().flatten()),
            BBox::of(b.1.iter().flatten()),
        );
        ba.min.x.partial_cmp(&bb.min.x).unwrap()
    });
    islands.into_iter().map(|(_, v)| v).collect()
}

/// Inside spans of the shape on one horizontal line `y`.
pub fn spans_at(rings: &[Ring], rule: FillRule, y: f64) -> Vec<(f64, f64)> {
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let p = r[i];
            let q = r[(i + 1) % n];
            if (p.y <= y && q.y > y) || (q.y <= y && p.y > y) {
                let x = p.x + (y - p.y) * (q.x - p.x) / (q.y - p.y);
                xs.push((x, if q.y > p.y { 1 } else { -1 }));
            }
        }
    }
    xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut out = Vec::new();
    let mut wind = 0i32;
    let mut from: Option<f64> = None;
    for (i, &(x, dir)) in xs.iter().enumerate() {
        let was_in = match rule {
            FillRule::NonZero => wind != 0,
            FillRule::EvenOdd => i % 2 == 1,
        };
        wind += dir;
        let now_in = match rule {
            FillRule::NonZero => wind != 0,
            FillRule::EvenOdd => (i + 1) % 2 == 1,
        };
        if !was_in && now_in {
            from = Some(x);
        } else if was_in && !now_in {
            if let Some(x0) = from.take() {
                out.push((x0, x));
            }
        }
    }
    out
}

/// Intersection of two sorted span lists.
pub fn intersect_spans(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a0, a1) in a {
        for &(b0, b1) in b {
            let (lo, hi) = (a0.max(b0), a1.min(b1));
            if hi > lo {
                out.push((lo, hi));
            }
        }
    }
    out
}

/// Remove `cut` spans from `keep` spans.
pub fn subtract_spans(keep: &[(f64, f64)], cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut cur: Vec<(f64, f64)> = keep.to_vec();
    for &(c0, c1) in cut {
        let mut next = Vec::new();
        for (k0, k1) in cur {
            if c1 <= k0 || c0 >= k1 {
                next.push((k0, k1));
            } else {
                if c0 > k0 {
                    next.push((k0, c0));
                }
                if c1 < k1 {
                    next.push((c1, k1));
                }
            }
        }
        cur = next;
    }
    cur
}

/// Area covered by upper layers, already rotated into the scan frame.
/// Each one is eroded by `overlap` so lower layers tuck under the edges.
pub struct Knockout<'a> {
    pub shapes: &'a [(Vec<Ring>, FillRule)],
    pub overlap: f64,
}

impl Knockout<'_> {
    fn cover(&self, y: f64) -> Vec<(f64, f64)> {
        let ov = self.overlap;
        let mut all = Vec::new();
        for (rings, rule) in self.shapes {
            let c = intersect_spans(
                &intersect_spans(&spans_at(rings, *rule, y - ov), &spans_at(rings, *rule, y)),
                &spans_at(rings, *rule, y + ov),
            );
            all.extend(
                c.into_iter()
                    .filter(|(a, b)| b - a > 2.0 * ov)
                    .map(|(a, b)| (a + ov, b - ov)),
            );
        }
        all
    }
}

/// Slice rings with horizontal scanlines `spacing` apart. Rows are aligned to a
/// global grid (row index * spacing) so neighbouring objects line up.
/// Areas covered by `knockout` (upper layers) are left out.
pub fn scan_ko(
    rings: &[Ring],
    rule: FillRule,
    spacing: f64,
    min_len: f64,
    ko: Option<&Knockout>,
) -> Vec<Vec<Seg>> {
    let b = BBox::of(rings.iter().flatten());
    if !b.is_valid() || spacing <= 0.0 {
        return vec![];
    }
    let first = (b.min.y / spacing).floor() as i64;
    let last = (b.max.y / spacing).ceil() as i64;
    let mut rows = Vec::new();
    for row in first..=last {
        // half-step offset avoids hitting vertices exactly
        let y = (row as f64 + 0.5) * spacing;
        if y <= b.min.y || y >= b.max.y {
            continue;
        }
        let mut spans = spans_at(rings, rule, y);
        if let Some(k) = ko {
            spans = subtract_spans(&spans, &k.cover(y));
        }
        let segs: Vec<Seg> = spans
            .into_iter()
            .filter(|(a, b)| b - a >= min_len)
            .map(|(x0, x1)| Seg { row, y, x0, x1 })
            .collect();
        if !segs.is_empty() {
            rows.push(segs);
        }
    }
    rows
}

pub fn scan(rings: &[Ring], rule: FillRule, spacing: f64, min_len: f64) -> Vec<Vec<Seg>> {
    scan_ko(rings, rule, spacing, min_len, None)
}

/// Area of the filled region (scanline integration, robust to holes and fill rules).
pub fn rings_area(rings: &[Ring], rule: FillRule) -> f64 {
    let s = 0.05;
    scan(rings, rule, s, 0.0)
        .iter()
        .flatten()
        .map(|g| g.len() * s)
        .sum()
}

/// Group the segments of consecutive rows into contiguous "columns" that can
/// each be sewn back and forth without jumping.
pub fn group_runs(rows: Vec<Vec<Seg>>) -> Vec<Vec<Seg>> {
    let mut runs: Vec<Vec<Seg>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut last_row: Option<i64> = None;
    for row in rows {
        let r = row[0].row;
        if last_row.map(|l| l + 1 != r).unwrap_or(true) {
            open.clear();
        }
        let mut used = vec![false; open.len()];
        let mut next_open = Vec::new();
        for s in row {
            let mut placed = false;
            for (k, &ri) in open.iter().enumerate() {
                if used[k] {
                    continue;
                }
                let prev = runs[ri].last().unwrap();
                if s.x0 < prev.x1 && s.x1 > prev.x0 {
                    runs[ri].push(s);
                    used[k] = true;
                    next_open.push(ri);
                    placed = true;
                    break;
                }
            }
            if !placed {
                runs.push(vec![s]);
                next_open.push(runs.len() - 1);
            }
        }
        open = next_open;
        last_row = Some(r);
    }
    runs
}

/// Dominant direction of a shape (radians), from the covariance of evenly
/// sampled boundary points. A long bar returns the angle along its length.
pub fn principal_angle(rings: &[Ring]) -> f64 {
    let mut pts = Vec::new();
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let p = r[i];
            let q = r[(i + 1) % n];
            let steps = (p.dist(q) / 0.5).ceil().max(1.0) as usize;
            for k in 0..steps {
                pts.push(p.lerp(q, k as f64 / steps as f64));
            }
        }
    }
    if pts.len() < 3 {
        return 0.0;
    }
    let n = pts.len() as f64;
    let c = pts
        .iter()
        .fold(Pt::new(0.0, 0.0), |a, p| a.add(*p))
        .mul(1.0 / n);
    let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let d = p.sub(c);
        xx += d.x * d.x;
        yy += d.y * d.y;
        xy += d.x * d.y;
    }
    0.5 * (2.0 * xy).atan2(xx - yy)
}

pub fn rotate_rings(rings: &[Ring], a: f64) -> Vec<Ring> {
    rings
        .iter()
        .map(|r| r.iter().map(|p| p.rot(a)).collect())
        .collect()
}

/// Resample an open polyline into even steps of at most `max_step`, keeping
/// sharp corners (over 30°) as needle points. Dense input gets thinned, long
/// segments get split.
pub fn resample(poly: &[Pt], max_step: f64) -> Vec<Pt> {
    let mut pts: Vec<Pt> = Vec::with_capacity(poly.len());
    for &p in poly {
        if pts.last().map(|l: &Pt| l.dist(p) > 1e-9).unwrap_or(true) {
            pts.push(p);
        }
    }
    if pts.len() < 2 {
        return pts;
    }
    // Split at sharp corners; walk each piece at even spacing.
    let mut pieces: Vec<Vec<Pt>> = vec![vec![pts[0]]];
    for i in 1..pts.len() {
        pieces.last_mut().unwrap().push(pts[i]);
        if i + 1 < pts.len() {
            let a = pts[i].sub(pts[i - 1]).norm();
            let b = pts[i + 1].sub(pts[i]).norm();
            if a.x * b.x + a.y * b.y < 30f64.to_radians().cos() {
                pieces.push(vec![pts[i]]);
            }
        }
    }
    let mut out = vec![pts[0]];
    for piece in pieces {
        let total: f64 = piece.windows(2).map(|w| w[0].dist(w[1])).sum();
        if total < 1e-9 {
            continue;
        }
        let n = (total / max_step).ceil().max(1.0) as usize;
        let step = total / n as f64;
        let mut seg = 0usize;
        let mut seg_start = 0.0;
        for k in 1..=n {
            let target = if k == n { total } else { step * k as f64 };
            while seg + 1 < piece.len() - 1 && seg_start + piece[seg].dist(piece[seg + 1]) < target
            {
                seg_start += piece[seg].dist(piece[seg + 1]);
                seg += 1;
            }
            let l = piece[seg].dist(piece[seg + 1]);
            let t = if l > 0.0 {
                ((target - seg_start) / l).clamp(0.0, 1.0)
            } else {
                1.0
            };
            out.push(piece[seg].lerp(piece[seg + 1], t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Ring {
        vec![
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ]
    }

    #[test]
    fn area_of_square_with_hole() {
        let rings = vec![rect(0.0, 0.0, 10.0, 10.0), rect(3.0, 3.0, 7.0, 7.0)];
        let a = rings_area(&rings, FillRule::EvenOdd);
        assert!((a - 84.0).abs() < 1.0, "area {a}");
    }

    #[test]
    fn nonzero_overlap_fills_once() {
        // Two overlapping same-direction squares: nonzero = union.
        let rings = vec![rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 0.0, 15.0, 10.0)];
        let a = rings_area(&rings, FillRule::NonZero);
        assert!((a - 150.0).abs() < 1.5, "area {a}");
    }

    #[test]
    fn knockout_leaves_underlap() {
        let lower = vec![rect(0.0, 0.0, 20.0, 10.0)];
        let upper = vec![(vec![rect(5.0, 0.0, 15.0, 10.0)], FillRule::NonZero)];
        let ko = Knockout {
            shapes: &upper,
            overlap: 0.5,
        };
        let rows = scan_ko(&lower, FillRule::NonZero, 0.4, 0.1, Some(&ko));
        let mid = &rows[rows.len() / 2];
        assert_eq!(mid.len(), 2);
        assert!((mid[0].x1 - 5.5).abs() < 1e-9 && (mid[1].x0 - 14.5).abs() < 1e-9);
    }

    #[test]
    fn resample_thins_dense_lines_and_keeps_corners() {
        let dense: Vec<Pt> = (0..=100)
            .map(|i| Pt::new(i as f64 * 0.1, 0.0))
            .chain((1..=100).map(|i| Pt::new(10.0, i as f64 * 0.1)))
            .collect();
        let r = resample(&dense, 2.5);
        assert_eq!(r.len(), 9, "{r:?}");
        assert!(
            r.iter().any(|p| p.dist(Pt::new(10.0, 0.0)) < 1e-9),
            "corner kept"
        );
        assert!(
            r.windows(2)
                .all(|w| w[0].dist(w[1]) <= 2.5 + 1e-9 && w[0].dist(w[1]) > 2.0)
        );
    }

    #[test]
    fn islands_keep_holes_with_their_letter() {
        // An "O" (square with hole, counter-wound) next to an "I".
        let mut hole = rect(3.0, 3.0, 7.0, 7.0);
        hole.reverse();
        let rings = vec![
            rect(0.0, 0.0, 10.0, 10.0),
            hole,
            rect(14.0, 0.0, 17.0, 10.0),
        ];
        let isl = split_islands(&rings, FillRule::NonZero);
        assert_eq!(isl.len(), 2);
        assert_eq!(isl[0].len(), 2, "O keeps its hole");
        assert_eq!(isl[1].len(), 1);
    }

    #[test]
    fn principal_angle_of_tall_bar() {
        let a = principal_angle(&[rect(0.0, 0.0, 2.0, 30.0)]);
        assert!(
            (a.abs() - std::f64::consts::FRAC_PI_2).abs() < 0.05,
            "angle {a}"
        );
    }

    #[test]
    fn hole_splits_into_runs() {
        let rings = vec![rect(0.0, 0.0, 10.0, 10.0), rect(3.0, 3.0, 7.0, 7.0)];
        let runs = group_runs(scan(&rings, FillRule::EvenOdd, 0.4, 0.1));
        assert_eq!(
            runs.len(),
            2,
            "top+left+bottom continue as one run; right side is the second"
        );
    }
}
