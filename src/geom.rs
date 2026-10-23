//! Geometry primitives: points, rings, scanline slicing.
//! All design coordinates are millimetres, x right, y down (SVG convention).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
                // Rows of a run are sewn back and forth, so both ends must
                // line up: a row that sticks far out past the one before it
                // would be reached by a long stitch across open fabric.
                let reach = (4.0 * (s.y - prev.y).abs()).max(3.5);
                if s.x0 < prev.x1
                    && s.x1 > prev.x0
                    && (s.x0 - prev.x0).abs() <= reach
                    && (s.x1 - prev.x1).abs() <= reach
                {
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

/// How elongated a shape is: ratio of its long to short spread (1.0 = no
/// clear direction, like a circle or square).
pub fn elongation(rings: &[Ring]) -> f64 {
    let mut pts = Vec::new();
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let (p, q) = (r[i], r[(i + 1) % n]);
            let steps = (p.dist(q) / 0.5).ceil().max(1.0) as usize;
            for k in 0..steps {
                pts.push(p.lerp(q, k as f64 / steps as f64));
            }
        }
    }
    if pts.len() < 3 {
        return 1.0;
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
    let tr = xx + yy;
    let det = xx * yy - xy * xy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(1e-12));
    (l1 / l2).sqrt()
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
        // top band, the two sides of the hole and the bottom band: a run never
        // turns around with a long stitch from one row end to the next
        assert_eq!(runs.len(), 4);
        for run in &runs {
            for w in run.windows(2) {
                assert!((w[0].x0 - w[1].x0).abs() <= 3.5 && (w[0].x1 - w[1].x1).abs() <= 3.5);
            }
        }
    }
}

// ---------------------------------------------------------------- vector paths

/// One anchor of an editable vector path, with its two Bézier handles
/// (equal to the anchor for a sharp corner or straight line).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub p: Pt,
    /// Handle on the side coming into this anchor.
    pub cin: Pt,
    /// Handle on the side going out of this anchor.
    pub cout: Pt,
}

impl Node {
    pub fn corner(p: Pt) -> Node {
        Node { p, cin: p, cout: p }
    }
    /// Handles in line with each other: moving one turns the other.
    pub fn is_smooth(&self) -> bool {
        let (a, b) = (self.cin.sub(self.p), self.cout.sub(self.p));
        a.len() > 1e-6 && b.len() > 1e-6 && (a.x * b.y - a.y * b.x).abs() < 0.05 * a.len() * b.len()
            && (a.x * b.x + a.y * b.y) < 0.0
    }
}

/// An editable vector path: the "vector representation" of a design
/// object. Stitches are generated from it; its flattened polyline is what
/// the stitch engine reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VPath {
    pub nodes: Vec<Node>,
    pub closed: bool,
}

/// Points along a cubic from `a` to `b`, excluding `a`.
pub fn flatten_cubic(a: &Node, b: &Node, out: &mut Vec<Pt>) {
    let (p0, p1, p2, p3) = (a.p, a.cout, b.cin, b.p);
    if p1.dist(p0) < 1e-9 && p2.dist(p3) < 1e-9 {
        out.push(p3);
        return;
    }
    let n = ((p0.dist(p1) + p1.dist(p2) + p2.dist(p3)) / 0.25)
        .ceil()
        .clamp(2.0, 96.0) as usize;
    for k in 1..=n {
        out.push(cubic_at(p0, p1, p2, p3, k as f64 / n as f64));
    }
}

pub fn cubic_at(p0: Pt, p1: Pt, p2: Pt, p3: Pt, u: f64) -> Pt {
    let a1 = p0.lerp(p1, u);
    let b1 = p1.lerp(p2, u);
    let c1 = p2.lerp(p3, u);
    a1.lerp(b1, u).lerp(b1.lerp(c1, u), u)
}

impl VPath {
    /// Polyline for stitching. Closed paths end back at the start.
    pub fn flatten(&self) -> Vec<Pt> {
        let mut out = Vec::new();
        let Some(first) = self.nodes.first() else {
            return out;
        };
        out.push(first.p);
        for w in self.nodes.windows(2) {
            flatten_cubic(&w[0], &w[1], &mut out);
        }
        if self.closed && self.nodes.len() > 1 {
            flatten_cubic(self.nodes.last().unwrap(), first, &mut out);
        }
        out.dedup_by(|a, b| a.dist(*b) < 1e-6);
        out
    }

    /// Number of segments (a closed path has one more, back to the start).
    pub fn segments(&self) -> usize {
        let n = self.nodes.len();
        if self.closed { n } else { n.saturating_sub(1) }
    }

    /// Split segment `seg` at `u` (0..1), adding an anchor there.
    pub fn split(&mut self, seg: usize, u: f64) {
        let n = self.nodes.len();
        let (i, j) = (seg, (seg + 1) % n);
        let (a, b) = (self.nodes[i], self.nodes[j]);
        let (p0, p1, p2, p3) = (a.p, a.cout, b.cin, b.p);
        let q0 = p0.lerp(p1, u);
        let q1 = p1.lerp(p2, u);
        let q2 = p2.lerp(p3, u);
        let r0 = q0.lerp(q1, u);
        let r1 = q1.lerp(q2, u);
        let s = r0.lerp(r1, u);
        self.nodes[i].cout = q0;
        self.nodes[j].cin = q2;
        let node = Node { p: s, cin: r0, cout: r1 };
        if j == 0 {
            self.nodes.push(node);
        } else {
            self.nodes.insert(j, node);
        }
    }

    /// Nearest point on the path: (segment, u, distance).
    pub fn nearest(&self, q: Pt) -> Option<(usize, f64, f64)> {
        let n = self.nodes.len();
        let mut best: Option<(usize, f64, f64)> = None;
        for s in 0..self.segments() {
            let (a, b) = (self.nodes[s], self.nodes[(s + 1) % n]);
            for k in 0..=40 {
                let u = k as f64 / 40.0;
                let d = cubic_at(a.p, a.cout, b.cin, b.p, u).dist(q);
                if best.map(|x| d < x.2).unwrap_or(true) {
                    best = Some((s, u, d));
                }
            }
        }
        best
    }

    /// A path from a polyline, simplified to the anchors that matter.
    pub fn from_polyline(pts: &[Pt], closed: bool, tol: f64) -> VPath {
        let keep = simplify(pts, tol);
        let mut nodes: Vec<Node> = keep.into_iter().map(Node::corner).collect();
        if closed && nodes.len() > 1 && nodes[0].p.dist(nodes.last().unwrap().p) < 1e-6 {
            nodes.pop();
        }
        VPath { nodes, closed }
    }
}

/// Ramer–Douglas–Peucker simplification.
pub fn simplify(pts: &[Pt], tol: f64) -> Vec<Pt> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (pts[a], pts[b]);
        let d = pb.sub(pa);
        let l = d.len();
        let mut far = (0.0, a);
        for (k, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let e = if l < 1e-12 {
                p.dist(pa)
            } else {
                ((p.x - pa.x) * d.y - (p.y - pa.y) * d.x).abs() / l
            };
            if e > far.0 {
                far = (e, k);
            }
        }
        if far.0 > tol {
            keep[far.1] = true;
            stack.push((a, far.1));
            stack.push((far.1, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

/// Is point `p` inside the closed ring `r` (even-odd)?
pub fn in_ring(r: &[Pt], p: Pt) -> bool {
    let mut inside = false;
    let n = r.len();
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x > p.x {
                inside = !inside;
            }
        }
    }
    inside
}

/// The outer outlines only (holes dropped): rings not inside another ring.
pub fn outer_rings(rings: &[Ring]) -> Vec<Ring> {
    rings
        .iter()
        .enumerate()
        .filter(|(i, r)| {
            let Some(&p) = r.first() else { return false };
            let depth = rings
                .iter()
                .enumerate()
                .filter(|(j, o)| j != i && o.len() >= 3 && in_ring(o, p))
                .count();
            depth % 2 == 0
        })
        .map(|(_, r)| r.clone())
        .collect()
}

// ---------------------------------------------------------------- curve fitting

fn bez_at(b: &[Pt; 4], t: f64) -> Pt {
    cubic_at(b[0], b[1], b[2], b[3], t)
}

fn bez_d1(b: &[Pt; 4], t: f64) -> Pt {
    let mt = 1.0 - t;
    b[1].sub(b[0])
        .mul(3.0 * mt * mt)
        .add(b[2].sub(b[1]).mul(6.0 * mt * t))
        .add(b[3].sub(b[2]).mul(3.0 * t * t))
}

fn bez_d2(b: &[Pt; 4], t: f64) -> Pt {
    let mt = 1.0 - t;
    b[2].sub(b[1].mul(2.0)).add(b[0])
        .mul(6.0 * mt)
        .add(b[3].sub(b[2].mul(2.0)).add(b[1]).mul(6.0 * t))
}

fn dot(a: Pt, b: Pt) -> f64 {
    a.x * b.x + a.y * b.y
}

/// Least-squares cubic through `pts` with fixed end tangents (`t1` leaving
/// the start, `t2` pointing back from the end).
fn fit_one(pts: &[Pt], u: &[f64], t1: Pt, t2: Pt) -> [Pt; 4] {
    let (p0, pn) = (pts[0], *pts.last().unwrap());
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let mt = 1.0 - t;
        let (b0, b1, b2, b3) = (mt * mt * mt, 3.0 * t * mt * mt, 3.0 * t * t * mt, t * t * t);
        let (a1, a2) = (t1.mul(b1), t2.mul(b2));
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        let tmp = p.sub(p0.mul(b0 + b1).add(pn.mul(b2 + b3)));
        x0 += dot(a1, tmp);
        x1 += dot(a2, tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg = p0.dist(pn);
    let (mut al, mut ar) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (seg / 3.0, seg / 3.0)
    };
    if al < seg * 1e-6 || ar < seg * 1e-6 || al > seg * 3.0 || ar > seg * 3.0 {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    [p0, p0.add(t1.mul(al)), pn.add(t2.mul(ar)), pn]
}

fn fit_rec(pts: &[Pt], t1: Pt, t2: Pt, tol: f64, out: &mut Vec<[Pt; 4]>, depth: usize) {
    let n = pts.len();
    if n == 2 {
        let d = pts[0].dist(pts[1]) / 3.0;
        out.push([pts[0], pts[0].add(t1.mul(d)), pts[1].add(t2.mul(d)), pts[1]]);
        return;
    }
    // chord-length parameters
    let mut u = vec![0.0; n];
    for i in 1..n {
        u[i] = u[i - 1] + pts[i].dist(pts[i - 1]);
    }
    let total = u[n - 1].max(1e-12);
    u.iter_mut().for_each(|x| *x /= total);
    let mut bez = fit_one(pts, &u, t1, t2);
    let err = |b: &[Pt; 4], u: &[f64]| -> (f64, usize) {
        let mut m = (0.0, n / 2);
        for i in 1..n - 1 {
            let d = bez_at(b, u[i]).dist(pts[i]);
            if d > m.0 {
                m = (d, i);
            }
        }
        m
    };
    let (mut e, mut split) = err(&bez, &u);
    if e <= tol {
        out.push(bez);
        return;
    }
    if e < tol * 6.0 {
        for _ in 0..6 {
            for i in 0..n {
                let q = bez_at(&bez, u[i]).sub(pts[i]);
                let d1 = bez_d1(&bez, u[i]);
                let d2 = bez_d2(&bez, u[i]);
                let den = dot(d1, d1) + dot(q, d2);
                if den.abs() > 1e-12 {
                    u[i] = (u[i] - dot(q, d1) / den).clamp(0.0, 1.0);
                }
            }
            bez = fit_one(pts, &u, t1, t2);
            (e, split) = err(&bez, &u);
            if e <= tol {
                out.push(bez);
                return;
            }
        }
    }
    if depth > 40 {
        out.push(bez);
        return;
    }
    let split = split.clamp(1, n - 2);
    let tc = pts[split - 1].sub(pts[split + 1]);
    let tc = if tc.len() < 1e-12 { t2 } else { tc.norm() };
    fit_rec(&pts[..=split], t1, tc, tol, out, depth + 1);
    fit_rec(&pts[split..], tc.mul(-1.0), t2, tol, out, depth + 1);
}

/// Smooth Bézier path through a dense polyline: anchors at real corners,
/// curves fitted (within `tol` mm) everywhere else. Turns traced or
/// computed outlines (booleans, offsets, text) into paths with few anchors.
pub fn fit_path(points: &[Pt], closed: bool, tol: f64) -> VPath {
    let mut src = points.to_vec();
    src.dedup_by(|a, b| a.dist(*b) < 1e-6);
    if closed && src.len() > 1 && src[0].dist(*src.last().unwrap()) < 1e-6 {
        src.pop();
    }
    if src.len() < 3 {
        return VPath::from_polyline(&src, closed, tol);
    }
    // even spacing so corner tests see the same scale everywhere
    let mut ring = src.clone();
    if closed {
        ring.push(src[0]);
    }
    let mut pts = resample(&ring, 0.15);
    if closed {
        pts.pop();
        while pts.len() > 3 && pts[0].dist(*pts.last().unwrap()) < 0.075 {
            pts.pop();
        }
        // start the loop on its straightest stretch, so a corner never sits
        // on the seam and gets counted twice
        let m = pts.len();
        if m > 8 {
            let straight = |i: usize| -> f64 {
                let (a, b, c) = (pts[(i + m - 3) % m], pts[i], pts[(i + 3) % m]);
                let (u, v) = (b.sub(a), c.sub(b));
                if u.len() < 1e-9 || v.len() < 1e-9 {
                    return 0.0;
                }
                dot(u.norm(), v.norm())
            };
            let best = (0..m)
                .max_by(|&i, &j| straight(i).partial_cmp(&straight(j)).unwrap())
                .unwrap_or(0);
            pts.rotate_left(best);
        }
    }
    let n = pts.len();
    if n < 4 {
        return VPath::from_polyline(&src, closed, tol);
    }
    let at = |i: isize| -> Pt {
        if closed {
            pts[i.rem_euclid(n as isize) as usize]
        } else {
            pts[i.clamp(0, n as isize - 1) as usize]
        }
    };
    // corners: direction changes of more than ~35° over ±0.45 mm
    let k = 3isize;
    let turn = |i: usize| -> f64 {
        let i = i as isize;
        if !closed && (i < k || i >= n as isize - k) {
            return 0.0;
        }
        let a = at(i).sub(at(i - k));
        let b = at(i + k).sub(at(i));
        if a.len() < 1e-9 || b.len() < 1e-9 {
            return 0.0;
        }
        dot(a.norm(), b.norm()).clamp(-1.0, 1.0).acos()
    };
    let turns: Vec<f64> = (0..n).map(turn).collect();
    let mut corners: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < n {
        if turns[i] > 35f64.to_radians() {
            let mut j = i;
            let mut best = i;
            while j < n && turns[j] > 35f64.to_radians() {
                if turns[j] > turns[best] {
                    best = j;
                }
                j += 1;
            }
            corners.push(best);
            i = j;
        } else {
            i += 1;
        }
    }
    // break points: corners (sharp) plus the open ends, or for a smooth
    // closed loop, two opposite points (smooth joins)
    let mut breaks: Vec<(usize, bool)> = corners.iter().map(|&c| (c, true)).collect();
    if !closed {
        breaks.retain(|b| b.0 > 0 && b.0 < n - 1);
        breaks.insert(0, (0, true));
        breaks.push((n - 1, true));
    } else if breaks.is_empty() {
        breaks = vec![(0, false), (n / 2, false)];
    } else if breaks.len() == 1 {
        let c = breaks[0].0;
        breaks.push(((c + n / 2) % n, false));
        breaks.sort();
    }
    let tangent_out = |i: usize, sharp: bool| -> Pt {
        let i = i as isize;
        let d = if sharp { at(i + 2).sub(at(i)) } else { at(i + 1).sub(at(i - 1)) };
        if d.len() < 1e-12 { Pt::new(1.0, 0.0) } else { d.norm() }
    };
    let tangent_in = |i: usize, sharp: bool| -> Pt {
        let ii = i as isize;
        let d = if sharp { at(ii - 2).sub(at(ii)) } else { at(ii - 1).sub(at(ii + 1)) };
        if d.len() < 1e-12 { Pt::new(-1.0, 0.0) } else { d.norm() }
    };
    let mut nodes: Vec<Node> = Vec::new();
    let runs = if closed { breaks.len() } else { breaks.len() - 1 };
    for r in 0..runs {
        let (s, s_sharp) = breaks[r];
        let (e, e_sharp) = breaks[(r + 1) % breaks.len()];
        let seg: Vec<Pt> = if e > s {
            pts[s..=e].to_vec()
        } else {
            pts[s..].iter().chain(pts[..=e].iter()).copied().collect()
        };
        if seg.len() < 2 {
            continue;
        }
        let mut bez = Vec::new();
        fit_rec(&seg, tangent_out(s, s_sharp), tangent_in(e, e_sharp), tol, &mut bez, 0);
        for (bi, b) in bez.iter().enumerate() {
            if bi == 0 {
                if let Some(last) = nodes.last_mut().filter(|l| l.p.dist(b[0]) < 1e-9) {
                    last.cout = b[1];
                } else {
                    nodes.push(Node { p: b[0], cin: b[0], cout: b[1] });
                }
            } else {
                nodes.last_mut().unwrap().cout = b[1];
            }
            nodes.push(Node { p: b[3], cin: b[2], cout: b[3] });
        }
    }
    if closed && nodes.len() > 1 && nodes[0].p.dist(nodes.last().unwrap().p) < 1e-9 {
        let last = nodes.pop().unwrap();
        nodes[0].cin = last.cin;
    }
    VPath { nodes, closed }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn circle_fits_with_few_anchors() {
        let ring: Vec<Pt> = (0..360)
            .map(|k| {
                let a = (k as f64).to_radians();
                Pt::new(20.0 * a.cos(), 20.0 * a.sin())
            })
            .collect();
        let v = fit_path(&ring, true, 0.05);
        assert!(v.nodes.len() <= 8, "{} anchors", v.nodes.len());
        // stays on the circle
        for p in v.flatten() {
            assert!((p.len() - 20.0).abs() < 0.12, "{}", p.len());
        }
    }

    #[test]
    fn square_keeps_its_corners() {
        let sq = vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0), Pt::new(0.0, 10.0)];
        let v = fit_path(&sq, true, 0.05);
        assert_eq!(v.nodes.len(), 4, "{:?}", v.nodes.iter().map(|n| n.p).collect::<Vec<_>>());
    }

    #[test]
    fn outer_rings_drop_holes() {
        let outer = vec![Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0), Pt::new(0.0, 10.0)];
        let hole = vec![Pt::new(3.0, 3.0), Pt::new(7.0, 3.0), Pt::new(7.0, 7.0), Pt::new(3.0, 7.0)];
        assert_eq!(outer_rings(&[outer, hole]).len(), 1);
    }
}
