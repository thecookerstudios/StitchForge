//! Guided fills: stitch rows that turn to follow direction lines the
//! digitizer draws across a shape.
//!
//! Each guide line says "stitches run this way here". The direction at any
//! point is a blend of the guides, nearer ones counting more (directions are
//! blended as doubled angles, so a guide drawn either way round means the
//! same). Rows are then the level lines of a smooth function whose gradient
//! is as close as possible to the row normal everywhere — so rows stay
//! evenly spaced where the direction is constant and fan gently where it
//! turns, with no seams between differently angled parts.

use crate::geom::{FillRule, Pt, Ring};
use std::collections::HashMap;

/// A direction guide (design mm): stitches run along a→b near it.
pub type Guide = (Pt, Pt);

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
    p.dist(a.lerp(b, t))
}

/// Stitch direction (radians, 0..π) at a point, blended from the guides.
pub fn direction_at(p: Pt, guides: &[Guide]) -> f64 {
    let (mut c, mut s) = (0.0, 0.0);
    for &(a, b) in guides {
        let ang = (b.y - a.y).atan2(b.x - a.x);
        let d = seg_dist(p, a, b);
        // nearer guides dominate; 3 mm keeps the blend smooth right at a line
        let w = 1.0 / (d * d + 9.0);
        c += w * (2.0 * ang).cos();
        s += w * (2.0 * ang).sin();
    }
    0.5 * s.atan2(c)
}

struct Grid {
    x0: f64,
    y0: f64,
    h: f64,
    nx: usize,
    inside: Vec<bool>,
}

impl Grid {
    fn at(&self, i: usize, j: usize) -> Pt {
        Pt::new(self.x0 + (i as f64 + 0.5) * self.h, self.y0 + (j as f64 + 0.5) * self.h)
    }
    fn idx(&self, i: usize, j: usize) -> usize {
        j * self.nx + i
    }
}

/// Rows for a guided fill: (row number, polyline in design mm). Row ends
/// sit on the shape's edge. `keep` says which points are to be stitched
/// (inside the shape and not hidden under upper layers).
pub fn guided_rows(
    rings: &[Ring],
    _rule: FillRule,
    keep: &dyn Fn(Pt) -> bool,
    guides: &[Guide],
    spacing: f64,
) -> Vec<(i64, Vec<Pt>)> {
    if guides.is_empty() || rings.is_empty() {
        return vec![];
    }
    // ---- grid over the shape
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in rings.iter().flatten() {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let span = (x1 - x0).max(y1 - y0).max(1.0);
    // fine enough to follow the shape, coarse enough to stay quick
    let h = (span / 240.0).clamp(0.15, 0.6).min(spacing * 1.5);
    let nx = ((x1 - x0) / h).ceil() as usize + 2;
    let ny = ((y1 - y0) / h).ceil() as usize + 2;
    let mut g = Grid { x0: x0 - h, y0: y0 - h, h, nx, inside: vec![false; nx * ny] };
    for j in 0..ny {
        for i in 0..nx {
            let p = g.at(i, j);
            let k = g.idx(i, j);
            g.inside[k] = keep(p);
        }
    }
    let n_in = g.inside.iter().filter(|b| **b).count();
    if n_in < 4 {
        return vec![];
    }

    // ---- row normals, made to agree in sign across the shape
    let mut nrm = vec![Pt::new(0.0, 0.0); nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let k = g.idx(i, j);
            if g.inside[k] {
                let t = direction_at(g.at(i, j), guides);
                nrm[k] = Pt::new(-t.sin(), t.cos());
            }
        }
    }
    let mut seen = vec![false; nx * ny];
    for start in 0..nx * ny {
        if !g.inside[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(k) = queue.pop_front() {
            let (i, j) = (k % nx, k / nx);
            let nb = [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)];
            for (a, b) in nb {
                if a >= nx || b >= ny {
                    continue;
                }
                let q = g.idx(a, b);
                if g.inside[q] && !seen[q] {
                    if nrm[q].x * nrm[k].x + nrm[q].y * nrm[k].y < 0.0 {
                        nrm[q] = nrm[q].mul(-1.0);
                    }
                    seen[q] = true;
                    queue.push_back(q);
                }
            }
        }
    }

    // ---- potential whose gradient follows the normals (least squares,
    // solved by over-relaxed Gauss–Seidel from a straight-rows start)
    let mean = {
        let (mut sx, mut sy) = (0.0, 0.0);
        for k in 0..nx * ny {
            if g.inside[k] {
                sx += nrm[k].x;
                sy += nrm[k].y;
            }
        }
        let l = (sx * sx + sy * sy).sqrt().max(1e-9);
        Pt::new(sx / l, sy / l)
    };
    let mut phi = vec![0.0f64; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let k = g.idx(i, j);
            if g.inside[k] {
                let p = g.at(i, j);
                phi[k] = p.x * mean.x + p.y * mean.y;
            }
        }
    }
    let iters = 60 + (nx.max(ny) * 3 / 2).min(420);
    let omega = 1.85;
    for _ in 0..iters {
        let mut change = 0.0f64;
        for j in 0..ny {
            for i in 0..nx {
                let k = g.idx(i, j);
                if !g.inside[k] {
                    continue;
                }
                let mut sum = 0.0;
                let mut deg = 0.0;
                // neighbour q in direction e: phi_q - phi_k ≈ h * n·e
                let mut add = |q: usize, ex: f64, ey: f64| {
                    if g.inside[q] {
                        let nx_ = (nrm[k].x + nrm[q].x) * 0.5;
                        let ny_ = (nrm[k].y + nrm[q].y) * 0.5;
                        let gq = h * (nx_ * ex + ny_ * ey);
                        sum += phi[q] - gq;
                        deg += 1.0;
                    }
                };
                if i > 0 { add(k - 1, -1.0, 0.0); }
                if i + 1 < nx { add(k + 1, 1.0, 0.0); }
                if j > 0 { add(k - nx, 0.0, -1.0); }
                if j + 1 < ny { add(k + nx, 0.0, 1.0); }
                if deg > 0.0 {
                    let target = sum / deg;
                    let d = omega * (target - phi[k]);
                    phi[k] += d;
                    change = change.max(d.abs());
                }
            }
        }
        if change < h * 1e-4 {
            break;
        }
    }

    // ---- level lines (marching squares over cells whose 4 corners are in)
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for k in 0..nx * ny {
        if g.inside[k] {
            lo = lo.min(phi[k]);
            hi = hi.max(phi[k]);
        }
    }
    let first = (lo / spacing).ceil() as i64;
    let last = (hi / spacing).floor() as i64;
    // edge key: the two grid nodes it joins (smaller index first)
    type Key = (usize, usize);
    let mut segs: HashMap<i64, Vec<(Key, Key, Pt, Pt)>> = HashMap::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let c = [g.idx(i, j), g.idx(i + 1, j), g.idx(i + 1, j + 1), g.idx(i, j + 1)];
            if !c.iter().all(|&k| g.inside[k]) {
                continue;
            }
            let v = c.map(|k| phi[k]);
            let vmin = v.iter().cloned().fold(f64::MAX, f64::min);
            let vmax = v.iter().cloned().fold(f64::MIN, f64::max);
            let l0 = (vmin / spacing).ceil() as i64;
            let l1 = (vmax / spacing).floor() as i64;
            let pos = [g.at(i, j), g.at(i + 1, j), g.at(i + 1, j + 1), g.at(i, j + 1)];
            for lv in l0.max(first)..=l1.min(last) {
                let t = lv as f64 * spacing;
                let mut cross: Vec<(Key, Pt)> = Vec::new();
                for e in 0..4 {
                    let (a, b) = (e, (e + 1) % 4);
                    let (va, vb) = (v[a] - t, v[b] - t);
                    if (va < 0.0) != (vb < 0.0) {
                        let f = va / (va - vb);
                        let key = (c[a].min(c[b]), c[a].max(c[b]));
                        cross.push((key, pos[a].lerp(pos[b], f)));
                    }
                }
                match cross.len() {
                    2 => segs.entry(lv).or_default().push((cross[0].0, cross[1].0, cross[0].1, cross[1].1)),
                    4 => {
                        // saddle: pair by the centre value
                        let centre = (v.iter().sum::<f64>() / 4.0) - t;
                        let (p, q) = if (centre < 0.0) == (v[0] - t < 0.0) { ((0, 3), (1, 2)) } else { ((0, 1), (2, 3)) };
                        for (x, y) in [p, q] {
                            segs.entry(lv).or_default().push((cross[x].0, cross[y].0, cross[x].1, cross[y].1));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // link each level's segments into polylines
    let mut rows = Vec::new();
    let mut levels: Vec<i64> = segs.keys().copied().collect();
    levels.sort();
    for lv in levels {
        let list = &segs[&lv];
        let mut by_key: HashMap<Key, Vec<usize>> = HashMap::new();
        for (n, s) in list.iter().enumerate() {
            by_key.entry(s.0).or_default().push(n);
            by_key.entry(s.1).or_default().push(n);
        }
        let mut used = vec![false; list.len()];
        // start from ends (keys used once) first, then any loops left
        let mut order: Vec<usize> = (0..list.len()).collect();
        order.sort_by_key(|&n| {
            let s = &list[n];
            let ends = (by_key[&s.0].len() == 1) as u8 + (by_key[&s.1].len() == 1) as u8;
            std::cmp::Reverse(ends)
        });
        for start in order {
            if used[start] {
                continue;
            }
            used[start] = true;
            let s = &list[start];
            // begin at the open end if there is one
            let (mut key, mut pts) = if by_key[&s.0].len() == 1 { (s.1, vec![s.2, s.3]) } else { (s.0, vec![s.3, s.2]) };
            loop {
                let next = by_key[&key].iter().copied().find(|&n| !used[n]);
                let Some(n) = next else { break };
                used[n] = true;
                let t = &list[n];
                if t.0 == key {
                    pts.push(t.3);
                    key = t.1;
                } else {
                    pts.push(t.2);
                    key = t.0;
                }
            }
            if pts.len() >= 2 {
                let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
                if len > 0.6 {
                    rows.push((lv, smooth(&pts)));
                }
            }
        }
    }
    // reach the real edge (the grid stops up to one cell short)
    for (_, r) in rows.iter_mut() {
        extend_to_edge(r, rings, keep, h * 1.5);
    }
    rows
}

/// Light smoothing that keeps the ends where they are.
fn smooth(p: &[Pt]) -> Vec<Pt> {
    if p.len() < 4 {
        return p.to_vec();
    }
    let mut out = p.to_vec();
    for _ in 0..2 {
        let prev = out.clone();
        for i in 1..prev.len() - 1 {
            out[i] = Pt::new(
                0.25 * prev[i - 1].x + 0.5 * prev[i].x + 0.25 * prev[i + 1].x,
                0.25 * prev[i - 1].y + 0.5 * prev[i].y + 0.25 * prev[i + 1].y,
            );
        }
    }
    out
}

fn extend_to_edge(r: &mut Vec<Pt>, _rings: &[Ring], keep: &dyn Fn(Pt) -> bool, max: f64) {
    for end in [false, true] {
        let n = r.len();
        if n < 2 {
            return;
        }
        let (tip, prev) = if end { (r[n - 1], r[n - 2]) } else { (r[0], r[1]) };
        let d = tip.sub(prev);
        let l = d.len();
        if l < 1e-9 {
            continue;
        }
        let u = d.mul(1.0 / l);
        // walk out in small steps while still on the shape
        let mut reach = 0.0;
        let step = max / 8.0;
        while reach + step <= max && keep(tip.add(u.mul(reach + step))) {
            reach += step;
        }
        if reach > 0.0 {
            let q = tip.add(u.mul(reach));
            if end { r[n - 1] = q } else { r[0] = q }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Ring {
        vec![Pt::new(x0, y0), Pt::new(x1, y0), Pt::new(x1, y1), Pt::new(x0, y1)]
    }

    fn inside(r: &Ring, p: Pt) -> bool {
        let mut c = false;
        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                c = !c;
            }
        }
        c
    }

    #[test]
    fn one_guide_gives_straight_even_rows() {
        let sq = rect(0.0, 0.0, 30.0, 20.0);
        let keep = |p: Pt| inside(&sq, p);
        let rows = guided_rows(&[sq.clone()], FillRule::NonZero, &keep, &[(Pt::new(5.0, 10.0), Pt::new(25.0, 10.0))], 0.5);
        // horizontal rows 0.5 mm apart across 20 mm: about 40 of them
        assert!((rows.len() as i64 - 40).abs() <= 2, "{}", rows.len());
        for (_, r) in &rows {
            let dy = r.iter().map(|p| p.y).fold(f64::MIN, f64::max) - r.iter().map(|p| p.y).fold(f64::MAX, f64::min);
            assert!(dy < 0.15, "row not straight: {dy}");
            let len = r.last().unwrap().x - r[0].x;
            assert!(len.abs() > 29.0, "row reaches both edges: {len}");
        }
    }

    #[test]
    fn two_guides_turn_the_rows() {
        // an L: horizontal guide in the foot, vertical guide up the leg
        let leg = rect(0.0, 0.0, 10.0, 40.0);
        let foot = rect(0.0, 30.0, 40.0, 40.0);
        let keep = |p: Pt| inside(&leg, p) || inside(&foot, p);
        let guides = [(Pt::new(5.0, 2.0), Pt::new(5.0, 20.0)), (Pt::new(20.0, 35.0), Pt::new(38.0, 35.0))];
        // direction is vertical up the leg and horizontal along the foot
        let up = direction_at(Pt::new(5.0, 5.0), &guides);
        let along = direction_at(Pt::new(35.0, 35.0), &guides);
        assert!((up.abs() - std::f64::consts::FRAC_PI_2).abs() < 0.2, "{up}");
        assert!(along.abs() < 0.2, "{along}");
        let rows = guided_rows(&[leg.clone(), foot.clone()], FillRule::NonZero, &keep, &guides, 0.5);
        assert!(rows.len() > 30);
        // every row point is on the shape
        for (_, r) in &rows {
            for p in r {
                assert!(keep(*p) || keep(Pt::new(p.x - 0.3, p.y)) || keep(Pt::new(p.x, p.y - 0.3)) || keep(Pt::new(p.x + 0.3, p.y)) || keep(Pt::new(p.x, p.y + 0.3)), "{p:?}");
            }
        }
    }
}
