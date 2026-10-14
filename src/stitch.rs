//! Stitch generation. Every object keeps its vector shape and settings;
//! stitches are always regenerated from them.

use crate::geom::*;
use crate::import::{Artwork, ShapeKind, StitchChoice};
use crate::settings::{Limits, Profile, Recipe};
use crate::thread::{Thread, nearest};
use serde::Serialize;
use std::f64::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Needle {
    pub p: Pt,
    /// Travel to this point without sewing (start of a new section).
    pub jump: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum StitchType {
    Fill,
    Satin,
    SatinColumn,
    Run,
    TripleRun,
}

impl StitchType {
    pub fn label(&self) -> &'static str {
        match self {
            StitchType::Fill => "Tatami fill",
            StitchType::Satin => "Satin",
            StitchType::SatinColumn => "Satin column",
            StitchType::Run => "Run",
            StitchType::TripleRun => "Triple run",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Planned {
    pub label: String,
    pub kind: StitchType,
    pub shape: ShapeKind,
    pub thread: usize,
    pub color_error: f64,
    pub stack: usize,
    pub bbox: BBox,
    /// Stitch direction (radians) for fills and satins.
    pub angle: f64,
    /// Upper-layer areas this object tucks under (not stitched twice).
    pub knockouts: Vec<(Vec<Ring>, FillRule)>,
    /// Layer this object came from, and that layer's stop setting.
    pub layer: usize,
    pub stop: Option<bool>,
}

/// Overlap left under an upper layer's edge so no fabric shows (mm).
const UNDERLAP: f64 = 0.5;

impl Planned {
    /// Will an upper layer cover this point?
    fn covered(&self, q: Pt) -> bool {
        self.knockouts.iter().any(|(r, ru)| inside(r, *ru, q))
    }

    /// Strokes: drop the parts hidden under upper layers (keeping an underlap).
    fn visible_paths(&self, paths: &[Vec<Pt>]) -> Vec<Vec<Pt>> {
        if self.knockouts.is_empty() {
            return paths.to_vec();
        }
        let o = UNDERLAP;
        let deep = |q: Pt| {
            [
                Pt::new(o, 0.0),
                Pt::new(-o, 0.0),
                Pt::new(0.0, o),
                Pt::new(0.0, -o),
                Pt::new(0.0, 0.0),
            ]
            .iter()
            .all(|d| self.covered(q.add(*d)))
        };
        let mut out = Vec::new();
        for path in paths {
            let mut cur = Vec::new();
            for q in resample(path, 0.25) {
                if deep(q) {
                    if cur.len() >= 2 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                } else {
                    cur.push(q);
                }
            }
            if cur.len() >= 2 {
                out.push(cur);
            }
        }
        out.into_iter().filter(|p| p.len() >= 4).collect()
    }

    /// Scan this object's area in the frame of `angle`, skipping covered parts.
    fn rows(
        &self,
        rings: &[Ring],
        rule: FillRule,
        angle: f64,
        spacing: f64,
        min_len: f64,
    ) -> Vec<Vec<Seg>> {
        let rot = rotate_rings(rings, -angle);
        let kos: Vec<(Vec<Ring>, FillRule)> = self
            .knockouts
            .iter()
            .map(|(r, ru)| (rotate_rings(r, -angle), *ru))
            .collect();
        let ko = Knockout {
            shapes: &kos,
            overlap: UNDERLAP,
        };
        scan_ko(
            &rot,
            rule,
            spacing,
            min_len,
            if kos.is_empty() { None } else { Some(&ko) },
        )
    }
}

/// Decide stitch type, angle and thread for every shape.
pub fn plan(
    art: &Artwork,
    chart: &[Thread],
    profile: Profile,
    warnings: &mut Vec<String>,
) -> Vec<Planned> {
    let mut out = Vec::new();
    for (i, s) in art.shapes.iter().enumerate() {
        let o = &s.opts;
        let (thread, color_error) = match o.thread.filter(|t| *t < chart.len()) {
            Some(t) => (t, 0.0),
            None => nearest(chart, s.rgb),
        };
        let bbox = BBox::of(s.points());
        let name = if s.label.is_empty() { format!("Object {}", i + 1) } else { s.label.clone() };
        // User angle: degrees counter-clockwise on screen (y runs down here).
        let user_angle = o.angle_deg.map(|d| -d.to_radians());
        let mk = |label: String, kind: StitchType, shape: ShapeKind, bbox: BBox, angle: f64| Planned {
            label,
            kind,
            shape,
            thread,
            color_error,
            stack: i,
            bbox,
            angle,
            knockouts: Vec::new(),
            layer: o.layer,
            stop: o.stop,
        };
        match &s.kind {
            ShapeKind::Area { rings: all_rings, rule } => {
                if matches!(o.stitch, StitchChoice::Run | StitchChoice::TripleRun) {
                    // Outline only: run around every ring.
                    let paths: Vec<Vec<Pt>> = all_rings
                        .iter()
                        .map(|r| {
                            let mut p = r.clone();
                            p.push(r[0]);
                            p
                        })
                        .collect();
                    let kind = if o.stitch == StitchChoice::Run { StitchType::Run } else { StitchType::TripleRun };
                    out.push(mk(name, kind, ShapeKind::Line { paths, width: 0.3 }, bbox, 0.0));
                    continue;
                }
                let islands = split_islands(all_rings, *rule);
                let many = islands.len() > 1;
                for (k, rings) in islands.iter().enumerate() {
                    let name = if many { format!("{name} {}", k + 1) } else { name.clone() };
                    let bbox = BBox::of(rings.iter().flatten());
                    let shape = ShapeKind::Area { rings: rings.clone(), rule: *rule };
                    let area = rings_area(rings, *rule);
                    if area < Limits::MIN_AREA {
                        warnings.push(format!("{name}: area {area:.2} mm² too small to stitch, skipped"));
                        continue;
                    }
                    let thickness = 2.0 * area / rings_perimeter(rings).max(1e-9);
                    let pa = principal_angle(rings);
                    let mut satin_angle = pa + FRAC_PI_2;
                    let mut kind = StitchType::Fill;
                    let consider_satin = match o.stitch {
                        StitchChoice::Fill => false,
                        StitchChoice::Satin => true,
                        _ => thickness <= Limits::SATIN_THICKNESS,
                    };
                    if consider_satin {
                        // Satin runs across the strokes: try angles every 15° and
                        // keep the one with the shortest average stitch.
                        let mut best = (f64::MAX, satin_angle, f64::MAX);
                        for k in 0..12 {
                            let a = (k as f64) * std::f64::consts::PI / 12.0;
                            let rows = scan(&rotate_rings(rings, -a), *rule, 0.3, 0.1);
                            let mut lens: Vec<f64> = rows.iter().flatten().map(|g| g.len()).collect();
                            if lens.is_empty() {
                                continue;
                            }
                            let mean = lens.iter().sum::<f64>() / lens.len() as f64;
                            lens.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            let p90 = lens[lens.len() * 9 / 10];
                            if mean < best.0 - 1e-6 {
                                best = (mean, a, p90);
                            }
                        }
                        satin_angle = best.1;
                        if o.stitch == StitchChoice::Satin || best.2 <= Limits::SATIN_MAX * 1.25 {
                            kind = StitchType::Satin;
                        }
                    }
                    let angle = match (user_angle, kind, profile) {
                        (Some(a), _, _) => a,
                        (None, StitchType::Satin, _) => satin_angle,
                        (None, _, Profile::Cap) => 0.0, // horizontal rows, sewn bottom to top
                        _ => pa,
                    };
                    out.push(mk(name, kind, shape, bbox, angle));
                }
            }
            ShapeKind::Line { width, .. } => {
                let kind = match o.stitch {
                    StitchChoice::Satin => StitchType::SatinColumn,
                    StitchChoice::Run => StitchType::Run,
                    StitchChoice::TripleRun => StitchType::TripleRun,
                    _ if *width >= Limits::STROKE_SATIN_MIN => StitchType::SatinColumn,
                    _ if *width >= 0.6 => StitchType::TripleRun,
                    _ => StitchType::Run,
                };
                if *width > Limits::SATIN_MAX && kind == StitchType::SatinColumn {
                    warnings.push(format!("{name}: stroke {width:.1} mm wide; satin stitches will be split"));
                }
                out.push(mk(name, kind, s.kind.clone(), bbox, 0.0));
            }
        }
    }
    // Knockout: each area skips what later (upper) areas will cover.
    for i in 0..out.len() {
        let kos: Vec<(Vec<Ring>, FillRule)> = out[i + 1..]
            .iter()
            .filter(|u| u.bbox.overlaps(&out[i].bbox, 0.0))
            .filter_map(|u| match &u.shape {
                ShapeKind::Area { rings, rule } => Some((rings.clone(), *rule)),
                _ => None,
            })
            .collect();
        out[i].knockouts = kos;
    }
    out
}

/// Generate needle points for one object. `from` is where the needle is now
/// (or the cap anchor); sections start at the closest available end.
pub fn generate(p: &Planned, r: &Recipe, profile: Profile, from: Pt) -> Vec<Needle> {
    let mut out = Vec::new();
    let pc = r.pull_comp + if profile == Profile::Cap { 0.05 } else { 0.0 };
    match (&p.shape, p.kind) {
        (ShapeKind::Area { rings, rule }, StitchType::Fill) => {
            let mut cur = from;
            // Underlay: perpendicular sparse rows; caps get a second, parallel layer.
            let mut layers = vec![p.angle + FRAC_PI_2];
            if profile == Profile::Cap {
                layers.push(p.angle);
            }
            let inset = r.underlay_inset;
            for a in layers {
                let rows = p.rows(rings, *rule, a, r.underlay_spacing, 2.0 * inset + 0.6);
                // Underlay travel is always buried under the top stitching.
                cur = sew_layer(
                    &mut out,
                    rings,
                    *rule,
                    rows,
                    a,
                    &|s| (s.x0 + inset, s.x1 - inset),
                    3.0,
                    1,
                    r.underlay_spacing,
                    cur,
                    &|_| true,
                );
            }
            let rows = p.rows(rings, *rule, p.angle, r.fill_spacing, 0.3);
            sew_layer(
                &mut out,
                rings,
                *rule,
                rows,
                p.angle,
                &|s| (s.x0 - pc, s.x1 + pc),
                r.fill_len,
                3,
                r.fill_spacing,
                cur,
                &|q| p.covered(q),
            );
        }
        (ShapeKind::Area { rings, rule }, _) => {
            // Satin from an outline. Stitches run across the stroke at the
            // object's angle; bands where that would make stitches too long
            // (crossbars, arch tops) are sewn at the perpendicular angle.
            let sp = r.satin_spacing / 2.0;
            let rows = p.rows(rings, *rule, p.angle, sp, 0.15);
            let mut lens: Vec<f64> = rows.iter().flatten().map(|g| g.len()).collect();
            lens.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = lens.get(lens.len() / 2).copied().unwrap_or(1.0);
            let limit = (median * 2.2).clamp(2.0, Limits::SATIN_MAX);
            let long: Vec<Vec<Seg>> = rows
                .iter()
                .map(|r| r.iter().filter(|s| s.len() > limit).copied().collect::<Vec<_>>())
                .filter(|r| !r.is_empty())
                .collect();
            let short: Vec<Vec<Seg>> = rows
                .into_iter()
                .map(|r| r.into_iter().filter(|s| s.len() <= limit).collect::<Vec<_>>())
                .filter(|r| !r.is_empty())
                .collect();
            let mut cur = from;
            let mut layers: Vec<(Vec<Vec<Seg>>, f64)> = vec![(short, p.angle)];
            if !long.is_empty() {
                // Band mask, tested in the main frame.
                let in_long = |q: Pt| -> bool {
                    let r = q.rot(-p.angle);
                    let row = (r.y / sp - 0.5).round() as i64;
                    long.iter().flatten().any(|s| (s.row - row).abs() <= 1 && r.x >= s.x0 && r.x <= s.x1)
                };
                let a2 = p.angle + FRAC_PI_2;
                let mut band: Vec<Vec<Seg>> = Vec::new();
                for row in p.rows(rings, *rule, a2, sp, 0.15) {
                    let mut segs = Vec::new();
                    for s in row {
                        let mut start: Option<f64> = None;
                        let steps = (s.len() / 0.1).ceil() as usize;
                        for k in 0..=steps {
                            let x = s.x0 + (s.len() * k as f64 / steps.max(1) as f64);
                            let m = in_long(Pt::new(x, s.y).rot(a2));
                            match (m, start) {
                                (true, None) => start = Some(x),
                                (false, Some(x0)) => {
                                    if x - x0 >= 0.4 {
                                        segs.push(Seg { row: s.row, y: s.y, x0, x1: x });
                                    }
                                    start = None;
                                }
                                _ => {}
                            }
                        }
                        if let Some(x0) = start {
                            if s.x1 - x0 >= 0.4 {
                                segs.push(Seg { row: s.row, y: s.y, x0, x1: s.x1 });
                            }
                        }
                    }
                    if !segs.is_empty() {
                        band.push(segs);
                    }
                }
                layers.push((band, a2));
            }
            for (rows, angle) in layers {
                cur = sew_satin(&mut out, p, rings, *rule, rows, angle, pc, r.satin_spacing, cur);
            }
        }
        (ShapeKind::Line { paths, width }, kind) => {
            let mut cur = from;
            for path in order_paths(&p.visible_paths(paths), cur) {
                match kind {
                    StitchType::SatinColumn => {
                        // Underlay runs forward along the whole path, then the
                        // satin comes back piece by piece. Sharp corners split the
                        // column and each piece runs past the corner by half the
                        // width, so corners come out square instead of notched.
                        let half = width / 2.0 + pc;
                        let closed = path.len() > 2 && path[0].dist(*path.last().unwrap()) < 0.05;
                        let pieces = split_at_corners(&path, 35f64.to_radians());
                        push_section(&mut out, &resample(&path, Limits::RUN_LEN), Limits::RUN_LEN);
                        let n = pieces.len();
                        for (k, piece) in pieces.iter().enumerate().rev() {
                            let mut pc_ = piece.clone();
                            let ext = width / 2.0;
                            let extend_start = closed || k > 0;
                            let extend_end = closed || k + 1 < n;
                            if extend_start && pc_.len() >= 2 {
                                let d = pc_[0].sub(pc_[1]).norm();
                                pc_[0] = pc_[0].add(d.mul(ext));
                            }
                            if extend_end && pc_.len() >= 2 {
                                let m = pc_.len();
                                let d = pc_[m - 1].sub(pc_[m - 2]).norm();
                                pc_[m - 1] = pc_[m - 1].add(d.mul(ext));
                            }
                            pc_.reverse();
                            let sides = satin_along(&pc_, half, r.satin_spacing / 2.0);
                            push_continue(&mut out, &split_long(&sides, Limits::SATIN_MAX), Limits::SATIN_MAX);
                        }
                    }
                    StitchType::TripleRun => {
                        let pts = resample(&path, Limits::RUN_LEN);
                        let mut tri = vec![pts[0]];
                        for w in pts.windows(2) {
                            tri.extend([w[1], w[0], w[1]]);
                        }
                        push_section(&mut out, &tri, Limits::RUN_LEN);
                    }
                    _ => push_section(&mut out, &resample(&path, Limits::RUN_LEN), Limits::RUN_LEN),
                }
                cur = out.last().map(|n| n.p).unwrap_or(cur);
            }
        }
    }
    out
}

/// Sew satin rows (one direction) as columns: centre-run underlay up the
/// column, then zig-zag back down it.
#[allow(clippy::too_many_arguments)]
fn sew_satin(
    out: &mut Vec<Needle>,
    p: &Planned,
    rings: &[Ring],
    rule: FillRule,
    rows: Vec<Vec<Seg>>,
    angle: f64,
    pc: f64,
    density: f64,
    mut cur: Pt,
) -> Pt {
    for (run, rev) in order_runs(group_runs(rows), angle, cur, &|x, y| travel_ok(rings, rule, x, y, &|q| p.covered(q))) {
        let mut segs = run;
        if rev {
            segs.reverse();
        }
        let step = ((1.5 / (density / 2.0)).round() as usize).max(1);
        let center: Vec<Pt> = segs
            .iter()
            .step_by(step)
            .chain(segs.last())
            .map(|s| Pt::new((s.x0 + s.x1) / 2.0, s.y).rot(angle))
            .collect();
        let sides: Vec<Pt> = segs
            .iter()
            .rev()
            .enumerate()
            .map(|(i, s)| {
                let x = if i % 2 == 0 { s.x0 - pc } else { s.x1 + pc };
                Pt::new(x, s.y).rot(angle)
            })
            .collect();
        if segs.len() >= 6 {
            start_in_shape(out, rings, rule, &center, Limits::RUN_LEN, &|_| true);
            push_continue(out, &split_long(&sides, Limits::SATIN_MAX), Limits::SATIN_MAX);
        } else {
            start_in_shape(out, rings, rule, &split_long(&sides, Limits::SATIN_MAX), Limits::SATIN_MAX, &|q| p.covered(q));
        }
        cur = out.last().map(|n| n.p).unwrap_or(cur);
    }
    cur
}

/// Longest travel run inside an object before a trim is used instead (mm):
/// mostly hidden paths up to MAX_TRAVEL, fully buried paths up to MAX_BURIED.
const MAX_TRAVEL: f64 = 15.0;
const MAX_BURIED: f64 = 30.0;

fn inside(rings: &[Ring], rule: FillRule, p: Pt) -> bool {
    spans_at(rings, rule, p.y)
        .iter()
        .any(|&(a, b)| a - 0.05 <= p.x && p.x <= b + 0.05)
}

/// Start a new section of an area object. If the straight path from the
/// needle stays inside the object's own outline, travel there with run
/// stitches (buried under the fill or an upper layer) instead of a trim.
/// `hidden(p)` says whether a travel stitch at `p` will be covered later
/// (by the top stitching or an upper layer); travel must stay hidden.
fn start_in_shape(
    out: &mut Vec<Needle>,
    rings: &[Ring],
    rule: FillRule,
    pts: &[Pt],
    max: f64,
    hidden: &dyn Fn(Pt) -> bool,
) {
    let (Some(last), Some(first)) = (out.last().map(|n| n.p), pts.first().copied()) else {
        return push_section(out, pts, max);
    };
    let d = last.dist(first);
    if d > 0.05 && travel_ok(rings, rule, last, first, hidden) {
        let travel = resample(&[last, first], Limits::RUN_LEN);
        push_continue(out, &travel[1..], Limits::RUN_LEN);
        push_continue(out, &pts[1..], max);
    } else {
        push_section(out, pts, max);
    }
}

/// Can the needle run from `a` to `b` without a trim and without showing?
fn travel_ok(rings: &[Ring], rule: FillRule, a: Pt, b: Pt, hidden: &dyn Fn(Pt) -> bool) -> bool {
    let d = a.dist(b);
    if d > MAX_BURIED {
        return false;
    }
    let n = (d / 0.5).ceil().max(1.0) as usize;
    let samples: Vec<Pt> = (1..n).map(|k| a.lerp(b, k as f64 / n as f64)).collect();
    if !samples.iter().all(|&q| inside(rings, rule, q)) {
        return false;
    }
    if d <= 2.0 {
        return true;
    }
    let hid = samples.iter().filter(|&&q| hidden(q)).count();
    if d > MAX_TRAVEL {
        hid == samples.len()
    } else {
        hid as f64 >= 0.85 * samples.len() as f64
    }
}

/// Split a polyline at corners sharper than `max_turn` (radians).
fn split_at_corners(path: &[Pt], max_turn: f64) -> Vec<Vec<Pt>> {
    let mut pts: Vec<Pt> = Vec::new();
    for &p in path {
        if pts.last().map(|l: &Pt| l.dist(p) > 1e-6).unwrap_or(true) {
            pts.push(p);
        }
    }
    let mut pieces = vec![vec![]];
    for i in 0..pts.len() {
        pieces.last_mut().unwrap().push(pts[i]);
        if i > 0 && i + 1 < pts.len() {
            let a = pts[i].sub(pts[i - 1]).norm();
            let b = pts[i + 1].sub(pts[i]).norm();
            if (a.x * b.x + a.y * b.y).clamp(-1.0, 1.0).acos() > max_turn {
                pieces.push(vec![pts[i]]);
            }
        }
    }
    pieces.into_iter().filter(|p| p.len() >= 2).collect()
}

/// Zig-zag points along a centre line, `half` to each side.
fn satin_along(path: &[Pt], half: f64, step: f64) -> Vec<Pt> {
    let samples = resample(path, step);
    let mut sides = Vec::with_capacity(samples.len());
    for i in 0..samples.len() {
        let a = samples[i.saturating_sub(1)];
        let b = samples[(i + 1).min(samples.len() - 1)];
        let n = b.sub(a).norm().rot(FRAC_PI_2);
        let sgn = if i % 2 == 0 { 1.0 } else { -1.0 };
        sides.push(samples[i].add(n.mul(half * sgn)));
    }
    sides
}

/// Start a new section: jump to the first point, then sew the rest.
fn push_section(out: &mut Vec<Needle>, pts: &[Pt], max: f64) {
    if let Some(first) = pts.first() {
        out.push(Needle {
            p: *first,
            jump: true,
        });
        push_continue(out, &pts[1..], max);
    }
}

/// Keep sewing from the current point, subdividing any stitch longer than `max`.
fn push_continue(out: &mut Vec<Needle>, pts: &[Pt], max: f64) {
    for &p in pts {
        if let Some(last) = out.last().map(|n| n.p) {
            let d = last.dist(p);
            if d > max {
                let n = (d / max).ceil() as usize;
                for k in 1..n {
                    out.push(Needle {
                        p: last.lerp(p, k as f64 / n as f64),
                        jump: false,
                    });
                }
            }
        }
        out.push(Needle { p, jump: false });
    }
}

/// Split satin stitches wider than `max` with staggered penetrations so the
/// split points don't line up into a visible seam.
fn split_long(sides: &[Pt], max: f64) -> Vec<Pt> {
    let mut out = Vec::with_capacity(sides.len());
    if sides.is_empty() {
        return out;
    }
    out.push(sides[0]);
    for k in 1..sides.len() {
        let (a, b) = (sides[k - 1], sides[k]);
        let d = a.dist(b);
        if d > max {
            let n = (d / max).ceil() as usize;
            let shift = [0.0, 0.3, -0.3][k % 3] / n as f64;
            for j in 1..n {
                out.push(a.lerp(b, (j as f64 / n as f64 + shift).clamp(0.05, 0.95)));
            }
        }
        out.push(b);
    }
    out
}

/// Sew one layer of back-and-forth rows. Chooses the order of the runs,
/// which side each starts on, and whether the needle can reach it with
/// hidden travel (inside the shape, over area still to be sewn or covered by
/// an upper layer) instead of a trim. Returns where the needle ends.
#[allow(clippy::too_many_arguments)]
fn sew_layer(
    out: &mut Vec<Needle>,
    rings: &[Ring],
    rule: FillRule,
    rows: Vec<Vec<Seg>>,
    angle: f64,
    ends: &dyn Fn(&Seg) -> (f64, f64),
    len: f64,
    stagger: i64,
    spacing: f64,
    from: Pt,
    covered: &dyn Fn(Pt) -> bool,
) -> Pt {
    let rows: Vec<Vec<Seg>> = rows
        .into_iter()
        .map(|r| {
            r.into_iter()
                .filter(|s| {
                    let (a, b) = ends(s);
                    b - a >= 0.2
                })
                .collect::<Vec<_>>()
        })
        .filter(|r| !r.is_empty())
        .collect();
    let mut runs = group_runs(rows);
    let mut cur = from;
    while !runs.is_empty() {
        // Candidate starts: either end row of each run, either side.
        let mut cands: Vec<(f64, usize, bool, bool, Pt)> = Vec::new();
        for (i, run) in runs.iter().enumerate() {
            for (rev, s) in [(false, &run[0]), (true, run.last().unwrap())] {
                let (a, b) = ends(s);
                for (left, x) in [(true, a), (false, b)] {
                    let p = Pt::new(x, s.y).rot(angle);
                    cands.push((p.dist(cur), i, rev, left, p));
                }
            }
        }
        cands.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let unsewn = |q: Pt| -> bool {
            let r = q.rot(-angle);
            runs.iter().any(|run| {
                let (y0, y1) = (
                    run[0].y.min(run.last().unwrap().y),
                    run[0].y.max(run.last().unwrap().y),
                );
                if r.y < y0 - spacing || r.y > y1 + spacing {
                    return false;
                }
                run.iter().any(|s| {
                    (s.y - r.y).abs() <= spacing * 0.75 && r.x >= s.x0 - 0.3 && r.x <= s.x1 + 0.3
                })
            })
        };
        let hidden = |q: Pt| covered(q) || unsewn(q);
        let have_needle = !out.is_empty();
        // Nearest start wins, unless it needs a trim and a hidden-travel start is
        // within 40 mm extra distance.
        let fallback = cands[0];
        let mut pick = (fallback, false);
        if have_needle {
            for c in &cands {
                if c.0 >= fallback.0 + 40.0 {
                    break;
                }
                if travel_ok(rings, rule, cur, c.4, &hidden) {
                    pick = (*c, true);
                    break;
                }
            }
        }
        let ((_, i, rev, left, start), travel) = pick;
        let mut run = runs.swap_remove(i);
        if rev {
            run.reverse();
        }
        let pts = sew_rows(&run, left, angle, ends, len, stagger);
        if travel && cur.dist(start) > 0.05 {
            let t = resample(&[cur, start], Limits::RUN_LEN);
            push_continue(out, &t[1..], Limits::RUN_LEN);
            push_continue(out, &pts[1..], len);
        } else {
            push_section(out, &pts, len);
        }
        cur = *pts.last().unwrap();
    }
    cur
}

/// Rows of one run, alternating direction, as tatami stitches with a brick
/// stagger. Returns design-frame points.
fn sew_rows(
    rows: &[Seg],
    start_left: bool,
    angle: f64,
    ends: &dyn Fn(&Seg) -> (f64, f64),
    len: f64,
    stagger: i64,
) -> Vec<Pt> {
    let mut left_to_right = start_left;
    let mut pts = Vec::new();
    let min = 0.6_f64.min(len * 0.3);
    for s in rows {
        let (a, b) = ends(s);
        let off = (s.row.rem_euclid(stagger) as f64) * len / stagger as f64;
        let mut xs = vec![a];
        let mut n = ((a + min - off) / len).ceil() as i64;
        loop {
            let g = off + n as f64 * len;
            if g >= b - min {
                break;
            }
            if g > a + min {
                xs.push(g);
            }
            n += 1;
        }
        xs.push(b);
        if !left_to_right {
            xs.reverse();
        }
        pts.extend(xs.into_iter().map(|x| Pt::new(x, s.y).rot(angle)));
        left_to_right = !left_to_right;
    }
    pts
}

/// Greedy order of runs: cheapest next start first; returns (run, reversed).
/// Starts the needle can reach with hidden travel win over ones needing a trim.
fn order_runs(
    mut runs: Vec<Vec<Seg>>,
    angle: f64,
    from: Pt,
    reach: &dyn Fn(Pt, Pt) -> bool,
) -> Vec<(Vec<Seg>, bool)> {
    let ends = |s: &Seg| [Pt::new(s.x0, s.y).rot(angle), Pt::new(s.x1, s.y).rot(angle)];
    let mid = |s: &Seg| Pt::new((s.x0 + s.x1) / 2.0, s.y).rot(angle);
    let mut out = Vec::new();
    let mut cur = from;
    while !runs.is_empty() {
        let mut best = (0usize, false, f64::MAX);
        for (i, r) in runs.iter().enumerate() {
            for (rev, first) in [(false, &r[0]), (true, r.last().unwrap())] {
                for e in ends(first) {
                    let d = e.dist(cur);
                    let cost = if d < best.2 && !reach(cur, e) {
                        d + 40.0
                    } else {
                        d
                    };
                    if cost < best.2 {
                        best = (i, rev, cost);
                    }
                }
            }
        }
        let r = runs.swap_remove(best.0);
        cur = if best.1 {
            mid(&r[0])
        } else {
            mid(r.last().unwrap())
        };
        out.push((r, best.1));
    }
    out
}

fn order_paths(paths: &[Vec<Pt>], from: Pt) -> Vec<Vec<Pt>> {
    let mut left: Vec<Vec<Pt>> = paths.iter().filter(|p| p.len() >= 2).cloned().collect();
    let mut out = Vec::new();
    let mut cur = from;
    while !left.is_empty() {
        let mut best = (0usize, false, f64::MAX);
        for (i, p) in left.iter().enumerate() {
            let d0 = p[0].dist(cur);
            let d1 = p.last().unwrap().dist(cur);
            if d0 < best.2 {
                best = (i, false, d0);
            }
            if d1 < best.2 {
                best = (i, true, d1);
            }
        }
        let mut p = left.swap_remove(best.0);
        if best.1 {
            p.reverse();
        }
        cur = *p.last().unwrap();
        out.push(p);
    }
    out
}
