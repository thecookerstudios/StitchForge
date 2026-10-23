//! Geometry for fills that follow the shape instead of straight rows.
//!
//! Contour and spiral fills sew rings at even depths inside the shape. The
//! rings come from a distance field — for every point, how far it is from
//! the nearest edge — traced at each depth with marching squares. That
//! handles holes, narrow necks and shapes that split into several lobes
//! without any special cases. Radial fills use rays from a centre point.

use crate::geom::*;
use std::collections::HashMap;

/// Signed distance to the shape's edge on a grid (mm): positive inside,
/// negative outside.
pub struct Field {
    x0: f64,
    y0: f64,
    res: f64,
    w: usize,
    h: usize,
    d: Vec<f32>,
    max: f64,
}

const FAR: f64 = 1e20;

/// Squared Euclidean distance transform of one row/column (Felzenszwalb).
fn dt1(f: &[f64], out: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let fq = f[q] + (q * q) as f64;
        let mut s = (fq - (f[v[k]] + (v[k] * v[k]) as f64)) / (2.0 * (q as f64 - v[k] as f64));
        // z[0] is -inf, so this never runs past the first parabola
        while s <= z[k] {
            k -= 1;
            s = (fq - (f[v[k]] + (v[k] * v[k]) as f64)) / (2.0 * (q as f64 - v[k] as f64));
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let dq = q as f64 - p as f64;
        *o = dq * dq + f[p];
    }
}

/// Squared distance (grid units) from every node to the nearest seed node.
fn edt(seed: &[bool], w: usize, h: usize) -> Vec<f64> {
    let mut g: Vec<f64> = seed.iter().map(|&s| if s { 0.0 } else { FAR }).collect();
    let n = w.max(h);
    let (mut f, mut out) = (vec![0.0; n], vec![0.0; n]);
    let (mut v, mut z) = (vec![0usize; n], vec![0.0; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = g[y * w + x];
        }
        dt1(&f[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            g[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..y * w + w]);
        dt1(&f[..w], &mut out[..w], &mut v, &mut z);
        g[y * w..y * w + w].copy_from_slice(&out[..w]);
    }
    g
}

impl Field {
    /// `res` is the grid step in mm; it is coarsened for very large shapes.
    pub fn new(rings: &[Ring], rule: FillRule, res: f64) -> Field {
        let b = BBox::of(rings.iter().flatten());
        let mut res = res.max(0.02);
        let pad = 1.0;
        let dims = |res: f64| {
            (
                ((b.width() + 2.0 * pad) / res).ceil() as usize + 1,
                ((b.height() + 2.0 * pad) / res).ceil() as usize + 1,
            )
        };
        let (mut w, mut h) = dims(res);
        while w * h > 3_000_000 {
            res *= 1.25;
            (w, h) = dims(res);
        }
        let (x0, y0) = (b.min.x - pad, b.min.y - pad);
        let mut inside = vec![false; w * h];
        for j in 0..h {
            let y = y0 + j as f64 * res;
            for (a, bx) in spans_at(rings, rule, y) {
                let i0 = ((a - x0) / res).ceil().max(0.0) as usize;
                let i1 = (((bx - x0) / res).floor() as isize).min(w as isize - 1);
                if i1 < 0 {
                    continue;
                }
                for i in i0..=(i1 as usize) {
                    inside[j * w + i] = true;
                }
            }
        }
        let outside: Vec<bool> = inside.iter().map(|&s| !s).collect();
        let din = edt(&outside, w, h);
        let dout = edt(&inside, w, h);
        let mut max = 0.0f64;
        let d: Vec<f32> = (0..w * h)
            .map(|k| {
                let v = if inside[k] {
                    (din[k].sqrt() - 0.5) * res
                } else {
                    -(dout[k].sqrt() - 0.5) * res
                };
                max = max.max(v);
                v as f32
            })
            .collect();
        Field {
            x0,
            y0,
            res,
            w,
            h,
            d,
            max,
        }
    }

    /// Grid size and step (mm).
    pub fn grid(&self) -> (usize, usize, f64) {
        (self.w, self.h, self.res)
    }

    /// Position of grid node (i, j) in mm.
    pub fn node(&self, i: usize, j: usize) -> Pt {
        Pt::new(self.x0 + i as f64 * self.res, self.y0 + j as f64 * self.res)
    }

    /// Depth at grid node (i, j), mm (negative outside).
    pub fn depth(&self, i: usize, j: usize) -> f64 {
        self.v(i, j)
    }

    /// Deepest point: half the widest part of the shape (mm).
    pub fn max(&self) -> f64 {
        self.max
    }

    fn v(&self, i: usize, j: usize) -> f64 {
        self.d[j * self.w + i] as f64
    }

    /// Distance from the edge at any point (bilinear).
    pub fn at(&self, p: Pt) -> f64 {
        let fx = ((p.x - self.x0) / self.res).clamp(0.0, (self.w - 1) as f64 - 1e-9);
        let fy = ((p.y - self.y0) / self.res).clamp(0.0, (self.h - 1) as f64 - 1e-9);
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (u, t) = (fx - i as f64, fy - j as f64);
        let a = self.v(i, j) * (1.0 - u) + self.v(i + 1, j) * u;
        let b = self.v(i, j + 1) * (1.0 - u) + self.v(i + 1, j + 1) * u;
        a * (1.0 - t) + b * t
    }

    /// Unit direction toward deeper inside (away from the nearest edge).
    pub fn inward(&self, p: Pt) -> Pt {
        let e = self.res;
        let g = Pt::new(
            self.at(Pt::new(p.x + e, p.y)) - self.at(Pt::new(p.x - e, p.y)),
            self.at(Pt::new(p.x, p.y + e)) - self.at(Pt::new(p.x, p.y - e)),
        );
        if g.len() < 1e-9 {
            Pt::new(0.0, 0.0)
        } else {
            g.norm()
        }
    }

    /// Closed rings at depth `level` (mm), all running the same way round.
    pub fn loops(&self, level: f64) -> Vec<Vec<Pt>> {
        let (w, h) = (self.w, self.h);
        let key_h = |i: usize, j: usize| ((j * w + i) * 2) as u64;
        let key_v = |i: usize, j: usize| ((j * w + i) * 2 + 1) as u64;
        let mut next: HashMap<u64, u64> = HashMap::new();
        let mut pos: HashMap<u64, Pt> = HashMap::new();
        let mut starts: Vec<u64> = Vec::new();
        let node = |i: usize, j: usize| Pt::new(self.x0 + i as f64 * self.res, self.y0 + j as f64 * self.res);
        let lerp_pt = |a: Pt, va: f64, b: Pt, vb: f64| {
            let t = if (vb - va).abs() < 1e-12 {
                0.5
            } else {
                ((level - va) / (vb - va)).clamp(0.0, 1.0)
            };
            a.lerp(b, t)
        };
        for j in 0..h - 1 {
            for i in 0..w - 1 {
                let c = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                let vals = c.map(|(x, y)| self.v(x, y));
                let ins = vals.map(|v| v >= level);
                let n = ins.iter().filter(|&&x| x).count();
                if n == 0 || n == 4 {
                    continue;
                }
                // edges: 0 top a-b, 1 right b-c, 2 bottom d-c, 3 left a-d
                let ekey = [key_h(i, j), key_v(i + 1, j), key_h(i, j + 1), key_v(i, j)];
                let ends = [(0, 1), (1, 2), (3, 2), (0, 3)];
                let mut epos = |e: usize| -> Pt {
                    *pos.entry(ekey[e]).or_insert_with(|| {
                        let (p, q) = ends[e];
                        lerp_pt(node(c[p].0, c[p].1), vals[p], node(c[q].0, c[q].1), vals[q])
                    })
                };
                let adj = [(0, 3), (0, 1), (1, 2), (2, 3)];
                let mut segs: Vec<(usize, usize, Pt)> = Vec::new();
                let saddle = n == 2 && ins[0] == ins[2];
                if saddle {
                    let centre_v = vals.iter().sum::<f64>() / 4.0;
                    let cin = centre_v >= level;
                    let centre = node(i, j).lerp(node(i + 1, j + 1), 0.5);
                    for k in 0..4 {
                        if ins[k] != cin {
                            let refp = if ins[k] { node(c[k].0, c[k].1) } else { centre };
                            segs.push((adj[k].0, adj[k].1, refp));
                        }
                    }
                } else {
                    let crossing: Vec<usize> = (0..4)
                        .filter(|&e| ins[ends[e].0] != ins[ends[e].1])
                        .collect();
                    let k = (0..4).find(|&k| ins[k]).unwrap();
                    segs.push((crossing[0], crossing[1], node(c[k].0, c[k].1)));
                }
                for (e1, e2, refp) in segs {
                    let (p, q) = (epos(e1), epos(e2));
                    let cross = (q.x - p.x) * (refp.y - p.y) - (q.y - p.y) * (refp.x - p.x);
                    let (s, t) = if cross >= 0.0 {
                        (ekey[e1], ekey[e2])
                    } else {
                        (ekey[e2], ekey[e1])
                    };
                    if next.insert(s, t).is_none() {
                        starts.push(s);
                    }
                }
            }
        }
        let mut out = Vec::new();
        for s in starts {
            if !next.contains_key(&s) {
                continue;
            }
            let mut ring = Vec::new();
            let mut cur = s;
            while let Some(n) = next.remove(&cur) {
                ring.push(pos[&cur]);
                cur = n;
                if cur == s {
                    break;
                }
            }
            if ring.len() >= 3 && perimeter(&ring) >= 1.0 {
                out.push(ring);
            }
        }
        out
    }
}

fn perimeter(r: &[Pt]) -> f64 {
    let mut l = 0.0;
    for k in 0..r.len() {
        l += r[k].dist(r[(k + 1) % r.len()]);
    }
    l
}

/// Area-weighted centre of the shape (falls back to the box centre).
pub fn centroid(rings: &[Ring]) -> Pt {
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for r in rings {
        for k in 0..r.len() {
            let (p, q) = (r[k], r[(k + 1) % r.len()]);
            let c = p.x * q.y - q.x * p.y;
            a += c;
            cx += (p.x + q.x) * c;
            cy += (p.y + q.y) * c;
        }
    }
    if a.abs() < 1e-9 {
        return BBox::of(rings.iter().flatten()).center();
    }
    Pt::new(cx / (3.0 * a), cy / (3.0 * a))
}

/// One radial row: from `x0` to `x1` mm out from the centre along `angle`.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub index: usize,
    pub angle: f64,
    pub x0: f64,
    pub x1: f64,
}

/// Rays from `centre`, spaced `spacing` apart at the outer edge. Rays near
/// the centre are thinned out (every 2nd, 4th, 8th… stops further out) so
/// the middle doesn't pile up. `pc` extends each end at the shape's edge.
pub fn rays(rings: &[Ring], rule: FillRule, centre: Pt, spacing: f64, pc: f64) -> Vec<Ray> {
    let rmax = rings
        .iter()
        .flatten()
        .map(|p| p.dist(centre))
        .fold(0.0, f64::max);
    let n = ((std::f64::consts::TAU * rmax / spacing).ceil() as usize).max(8);
    let rel: Vec<Ring> = rings
        .iter()
        .map(|r| r.iter().map(|p| p.sub(centre)).collect())
        .collect();
    let mut out = Vec::new();
    for i in 0..n {
        let angle = std::f64::consts::TAU * i as f64 / n as f64;
        let tz = if i == 0 { 30 } else { i.trailing_zeros().min(30) };
        // Rays of this level stop where their own spacing would drop to 0.7×
        // the row spacing; inside that, every other ray carries on (1.4×).
        let r_in = (0.7 * spacing * n as f64 / (std::f64::consts::TAU * (1u64 << tz) as f64)).max(0.3);
        let rot: Vec<Ring> = rel
            .iter()
            .map(|r| r.iter().map(|p| p.rot(-angle)).collect())
            .collect();
        for (a, b) in spans_at(&rot, rule, 0.0) {
            if b <= r_in {
                continue;
            }
            let x0 = if a > r_in { a - pc } else { r_in };
            let x1 = b + pc;
            if x1 - x0 >= 0.5 {
                out.push(Ray {
                    index: i,
                    angle,
                    x0: x0.max(0.0),
                    x1,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(s: f64) -> Vec<Ring> {
        vec![vec![
            Pt::new(0.0, 0.0),
            Pt::new(s, 0.0),
            Pt::new(s, s),
            Pt::new(0.0, s),
        ]]
    }

    #[test]
    fn field_depth_of_square() {
        let f = Field::new(&square(20.0), FillRule::NonZero, 0.1);
        assert!((f.max() - 10.0).abs() < 0.2, "max {}", f.max());
        assert!((f.at(Pt::new(10.0, 2.0)) - 2.0).abs() < 0.15);
        assert!(f.at(Pt::new(-0.8, 10.0)) < -0.6);
    }

    #[test]
    fn loops_are_closed_and_nested() {
        let f = Field::new(&square(20.0), FillRule::NonZero, 0.1);
        let l1 = f.loops(1.0);
        assert_eq!(l1.len(), 1);
        // a ring 1 mm in from a 20 mm square is about 18 mm across
        let b = BBox::of(l1[0].iter());
        assert!((b.width() - 18.0).abs() < 0.3, "{}", b.width());
        // ring with a hole gives two loops at a shallow depth
        let mut rings = square(20.0);
        rings.push(vec![
            Pt::new(7.0, 7.0),
            Pt::new(7.0, 13.0),
            Pt::new(13.0, 13.0),
            Pt::new(13.0, 7.0),
        ]);
        let f = Field::new(&rings, FillRule::EvenOdd, 0.1);
        assert_eq!(f.loops(1.0).len(), 2);
        // both loops run the same way round relative to the inside
        let area = |r: &Vec<Pt>| {
            let mut a = 0.0;
            for k in 0..r.len() {
                let (p, q) = (r[k], r[(k + 1) % r.len()]);
                a += p.x * q.y - q.x * p.y;
            }
            a
        };
        let ls = f.loops(1.0);
        assert!(area(&ls[0]).signum() != area(&ls[1]).signum());
    }

    #[test]
    fn radial_rays_cover_circle() {
        let ring: Ring = (0..72)
            .map(|k| {
                let a = k as f64 * 5f64.to_radians();
                Pt::new(10.0 * a.cos(), 10.0 * a.sin())
            })
            .collect();
        let rs = rays(&[ring], FillRule::NonZero, Pt::new(0.0, 0.0), 0.4, 0.0);
        assert!(rs.len() > 140);
        assert!(rs.iter().all(|r| (r.x1 - 10.0).abs() < 0.2));
        // fewer rays reach right into the middle
        let deep = rs.iter().filter(|r| r.x0 < 1.0).count();
        assert!(deep < rs.len() / 4);
    }
}
