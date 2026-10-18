//! Cutting and erasing: the geometry behind the Knife and Eraser tools and
//! the stitch clean-up commands. Everything here works in design mm.

use crate::geom::{split_islands, FillRule, Pt, Ring};
use crate::stitch::Needle;
use crate::vector::{self, BoolOp};

/// Point in a simple polygon (even-odd).
pub fn point_in_poly(poly: &[Pt], p: Pt) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

fn inside_rings(rings: &[Ring], rule: FillRule, p: Pt) -> bool {
    let mut wind = 0i32;
    let mut odd = false;
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) {
                let x = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
                if p.x < x {
                    odd = !odd;
                    wind += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
    }
    match rule {
        FillRule::EvenOdd => odd,
        FillRule::NonZero => wind != 0,
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
    p.dist(a.lerp(b, t))
}

fn edge_dist(rings: &[Ring], p: Pt) -> f64 {
    let mut d = f64::MAX;
    for r in rings {
        for i in 0..r.len() {
            d = d.min(seg_dist(p, r[i], r[(i + 1) % r.len()]));
        }
    }
    d
}

/// Which side of the line a→b a point is on (true = left).
fn left_of(a: Pt, b: Pt, p: Pt) -> bool {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) > 0.0
}

fn centroid(rings: &[Ring]) -> Pt {
    let (mut x, mut y, mut n) = (0.0, 0.0, 0.0);
    for p in rings.iter().flatten() {
        x += p.x;
        y += p.y;
        n += 1.0;
    }
    if n == 0.0 { Pt::new(0.0, 0.0) } else { Pt::new(x / n, y / n) }
}

/// Width of the knife cut (mm): small enough that pull compensation closes
/// it, so the two pieces sew edge to edge.
pub const KERF: f64 = 0.1;

/// Cut an area along the segment a→b. Returns the pieces on each side of
/// the line, or None if the cut didn't separate anything.
pub fn knife_rings(rings: &[Ring], rule: FillRule, a: Pt, b: Pt) -> Option<(Vec<Ring>, Vec<Ring>)> {
    let d = b.sub(a);
    let len = d.len();
    if len < 0.5 {
        return None;
    }
    let u = d.mul(1.0 / len);
    let n = Pt::new(-u.y, u.x).mul(KERF / 2.0);
    let (a0, b0) = (a.sub(u.mul(0.2)), b.add(u.mul(0.2)));
    let strip = vec![a0.add(n), b0.add(n), b0.sub(n), a0.sub(n)];
    let before = split_islands(rings, rule).len();
    let cut = vector::boolean(&[(rings.to_vec(), rule), (vec![strip], FillRule::NonZero)], BoolOp::Subtract);
    let islands = split_islands(&cut, FillRule::EvenOdd);
    if islands.len() <= before {
        return None;
    }
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for isl in islands {
        if left_of(a, b, centroid(&isl)) {
            l.extend(isl);
        } else {
            r.extend(isl);
        }
    }
    if l.is_empty() || r.is_empty() {
        return None;
    }
    Some((l, r))
}

/// Remove the area inside `lasso` from an area. Returns what is left (empty
/// if nothing is).
pub fn erase_rings(rings: &[Ring], rule: FillRule, lasso: &[Pt]) -> Vec<Ring> {
    vector::boolean(&[(rings.to_vec(), rule), (vec![lasso.to_vec()], FillRule::NonZero)], BoolOp::Subtract)
}

/// Does the lasso touch this area at all?
pub fn lasso_touches(rings: &[Ring], rule: FillRule, lasso: &[Pt]) -> bool {
    !vector::boolean(&[(rings.to_vec(), rule), (vec![lasso.to_vec()], FillRule::NonZero)], BoolOp::Intersect).is_empty()
}

fn resample(path: &[Pt], step: f64) -> Vec<Pt> {
    let mut out = Vec::new();
    for w in path.windows(2) {
        let n = (w[0].dist(w[1]) / step).ceil().max(1.0) as usize;
        for k in 0..n {
            out.push(w[0].lerp(w[1], k as f64 / n as f64));
        }
    }
    if let Some(l) = path.last() {
        out.push(*l);
    }
    out
}

/// Remove the parts of open paths (strokes, runs) inside the lasso.
pub fn erase_paths(paths: &[Vec<Pt>], lasso: &[Pt]) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    for path in paths {
        let mut run: Vec<Pt> = Vec::new();
        for p in resample(path, 0.25) {
            if point_in_poly(lasso, p) {
                if run.len() >= 2 {
                    out.push(crate::geom::simplify(&run, 0.03));
                }
                run.clear();
            } else {
                run.push(p);
            }
        }
        if run.len() >= 2 {
            out.push(crate::geom::simplify(&run, 0.03));
        }
    }
    out.retain(|p| p.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() >= 0.5);
    out
}

/// Where segment p→q crosses segment a→b (as a fraction along p→q).
fn cross_at(p: Pt, q: Pt, a: Pt, b: Pt) -> Option<f64> {
    let r = q.sub(p);
    let s = b.sub(a);
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-12 {
        return None;
    }
    let ap = a.sub(p);
    let t = (ap.x * s.y - ap.y * s.x) / den;
    let u = (ap.x * r.y - ap.y * r.x) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// Cut open paths where they cross the segment a→b. Returns the pieces on
/// each side, or None if nothing was crossed.
pub fn knife_paths(paths: &[Vec<Pt>], a: Pt, b: Pt) -> Option<(Vec<Vec<Pt>>, Vec<Vec<Pt>>)> {
    let mut pieces: Vec<Vec<Pt>> = Vec::new();
    let mut crossed = false;
    for path in paths {
        let mut cur: Vec<Pt> = path.first().copied().into_iter().collect();
        for w in path.windows(2) {
            if let Some(t) = cross_at(w[0], w[1], a, b) {
                let x = w[0].lerp(w[1], t);
                cur.push(x);
                if cur.len() >= 2 {
                    pieces.push(std::mem::take(&mut cur));
                }
                cur = vec![x];
                crossed = true;
            }
            cur.push(w[1]);
        }
        if cur.len() >= 2 {
            pieces.push(cur);
        }
    }
    if !crossed {
        return None;
    }
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for p in pieces {
        let len: f64 = p.windows(2).map(|w| w[0].dist(w[1])).sum();
        if len < 0.2 {
            continue;
        }
        let mid = p[p.len() / 2].lerp(p[(p.len() - 1) / 2], 0.5);
        if left_of(a, b, mid) { l.push(p) } else { r.push(p) }
    }
    if l.is_empty() || r.is_empty() {
        return None;
    }
    Some((l, r))
}

/// Needle points that sit outside the shape by more than `tol` mm (pull
/// compensation puts good ones just past the edge).
pub fn strays(needles: &[Needle], rings: &[Ring], rule: FillRule, tol: f64) -> Vec<usize> {
    (0..needles.len())
        .filter(|&i| {
            let p = needles[i].p;
            !inside_rings(rings, rule, p) && edge_dist(rings, p) > tol
        })
        .collect()
}

/// Stitches (by the index of their end point) that run outside the shape
/// between two points that are on it: they bridge a gap or a notch.
pub fn bridges(needles: &[Needle], rings: &[Ring], rule: FillRule, tol: f64) -> Vec<usize> {
    let out_pt = |q: Pt| !inside_rings(rings, rule, q) && edge_dist(rings, q) > tol;
    (1..needles.len())
        .filter(|&i| {
            if needles[i].jump {
                return false;
            }
            let (a, p) = (needles[i - 1].p, needles[i].p);
            let n = (a.dist(p) / 0.4).ceil() as usize;
            (1..n).any(|k| out_pt(a.lerp(p, k as f64 / n as f64)))
        })
        .collect()
}

/// Clean up hand-edited or imported stitches against their shape: drop the
/// points that are off it, and turn stitches that would still cross open
/// fabric into jumps. Returns (stitches, points removed, stitches bridged).
pub fn clean_strays(needles: &[Needle], rings: &[Ring], rule: FillRule, tol: f64) -> (Vec<Needle>, usize, usize) {
    let off = strays(needles, rings, rule, tol);
    let mut kept = crate::project::remove_needles(needles, |i| off.binary_search(&i).is_ok());
    let br = bridges(&kept, rings, rule, tol);
    for &i in &br {
        kept[i].jump = true;
    }
    (kept, off.len(), br.len())
}

/// Needle points that make a stitch shorter than `min` mm (thread breaks,
/// bird nests). The first of each pair of crowded points is kept.
pub fn tiny(needles: &[Needle], min: f64) -> Vec<usize> {
    let mut out = Vec::new();
    let mut last: Option<Pt> = None;
    for (i, n) in needles.iter().enumerate() {
        match last {
            Some(l) if !n.jump && l.dist(n.p) < min => out.push(i),
            _ => last = Some(n.p),
        }
    }
    out
}

/// Split stitches longer than `max` mm into equal parts (machines cap a
/// single stitch, and long loose stitches snag).
pub fn split_long(needles: &[Needle], max: f64) -> (Vec<Needle>, usize) {
    let mut out: Vec<Needle> = Vec::with_capacity(needles.len());
    let mut added = 0;
    for n in needles {
        if let (Some(prev), false) = (out.last().copied(), n.jump) {
            let d = prev.p.dist(n.p);
            if d > max {
                let k = (d / max).ceil() as usize;
                for j in 1..k {
                    out.push(Needle { p: prev.p.lerp(n.p, j as f64 / k as f64), jump: false });
                    added += 1;
                }
            }
        }
        out.push(*n);
    }
    (out, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Ring {
        vec![Pt::new(x0, y0), Pt::new(x1, y0), Pt::new(x1, y1), Pt::new(x0, y1)]
    }
    fn area(rings: &[Ring]) -> f64 {
        crate::geom::rings_area(rings, FillRule::EvenOdd)
    }

    #[test]
    fn knife_cuts_a_square_in_two() {
        let sq = vec![rect(0.0, 0.0, 20.0, 10.0)];
        let (l, r) = knife_rings(&sq, FillRule::NonZero, Pt::new(10.0, -5.0), Pt::new(10.0, 15.0)).unwrap();
        assert!((area(&l) - 100.0).abs() < 1.0 && (area(&r) - 100.0).abs() < 1.0, "{} {}", area(&l), area(&r));
        // a line that misses, or only nicks the edge, cuts nothing
        assert!(knife_rings(&sq, FillRule::NonZero, Pt::new(30.0, 0.0), Pt::new(30.0, 10.0)).is_none());
        assert!(knife_rings(&sq, FillRule::NonZero, Pt::new(10.0, -5.0), Pt::new(10.0, 3.0)).is_none());
    }

    #[test]
    fn eraser_takes_a_bite() {
        let sq = vec![rect(0.0, 0.0, 20.0, 10.0)];
        let left = erase_rings(&sq, FillRule::NonZero, &rect(15.0, -5.0, 30.0, 15.0));
        assert!((area(&left) - 150.0).abs() < 1.0);
        assert!(erase_rings(&sq, FillRule::NonZero, &rect(-5.0, -5.0, 30.0, 15.0)).is_empty());
    }

    #[test]
    fn eraser_and_knife_on_strokes() {
        let path = vec![vec![Pt::new(0.0, 0.0), Pt::new(20.0, 0.0)]];
        let left = erase_paths(&path, &rect(5.0, -1.0, 10.0, 1.0));
        assert_eq!(left.len(), 2);
        let (a, b) = knife_paths(&path, Pt::new(8.0, -3.0), Pt::new(8.0, 3.0)).unwrap();
        assert_eq!((a.len(), b.len()), (1, 1));
    }

    #[test]
    fn cleanups_find_strays_and_tiny_stitches() {
        let sq = vec![rect(0.0, 0.0, 10.0, 10.0)];
        let n = |x: f64, y: f64, jump: bool| Needle { p: Pt::new(x, y), jump };
        let st = vec![n(1.0, 1.0, true), n(5.0, 1.0, false), n(5.0, 15.0, false), n(6.0, 2.0, false), n(6.05, 2.0, false), n(20.0, 20.0, true)];
        assert_eq!(strays(&st, &sq, FillRule::NonZero, 0.5), vec![2, 5]);
        let (clean, gone, bridged) = clean_strays(&st, &sq, FillRule::NonZero, 0.5);
        assert_eq!((clean.len(), gone, bridged), (4, 2, 0));
        // a stitch across a notch between two good points becomes a jump
        let u = vec![rect(0.0, 0.0, 10.0, 10.0), rect(4.0, 0.0, 6.0, 8.0)];
        let across = vec![n(1.0, 2.0, true), n(9.0, 2.0, false)];
        let (c, _, b) = clean_strays(&across, &u, FillRule::EvenOdd, 0.5);
        assert!(b == 1 && c[1].jump);
        assert_eq!(tiny(&st, 0.3), vec![4]);
        let (long, added) = split_long(&[n(0.0, 0.0, true), n(25.0, 0.0, false)], 12.0);
        assert_eq!((long.len(), added), (4, 2));
    }
}
