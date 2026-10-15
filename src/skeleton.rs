//! Centre lines of strokes, for turning satin.
//!
//! A letter's strokes are found by thinning the shape down to a one-pixel
//! centre line (Zhang–Suen), then reading that line as a graph: free ends,
//! junctions where strokes meet, and the stroke paths between them. Small
//! side spurs (from corners and bumps in the outline) are pruned so each
//! stroke becomes one clean path. The satin then runs across each path,
//! turning as the stroke curves.

use crate::fills::Field;
use crate::geom::*;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Edge {
    pub a: usize,
    pub b: usize,
    /// Path from node `a` to node `b` (mm), smoothed.
    pub pts: Vec<Pt>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Pt>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn degree(&self, n: usize) -> usize {
        self.edges
            .iter()
            .map(|e| (e.a == n) as usize + (e.b == n) as usize)
            .sum()
    }
}

fn path_len(p: &[Pt]) -> f64 {
    p.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// Zhang–Suen thinning of a binary image (true = shape).
fn thin(img: &mut [bool], w: usize, h: usize) {
    let idx = |i: usize, j: usize| j * w + i;
    loop {
        let mut changed = false;
        for pass in 0..2 {
            let mut kill = Vec::new();
            for j in 1..h - 1 {
                for i in 1..w - 1 {
                    if !img[idx(i, j)] {
                        continue;
                    }
                    // p2..p9 clockwise from north
                    let n = [
                        img[idx(i, j - 1)],
                        img[idx(i + 1, j - 1)],
                        img[idx(i + 1, j)],
                        img[idx(i + 1, j + 1)],
                        img[idx(i, j + 1)],
                        img[idx(i - 1, j + 1)],
                        img[idx(i - 1, j)],
                        img[idx(i - 1, j - 1)],
                    ];
                    let b = n.iter().filter(|&&x| x).count();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let a = (0..8).filter(|&k| !n[k] && n[(k + 1) % 8]).count();
                    if a != 1 {
                        continue;
                    }
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let ok = if pass == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    if ok {
                        kill.push(idx(i, j));
                    }
                }
            }
            changed |= !kill.is_empty();
            for k in kill {
                img[k] = false;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Centre-line graph of a shape. `res` is the grid step in mm.
pub fn skeleton(field: &Field) -> Graph {
    let (w, h, _res) = field.grid();
    let mut img: Vec<bool> = (0..w * h).map(|k| field.depth(k % w, k / w) > 0.0).collect();
    thin(&mut img, w, h);
    let at = |i: isize, j: isize| -> bool {
        i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h && img[j as usize * w + i as usize]
    };
    // m-adjacency: diagonal links only where no 4-path joins the two pixels,
    // so a thinned line gives each middle pixel exactly two neighbours.
    let nbrs = |i: usize, j: usize| -> Vec<(usize, usize)> {
        let (i, j) = (i as isize, j as isize);
        let mut v = Vec::with_capacity(4);
        for (di, dj) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            if at(i + di, j + dj) {
                v.push(((i + di) as usize, (j + dj) as usize));
            }
        }
        for (di, dj) in [(1, -1), (1, 1), (-1, 1), (-1, -1)] {
            if at(i + di, j + dj) && !at(i + di, j) && !at(i, j + dj) {
                v.push(((i + di) as usize, (j + dj) as usize));
            }
        }
        v
    };
    let pix: Vec<(usize, usize)> = (0..w * h).filter(|&k| img[k]).map(|k| (k % w, k / w)).collect();
    if pix.is_empty() {
        return Graph::default();
    }
    let deg: HashMap<(usize, usize), usize> = pix.iter().map(|&p| (p, nbrs(p.0, p.1).len())).collect();
    let is_node = |p: (usize, usize)| deg[&p] != 2;
    // junction pixels that touch become one node
    let mut node_of: HashMap<(usize, usize), usize> = HashMap::new();
    let mut members: Vec<Vec<(usize, usize)>> = Vec::new();
    for &p in &pix {
        if !is_node(p) || node_of.contains_key(&p) {
            continue;
        }
        let id = members.len();
        let mut stack = vec![p];
        let mut group = Vec::new();
        node_of.insert(p, id);
        while let Some(q) = stack.pop() {
            group.push(q);
            for r in nbrs(q.0, q.1) {
                if deg[&r] >= 3 && deg[&q] >= 3 && !node_of.contains_key(&r) {
                    node_of.insert(r, id);
                    stack.push(r);
                }
            }
        }
        members.push(group);
    }
    let mut g = Graph::default();
    for m in &members {
        let c = m
            .iter()
            .fold(Pt::new(0.0, 0.0), |acc, &(i, j)| acc.add(field.node(i, j)))
            .mul(1.0 / m.len() as f64);
        g.nodes.push(c);
    }
    let pt = |p: (usize, usize)| field.node(p.0, p.1);
    let mut used: std::collections::HashSet<((usize, usize), (usize, usize))> = Default::default();
    let mut seen_path: std::collections::HashSet<(usize, usize)> = Default::default();
    // trace from every node pixel along each neighbour
    for (ni, m) in members.iter().enumerate() {
        for &start in m {
            for first in nbrs(start.0, start.1) {
                if node_of.get(&first) == Some(&ni) || used.contains(&(start, first)) {
                    continue;
                }
                let mut path = vec![g.nodes[ni], pt(first)];
                let (mut prev, mut cur) = (start, first);
                used.insert((start, first));
                let end = loop {
                    if let Some(&n) = node_of.get(&cur) {
                        used.insert((cur, prev));
                        break Some(n);
                    }
                    seen_path.insert(cur);
                    let next = nbrs(cur.0, cur.1).into_iter().find(|&q| q != prev);
                    match next {
                        Some(q) => {
                            prev = cur;
                            cur = q;
                            path.push(pt(cur));
                        }
                        None => break None,
                    }
                };
                if let Some(b) = end {
                    *path.last_mut().unwrap() = g.nodes[b];
                    if path.len() >= 2 && !(b == ni && path.len() <= 3) {
                        g.edges.push(Edge { a: ni, b, pts: path });
                    }
                }
            }
        }
    }
    // closed loops with no ends or junctions (an "o")
    for &p in &pix {
        if is_node(p) || seen_path.contains(&p) {
            continue;
        }
        let id = g.nodes.len();
        g.nodes.push(pt(p));
        let mut path = vec![pt(p)];
        seen_path.insert(p);
        let (mut prev, mut cur) = (p, nbrs(p.0, p.1)[0]);
        while cur != p {
            seen_path.insert(cur);
            path.push(pt(cur));
            let Some(n) = nbrs(cur.0, cur.1).into_iter().find(|&q| q != prev) else {
                break;
            };
            prev = cur;
            cur = n;
        }
        path.push(pt(p));
        if path.len() > 4 {
            g.edges.push(Edge { a: id, b: id, pts: path });
        }
    }
    prune(&mut g, field);
    for e in &mut g.edges {
        smooth(&mut e.pts);
    }
    g
}

/// Remove short side spurs and join paths through nodes left with two.
fn prune(g: &mut Graph, field: &Field) {
    for _ in 0..12 {
        let mut changed = false;
        let degs: Vec<usize> = (0..g.nodes.len()).map(|n| g.degree(n)).collect();
        let before = g.edges.len();
        if before > 1 {
            let nodes = g.nodes.clone();
            g.edges.retain(|e| {
                let (leaf, hub) = if degs[e.a] == 1 && degs[e.b] >= 3 {
                    (e.a, e.b)
                } else if degs[e.b] == 1 && degs[e.a] >= 3 {
                    (e.b, e.a)
                } else {
                    return true;
                };
                let _ = leaf;
                let r = field.at(nodes[hub]).max(0.2);
                path_len(&e.pts) > r * 2.0 + 0.5
            });
            changed |= g.edges.len() != before;
        }
        // A fork of two short spurs at the end of a stroke (serif feet): drop
        // both, so the stroke runs on to the edge and its satin covers the
        // serif straight across.
        let degs: Vec<usize> = (0..g.nodes.len()).map(|n| g.degree(n)).collect();
        for n in 0..g.nodes.len() {
            if degs[n] != 3 {
                continue;
            }
            let r = field.at(g.nodes[n]).max(0.2);
            let ids: Vec<usize> = (0..g.edges.len())
                .filter(|&k| (g.edges[k].a == n) != (g.edges[k].b == n))
                .collect();
            if ids.len() != 3 {
                continue;
            }
            let stub = |k: usize| {
                let e = &g.edges[k];
                let other = if e.a == n { e.b } else { e.a };
                degs[other] == 1 && path_len(&e.pts) < r * 3.2 + 0.8
            };
            let stubs: Vec<usize> = ids.iter().copied().filter(|&k| stub(k)).collect();
            if stubs.len() == 2 {
                let (hi, lo) = (stubs[0].max(stubs[1]), stubs[0].min(stubs[1]));
                g.edges.remove(hi);
                g.edges.remove(lo);
                changed = true;
                break;
            }
        }
        // join the two paths at a node that now has exactly two
        for n in 0..g.nodes.len() {
            let ids: Vec<usize> = (0..g.edges.len())
                .filter(|&k| g.edges[k].a == n || g.edges[k].b == n)
                .collect();
            if ids.len() != 2 || ids[0] == ids[1] {
                continue;
            }
            let (e1, e2) = (g.edges[ids[0]].clone(), g.edges[ids[1]].clone());
            if e1.a == e1.b || e2.a == e2.b {
                continue;
            }
            // orient e1 to end at n, e2 to start at n
            let mut p1 = e1.pts.clone();
            let a1 = if e1.b == n { e1.a } else {
                p1.reverse();
                e1.b
            };
            let mut p2 = e2.pts.clone();
            let b2 = if e2.a == n { e2.b } else {
                p2.reverse();
                e2.a
            };
            p1.extend(p2.into_iter().skip(1));
            let (hi, lo) = (ids[0].max(ids[1]), ids[0].min(ids[1]));
            g.edges.remove(hi);
            g.edges[lo] = Edge { a: a1, b: b2, pts: p1 };
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// Smooth pixel steps out of a path, keeping its ends.
fn smooth(p: &mut Vec<Pt>) {
    let n = p.len();
    if n < 5 {
        return;
    }
    let closed = p[0].dist(p[n - 1]) < 1e-6;
    for _ in 0..6 {
        let q = p.clone();
        for k in 1..n - 1 {
            p[k] = Pt::new(
                (q[k - 1].x + 2.0 * q[k].x + q[k + 1].x) / 4.0,
                (q[k - 1].y + 2.0 * q[k].y + q[k + 1].y) / 4.0,
            );
        }
        if closed {
            let m = Pt::new(
                (q[n - 2].x + 2.0 * q[0].x + q[1].x) / 4.0,
                (q[n - 2].y + 2.0 * q[0].y + q[1].y) / 4.0,
            );
            p[0] = m;
            p[n - 1] = m;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(rings: &[Ring]) -> Field {
        Field::new(rings, FillRule::EvenOdd, 0.05)
    }

    #[test]
    fn bar_is_one_stroke() {
        let r = vec![vec![
            Pt::new(0.0, 0.0),
            Pt::new(20.0, 0.0),
            Pt::new(20.0, 3.0),
            Pt::new(0.0, 3.0),
        ]];
        let g = skeleton(&field(&r));
        assert_eq!(g.edges.len(), 1, "{:?}", g.edges.iter().map(|e| e.pts.len()).collect::<Vec<_>>());
        let e = &g.edges[0];
        assert!(path_len(&e.pts) > 15.0);
    }

    #[test]
    fn ring_is_one_loop() {
        let circle = |r: f64| -> Ring {
            (0..90)
                .map(|k| {
                    let a = k as f64 * 4f64.to_radians();
                    Pt::new(10.0 + r * a.cos(), 10.0 + r * a.sin())
                })
                .collect()
        };
        let g = skeleton(&field(&[circle(8.0), circle(5.0)]));
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].a, g.edges[0].b);
    }

    #[test]
    fn tee_has_three_strokes() {
        // T: bar 20×3 on a 3×15 stem
        let r = vec![vec![
            Pt::new(0.0, 0.0),
            Pt::new(20.0, 0.0),
            Pt::new(20.0, 3.0),
            Pt::new(11.5, 3.0),
            Pt::new(11.5, 18.0),
            Pt::new(8.5, 18.0),
            Pt::new(8.5, 3.0),
            Pt::new(0.0, 3.0),
        ]];
        let g = skeleton(&field(&r));
        assert_eq!(g.edges.len(), 3);
    }
}
