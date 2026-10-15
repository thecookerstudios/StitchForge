//! Stitch generation. Every object keeps its vector shape and settings;
//! stitches are always regenerated from them.

use crate::geom::*;
use crate::fills::{self, Field};
use crate::import::{Artwork, Cap, Corner, FillPattern, ShapeKind, ShapeOpts, StitchChoice};
use crate::settings::{Limits, Profile, Recipe};
use crate::thread::{Thread, nearest};
use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The layer's stitch choices (underlay, cut-out, widths, corners, caps).
    pub opts: ShapeOpts,
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
        let name = if s.label.is_empty() {
            format!("Object {}", i + 1)
        } else {
            s.label.clone()
        };
        // User angle: degrees counter-clockwise on screen (y runs down here).
        let user_angle = o.angle_deg.map(|d| -d.to_radians());
        let mk =
            |label: String, kind: StitchType, shape: ShapeKind, bbox: BBox, angle: f64| Planned {
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
                opts: o.clone(),
            };
        match &s.kind {
            ShapeKind::Area {
                rings: all_rings,
                rule,
            } => {
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
                    let kind = if o.stitch == StitchChoice::Run {
                        StitchType::Run
                    } else {
                        StitchType::TripleRun
                    };
                    out.push(mk(
                        name,
                        kind,
                        ShapeKind::Line { paths, width: 0.3 },
                        bbox,
                        0.0,
                    ));
                    continue;
                }
                let islands = split_islands(all_rings, *rule);
                let many = islands.len() > 1;
                for (k, rings) in islands.iter().enumerate() {
                    let name = if many {
                        format!("{name} {}", k + 1)
                    } else {
                        name.clone()
                    };
                    let bbox = BBox::of(rings.iter().flatten());
                    let shape = ShapeKind::Area {
                        rings: rings.clone(),
                        rule: *rule,
                    };
                    let area = rings_area(rings, *rule);
                    if area < Limits::MIN_AREA {
                        warnings.push(format!(
                            "{name}: area {area:.2} mm² too small to stitch, skipped"
                        ));
                        continue;
                    }
                    let thickness = 2.0 * area / rings_perimeter(rings).max(1e-9);
                    // Shapes without a clear long direction (squares, circles) get
                    // the standard 45° fill instead of an arbitrary slight tilt.
                    let pa = if elongation(rings) < 1.2 {
                        -std::f64::consts::FRAC_PI_4
                    } else {
                        principal_angle(rings)
                    };
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
                            let mut lens: Vec<f64> =
                                rows.iter().flatten().map(|g| g.len()).collect();
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
                let width = &o.column_width.unwrap_or(*width);
                let kind = match o.stitch {
                    StitchChoice::Satin => StitchType::SatinColumn,
                    StitchChoice::Run => StitchType::Run,
                    StitchChoice::TripleRun => StitchType::TripleRun,
                    _ if *width >= Limits::STROKE_SATIN_MIN => StitchType::SatinColumn,
                    _ if *width >= 0.6 => StitchType::TripleRun,
                    _ => StitchType::Run,
                };
                if *width > Limits::SATIN_MAX && kind == StitchType::SatinColumn {
                    warnings.push(format!(
                        "{name}: stroke {width:.1} mm wide; satin stitches will be split"
                    ));
                }
                out.push(mk(name, kind, s.kind.clone(), bbox, 0.0));
            }
        }
    }
    // Knockout: each area skips what later (upper) areas will cover.
    for i in 0..out.len() {
        if out[i].opts.keep_under {
            continue; // this layer keeps sewing underneath
        }
        let kos: Vec<(Vec<Ring>, FillRule)> = out[i + 1..]
            .iter()
            .filter(|u| !u.opts.overlay && u.bbox.overlaps(&out[i].bbox, 0.0))
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
/// The fabric's stitch settings with this layer's own changes applied.
pub fn effective(r: &Recipe, o: &ShapeOpts) -> Recipe {
    let mut e = r.clone();
    if let Some(v) = o.density {
        e.fill_spacing = v.clamp(0.15, 3.0);
        e.satin_spacing = v.clamp(0.15, 3.0);
    }
    if let Some(v) = o.stitch_len {
        e.fill_len = v.clamp(0.8, 12.0);
    }
    if let Some(v) = o.pull_comp {
        e.pull_comp = v.clamp(-0.5, 2.0);
    }
    if let Some(v) = o.underlay_spacing {
        e.underlay_spacing = v.clamp(0.5, 8.0);
    }
    if let Some(v) = o.underlay_inset {
        e.underlay_inset = v.clamp(0.0, 3.0);
    }
    e
}

impl Planned {
    /// What the operator sheet and layer list call this object's stitching.
    pub fn stitch_label(&self) -> &'static str {
        match self.kind {
            StitchType::Fill => self.opts.fill.short(),
            // auto-angle satin areas follow each stroke (turning satin)
            StitchType::Satin if self.opts.angle_deg.is_none() && self.knockouts.is_empty() => {
                "Turning satin"
            }
            k => k.label(),
        }
    }
}

pub fn generate(p: &Planned, r: &Recipe, profile: Profile, from: Pt) -> Vec<Needle> {
    let r = &effective(r, &p.opts);
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
            if p.opts.no_underlay {
                layers.clear();
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
            match p.opts.fill {
                FillPattern::Tatami => {
                    let rows = p.rows(rings, *rule, p.angle, r.fill_spacing, 0.3);
                    sew_layer(
                        &mut out,
                        rings,
                        *rule,
                        rows,
                        p.angle,
                        &|s| (s.x0 - pc, s.x1 + pc),
                        r.fill_len,
                        p.opts.stagger.unwrap_or(3).clamp(1, 8) as i64,
                        r.fill_spacing,
                        cur,
                        &|q| p.covered(q),
                    );
                }
                FillPattern::Contour | FillPattern::Spiral => {
                    let spiral = p.opts.fill == FillPattern::Spiral;
                    contour_fill(&mut out, p, rings, *rule, r, pc, cur, spiral);
                }
                FillPattern::Radial => {
                    radial_fill(&mut out, p, rings, *rule, r, pc, cur);
                }
            }
        }
        (ShapeKind::Area { rings, rule }, _)
            if p.opts.angle_deg.is_none() && p.knockouts.is_empty() && {
                let pc = pc + p.opts.widen / 2.0;
                turning_satin(&mut out, p, rings, *rule, r, pc, from)
            } => {}
        (ShapeKind::Area { rings, rule }, _) => {
            out.clear();
            let pc = pc + p.opts.widen / 2.0;
            // Satin from an outline. Stitches run across the stroke at the
            // object's angle; bands where that would make stitches too long
            // (crossbars, arch tops) are sewn at the perpendicular angle.
            let sp = r.satin_spacing / 2.0;
            let rows = p.rows(rings, *rule, p.angle, sp, 0.15);
            let mut lens: Vec<f64> = rows.iter().flatten().map(|g| g.len()).collect();
            lens.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = lens.get(lens.len() / 2).copied().unwrap_or(1.0);
            // Only rows far longer than this shape's normal stitch (crossbars,
            // arch tops) are re-sewn crosswise; plain wide columns are not.
            let limit = (median * 2.2).max(2.0);
            let long: Vec<Vec<Seg>> = rows
                .iter()
                .map(|r| {
                    r.iter()
                        .filter(|s| s.len() > limit)
                        .copied()
                        .collect::<Vec<_>>()
                })
                .filter(|r| !r.is_empty())
                .collect();
            let short: Vec<Vec<Seg>> = rows
                .into_iter()
                .map(|r| {
                    r.into_iter()
                        .filter(|s| s.len() <= limit)
                        .collect::<Vec<_>>()
                })
                .filter(|r| !r.is_empty())
                .collect();
            let mut cur = from;
            let mut layers: Vec<(Vec<Vec<Seg>>, f64)> = vec![(short, p.angle)];
            if !long.is_empty() {
                // Band mask, tested in the main frame.
                let in_long = |q: Pt| -> bool {
                    let r = q.rot(-p.angle);
                    let row = (r.y / sp - 0.5).round() as i64;
                    long.iter()
                        .flatten()
                        .any(|s| (s.row - row).abs() <= 1 && r.x >= s.x0 && r.x <= s.x1)
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
                                        segs.push(Seg {
                                            row: s.row,
                                            y: s.y,
                                            x0,
                                            x1: x,
                                        });
                                    }
                                    start = None;
                                }
                                _ => {}
                            }
                        }
                        if let Some(x0) = start {
                            if s.x1 - x0 >= 0.4 {
                                segs.push(Seg {
                                    row: s.row,
                                    y: s.y,
                                    x0,
                                    x1: s.x1,
                                });
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
                cur = sew_satin(
                    &mut out,
                    p,
                    rings,
                    *rule,
                    rows,
                    angle,
                    pc,
                    r.satin_spacing,
                    cur,
                );
            }
        }
        (ShapeKind::Line { paths, width }, kind) => {
            let mut cur = from;
            for path in order_paths(&p.visible_paths(paths), cur) {
                match kind {
                    StitchType::SatinColumn if p.opts.corner != Corner::Square => {
                        let width = p.opts.column_width.unwrap_or(*width);
                        let half = width / 2.0 + pc;
                        let closed = path.len() > 2 && path[0].dist(*path.last().unwrap()) < 0.05;
                        let ribs = column_ribs(&path, half, p.opts.corner, p.opts.cap, closed);
                        if !p.opts.no_underlay {
                            // centre run out, satin back over it
                            push_section(
                                &mut out,
                                &resample(&path, Limits::RUN_LEN),
                                Limits::RUN_LEN,
                            );
                            let back: Vec<(Pt, Pt)> = ribs.iter().rev().copied().collect();
                            let zz = ribs_to_zigzag(&back, r.satin_spacing / 2.0);
                            push_continue(
                                &mut out,
                                &split_long(&zz, Limits::SATIN_MAX),
                                Limits::SATIN_MAX,
                            );
                        } else {
                            let zz = ribs_to_zigzag(&ribs, r.satin_spacing / 2.0);
                            push_section(
                                &mut out,
                                &split_long(&zz, Limits::SATIN_MAX),
                                Limits::SATIN_MAX,
                            );
                        }
                    }
                    StitchType::SatinColumn => {
                        // Square corners: underlay runs forward along the whole
                        // path, then the satin comes back piece by piece. Sharp
                        // corners split the column and each piece runs past the
                        // corner by half the width, overlapping into a square.
                        let width = p.opts.column_width.unwrap_or(*width);
                        let half = width / 2.0 + pc;
                        let closed = path.len() > 2 && path[0].dist(*path.last().unwrap()) < 0.05;
                        let pieces = split_at_corners(&path, 35f64.to_radians());
                        if !p.opts.no_underlay {
                            push_section(
                                &mut out,
                                &resample(&path, Limits::RUN_LEN),
                                Limits::RUN_LEN,
                            );
                        }
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
                            if out.is_empty() || (p.opts.no_underlay && k + 1 == n) {
                                push_section(
                                    &mut out,
                                    &split_long(&sides, Limits::SATIN_MAX),
                                    Limits::SATIN_MAX,
                                );
                                continue;
                            }
                            push_continue(
                                &mut out,
                                &split_long(&sides, Limits::SATIN_MAX),
                                Limits::SATIN_MAX,
                            );
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

/// Join the next piece of a shaped fill: a direct stitch when it is close,
/// hidden travel inside the shape when possible, otherwise a jump.
#[allow(clippy::too_many_arguments)]
fn join_piece(
    out: &mut Vec<Needle>,
    rings: &[Ring],
    rule: FillRule,
    pts: &[Pt],
    len: f64,
    near: f64,
    hidden: &dyn Fn(Pt) -> bool,
) {
    let Some(first) = pts.first().copied() else {
        return;
    };
    match out.last().map(|n| n.p) {
        Some(c) if c.dist(first) <= near => {
            // skip points right on top of the needle (e.g. where spiral
            // rings meet) so there are no tiny stitches
            let k = pts.iter().take_while(|q| q.dist(c) < 0.3).count();
            push_continue(out, &pts[k.min(pts.len())..], len)
        }
        _ => start_in_shape(out, rings, rule, pts, len, hidden),
    }
}

/// Contour fill: rings that follow the outline inward, outermost first.
/// Spiral: the same rings, each one drifting inward as it goes round so
/// they join into one continuous line.
#[allow(clippy::too_many_arguments)]
fn contour_fill(
    out: &mut Vec<Needle>,
    p: &Planned,
    rings: &[Ring],
    rule: FillRule,
    r: &Recipe,
    pc: f64,
    from: Pt,
    spiral: bool,
) -> Pt {
    let sp = r.fill_spacing;
    // Rings curve all the way round, so stitches stay shorter than tatami;
    // the outermost ring (the visible edge) shortest of all.
    let len_in = r.fill_len.min(2.8);
    let field = Field::new(rings, rule, (sp / 3.0).clamp(0.05, 0.2));
    let mut cur = from;
    let mut k = 0usize;
    loop {
        let level = sp * (k as f64 + 0.5) - pc;
        if level > field.max() - sp * 0.3 || k > 2000 {
            break;
        }
        let mut loops = field.loops(level);
        let deeper = |q: Pt| p.covered(q) || field.at(q) > level + sp * 0.6;
        while !loops.is_empty() {
            // nearest ring next, starting at its point nearest the needle
            let (li, si) = loops
                .iter()
                .enumerate()
                .flat_map(|(i, l)| l.iter().enumerate().map(move |(j, q)| (i, j, q.dist(cur))))
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
                .map(|(i, j, _)| (i, j))
                .unwrap();
            let mut ring = loops.swap_remove(li);
            ring.rotate_left(si);
            let len = if k == 0 { len_in.min(2.0) } else { len_in };
            let mut pts = even_loop(&ring, len);
            if spiral {
                let n = pts.len().max(2) - 1;
                for (i, q) in pts.iter_mut().enumerate() {
                    let u = i as f64 / n as f64;
                    *q = q.add(field.inward(*q).mul(u * sp));
                }
            }
            let pieces = if p.knockouts.is_empty() {
                vec![pts]
            } else {
                p.visible_paths(&[pts])
                    .into_iter()
                    .map(|v| resample(&v, len))
                    .collect()
            };
            for piece in pieces {
                join_piece(out, rings, rule, &piece, len, sp * 3.0 + 0.3, &deeper);
                cur = out.last().map(|n| n.p).unwrap_or(cur);
            }
        }
        k += 1;
    }
    cur
}

/// Turning satin: satin that runs across each stroke and turns with it as
/// it curves, the way lettering is hand-digitized. Strokes come from the
/// shape's centre line (see `skeleton`). Each stroke gets a centre-run
/// underlay on the way out (which is also how the needle travels, hidden
/// under the satin) and its satin on the way back; strokes that carry on
/// straight through a junction are sewn as one unbroken column.
/// Returns false (and sews nothing) if the shape has no usable centre line.
#[allow(clippy::too_many_arguments)]
fn turning_satin(
    out: &mut Vec<Needle>,
    p: &Planned,
    rings: &[Ring],
    rule: FillRule,
    r: &Recipe,
    pc: f64,
    from: Pt,
) -> bool {
    use crate::skeleton::{Graph, skeleton};
    let b = BBox::of(rings.iter().flatten());
    let res = (b.width().max(b.height()) / 500.0).clamp(0.04, 0.12);
    let field = Field::new(rings, rule, res);
    let g: Graph = skeleton(&field);
    let total: f64 = g
        .edges
        .iter()
        .map(|e| e.pts.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>())
        .sum();
    if g.edges.is_empty() || total < 1.0 {
        return false;
    }
    let deg: Vec<usize> = (0..g.nodes.len()).map(|n| g.degree(n)).collect();
    // Edge path as seen leaving node `n` (self-loops: as stored).
    let leaving = |e: usize, n: usize| -> Vec<Pt> {
        let ed = &g.edges[e];
        if ed.a == n {
            ed.pts.clone()
        } else {
            ed.pts.iter().rev().copied().collect()
        }
    };
    let dir_from = |e: usize, n: usize| -> Pt {
        let pts = leaving(e, n);
        let mut acc = 0.0;
        for w in pts.windows(2) {
            acc += w[0].dist(w[1]);
            if acc >= 1.0 {
                return w[1].sub(pts[0]).norm();
            }
        }
        pts.last().unwrap().sub(pts[0]).norm()
    };
    // Straight-through pairs at junctions: the two strokes that line up best.
    let mut through: std::collections::HashMap<(usize, usize), usize> = Default::default();
    for n in 0..g.nodes.len() {
        if deg[n] < 3 {
            continue;
        }
        let es: Vec<usize> = (0..g.edges.len())
            .filter(|&k| (g.edges[k].a == n) != (g.edges[k].b == n))
            .collect();
        let mut best: Option<(f64, usize, usize)> = None;
        for i in 0..es.len() {
            for j in i + 1..es.len() {
                let (d1, d2) = (dir_from(es[i], n), dir_from(es[j], n));
                let dot = d1.x * d2.x + d1.y * d2.y; // -1 = straight on
                if dot < -0.77 && best.map(|b| dot < b.0).unwrap_or(true) {
                    best = Some((dot, es[i], es[j]));
                }
            }
        }
        if let Some((_, e1, e2)) = best {
            through.insert((n, e1), e2);
            through.insert((n, e2), e1);
        }
    }
    // How each end of a stroke piece is finished.
    #[derive(Clone, Copy, PartialEq)]
    enum End {
        /// A free end: run on to the tip, satin covers serifs straight on.
        Leaf,
        /// A sharp turn inside a stroke (the corners of an N): each side
        /// runs past the corner and overlaps, like a hand-made square corner.
        Corner,
        /// Joins another stroke: tuck a little way under it.
        Tuck,
        /// Carries straight on into the next stroke as one column.
        Through,
    }
    let end_kind = |n: usize, e: usize| -> End {
        if deg[n] == 1 {
            End::Leaf
        } else if through.contains_key(&(n, e)) || g.edges[e].a == g.edges[e].b {
            End::Through
        } else {
            End::Tuck
        }
    };
    let end_ext = |k: End, d: f64| -> f64 {
        match k {
            End::Leaf => d * 1.3 + 0.3,
            End::Corner => d + 0.1,
            End::Tuck => d * 0.7,
            End::Through => 0.0,
        }
    };
    // How far an end's stitches may reach past the stroke's own width
    // (serifs widen a free end; a tuck must not run along the other stroke).
    let end_reach = |k: End| -> f64 {
        match k {
            End::Leaf => 2.4,
            End::Corner => 1.5,
            End::Tuck => 1.15,
            End::Through => 1.6,
        }
    };
    // Split a stroke path where it turns sharply.
    let split_sharp = |path: &[Pt]| -> Vec<Vec<Pt>> {
        let sp = resample(path, 0.1);
        let n = sp.len();
        if n < 12 {
            return vec![sp];
        }
        let turn = |i: usize| -> f64 {
            let k = ((field.at(sp[i]) * 1.5 / 0.1) as usize).clamp(5, 25);
            if i < k || i + k >= n {
                return 0.0;
            }
            let (a, b) = (sp[i].sub(sp[i - k]).norm(), sp[i + k].sub(sp[i]).norm());
            (a.x * b.x + a.y * b.y).clamp(-1.0, 1.0).acos()
        };
        let turns: Vec<f64> = (0..n).map(turn).collect();
        let mut cuts = Vec::new();
        let mut i = 0;
        while i < n {
            if turns[i] > 55f64.to_radians() {
                let mut j = i;
                let mut best = i;
                while j < n && turns[j] > 55f64.to_radians() {
                    if turns[j] > turns[best] {
                        best = j;
                    }
                    j += 1;
                }
                let last = cuts.last().copied().unwrap_or(0);
                if best >= last + 6 && best + 6 < n {
                    cuts.push(best);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        let mut out = Vec::new();
        let mut from = 0;
        for c in cuts {
            out.push(sp[from..=c].to_vec());
            from = c;
        }
        out.push(sp[from..].to_vec());
        out
    };
    // Ribs across a stroke piece (in its direction), from edge to edge.
    let ribs = |path: &[Pt], k0: End, k1: End| -> Vec<(Pt, Pt)> {
        let mut pts = path.to_vec();
        if pts.len() < 2 {
            return Vec::new();
        }
        let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
        let (dd0, dd1) = (field.at(pts[0]).max(0.1), field.at(*pts.last().unwrap()).max(0.1));
        let (e0, e1) = (end_ext(k0, dd0), end_ext(k1, dd1));
        let back = |v: &[Pt], from_end: bool| -> Pt {
            // direction over the last ~0.5 mm, for straight extensions
            let n = v.len();
            let k = 5.min(n - 1);
            if from_end {
                v[n - 1].sub(v[n - 1 - k]).norm()
            } else {
                v[0].sub(v[k]).norm()
            }
        };
        let (d0, d1) = (back(&pts, false), back(&pts, true));
        if e0 > 0.0 {
            pts.insert(0, pts[0].add(d0.mul(e0)));
        }
        if e1 > 0.0 {
            let l = *pts.last().unwrap();
            pts.push(l.add(d1.mul(e1)));
        }
        let sp = resample(&pts, 0.1);
        let n = sp.len();
        // near the ends, hold the stitch direction steady
        let zone = |k: End, d: f64| if k == End::Through { 0.0 } else { (d * 2.0 + 0.4).min(len * 0.45) };
        let i0 = (((e0 + zone(k0, dd0)) / 0.1) as usize).min(n - 1);
        let i1 = n.saturating_sub(1 + ((e1 + zone(k1, dd1)) / 0.1) as usize).max(i0);
        let k = 8usize;
        let mut raw: Vec<(usize, Pt, Pt, f64, f64)> = Vec::with_capacity(n);
        for i in 0..n {
            let c = sp[i];
            let depth = field.at(c);
            if depth < 0.08 {
                continue;
            }
            let j = i.clamp(i0, i1);
            let t = sp[(j + k).min(n - 1)].sub(sp[j.saturating_sub(k)]);
            if t.len() < 1e-9 {
                continue;
            }
            let t = t.norm();
            let nrm = Pt::new(-t.y, t.x);
            let limit = depth * 2.4 + 0.8;
            let march = |dir: Pt| -> f64 {
                let mut lo = 0.0;
                let mut x = 0.05;
                while x <= limit {
                    if field.at(c.add(dir.mul(x))) <= 0.0 {
                        let mut hi = x;
                        for _ in 0..8 {
                            let m = (lo + hi) / 2.0;
                            if field.at(c.add(dir.mul(m))) > 0.0 {
                                lo = m;
                            } else {
                                hi = m;
                            }
                        }
                        return (lo + hi) / 2.0;
                    }
                    lo = x;
                    x += 0.05;
                }
                limit
            };
            raw.push((i, c, nrm, march(nrm), march(nrm.mul(-1.0))));
        }
        // the stroke's own half-widths just inside each end zone
        let med = |lo: usize, hi: usize| -> (f64, f64) {
            let mut l: Vec<f64> = raw.iter().filter(|r| r.0 >= lo && r.0 <= hi).map(|r| r.3).collect();
            let mut rr: Vec<f64> = raw.iter().filter(|r| r.0 >= lo && r.0 <= hi).map(|r| r.4).collect();
            if l.is_empty() {
                l = raw.iter().map(|r| r.3).collect();
                rr = raw.iter().map(|r| r.4).collect();
            }
            l.sort_by(|a, b| a.partial_cmp(b).unwrap());
            rr.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (l.get(l.len() / 2).copied().unwrap_or(1.0), rr.get(rr.len() / 2).copied().unwrap_or(1.0))
        };
        let (m0, m1) = (med(i0, i0 + 15), med(i1.saturating_sub(15), i1));
        raw.into_iter()
            .map(|(i, c, nrm, mut l, mut rr)| {
                if i < i0 {
                    l = l.min(m0.0 * end_reach(k0) + 0.1);
                    rr = rr.min(m0.1 * end_reach(k0) + 0.1);
                } else if i > i1 {
                    l = l.min(m1.0 * end_reach(k1) + 0.1);
                    rr = rr.min(m1.1 * end_reach(k1) + 0.1);
                }
                (c.add(nrm.mul(l + pc)), c.sub(nrm.mul(rr + pc)))
            })
            .collect()
    };
    // A stroke as pieces of ribs, split at sharp turns.
    let stroke_ribs = |path: &[Pt], k0: End, k1: End| -> Vec<Vec<(Pt, Pt)>> {
        let pieces = split_sharp(path);
        let m = pieces.len();
        pieces
            .iter()
            .enumerate()
            .map(|(i, pc_)| {
                let a = if i == 0 { k0 } else { End::Corner };
                let b = if i + 1 == m { k1 } else { End::Corner };
                ribs(pc_, a, b)
            })
            .filter(|v| v.len() >= 2)
            .collect()
    };
    let underlay = !p.opts.no_underlay;
    let step = r.satin_spacing / 2.0;
    let sew_back = |out: &mut Vec<Needle>, back: &[Vec<(Pt, Pt)>]| {
        for piece in back {
            if piece.len() < 2 {
                continue;
            }
            let zz = ribs_to_zigzag(piece, step);
            push_continue(out, &split_long(&zz, Limits::SATIN_MAX), Limits::SATIN_MAX);
        }
    };
    // Out along a stroke: centre run (or a light zig-zag on wide strokes).
    let run_out = |out: &mut Vec<Needle>, path: &[Pt], fwd: &[Vec<(Pt, Pt)>]| {
        let fwd: Vec<(Pt, Pt)> = fwd.iter().flatten().copied().collect();
        let wide = fwd.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / fwd.len().max(1) as f64 > 3.5;
        if underlay && wide {
            let inset = r.underlay_inset + pc;
            let inner: Vec<(Pt, Pt)> = fwd
                .iter()
                .map(|(a, b)| {
                    let m = a.lerp(*b, 0.5);
                    let h = (a.dist(*b) / 2.0 - inset).max(0.2);
                    let d = a.sub(*b).norm();
                    (m.add(d.mul(h)), m.sub(d.mul(h)))
                })
                .collect();
            let zz = ribs_to_zigzag(&inner, r.underlay_spacing.min(2.0) / 2.0);
            push_continue(out, &zz, Limits::SATIN_MAX);
        } else {
            let pts = resample(path, Limits::RUN_LEN);
            push_continue(out, &pts[1..], Limits::RUN_LEN);
        }
    };
    let mut edge_done = vec![false; g.edges.len()];
    let mut node_seen = vec![false; g.nodes.len()];
    // Depth-first over strokes, kept as an explicit stack. Each frame is a
    // stroke on its way out; side strokes at its far end are sewn first,
    // then the straight-through stroke, whose satin is owed back and sewn
    // together with this one as one unbroken column.
    struct Frame {
        e: usize,
        y: usize,
        fwd: Vec<Vec<(Pt, Pt)>>,
        pending: Vec<usize>,
        thru: Option<usize>,
        owed: Vec<Vec<(Pt, Pt)>>,
        stage: u8,
    }
    let start = (0..g.nodes.len())
        .filter(|&n| deg[n] == 1)
        .min_by(|&a, &b| {
            g.nodes[a]
                .dist(from)
                .partial_cmp(&g.nodes[b].dist(from))
                .unwrap()
        })
        .or_else(|| {
            (0..g.nodes.len())
                .filter(|&n| deg[n] > 0)
                .min_by(|&a, &b| g.nodes[a].dist(from).partial_cmp(&g.nodes[b].dist(from)).unwrap())
        });
    let Some(mut root) = start else {
        return false;
    };
    let begin = out.len();
    loop {
        node_seen[root] = true;
        // the start point: a free end begins at its tip
        let first_e = (0..g.edges.len()).find(|&k| !edge_done[k] && (g.edges[k].a == root || g.edges[k].b == root));
        let Some(first_e) = first_e else { break };
        let start_pt = {
            let path = leaving(first_e, root);
            let e0 = end_ext(end_kind(root, first_e), field.at(g.nodes[root]).max(0.1));
            let d = path[0].sub(path[1.min(path.len() - 1)]).norm();
            let tip = path[0].add(d.mul((e0 - 0.4).max(0.0)));
            if field.at(tip) > 0.05 { tip } else { path[0] }
        };
        if out.is_empty() {
            out.push(Needle { p: start_pt, jump: true });
        } else {
            start_in_shape(out, rings, rule, &[start_pt], Limits::RUN_LEN, &|q| p.covered(q));
        }
        let mut stack: Vec<Frame> = Vec::new();
        // root level: each stroke at the root is its own column
        let mut root_edges: Vec<usize> = (0..g.edges.len())
            .filter(|&k| !edge_done[k] && (g.edges[k].a == root || g.edges[k].b == root))
            .collect();
        root_edges.reverse();
        let open = |e: usize, x: usize, edge_done: &mut Vec<bool>, out: &mut Vec<Needle>| -> Frame {
            edge_done[e] = true;
            let path = leaving(e, x);
            let y = if g.edges[e].a == x { g.edges[e].b } else { g.edges[e].a };
            let fwd = stroke_ribs(&path, end_kind(x, e), end_kind(y, e));
            run_out(out, &path, &fwd);
            Frame { e, y, fwd, pending: Vec::new(), thru: None, owed: Vec::new(), stage: 0 }
        };
        while let Some(e) = root_edges.pop() {
            if edge_done[e] {
                continue;
            }
            stack.push(open(e, root, &mut edge_done, out));
            while let Some(top) = stack.last_mut() {
                match top.stage {
                    0 => {
                        let y = top.y;
                        top.stage = 1;
                        if !node_seen[y] {
                            node_seen[y] = true;
                            let thru = through.get(&(y, top.e)).copied().filter(|&f| !edge_done[f]);
                            top.thru = thru;
                            top.pending = (0..g.edges.len())
                                .filter(|&k| {
                                    !edge_done[k]
                                        && Some(k) != thru
                                        && (g.edges[k].a == y || g.edges[k].b == y)
                                })
                                .collect();
                        }
                    }
                    1 => {
                        // side strokes at y: each sewn completely, back to y
                        if let Some(k) = top.pending.pop() {
                            if !edge_done[k] {
                                let y = top.y;
                                let f = open(k, y, &mut edge_done, out);
                                stack.push(f);
                            }
                        } else {
                            top.stage = 2;
                            if let Some(f) = top.thru.filter(|&f| !edge_done[f]) {
                                let y = top.y;
                                let fr = open(f, y, &mut edge_done, out);
                                stack.last_mut().unwrap().stage = 3; // wait for through
                                stack.push(fr);
                            }
                        }
                    }
                    2 | 3 => {
                        let mut fr = stack.pop().unwrap();
                        // owed pieces (from the far end), then this stroke's
                        // pieces backwards; the two meet in one column
                        let mut back: Vec<Vec<(Pt, Pt)>> = std::mem::take(&mut fr.owed);
                        for (i, piece) in fr.fwd.iter().rev().enumerate() {
                            let rev: Vec<(Pt, Pt)> = piece.iter().rev().map(|(a, b)| (*b, *a)).collect();
                            match back.last_mut() {
                                Some(last) if i == 0 => last.extend(rev),
                                _ => back.push(rev),
                            }
                        }
                        // owed to the parent if this was its through-stroke
                        match stack.last_mut() {
                            Some(parent) if parent.stage == 3 && parent.thru == Some(fr.e) => {
                                parent.owed = back;
                                parent.stage = 2;
                            }
                            _ => sew_back(out, &back),
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        // anything not reached (a separate piece of centre line)
        let next = (0..g.edges.len()).find(|&k| !edge_done[k]);
        match next {
            Some(k) => {
                let (a, b) = (g.edges[k].a, g.edges[k].b);
                let cur = out.last().map(|n| n.p).unwrap_or(from);
                root = if g.nodes[a].dist(cur) <= g.nodes[b].dist(cur) { a } else { b };
            }
            None => break,
        }
    }
    out.len() > begin + 2
}

/// A traced ring smoothed (the grid leaves tiny steps) and cut into even
/// stitches, closed back to its start. Small, tight rings get shorter
/// stitches so they stay round.
fn even_loop(ring: &[Pt], len: f64) -> Vec<Pt> {
    let n = ring.len();
    let mut pts = ring.to_vec();
    for _ in 0..4 {
        pts = (0..n)
            .map(|i| {
                let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
                Pt::new((a.x + 2.0 * b.x + c.x) / 4.0, (a.y + 2.0 * b.y + c.y) / 4.0)
            })
            .collect();
    }
    pts.push(pts[0]);
    let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let len = len.min((total / 14.0).max(0.8));
    let k = (total / len).ceil().max(3.0) as usize;
    let step = total / k as f64;
    let mut out = vec![pts[0]];
    let (mut acc, mut next) = (0.0, step);
    for w in pts.windows(2) {
        let d = w[0].dist(w[1]);
        while d > 0.0 && acc + d >= next && out.len() < k {
            out.push(w[0].lerp(w[1], (next - acc) / d));
            next += step;
        }
        acc += d;
    }
    out.push(pts[0]);
    out
}

/// Radial fill: rows from the middle of the shape out to its edge, with
/// alternating rays stopping short of the centre so it doesn't pile up.
#[allow(clippy::too_many_arguments)]
fn radial_fill(
    out: &mut Vec<Needle>,
    p: &Planned,
    rings: &[Ring],
    rule: FillRule,
    r: &Recipe,
    pc: f64,
    from: Pt,
) -> Pt {
    let centre = fills::centroid(rings);
    let len = r.fill_len;
    let stagger = p.opts.stagger.unwrap_or(3).clamp(1, 8) as usize;
    let mut rays = fills::rays(rings, rule, centre, r.fill_spacing, pc);
    // start at the ray nearest the needle and go round
    if let Some(k) = rays
        .iter()
        .enumerate()
        .min_by(|a, b| {
            let pa = centre.add(Pt::new(a.1.x1, 0.0).rot(a.1.angle));
            let pb = centre.add(Pt::new(b.1.x1, 0.0).rot(b.1.angle));
            pa.dist(from).partial_cmp(&pb.dist(from)).unwrap()
        })
        .map(|(k, _)| k)
    {
        rays.rotate_left(k);
    }
    let mut outward = false;
    let mut cur = from;
    let n_rays = rays.iter().map(|r| r.index).max().unwrap_or(0) + 1;
    let mut prev: Option<usize> = None;
    for ray in &rays {
        // Neighbouring rays: the step between their inner ends runs along
        // the next ray, which sews straight over it, so it never shows.
        let adjacent = prev
            .map(|i| {
                let d = (ray.index + n_rays - i) % n_rays;
                // (rays that would end before reaching the edge are skipped,
                // so neighbours can be a few indexes apart)
                d <= 8 || d >= n_rays - 8
            })
            .unwrap_or(false);
        prev = Some(ray.index);
        let off = (ray.index % stagger) as f64 * len / stagger as f64;
        let mut xs = vec![ray.x0];
        let mut m = ((ray.x0 + 0.6 - off) / len).ceil() as i64;
        loop {
            let x = off + m as f64 * len;
            if x >= ray.x1 - 0.6 {
                break;
            }
            if x > ray.x0 + 0.6 {
                xs.push(x);
            }
            m += 1;
        }
        xs.push(ray.x1);
        if !outward {
            xs.reverse();
        }
        outward = !outward;
        let pts: Vec<Pt> = xs
            .into_iter()
            .map(|x| centre.add(Pt::new(x, 0.0).rot(ray.angle)))
            .collect();
        let pieces = if p.knockouts.is_empty() {
            vec![pts]
        } else {
            p.visible_paths(&[pts])
                .into_iter()
                .map(|v| resample(&v, len))
                .collect()
        };
        for piece in pieces {
            let near = if adjacent { 15.0 } else { 2.5 };
            join_piece(out, rings, rule, &piece, len, near, &|q| p.covered(q));
            cur = out.last().map(|n| n.p).unwrap_or(cur);
        }
    }
    cur
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
    for (run, rev) in order_runs(group_runs(rows), angle, cur, &|x, y| {
        travel_ok(rings, rule, x, y, &|q| p.covered(q))
    }) {
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
        if segs.len() >= 6 && !p.opts.no_underlay {
            start_in_shape(out, rings, rule, &center, Limits::RUN_LEN, &|_| true);
            push_continue(
                out,
                &split_long(&sides, Limits::SATIN_MAX),
                Limits::SATIN_MAX,
            );
        } else {
            start_in_shape(
                out,
                rings,
                rule,
                &split_long(&sides, Limits::SATIN_MAX),
                Limits::SATIN_MAX,
                &|q| p.covered(q),
            );
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

/// Left-hand normal of a direction.
fn left_of(d: Pt) -> Pt {
    Pt::new(-d.y, d.x)
}

/// Matched (left, right) edge points across a satin column, following the
/// centre line, with mitered or rounded corners and the chosen end caps.
pub fn column_ribs(
    path: &[Pt],
    half: f64,
    corner: Corner,
    cap: Cap,
    closed: bool,
) -> Vec<(Pt, Pt)> {
    let mut pts: Vec<Pt> = Vec::new();
    for &q in path {
        if pts.last().map(|l: &Pt| l.dist(q) > 1e-6).unwrap_or(true) {
            pts.push(q);
        }
    }
    if closed && pts.len() > 2 && pts[0].dist(*pts.last().unwrap()) < 0.05 {
        pts.pop();
    }
    let n = pts.len();
    if n < 2 {
        return vec![];
    }
    let dir = |i: usize, j: usize| pts[j].sub(pts[i]).norm();
    let mut ribs: Vec<(Pt, Pt)> = Vec::new();
    let smooth = 25f64.to_radians().cos();
    for i in 0..n {
        let c = pts[i];
        let din = if i > 0 {
            Some(dir(i - 1, i))
        } else if closed {
            Some(dir(n - 1, 0))
        } else {
            None
        };
        let dout = if i + 1 < n {
            Some(dir(i, i + 1))
        } else if closed {
            Some(dir(n - 1, 0))
        } else {
            None
        };
        match (din, dout) {
            (None, Some(d)) => {
                let nl = left_of(d);
                // start cap: from the tip toward the column
                ribs.extend(cap_ribs(c, d.mul(-1.0), nl, half, cap).into_iter().rev());
                ribs.push((c.add(nl.mul(half)), c.sub(nl.mul(half))));
            }
            (Some(d), None) => {
                let nl = left_of(d);
                ribs.push((c.add(nl.mul(half)), c.sub(nl.mul(half))));
                ribs.extend(cap_ribs(c, d, nl, half, cap));
            }
            (Some(a), Some(b)) => {
                let (na, nb) = (left_of(a), left_of(b));
                let cosang = a.x * b.x + a.y * b.y;
                let bis = na.add(nb).norm();
                let k = (bis.x * na.x + bis.y * na.y).max(1e-6); // cos(turn/2)
                let m = half / k;
                // Keep stitches square to the column until half a width (more on
                // sharper turns) before the corner, then fan through it.
                let prev = if i > 0 { pts[i - 1] } else { pts[n - 1] };
                let next = if i + 1 < n { pts[i + 1] } else { pts[0] };
                let half_turn = cosang.clamp(-1.0, 1.0).acos() / 2.0;
                let e = (half * half_turn.tan())
                    .min(0.45 * c.dist(prev))
                    .min(0.45 * c.dist(next));
                let sharp = cosang < smooth;
                if sharp && e > 0.05 {
                    let q = c.sub(a.mul(e));
                    ribs.push((q.add(na.mul(half)), q.sub(na.mul(half))));
                }
                if !sharp || (corner == Corner::Miter && m <= half * 3.0) {
                    ribs.push((c.add(bis.mul(m)), c.sub(bis.mul(m))));
                } else {
                    // Inner side: the miter point (clamped). Outer side: a bevel
                    // (two points) or an arc.
                    let left_inner = b.x * na.x + b.y * na.y > 0.0;
                    let mi = m.min(half * 3.0);
                    let inner = if left_inner {
                        c.add(bis.mul(mi))
                    } else {
                        c.sub(bis.mul(mi))
                    };
                    let (oa, ob) = if left_inner {
                        (na.mul(-1.0), nb.mul(-1.0))
                    } else {
                        (na, nb)
                    };
                    let steps = if corner == Corner::Round {
                        let ang = cosang.clamp(-1.0, 1.0).acos();
                        (ang / 12f64.to_radians()).ceil().max(2.0) as usize
                    } else {
                        1
                    };
                    let (a0, a1) = (oa.y.atan2(oa.x), ob.y.atan2(ob.x));
                    let mut da = a1 - a0;
                    while da > std::f64::consts::PI {
                        da -= 2.0 * std::f64::consts::PI;
                    }
                    while da < -std::f64::consts::PI {
                        da += 2.0 * std::f64::consts::PI;
                    }
                    for s in 0..=steps {
                        let t = a0 + da * s as f64 / steps as f64;
                        let outer = c.add(Pt::new(t.cos(), t.sin()).mul(half));
                        ribs.push(if left_inner {
                            (inner, outer)
                        } else {
                            (outer, inner)
                        });
                    }
                }
                if sharp && e > 0.05 {
                    let q = c.add(b.mul(e));
                    ribs.push((q.add(nb.mul(half)), q.sub(nb.mul(half))));
                }
            }
            (None, None) => {}
        }
    }
    if closed && !ribs.is_empty() {
        ribs.push(ribs[0]);
    }
    ribs
}

/// Ribs beyond an open end at `c` going in direction `d` (outward).
fn cap_ribs(c: Pt, d: Pt, nl: Pt, half: f64, cap: Cap) -> Vec<(Pt, Pt)> {
    let min_half = 0.2; // keep the last stitch at least 0.4 mm wide
    match cap {
        Cap::Flat => vec![],
        Cap::Square => {
            let e = c.add(d.mul(half));
            vec![(e.add(nl.mul(half)), e.sub(nl.mul(half)))]
        }
        Cap::Round => {
            let max_phi = (min_half / half).clamp(0.0, 1.0).acos();
            let k = ((max_phi / 15f64.to_radians()).ceil() as usize).max(2);
            (1..=k)
                .map(|i| {
                    let phi = max_phi * i as f64 / k as f64;
                    let e = c.add(d.mul(half * phi.sin()));
                    let w = half * phi.cos();
                    (e.add(nl.mul(w)), e.sub(nl.mul(w)))
                })
                .collect()
        }
        Cap::Point => {
            let k = 4;
            (1..=k)
                .map(|i| {
                    let t = i as f64 / k as f64;
                    let w = (half * (1.0 - t)).max(min_half);
                    let e = c.add(d.mul(half * 1.2 * t));
                    (e.add(nl.mul(w)), e.sub(nl.mul(w)))
                })
                .collect()
        }
    }
}

/// Zig-zag between the two edges, alternating sides every `step`, measured
/// continuously along whichever edge moves more (so corners stay covered).
fn ribs_to_zigzag(ribs: &[(Pt, Pt)], step: f64) -> Vec<Pt> {
    if ribs.len() < 2 {
        return ribs.iter().map(|r| r.0).collect();
    }
    let seg: Vec<f64> = ribs
        .windows(2)
        .map(|w| w[0].0.dist(w[1].0).max(w[0].1.dist(w[1].1)))
        .collect();
    let total: f64 = seg.iter().sum();
    let n = (total / step).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(n + 1);
    let (mut k, mut start) = (0usize, 0.0);
    for j in 0..=n {
        let target = total * j as f64 / n as f64;
        while k + 1 < seg.len() && start + seg[k] < target {
            start += seg[k];
            k += 1;
        }
        let t = if seg[k] > 0.0 {
            ((target - start) / seg[k]).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let (a, b) = (ribs[k], ribs[k + 1]);
        out.push(if j % 2 == 0 {
            a.0.lerp(b.0, t)
        } else {
            a.1.lerp(b.1, t)
        });
    }
    out
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
