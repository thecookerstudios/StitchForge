//! Stitch generation. Every object keeps its vector shape and settings;
//! stitches are always regenerated from them.

use crate::geom::*;
use crate::fills::{self, Field};
use crate::import::{Artwork, Cap, Corner, FillPattern, ShapeKind, ShapeOpts, StitchChoice, Underlay};
use crate::settings::{Limits, Profile, Recipe};
use crate::thread::{Thread, nearest};
use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Needle {
    pub p: Pt,
    /// Travel to this point without sewing (start of a new section).
    pub jump: bool,
    /// Part of the underlay (the foundation sewn before the top stitches).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub under: bool,
}

impl Needle {
    pub fn new(p: Pt, jump: bool) -> Self {
        Needle { p, jump, under: false }
    }
}

/// Mark needle points from `from` on as underlay.
fn mark_under(out: &mut [Needle], from: usize) {
    for n in out.iter_mut().skip(from) {
        n.under = true;
    }
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
    /// 3D puff: 0 = not puff, 1 = the contour sewn before the foam goes on,
    /// 2 = the tack-down, satin and end caps sewn over the foam.
    pub puff_stage: u8,
    /// Appliqué: 0 = not appliqué, 1 = placement line, 2 = tack-down,
    /// 3 = cover stitch over the edge.
    pub applique_stage: u8,
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
                puff_stage: 0,
                applique_stage: 0,
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
                if o.applique.is_some() {
                    // appliqué: each piece becomes its placement, tack-down
                    // and cover (split into stages after this loop)
                    for (k, rings) in islands.iter().enumerate() {
                        let area = rings_area(rings, *rule);
                        if area < Limits::MIN_AREA {
                            continue;
                        }
                        let name = if many { format!("{name} {}", k + 1) } else { name.clone() };
                        let bbox = BBox::of(rings.iter().flatten());
                        let shape = ShapeKind::Area { rings: rings.clone(), rule: *rule };
                        let mut q = mk(name, StitchType::Run, shape, bbox, 0.0);
                        q.applique_stage = 1;
                        out.push(q);
                    }
                    continue;
                }
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
                        _ if o.puff => true,
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
                        if o.stitch == StitchChoice::Satin || o.puff || best.2 <= Limits::SATIN_MAX * 1.25 {
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
    // Appliqué pieces of one layer sew stage by stage: every placement line,
    // (stop) every tack-down, (stop to trim if cut in place) every cover.
    if out.iter().any(|p| p.applique_stage > 0) {
        let mut ex: Vec<Planned> = Vec::with_capacity(out.len() * 3);
        let mut i = 0;
        while i < out.len() {
            if out[i].applique_stage == 0 {
                ex.push(out[i].clone());
                i += 1;
                continue;
            }
            let layer = out[i].layer;
            let mut j = i;
            while j < out.len() && out[j].applique_stage > 0 && out[j].layer == layer {
                j += 1;
            }
            let a = out[i].opts.applique.clone().unwrap_or_default();
            let stages: Vec<u8> = [
                (1u8, a.placement),
                (2u8, a.tack != crate::applique::Tack::None),
                (3u8, a.cover != crate::applique::Cover::None),
            ]
            .iter()
            .filter(|(_, on)| *on)
            .map(|(s, _)| *s)
            .collect();
            for st in stages {
                for piece in &out[i..j] {
                    let mut q = piece.clone();
                    q.applique_stage = st;
                    if st == 3 && a.cover == crate::applique::Cover::Satin {
                        q.kind = StitchType::SatinColumn;
                    }
                    ex.push(q);
                }
            }
            i = j;
        }
        out = ex;
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
    // recipes are tuned for 40 wt: finer thread sews closer, heavier wider
    let w = o.weight.unwrap_or(r.weight);
    let k = w.spacing_factor();
    e.fill_spacing *= k;
    e.satin_spacing *= k;
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
    if let Some(v) = o.underlay_len {
        e.underlay_len = v.clamp(1.0, 7.0);
    }
    // 3D puff: satin packed about twice as tight so it covers and cuts the
    // foam cleanly, with a little more pull compensation over the foam
    if o.puff {
        if o.density.is_none() {
            e.satin_spacing = 0.2;
        }
        if o.pull_comp.is_none() {
            e.pull_comp += 0.1;
        }
    }
    e
}

impl Planned {
    /// Longest satin stitch before it is split (mm).
    pub fn satin_max(&self) -> f64 {
        // puff columns stay whole (a split would cut the foam open)
        let default = if self.opts.puff { 12.0 } else { Limits::SATIN_MAX };
        self.opts.split_width.unwrap_or(default).clamp(3.0, 12.5)
    }
    fn short_stitches(&self) -> bool {
        self.opts.short_stitches.unwrap_or(true)
    }
    /// Satin underlay actually used for a column of this average width (mm).
    fn satin_underlay(&self, width: f64, profile: Profile) -> Underlay {
        if self.opts.no_underlay || self.puff_stage == 2 {
            return Underlay::Off;
        }
        match self.opts.satin_underlay {
            Underlay::Auto | Underlay::Tatami | Underlay::EdgeTatami | Underlay::DoubleTatami => {
                if width <= 3.5 {
                    Underlay::CenterRun
                } else if profile == Profile::Cap {
                    Underlay::EdgeZigzag
                } else {
                    Underlay::Zigzag
                }
            }
            u => u,
        }
    }
    /// The underlay this object actually gets, in words with its numbers
    /// (what Auto resolved to, row spacing, inset).
    pub fn underlay_summary(&self, r: &Recipe, profile: Profile) -> String {
        let e = effective(r, &self.opts);
        let sp = e.underlay_spacing;
        let inset = e.underlay_inset;
        match self.kind {
            StitchType::Fill => {
                let kind = if self.opts.no_underlay { Underlay::Off } else { self.opts.fill_underlay };
                match kind {
                    Underlay::Off => "None".into(),
                    Underlay::Tatami => format!("Tatami across · rows {sp:.1} mm · inset {inset:.1} mm · {:.1} mm stitches", e.underlay_len),
                    Underlay::EdgeRun => format!("Edge run · inset {:.1} mm", inset.max(0.4)),
                    Underlay::EdgeTatami => format!("Edge run + tatami across · rows {sp:.1} mm · inset {inset:.1} mm"),
                    Underlay::DoubleTatami => format!("Double tatami (across, then along) · rows {sp:.1} mm · inset {inset:.1} mm"),
                    _ if profile == Profile::Cap => format!("Auto: edge run + double tatami · rows {sp:.1} mm · inset {inset:.1} mm"),
                    _ => format!("Auto: tatami across · rows {sp:.1} mm · inset {inset:.1} mm · {:.1} mm stitches", e.underlay_len),
                }
            }
            StitchType::Satin | StitchType::SatinColumn => {
                if self.puff_stage == 2 {
                    return "None (sews over foam)".into();
                }
                let width = match &self.shape {
                    ShapeKind::Line { width, .. } => self.opts.column_width.unwrap_or(*width),
                    ShapeKind::Area { rings, rule } => {
                        2.0 * crate::geom::rings_area(rings, *rule) / crate::geom::rings_perimeter(rings).max(1e-9)
                    }
                };
                let kind = self.satin_underlay(width, profile);
                let auto = if self.opts.satin_underlay == Underlay::Auto && !self.opts.no_underlay { "Auto: " } else { "" };
                let zz = sp.min(2.0);
                let detail = match kind {
                    Underlay::Off => return "None".into(),
                    Underlay::CenterRun => format!("for a {width:.1} mm column"),
                    Underlay::EdgeRun => format!("inset {:.1} mm", (inset * 0.6).max(0.35)),
                    Underlay::Zigzag | Underlay::DoubleZigzag => format!("{zz:.1} mm apart · inset {inset:.1} mm"),
                    Underlay::EdgeZigzag => format!("zigzag {zz:.1} mm apart · inset {inset:.1} mm"),
                    _ => String::new(),
                };
                format!("{auto}{} · {detail}", kind.label())
            }
            StitchType::Run | StitchType::TripleRun => "None".into(),
        }
    }

    /// What the operator sheet and layer list call this object's stitching.
    pub fn stitch_label(&self) -> &'static str {
        match self.kind {
            StitchType::Fill => self.opts.fill.short(),
            // auto-angle satin areas follow each stroke (turning satin)
            _ if self.applique_stage == 1 => "Appliqué placement",
            _ if self.applique_stage == 2 => match self.opts.applique.as_ref().map(|a| a.tack) {
                Some(crate::applique::Tack::EStitch) => "Tack-down (E-stitch)",
                Some(crate::applique::Tack::DoubleRun) => "Tack-down (double run)",
                _ => "Tack-down (zigzag)",
            },
            _ if self.applique_stage == 3 => match self.opts.applique.as_ref().map(|a| a.cover) {
                Some(crate::applique::Cover::EStitch) => "Appliqué E-stitch",
                Some(crate::applique::Cover::Zigzag) => "Appliqué open zigzag",
                Some(crate::applique::Cover::DoubleRun) => "Appliqué double run",
                Some(crate::applique::Cover::Bean) => "Appliqué bean",
                _ => "Appliqué satin border",
            },
            _ if self.puff_stage == 1 => "Puff contour",
            _ if self.puff_stage == 2 => "3D puff satin",
            StitchType::Satin if self.opts.angle_deg.is_none() && self.knockouts.is_empty() => {
                "Turning satin"
            }
            k => k.label(),
        }
    }
}

pub fn generate(p: &Planned, r: &Recipe, profile: Profile, from: Pt) -> Vec<Needle> {
    if p.applique_stage > 0 {
        return applique_stage(p, r, profile, from);
    }
    match (p.puff_stage, &p.shape) {
        // before the foam: the outline, so the foam has a place and the
        // fabric is held
        (1, ShapeKind::Area { rings, rule }) => {
            let mut out = Vec::new();
            edge_run(&mut out, rings, *rule, 0.3, from);
            out
        }
        (1, ShapeKind::Line { paths, .. }) => {
            let mut out = Vec::new();
            for path in paths {
                push_section(&mut out, &resample(path, Limits::RUN_LEN), Limits::RUN_LEN);
            }
            out
        }
        // over the foam: tack it down at the outline, satin over it with no
        // underlay, then cap the column ends
        (2, ShapeKind::Area { rings, rule }) => {
            let e = effective(r, &p.opts);
            let mut out = Vec::new();
            let cur = edge_run(&mut out, rings, *rule, 0.5, from);
            let top = generate_top(p, r, profile, cur);
            match top.first() {
                Some(first) if !out.is_empty() => {
                    start_in_shape(&mut out, rings, *rule, &[first.p], Limits::RUN_LEN, &|_| true);
                    out.extend(top.into_iter().skip(1));
                }
                _ => out.extend(top),
            }
            puff_caps(&mut out, rings, *rule, e.satin_spacing);
            out
        }
        _ => generate_top(p, r, profile, from),
    }
}

fn generate_top(p: &Planned, r: &Recipe, profile: Profile, from: Pt) -> Vec<Needle> {
    let r = &effective(r, &p.opts);
    let smax = p.satin_max();
    let mut out = Vec::new();
    let pc = r.pull_comp + if profile == Profile::Cap { 0.05 } else { 0.0 };
    match (&p.shape, p.kind) {
        (ShapeKind::Area { rings, rule }, StitchType::Fill) => {
            let mut cur = from;
            // Underlay. Auto: rows across the fill; on caps an edge run
            // first and a second, parallel layer of rows.
            let across = if p.opts.guides.is_empty() {
                p.angle + FRAC_PI_2
            } else {
                // under a guided fill: across the main stitch direction
                let mut b = crate::geom::BBox::empty();
                for q in rings.iter().flatten() {
                    b.add(*q);
                }
                crate::flow::direction_at(b.center(), &p.opts.guides) + FRAC_PI_2
            };
            let cap = profile == Profile::Cap;
            let kind = if p.opts.no_underlay { Underlay::Off } else { p.opts.fill_underlay };
            let (edge, layers): (bool, Vec<f64>) = match kind {
                Underlay::Off => (false, vec![]),
                Underlay::Tatami => (false, vec![across]),
                Underlay::EdgeRun => (true, vec![]),
                Underlay::EdgeTatami => (true, vec![across]),
                Underlay::DoubleTatami => (false, vec![across, p.angle]),
                _ if cap => (true, vec![across, p.angle]),
                _ => (false, vec![across]),
            };
            let inset = r.underlay_inset;
            if edge {
                cur = edge_run(&mut out, rings, *rule, inset.max(0.4), cur);
            }
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
                    r.underlay_len,
                    1,
                    r.underlay_spacing,
                    cur,
                    &|_| true,
                );
            }
            mark_under(&mut out, 1);
            match p.opts.fill {
                FillPattern::Tatami if !p.opts.guides.is_empty() => {
                    guided_fill(&mut out, p, rings, *rule, r, pc, cur);
                }
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
                turning_satin(&mut out, p, rings, *rule, r, pc, from, profile)
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
                    r,
                    cur,
                    profile,
                );
            }
        }
        (ShapeKind::Line { paths, width }, kind) => {
            let mut cur = from;
            for path in order_paths(&p.visible_paths(paths), cur) {
                // satin position: centred on the art's line, or moved to one side
                let path = if p.opts.satin_offset != 0.0
                    && matches!(kind, StitchType::SatinColumn)
                {
                    shift_path(&path, p.opts.satin_offset)
                } else {
                    path
                };
                match kind {
                    StitchType::SatinColumn if p.opts.corner != Corner::Square => {
                        let width = p.opts.column_width.unwrap_or(*width);
                        let half = width / 2.0 + pc;
                        let closed = path.len() > 2 && path[0].dist(*path.last().unwrap()) < 0.05;
                        let ribs = column_ribs(&path, half, p.opts.corner, p.opts.cap, closed);
                        let kind = p.satin_underlay(width, profile);
                        if kind != Underlay::Off {
                            // underlay out, satin back over it
                            satin_underlay_out(&mut out, kind, &path, &ribs, r, pc, true);
                            let back: Vec<(Pt, Pt)> = ribs.iter().rev().copied().collect();
                            let mut zz = ribs_to_zigzag(&back, r.satin_spacing / 2.0);
                            if p.short_stitches() {
                                zz = short_stitches(&zz, r.satin_spacing);
                            }
                            push_continue(
                                &mut out,
                                &split_long(&zz, smax),
                                smax,
                            );
                        } else {
                            let mut zz = ribs_to_zigzag(&ribs, r.satin_spacing / 2.0);
                            if p.short_stitches() {
                                zz = short_stitches(&zz, r.satin_spacing);
                            }
                            push_section(
                                &mut out,
                                &split_long(&zz, smax),
                                smax,
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
                            let s0 = out.len() + 1;
                            push_section(
                                &mut out,
                                &resample(&path, Limits::RUN_LEN),
                                Limits::RUN_LEN,
                            );
                            mark_under(&mut out, s0);
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
                                    &split_long(&sides, smax),
                                    smax,
                                );
                                continue;
                            }
                            push_continue(
                                &mut out,
                                &split_long(&sides, smax),
                                smax,
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


/// Edge run: a running stitch just inside every outline of a shape, nearest
/// outline first. Returns where the needle ends.
fn edge_run(out: &mut Vec<Needle>, rings: &[Ring], rule: FillRule, inset: f64, mut cur: Pt) -> Pt {
    let inner = crate::vector::offset(rings, rule, -inset.max(0.1), true);
    let mut left: Vec<Ring> = inner
        .into_iter()
        .filter(|r| r.len() >= 3 && r.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() > 1.0)
        .collect();
    while !left.is_empty() {
        let (ri, vi) = left
            .iter()
            .enumerate()
            .flat_map(|(ri, r)| r.iter().enumerate().map(move |(vi, q)| (ri, vi, q.dist(cur))))
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
            .map(|(ri, vi, _)| (ri, vi))
            .unwrap();
        let mut ring = left.remove(ri);
        ring.rotate_left(vi);
        let first = ring[0];
        ring.push(first);
        let pts = resample(&ring, Limits::RUN_LEN);
        start_in_shape(out, rings, rule, &pts, Limits::RUN_LEN, &|_| true);
        cur = out.last().map(|n| n.p).unwrap_or(cur);
    }
    cur
}

/// Underlay along a satin column on the way out, ending at its far end so
/// the satin can come back over it. `ribs` run edge to edge along `path`.
#[allow(clippy::too_many_arguments)]
fn satin_underlay_out(
    out: &mut Vec<Needle>,
    kind: Underlay,
    path: &[Pt],
    ribs: &[(Pt, Pt)],
    r: &Recipe,
    pc: f64,
    first: bool,
) {
    let start = out.len();
    satin_underlay_body(out, kind, path, ribs, r, pc, first);
    // the jump that starts a section isn't underlay; everything after is
    let from = if first { start + 1 } else { start };
    mark_under(out, from);
}

fn satin_underlay_body(
    out: &mut Vec<Needle>,
    kind: Underlay,
    path: &[Pt],
    ribs: &[(Pt, Pt)],
    r: &Recipe,
    pc: f64,
    first: bool,
) {
    let run_len = r.underlay_len.min(Limits::RUN_LEN);
    let push = |out: &mut Vec<Needle>, pts: &[Pt], max: f64, first: bool| {
        if pts.len() < 2 {
            return;
        }
        if first {
            push_section(out, pts, max);
        } else {
            push_continue(out, &pts[1..], max);
        }
    };
    let inset_ribs = |inset: f64| -> Vec<(Pt, Pt)> {
        ribs.iter()
            .map(|(a, b)| {
                let m = a.lerp(*b, 0.5);
                let h = (a.dist(*b) / 2.0 - inset).max(0.15);
                let d = a.sub(*b).norm();
                (m.add(d.mul(h)), m.sub(d.mul(h)))
            })
            .collect()
    };
    let center = resample(path, run_len);
    let edges = |out: &mut Vec<Needle>, first: bool| {
        // out along one edge, back along the other
        let e = inset_ribs((r.underlay_inset * 0.6 + pc).max(0.35));
        let left: Vec<Pt> = e.iter().map(|x| x.0).collect();
        let right: Vec<Pt> = e.iter().rev().map(|x| x.1).collect();
        push(out, &resample(&left, run_len), run_len, first);
        let right = resample(&right, run_len);
        push_continue(out, &right, run_len);
    };
    let zig = |out: &mut Vec<Needle>, first: bool| {
        let inner = inset_ribs(r.underlay_inset + pc);
        let zz = ribs_to_zigzag(&inner, r.underlay_spacing.min(2.0) / 2.0);
        push(out, &zz, Limits::SATIN_MAX, first);
    };
    match kind {
        Underlay::EdgeRun => {
            edges(out, first);
            push_continue(out, &center, run_len);
        }
        Underlay::Zigzag => zig(out, first),
        Underlay::EdgeZigzag => {
            edges(out, first);
            zig(out, false);
        }
        Underlay::DoubleZigzag => {
            zig(out, first);
            let back: Vec<(Pt, Pt)> = inset_ribs(r.underlay_inset + pc).into_iter().rev().map(|(a, b)| (b, a)).collect();
            let zz = ribs_to_zigzag(&back, r.underlay_spacing.min(2.0) / 2.0 * 0.8);
            push_continue(out, &zz, Limits::SATIN_MAX);
            push_continue(out, &center, run_len);
        }
        // center run (also how the needle travels when there is no underlay)
        _ => push(out, &center, run_len, first),
    }
}

/// Short stitches: where a satin column curves, the inside edge crowds with
/// needle holes. Every other stitch there stops short of the inside edge.
/// `zz` alternates sides (even = one edge, odd = the other).
fn short_stitches(zz: &[Pt], density: f64) -> Vec<Pt> {
    let n = zz.len();
    if n < 6 {
        return zz.to_vec();
    }
    let mut out = zz.to_vec();
    let mut flip = [false, false];
    for i in 2..n - 2 {
        let same = zz[i].dist(zz[i - 2]);
        let other = zz[i - 1].dist(zz[i + 1]);
        let side = i % 2;
        if same < other * 0.55 && same < density * 0.8 {
            flip[side] = !flip[side];
            if flip[side] {
                // pull this end 1/3 of the way across, toward the other edge
                let across = zz[i - 1].lerp(zz[i + 1], 0.5);
                out[i] = zz[i].lerp(across, 0.33);
            }
        } else {
            flip[side] = false;
        }
    }
    out
}

#[doc(hidden)]
pub fn short_stitches_for_test(zz: &[Pt], density: f64) -> Vec<Pt> {
    short_stitches(zz, density)
}

/// 3D puff end caps: at every free end of a column, a few tight stitches
/// run along the column and slightly past its end, so the satin wraps and
/// cuts the foam there instead of leaving it open.
fn puff_caps(out: &mut Vec<Needle>, rings: &[Ring], rule: FillRule, density: f64) {
    use crate::skeleton::skeleton;
    let b = BBox::of(rings.iter().flatten());
    let res = (b.width().max(b.height()) / 500.0).clamp(0.04, 0.12);
    let field = Field::new(rings, rule, res);
    let g = skeleton(&field);
    for (n, node) in g.nodes.iter().enumerate() {
        if g.degree(n) != 1 {
            continue;
        }
        let Some(e) = g.edges.iter().find(|e| e.a == n || e.b == n) else { continue };
        let pts: Vec<Pt> = if e.a == n { e.pts.clone() } else { e.pts.iter().rev().copied().collect() };
        if pts.len() < 2 {
            continue;
        }
        // direction into the column over its first ~1 mm
        let mut k = 1;
        while k + 1 < pts.len() && pts[k].dist(pts[0]) < 1.0 {
            k += 1;
        }
        let into = pts[k].sub(pts[0]);
        if into.len() < 1e-6 {
            continue;
        }
        let into = into.norm();
        let outw = into.mul(-1.0);
        let nrm = Pt::new(-into.y, into.x);
        let hw = field.at(*node).max(0.3);
        // where the shape actually ends past the node
        let reach = |c: Pt| -> f64 {
            let mut x = 0.0;
            while x < hw * 3.0 + 1.0 && field.at(c.add(outw.mul(x))) > 0.0 {
                x += 0.05;
            }
            x
        };
        let span = hw * 1.15;
        // satin across the end: alternate base and tip, density/2 apart
        let steps = ((2.0 * span) / (density / 2.0)).ceil().max(4.0) as usize;
        let mut cap: Vec<Pt> = Vec::new();
        for j in 0..=steps {
            let t = -span + 2.0 * span * j as f64 / steps as f64;
            let c = node.add(nrm.mul(t));
            if field.at(c) < -0.3 {
                continue;
            }
            if j % 2 == 0 {
                cap.push(c.sub(outw.mul(hw.min(1.2))));
            } else {
                cap.push(c.add(outw.mul(reach(c) + 0.3)));
            }
        }
        if cap.len() >= 4 {
            start_in_shape(out, rings, rule, &cap, Limits::SATIN_MAX, &|_| true);
        }
    }
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
        Some(c) if c.dist(first) <= near && link_inside(rings, rule, c, first) => {
            // skip points right on top of the needle (e.g. where spiral
            // rings meet) so there are no tiny stitches
            let k = pts.iter().take_while(|q| q.dist(c) < 0.3).count();
            push_continue(out, &pts[k.min(pts.len())..], len)
        }
        _ => start_in_shape(out, rings, rule, pts, len, hidden),
    }
}

/// A straight link between two needle points stays on the shape (its ends
/// may sit just past the edge, by pull compensation).
fn link_inside(rings: &[Ring], rule: FillRule, a: Pt, b: Pt) -> bool {
    let d = a.dist(b);
    if d <= 1.2 {
        return true;
    }
    let n = (d / 0.4).ceil() as usize;
    (1..n)
        .map(|k| a.lerp(b, k as f64 / n as f64))
        .filter(|q| q.dist(a) > 0.5 && q.dist(b) > 0.5)
        .all(|q| inside(rings, rule, q))
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

/// Tatami rows that turn to follow the layer's direction lines.
fn guided_fill(out: &mut Vec<Needle>, p: &Planned, rings: &[Ring], rule: FillRule, r: &Recipe, pc: f64, from: Pt) -> Pt {
    let o = UNDERLAP;
    let hidden = |q: Pt| {
        [Pt::new(o, 0.0), Pt::new(-o, 0.0), Pt::new(0.0, o), Pt::new(0.0, -o), Pt::new(0.0, 0.0)]
            .iter()
            .all(|d| p.covered(q.add(*d)))
    };
    let keep = |q: Pt| inside(rings, rule, q) && (p.knockouts.is_empty() || !hidden(q));
    let rows = crate::flow::guided_rows(rings, rule, &keep, &p.opts.guides, r.fill_spacing);
    let len = r.fill_len;
    let stagger = p.opts.stagger.unwrap_or(3).clamp(1, 8) as i64;
    let tm = {
        let mut b = crate::geom::BBox::empty();
        for q in rings.iter().flatten() {
            b.add(*q);
        }
        let a = crate::flow::direction_at(b.center(), &p.opts.guides);
        Pt::new(a.cos(), a.sin())
    };
    // each row as stitches: ends pushed out by pull compensation, needle
    // points staggered row to row (brick pattern) along the row
    let stitched: Vec<Vec<Pt>> = rows
        .iter()
        .map(|(lv, row)| {
            let mut row = row.clone();
            let n = row.len();
            if n >= 2 && pc > 0.0 {
                let u0 = row[0].sub(row[1]).norm();
                row[0] = row[0].add(u0.mul(pc));
                let u1 = row[n - 1].sub(row[n - 2]).norm();
                row[n - 1] = row[n - 1].add(u1.mul(pc));
            }
            let total: f64 = row.windows(2).map(|w| w[0].dist(w[1])).sum();
            // needle points line up across rows like straight tatami: measure
            // along the main direction from a fixed origin, not from each
            // row's own start (which wanders with the edge)
            let dir = row[row.len() - 1].sub(row[0]);
            let sign = if dir.x * tm.x + dir.y * tm.y < 0.0 { -1.0 } else { 1.0 };
            let base = sign * (row[0].x * tm.x + row[0].y * tm.y);
            let off = ((lv.rem_euclid(stagger) as f64) * len / stagger as f64 - base).rem_euclid(len);
            let min = 0.6_f64.min(len * 0.3);
            let mut marks = vec![0.0];
            let mut m = ((min - off) / len).ceil() as i64;
            loop {
                let s = off + m as f64 * len;
                if s >= total - min {
                    break;
                }
                if s > min {
                    marks.push(s);
                }
                m += 1;
            }
            marks.push(total);
            // the point on the row at a distance along it
            let cum: Vec<f64> = std::iter::once(0.0)
                .chain(row.windows(2).scan(0.0, |a, w| {
                    *a += w[0].dist(w[1]);
                    Some(*a)
                }))
                .collect();
            let at = |s: f64| -> Pt {
                let k = cum.partition_point(|&c| c < s).clamp(1, row.len() - 1);
                let (a, b) = (row[k - 1], row[k]);
                let l = (cum[k] - cum[k - 1]).max(1e-9);
                a.lerp(b, ((s - cum[k - 1]) / l).clamp(0.0, 1.0))
            };
            // a stitch is straight: where the row bends (round a corner),
            // add points so no stitch cuts across the bend
            let bend = |a: f64, b: f64| -> f64 {
                let (pa, pb) = (at(a), at(b));
                cum.iter()
                    .zip(&row)
                    .filter(|(c, _)| **c > a && **c < b)
                    .map(|(_, q)| seg_dist(*q, pa, pb))
                    .fold(0.0, f64::max)
            };
            let mut fine = vec![marks[0]];
            for w in marks.windows(2) {
                let mut stack = vec![(w[0], w[1], 0)];
                let mut out_s = Vec::new();
                while let Some((a, b, depth)) = stack.pop() {
                    if depth < 4 && bend(a, b) > 0.2 && b - a > 0.6 {
                        let m = (a + b) / 2.0;
                        stack.push((m, b, depth + 1));
                        stack.push((a, m, depth + 1));
                    } else {
                        out_s.push(b);
                    }
                }
                fine.extend(out_s);
            }
            fine.into_iter().map(at).collect::<Vec<Pt>>()
        })
        .filter(|v| v.len() >= 2)
        .collect();
    // sew nearest row end next, back and forth
    let mut left: Vec<usize> = (0..stitched.len()).collect();
    let mut cur = out.last().map(|n| n.p).unwrap_or(from);
    let near = (3.0 * r.fill_spacing).max(1.5);
    while !left.is_empty() {
        let (pick, rev) = left
            .iter()
            .enumerate()
            .flat_map(|(i, &k)| {
                let v = &stitched[k];
                [(i, false, v[0].dist(cur)), (i, true, v[v.len() - 1].dist(cur))]
            })
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
            .map(|(i, rv, _)| (i, rv))
            .unwrap();
        let k = left.swap_remove(pick);
        let mut pts = stitched[k].clone();
        if rev {
            pts.reverse();
        }
        join_piece(out, rings, rule, &pts, len, near, &|q| p.covered(q));
        cur = out.last().map(|n| n.p).unwrap_or(cur);
    }
    cur
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
    profile: Profile,
) -> bool {
    let smax = p.satin_max();
    use crate::skeleton::{Graph, skeleton};
    let b = BBox::of(rings.iter().flatten());
    let res = (b.width().max(b.height()) / 500.0).clamp(0.04, 0.12);
    let field = Field::new(rings, rule, res);
    let g: Graph = skeleton(&field);
    if std::env::var("SF_DEBUG_SKEL").is_ok() {
        eprintln!("skeleton: {} nodes, {} edges", g.nodes.len(), g.edges.len());
        for (i, e) in g.edges.iter().enumerate() {
            let l: f64 = e.pts.windows(2).map(|w| w[0].dist(w[1])).sum();
            eprintln!("  e{i}: {}({:.1},{:.1}) -> {}({:.1},{:.1}) len {l:.2}", e.a, g.nodes[e.a].x, g.nodes[e.a].y, e.b, g.nodes[e.b].x, g.nodes[e.b].y);
        }
    }
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
    let step = r.satin_spacing / 2.0;
    let shorten = p.short_stitches();
    let sew_back = |out: &mut Vec<Needle>, back: &[Vec<(Pt, Pt)>]| {
        for piece in back {
            if piece.len() < 2 {
                continue;
            }
            let mut zz = ribs_to_zigzag(piece, step);
            if shorten {
                zz = short_stitches(&zz, r.satin_spacing);
            }
            push_continue(out, &split_long(&zz, smax), smax);
        }
    };
    // Out along a stroke: its underlay (a centre run on narrow strokes,
    // zig-zag or edge runs on wide ones), which is also how the needle
    // travels to the far end, hidden under the satin.
    let run_out = |out: &mut Vec<Needle>, path: &[Pt], fwd: &[Vec<(Pt, Pt)>]| {
        let fwd: Vec<(Pt, Pt)> = fwd.iter().flatten().copied().collect();
        let width = fwd.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / fwd.len().max(1) as f64;
        let kind = p.satin_underlay(width, profile);
        satin_underlay_out(out, kind, path, &fwd, r, pc, false);
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
            out.push(Needle::new(start_pt, true));
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
    if out.len() <= begin + 2 {
        return false;
    }
    // Safety net: the satin must actually cover the shape. If a stroke was
    // missed (an odd centre line), use the regular satin instead.
    let cov = coverage(&out[begin..], &field, b);
    if cov < 0.95 {
        if std::env::var("SF_DEBUG_SKEL").is_ok() {
            eprintln!("turning satin covered {:.0}% of {}; using regular satin", cov * 100.0, p.label);
        }
        return false;
    }
    true
}

/// Share of the shape (sampled every 0.4 mm, away from the very edge) that
/// lies within reach of a sewn stitch.
fn coverage(needles: &[Needle], field: &Field, b: BBox) -> f64 {
    let cell = 1.0;
    let mut grid: std::collections::HashMap<(i64, i64), Vec<(Pt, Pt)>> = Default::default();
    for w in needles.windows(2) {
        if w[1].jump {
            continue;
        }
        let (a, c) = (w[0].p, w[1].p);
        let (x0, x1) = (a.x.min(c.x), a.x.max(c.x));
        let (y0, y1) = (a.y.min(c.y), a.y.max(c.y));
        for gx in (x0 / cell).floor() as i64..=(x1 / cell).floor() as i64 {
            for gy in (y0 / cell).floor() as i64..=(y1 / cell).floor() as i64 {
                grid.entry((gx, gy)).or_default().push((a, c));
            }
        }
    }
    let seg_dist = |q: Pt, a: Pt, c: Pt| -> f64 {
        let d = c.sub(a);
        let l2 = d.x * d.x + d.y * d.y;
        let t = if l2 < 1e-12 { 0.0 } else { (((q.x - a.x) * d.x + (q.y - a.y) * d.y) / l2).clamp(0.0, 1.0) };
        q.dist(a.add(d.mul(t)))
    };
    let (mut inside, mut hit) = (0usize, 0usize);
    let step = 0.4;
    let mut y = b.min.y;
    while y <= b.max.y {
        let mut x = b.min.x;
        while x <= b.max.x {
            let q = Pt::new(x, y);
            if field.at(q) > 0.3 {
                inside += 1;
                let (gx, gy) = ((x / cell).floor() as i64, (y / cell).floor() as i64);
                let near = (gx - 1..=gx + 1).any(|i| {
                    (gy - 1..=gy + 1).any(|j| {
                        grid.get(&(i, j))
                            .map(|v| v.iter().any(|(a, c)| seg_dist(q, *a, *c) < 0.45))
                            .unwrap_or(false)
                    })
                });
                hit += near as usize;
            }
            x += step;
        }
        y += step;
    }
    if inside == 0 { 1.0 } else { hit as f64 / inside as f64 }
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
    r: &Recipe,
    mut cur: Pt,
    profile: Profile,
) -> Pt {
    let density = r.satin_spacing;
    let smax = p.satin_max();
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
        let sides = if p.short_stitches() { short_stitches(&sides, density) } else { sides };
        let width = segs.iter().map(|s| s.len()).sum::<f64>() / segs.len().max(1) as f64;
        let kind = p.satin_underlay(width, profile);
        if segs.len() >= 6 && kind != Underlay::Off {
            let ribs: Vec<(Pt, Pt)> = segs
                .iter()
                .map(|s| (Pt::new(s.x0, s.y).rot(angle), Pt::new(s.x1, s.y).rot(angle)))
                .collect();
            start_in_shape(out, rings, rule, &center[..1], Limits::RUN_LEN, &|_| true);
            satin_underlay_out(out, kind, &center, &ribs, r, pc, false);
            push_continue(out, &split_long(&sides, smax), smax);
        } else {
            start_in_shape(out, rings, rule, &split_long(&sides, smax), smax, &|q| p.covered(q));
        }
        cur = out.last().map(|n| n.p).unwrap_or(cur);
    }
    cur
}

// ------------------------------------------------------------------ appliqué

/// The piece's outlines as closed paths starting near `from` (nearest ring
/// first), each with the sign that moves it out of the piece.
fn applique_rings(rings: &[Ring], rule: FillRule, from: Pt) -> Vec<(Vec<Pt>, f64)> {
    let mut left: Vec<Ring> = rings.iter().filter(|r| r.len() >= 3).cloned().collect();
    let mut cur = from;
    let mut out = Vec::new();
    while !left.is_empty() {
        let (ri, vi) = left
            .iter()
            .enumerate()
            .flat_map(|(ri, r)| r.iter().enumerate().map(move |(vi, q)| (ri, vi, q.dist(cur))))
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(ri, vi, _)| (ri, vi))
            .unwrap();
        let mut ring = left.remove(ri);
        ring.rotate_left(vi);
        ring.push(ring[0]);
        // which way is out of the piece (a hole's outline turns the other way)
        let probe = shift_path(&ring, 0.25);
        let n = probe.len().saturating_sub(1).max(1);
        let step = (n / 12).max(1);
        let (mut ins, mut tot) = (0, 0);
        for k in (0..n).step_by(step) {
            // midpoints of edges are safer than corners
            let a = probe[k];
            let b = probe[(k + 1) % n];
            if inside(rings, rule, a.lerp(b, 0.5)) {
                ins += 1;
            }
            tot += 1;
        }
        let sign = if ins * 2 > tot { -1.0 } else { 1.0 };
        cur = ring[0];
        out.push((ring, sign));
    }
    out
}

/// Zigzag between two parallel lines `d - w/2` and `d + w/2` mm out of the
/// piece, `spacing` mm from one point on a side to the next on that side.
fn applique_zigzag(path: &[Pt], sign: f64, d: f64, w: f64, spacing: f64) -> Vec<Pt> {
    let c = resample(path, (spacing / 2.0).max(0.3));
    let a = shift_path(&c, sign * (d + w / 2.0));
    let b = shift_path(&c, sign * (d - w / 2.0));
    a.iter()
        .zip(b.iter())
        .enumerate()
        .map(|(i, (p, q))| if i % 2 == 0 { *p } else { *q })
        .collect()
}

/// E-stitch (blanket): a run along `base` mm out of the piece with a prong
/// `prong` mm onto the piece every `spacing` mm.
fn applique_estitch(path: &[Pt], sign: f64, base: f64, prong: f64, spacing: f64) -> Vec<Pt> {
    let c = resample(path, spacing.max(0.8));
    let a = shift_path(&c, sign * base);
    let b = shift_path(&c, sign * (base - prong));
    let mut out = Vec::with_capacity(a.len() * 3);
    for (i, (p, q)) in a.iter().zip(b.iter()).enumerate() {
        out.push(*p);
        // no prong on the closing point (it repeats the first)
        if i + 1 < a.len() {
            out.push(*q);
            out.push(*p);
        }
    }
    out
}

/// One appliqué stage: placement line, tack-down or cover.
fn applique_stage(p: &Planned, r: &Recipe, profile: Profile, from: Pt) -> Vec<Needle> {
    use crate::applique::{Cover, Method, Tack};
    let (Some(a), ShapeKind::Area { rings, rule }) = (&p.opts.applique, &p.shape) else {
        return Vec::new();
    };
    let mut out: Vec<Needle> = Vec::new();
    let mut cur = from;
    let run_len = 2.0;
    for (path, sign) in applique_rings(rings, *rule, cur) {
        let pts: Vec<Pt> = match p.applique_stage {
            1 => {
                let d = if a.method == Method::PreCut { a.cut_offset } else { 0.0 };
                resample(&shift_path(&path, sign * d), run_len)
            }
            2 => {
                let d = -a.tack_inset;
                match a.tack {
                    Tack::Zigzag => applique_zigzag(&path, sign, d, a.tack_width.max(0.3), 1.8),
                    Tack::EStitch => applique_estitch(&path, sign, d + a.tack_width / 2.0, a.tack_width.max(0.3), 2.0),
                    Tack::DoubleRun => {
                        let mut f = resample(&shift_path(&path, sign * d), run_len);
                        let back: Vec<Pt> = f.iter().rev().skip(1).copied().collect();
                        f.extend(back);
                        f
                    }
                    Tack::None => Vec::new(),
                }
            }
            _ => match a.cover {
                Cover::Satin => {
                    // a satin column along the edge, reusing the satin engine
                    let w = a.cover_width.max(0.5);
                    // tiny outline segments (common in fonts) make ragged
                    // mitres: smooth them out first
                    let simple = crate::geom::simplify(&path, 0.08);
                    let centre = shift_path(&simple, sign * (a.cover_outside - w / 2.0));
                    let mut q = p.clone();
                    q.applique_stage = 0;
                    q.kind = StitchType::SatinColumn;
                    q.shape = ShapeKind::Line { paths: vec![centre], width: w };
                    q.knockouts.clear();
                    q.opts.column_width = Some(w);
                    q.opts.satin_offset = 0.0;
                    q.opts.density = Some(a.cover_spacing.clamp(0.25, 1.0));
                    // the tack-down already holds the piece
                    q.opts.no_underlay = a.tack != Tack::None;
                    let n = generate_top(&q, r, profile, cur);
                    if let Some(l) = n.last() {
                        cur = l.p;
                    }
                    out.extend(n);
                    continue;
                }
                Cover::EStitch => applique_estitch(&path, sign, a.cover_outside, a.cover_width.max(0.5), a.cover_spacing.max(0.8)),
                Cover::Zigzag => {
                    let w = a.cover_width.max(0.5);
                    applique_zigzag(&path, sign, a.cover_outside - w / 2.0, w, a.cover_spacing.max(0.4))
                }
                Cover::DoubleRun => {
                    let mut f = resample(&shift_path(&path, sign * a.cover_outside), a.cover_spacing.clamp(1.0, 4.0));
                    let back: Vec<Pt> = f.iter().rev().skip(1).copied().collect();
                    f.extend(back);
                    f
                }
                Cover::Bean => {
                    let f = resample(&shift_path(&path, sign * a.cover_outside), a.cover_spacing.clamp(1.0, 4.0));
                    let mut b = vec![f[0]];
                    for w in f.windows(2) {
                        b.extend([w[1], w[0], w[1]]);
                    }
                    b
                }
                Cover::None => Vec::new(),
            },
        };
        if pts.len() >= 2 {
            push_section(&mut out, &pts, Limits::MAX_STITCH.min(7.0));
            cur = *pts.last().unwrap();
        }
    }
    out
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

/// Move a path sideways by `d` mm. Closed paths move outward for +d (inward
/// for −d); open paths move to the right of their direction of travel.
/// Corners keep their shape (mitred, limited at very sharp turns).
pub fn shift_path(path: &[Pt], d: f64) -> Vec<Pt> {
    let n = path.len();
    if n < 2 || d == 0.0 {
        return path.to_vec();
    }
    let closed = n > 2 && path[0].dist(path[n - 1]) < 0.05;
    // the unique points (a closed path repeats its first point at the end)
    let pts: Vec<Pt> = if closed { path[..n - 1].to_vec() } else { path.to_vec() };
    let m = pts.len();
    if m < 2 {
        return path.to_vec();
    }
    // which way is "out": for a closed ring, away from its inside
    let sign = if closed {
        let mut a = 0.0;
        for i in 0..m {
            let (p, q) = (pts[i], pts[(i + 1) % m]);
            a += p.x * q.y - q.x * p.y;
        }
        // a > 0: the left normal points inside, so outward is to the right
        if a > 0.0 { 1.0 } else { -1.0 }
    } else {
        1.0
    };
    let right = |a: Pt, b: Pt| -> Pt {
        let u = b.sub(a).norm();
        Pt::new(u.y, -u.x)
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..m {
        let prev = if i > 0 { Some(pts[i - 1]) } else if closed { Some(pts[m - 1]) } else { None };
        let next = if i + 1 < m { Some(pts[i + 1]) } else if closed { Some(pts[0]) } else { None };
        let nrm = match (prev, next) {
            (Some(a), Some(b)) => {
                let n1 = right(a, pts[i]);
                let n2 = right(pts[i], b);
                let s = n1.add(n2);
                let len = s.len();
                if len < 1e-9 {
                    n1
                } else {
                    // mitre: longer at corners so the sides stay parallel
                    let cos = (len / 2.0).max(0.35);
                    s.mul(1.0 / len).mul(1.0 / cos)
                }
            }
            (None, Some(b)) => right(pts[i], b),
            (Some(a), None) => right(a, pts[i]),
            (None, None) => Pt::new(0.0, 0.0),
        };
        out.push(pts[i].add(nrm.mul(d * sign)));
    }
    if closed {
        out.push(out[0]);
    }
    out
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
        out.push(Needle::new(*first, true));
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
                    out.push(Needle::new(last.lerp(p, k as f64 / n as f64), false));
                }
            }
        }
        out.push(Needle::new(p, false));
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
            let shift = [0.0, 0.12, -0.12][k % 3] / n as f64;
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
        let all = sew_rows(&run, left, angle, ends, len, stagger);
        // a turn from one row to the next that would leave the shape (round
        // a notch) splits the run there
        let mut chunks: Vec<Vec<Pt>> = vec![vec![all[0]]];
        for w in all.windows(2) {
            if w[0].dist(w[1]) > 1.2 && !link_inside(rings, rule, w[0], w[1]) {
                chunks.push(vec![w[1]]);
            } else {
                chunks.last_mut().unwrap().push(w[1]);
            }
        }
        let pts = &chunks[0];
        if travel && cur.dist(start) > 0.05 {
            let t = resample(&[cur, start], Limits::RUN_LEN);
            push_continue(out, &t[1..], Limits::RUN_LEN);
            push_continue(out, &pts[1..], len);
        } else {
            push_section(out, pts, len);
        }
        for c in &chunks[1..] {
            join_piece(out, rings, rule, c, len, 0.0, covered);
        }
        cur = *chunks.last().unwrap().last().unwrap();
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
