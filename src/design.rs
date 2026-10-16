//! Sequencing (garment profiles) and assembly into machine commands.

use crate::geom::{BBox, Pt};
use crate::import::Artwork;
use crate::settings::{Limits, Profile, Recipe};
use crate::stitch::{Needle, Planned, StitchType, generate, plan};
use crate::thread::{Thread, ThreadWeight};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub enum Cmd {
    Stitch(Pt),
    Jump(Pt),
    Trim,
    /// Switch to the colour stop with this index.
    Color(usize),
    End,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stop {
    pub thread: Thread,
    /// Thread weight loaded on this needle (40 wt unless changed).
    pub weight: ThreadWeight,
    pub needle: usize,
    pub stitches: usize,
    pub objects: Vec<String>,
    /// What the operator does at this stop (e.g. place the puff foam).
    pub note: String,
    /// Estimated top thread and bobbin used by this stop (m).
    pub top_m: f64,
    pub bobbin_m: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObjectInfo {
    pub sequence: usize,
    pub label: String,
    pub stitch_type: &'static str,
    pub thread_code: String,
    pub weight: ThreadWeight,
    /// Sewn over puff foam (drawn raised in 3D).
    pub puff: bool,
    pub color_match_delta_e: f64,
    pub stitches: usize,
    pub trims: usize,
    /// Range of `Design::cmds` this object produced.
    #[serde(skip)]
    pub cmd_range: (usize, usize),
    /// Layer the object belongs to.
    pub layer: usize,
    /// Colour stop it sews in, and whether a stop happens right before it.
    pub stop: usize,
    pub starts_stop: bool,
    /// Stitch direction actually used, degrees counter-clockwise (0–180).
    pub angle_deg: f64,
    /// Thread the layer asked for (may differ from the stop's thread if the
    /// stop before it was switched off).
    #[serde(skip)]
    pub wanted_thread: usize,
}

/// How the design is put together.
#[derive(Clone, Copy, Debug)]
pub struct BuildOptions {
    /// Sew layers exactly in their list order (true), or let the garment
    /// profile reorder them (false).
    pub keep_order: bool,
    /// Add a colour stop automatically whenever the thread changes.
    pub auto_stops: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            keep_order: false,
            auto_stops: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub stitches: usize,
    pub jumps: usize,
    pub trims: usize,
    pub color_changes: usize,
    pub width_mm: f64,
    pub height_mm: f64,
    pub run_minutes: f64,
    pub short_stitches: usize,
    pub longest_stitch_mm: f64,
    /// Estimated thread use for one piece (m): top thread and bobbin.
    pub top_m: f64,
    pub bobbin_m: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Design {
    pub name: String,
    pub profile: Profile,
    pub recipe: Recipe,
    pub cmds: Vec<Cmd>,
    pub stops: Vec<Stop>,
    pub objects: Vec<ObjectInfo>,
    pub stats: Stats,
    pub bbox: BBox,
    pub warnings: Vec<String>,
    /// Each layer's needle points as sewn (design mm, layer index → points):
    /// the embroidery side of each design object, kept with the project.
    #[serde(skip)]
    pub layer_needles: std::collections::BTreeMap<usize, Vec<Needle>>,
}

impl Design {
    /// An empty design (a blank canvas to draw on).
    pub fn empty(profile: Profile, recipe: &Recipe) -> Design {
        let mut bbox = BBox::empty();
        bbox.add(Pt::new(-50.0, -50.0));
        bbox.add(Pt::new(50.0, 50.0));
        Design {
            name: "design".into(),
            profile,
            recipe: recipe.clone(),
            cmds: Vec::new(),
            stops: Vec::new(),
            objects: Vec::new(),
            stats: Stats::default(),
            bbox,
            warnings: Vec::new(),
            layer_needles: Default::default(),
        }
    }
}

/// Sequence objects. Stacking order is respected wherever objects overlap
/// (a lower layer always sews before what covers it); within that rule:
/// Flat — keep the current colour as long as possible, then stack order.
/// Cap  — stay on the current colour while it has work; within a colour,
///        closest to the vertical centre line first, bottom first.
pub fn sequence(objs: &[Planned], profile: Profile, design: &BBox, keep_order: bool) -> Vec<usize> {
    let n = objs.len();
    if keep_order {
        // Layers in list order; inside a layer, caps still go center-out.
        let cx = design.center().x;
        let mut idx: Vec<usize> = (0..n).collect();
        idx.sort_by_key(|&i| {
            let o = &objs[i];
            let b = &o.bbox;
            let d = if b.min.x <= cx && b.max.x >= cx {
                0.0
            } else {
                (b.min.x - cx).abs().min((b.max.x - cx).abs())
            };
            match profile {
                Profile::Flat => (o.puff_stage as usize, o.layer, 0, 0, i),
                Profile::Cap => (
                    o.puff_stage as usize,
                    o.layer,
                    (d / 5.0).floor() as i64,
                    -(b.max.y * 10.0) as i64,
                    i,
                ),
            }
        });
        return idx;
    }
    let mut preds = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for j in 0..n {
        for i in 0..j {
            // 3D puff sews after everything flat (all contours, then the
            // foam, then all puff satin), whatever the stacking says
            let (pi, pj) = (objs[i].puff_stage, objs[j].puff_stage);
            if (pi > 0 || pj > 0) && !(objs[i].layer == objs[j].layer && pi < pj) {
                continue;
            }
            if objs[i].bbox.overlaps(&objs[j].bbox, 0.3) {
                preds[j] += 1;
                succ[i].push(j);
            }
        }
    }
    let cx = design.center().x;
    let center_bucket = |b: &BBox| -> i64 {
        let d = if b.min.x <= cx && b.max.x >= cx {
            0.0
        } else {
            (b.min.x - cx).abs().min((b.max.x - cx).abs())
        };
        (d / 5.0).floor() as i64
    };
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut cur_thread: Option<(usize, Option<ThreadWeight>)> = None;
    for _ in 0..n {
        let pick = (0..n)
            .filter(|&i| !done[i] && preds[i] == 0)
            .min_by_key(|&i| {
                let o = &objs[i];
                let color_change = (Some((o.thread, o.opts.weight)) != cur_thread) as i64;
                match profile {
                    Profile::Flat => (o.puff_stage as i64, 0, color_change, o.stack as i64, 0),
                    // each colour center-out from the bottom; change colour
                    // only when that colour has nothing left that can sew
                    Profile::Cap => (
                        o.puff_stage as i64,
                        color_change,
                        center_bucket(&o.bbox),
                        -(o.bbox.max.y * 10.0) as i64,
                        o.stack as i64,
                    ),
                }
            })
            .expect("stacking constraints are acyclic");
        done[pick] = true;
        cur_thread = Some((objs[pick].thread, objs[pick].opts.weight));
        for &s in &succ[pick] {
            preds[s] -= 1;
        }
        order.push(pick);
    }
    order
}

pub fn build(
    name: &str,
    art: &Artwork,
    chart: &[Thread],
    recipe: &Recipe,
    profile: Profile,
) -> Result<Design, String> {
    build_with(name, art, chart, recipe, profile, BuildOptions::default())
}

pub fn build_with(
    name: &str,
    art: &Artwork,
    chart: &[Thread],
    recipe: &Recipe,
    profile: Profile,
    opts: BuildOptions,
) -> Result<Design, String> {
    let mut warnings = art.warnings.clone();
    let mut objs = plan(art, chart, profile, &mut warnings);
    // every object knows its thread weight (the design's unless the layer says)
    for o in objs.iter_mut() {
        o.opts.weight = Some(o.opts.weight.unwrap_or(recipe.weight));
    }
    // 3D puff objects sew in two stages: the contour before the foam goes
    // on, then (after a stop to place the foam) the satin over it
    if objs.iter().any(|o| o.opts.puff) {
        let mut ex = Vec::with_capacity(objs.len() + 4);
        for o in objs {
            if o.opts.puff && !art.fixed.contains_key(&o.layer) {
                let mut a = o.clone();
                a.puff_stage = 1;
                a.label = format!("{} (puff contour)", o.label);
                let mut b = o;
                b.puff_stage = 2;
                ex.push(a);
                ex.push(b);
            } else {
                ex.push(o);
            }
        }
        objs = ex;
    }
    // Hand-edited layers sew their own stitches: one object per layer.
    if !art.fixed.is_empty() {
        let mut seen = std::collections::HashSet::new();
        objs.retain(|o| !art.fixed.contains_key(&o.layer) || seen.insert(o.layer));
    }
    if objs.is_empty() {
        return Err("nothing left to stitch after skipping tiny shapes".into());
    }
    let mut bbox = BBox::empty();
    for o in &objs {
        bbox.add(o.bbox.min);
        bbox.add(o.bbox.max);
    }
    let order = sequence(&objs, profile, &bbox, opts.keep_order);
    // the first puff satin object gets a stop: the operator places the foam
    let foam_at = order.iter().copied().find(|&i| objs[i].puff_stage == 2);
    let anchor = Pt::new(bbox.center().x, bbox.max.y);

    let mut cmds = Vec::new();
    let mut stops: Vec<Stop> = Vec::new();
    let mut infos = Vec::new();
    let mut needles_for_thread: Vec<(usize, ThreadWeight)> = Vec::new();
    let mut cur: Option<Pt> = None;
    let mut cur_thread: Option<(usize, ThreadWeight)> = None;
    let mut last_dir = Pt::new(1.0, 0.0);
    let mut last_layer: Option<usize> = None;
    let mut layer_needles: std::collections::BTreeMap<usize, Vec<Needle>> = Default::default();
    // thinnest satin stroke per layer, for one weight warning per layer
    let mut thin: std::collections::BTreeMap<usize, (String, ThreadWeight, f64)> =
        Default::default();

    for (seq, &oi) in order.iter().enumerate() {
        let o = &objs[oi];
        let from = match (profile, cur) {
            (Profile::Cap, _) => anchor,
            (_, Some(p)) => p,
            (_, None) => bbox.min,
        };
        let needles = match art.fixed.get(&o.layer) {
            Some(n) => n.clone(),
            None => generate(o, recipe, profile, from),
        };
        layer_needles
            .entry(o.layer)
            .or_insert_with(Vec::new)
            .extend(needles.iter().copied());
        if needles.is_empty() {
            warnings.push(format!("{}: produced no stitches", o.label));
            continue;
        }
        // Colour stop before this object?
        let layer_start = last_layer != Some(o.layer);
        last_layer = Some(o.layer);
        let wt = o.opts.weight.unwrap_or(recipe.weight);
        let differs = cur_thread != Some((o.thread, wt));
        let foam_here = Some(oi) == foam_at && !needles.is_empty();
        let new_stop = if cur_thread.is_none() || foam_here {
            true
        } else if layer_start && o.stop.is_some() {
            o.stop.unwrap()
        } else if o.stop == Some(false) || !layer_start {
            // A layer's stop setting covers all of its pieces: no surprise
            // stops partway through a layer.
            false
        } else {
            differs && opts.auto_stops
        };
        let mut starts_stop = false;
        if new_stop {
            if cur_thread.is_some() {
                tie_off(&mut cmds, cur, last_dir);
                cmds.push(Cmd::Trim);
            }
            let needle = match needles_for_thread.iter().position(|&t| t == (o.thread, wt)) {
                Some(i) => i + 1,
                None => {
                    needles_for_thread.push((o.thread, wt));
                    needles_for_thread.len()
                }
            };
            cmds.push(Cmd::Color(stops.len()));
            stops.push(Stop {
                thread: chart[o.thread].clone(),
                weight: wt,
                needle,
                stitches: 0,
                objects: Vec::new(),
                note: if foam_here {
                    format!(
                        "STOP: place {:.0} mm puff foam over the puff area, then continue",
                        o.opts.foam_mm.unwrap_or(3.0)
                    )
                } else {
                    String::new()
                },
                top_m: 0.0,
                bobbin_m: 0.0,
            });
            cur_thread = Some((o.thread, wt));
            starts_stop = true;
        } else if differs && layer_start {
            let (ct, cw) = cur_thread.unwrap();
            if ct != o.thread {
                warnings.push(format!(
                    "{}: no stop before it, so it sews in {} instead of {}",
                    o.label, chart[ct].name, chart[o.thread].name
                ));
            } else {
                warnings.push(format!(
                    "{}: no stop before it, so it sews in {} instead of {}",
                    o.label,
                    cw.label(),
                    wt.label()
                ));
            }
        }
        let stop = stops.last_mut().unwrap();
        stop.objects.push(o.label.clone());
        let cmd_start = cmds.len();
        let before = count_stitches(&cmds);
        let trims_before = cmds.iter().filter(|c| matches!(c, Cmd::Trim)).count();
        let mut pending_tie_in: Option<Pt> = None;
        for nd in needles {
            let here = cur.unwrap_or(Pt::new(f64::NAN, f64::NAN));
            let d = here.dist(nd.p);
            if nd.jump && !(d < 1.2) {
                if d > Limits::TRIM_AFTER || cur.is_none() {
                    if cur.is_some()
                        && !matches!(cmds.last(), Some(Cmd::Trim) | Some(Cmd::Color(_)))
                    {
                        tie_off(&mut cmds, cur, last_dir);
                        cmds.push(Cmd::Trim);
                    }
                }
                cmds.push(Cmd::Jump(nd.p));
                cmds.push(Cmd::Stitch(nd.p));
                pending_tie_in = Some(nd.p);
            } else {
                if let Some(p) = pending_tie_in.take() {
                    let u = nd.p.sub(p).norm();
                    if nd.p.dist(p) > 0.8 {
                        cmds.push(Cmd::Stitch(p.add(u.mul(0.5))));
                        cmds.push(Cmd::Stitch(p));
                    }
                }
                if let Some(c) = cur {
                    if c.dist(nd.p) > 0.05 {
                        last_dir = nd.p.sub(c).norm();
                    }
                }
                cmds.push(Cmd::Stitch(nd.p));
            }
            cur = Some(nd.p);
        }
        let added = count_stitches(&cmds) - before;
        stop.stitches += added;
        infos.push(ObjectInfo {
            sequence: seq + 1,
            label: o.label.clone(),
            stitch_type: o.stitch_label(),
            thread_code: chart[o.thread].code.clone(),
            weight: wt,
            puff: o.puff_stage == 2,
            color_match_delta_e: (o.color_error * 10.0).round() / 10.0,
            stitches: added,
            trims: cmds.iter().filter(|c| matches!(c, Cmd::Trim)).count() - trims_before,
            cmd_range: (cmd_start, cmds.len()),
            layer: o.layer,
            stop: stops.len() - 1,
            starts_stop,
            angle_deg: {
                let d = ((-o.angle).to_degrees().rem_euclid(180.0) * 10.0).round() / 10.0;
                if d >= 180.0 { 0.0 } else { d }
            },
            wanted_thread: o.thread,
        });
        if o.kind == StitchType::Satin {
            let narrow = match &o.shape {
                crate::import::ShapeKind::Area { rings, rule } => {
                    // typical stroke width of a lettering or satin area
                    2.0 * crate::geom::rings_area(rings, *rule)
                        / crate::geom::rings_perimeter(rings).max(1e-9)
                }
                _ => o.bbox.width().min(o.bbox.height()),
            };
            if o.puff_stage == 2 && narrow < 3.0 {
                let e = thin.entry(usize::MAX - o.layer).or_insert((format!("{} (3D puff)", piece_name(&o.label)), wt, narrow));
                e.2 = e.2.min(narrow);
            } else if narrow < wt.min_column() - 0.05 {
                let e = thin.entry(o.layer).or_insert((piece_name(&o.label), wt, narrow));
                e.2 = e.2.min(narrow);
            }
        }
    }
    tie_off(&mut cmds, cur, last_dir);
    cmds.push(Cmd::Trim);
    cmds.push(Cmd::End);

    for (key, (name, wt, narrow)) in thin.iter() {
        if *key > usize::MAX / 2 {
            warnings.push(format!(
                "{name}: columns as thin as {narrow:.1} mm; puff needs at least 3 mm to rise and cover the foam"
            ));
            continue;
        }
        let finer = ThreadWeight::ALL
            .iter()
            .filter(|w| w.min_column() <= *narrow + 0.05 && **w != *wt)
            .max_by(|a, b| a.min_column().partial_cmp(&b.min_column()).unwrap());
        let tip = match finer {
            Some(f) if f.number() > wt.number() => format!(" — try {}", f.label()),
            _ => " — enlarge it or use a run stitch".into(),
        };
        warnings.push(format!(
            "{name}: strokes as thin as {:.1} mm; {} needs {:.1} mm{tip}",
            narrow,
            wt.label(),
            wt.min_column()
        ));
    }
    if foam_at.is_some() {
        warnings.push(
            "3D puff: run at 500–700 stitches/min; tear away the loose foam and touch up with a little heat after sewing"
                .into(),
        );
    }
    let mut stats = compute_stats(&cmds, &bbox, profile);
    thread_usage(&cmds, &mut stops, &mut stats);
    match profile {
        Profile::Cap => {
            let (fw, fh) = recipe.cap_size.limit();
            if bbox.width() > fw + 0.5 || bbox.height() > fh + 0.5 {
                warnings.push(format!(
                    "design {:.2} × {:.2} in is larger than the {} cap front limit ({:.2} × {:.2} in max)",
                    bbox.width() / 25.4,
                    bbox.height() / 25.4,
                    match recipe.cap_size {
                        crate::settings::CapSize::Adult => "adult",
                        crate::settings::CapSize::Youth => "youth (kids)",
                    },
                    fw / 25.4,
                    fh / 25.4
                ));
            }
        }
        Profile::Flat => {
            let (fw, fh) = profile.frame_limit();
            if bbox.width() > fw || bbox.height() > fh {
                warnings.push(format!(
                    "design {:.0} × {:.0} mm exceeds the {} frame limit of {:.0} × {:.0} mm",
                    bbox.width(),
                    bbox.height(),
                    profile.label(),
                    fw,
                    fh
                ));
            }
        }
    }
    if stats.short_stitches > 0 {
        warnings.push(format!(
            "{} stitches shorter than {} mm",
            stats.short_stitches,
            Limits::SHORT_STITCH
        ));
    }
    // one colour warning per layer, not per letter or piece
    let mut warned = std::collections::HashSet::new();
    for i in &infos {
        if i.color_match_delta_e > 10.0 && warned.insert(i.layer) {
            let name = objs
                .iter()
                .find(|o| o.layer == i.layer)
                .map(|o| piece_name(&o.label))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| i.label.clone());
            warnings.push(format!(
                "{}: closest thread {} is a loose color match (ΔE {:.0}); check the chart",
                name, i.thread_code, i.color_match_delta_e
            ));
        }
    }
    Ok(Design {
        name: name.into(),
        profile,
        recipe: recipe.clone(),
        cmds,
        stops,
        objects: infos,
        stats,
        bbox,
        warnings,
        layer_needles,
    })
}

/// "Name 3" is piece 3 of layer "Name".
fn piece_name(label: &str) -> String {
    match label.rsplit_once(' ') {
        Some((head, n)) if !head.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            head.to_string()
        }
        _ => label.to_string(),
    }
}

fn tie_off(cmds: &mut Vec<Cmd>, cur: Option<Pt>, dir: Pt) {
    if let Some(p) = cur {
        if matches!(cmds.last(), Some(Cmd::Stitch(_))) {
            cmds.push(Cmd::Stitch(p.sub(dir.mul(0.5))));
            cmds.push(Cmd::Stitch(p));
        }
    }
}

fn count_stitches(cmds: &[Cmd]) -> usize {
    cmds.iter().filter(|c| matches!(c, Cmd::Stitch(_))).count()
}

/// Build a viewable design from an existing stitch file (DST/EXP/PES):
/// one "object" per colour stop, threads chosen by the user.
pub fn from_machine(
    name: &str,
    cmds: Vec<Cmd>,
    threads: &[Thread],
    profile: Profile,
    recipe: &Recipe,
) -> Result<Design, String> {
    let mut bbox = BBox::empty();
    for c in &cmds {
        if let Cmd::Stitch(p) = c {
            bbox.add(*p);
        }
    }
    if !bbox.is_valid() {
        return Err("the file contains no stitches".into());
    }
    let mut stops: Vec<Stop> = Vec::new();
    let mut objects = Vec::new();
    let mut needles: Vec<Thread> = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    let mut trims = 0usize;
    let close = |stops: &mut Vec<Stop>,
                 objects: &mut Vec<ObjectInfo>,
                 end: usize,
                 count: usize,
                 trims: usize,
                 start: usize| {
        let i = stops.len().wrapping_sub(1);
        if let Some(st) = stops.last_mut() {
            st.stitches = count;
            objects.push(ObjectInfo {
                sequence: i + 1,
                label: format!("Stop {}", i + 1),
                stitch_type: "Stitches",
                thread_code: st.thread.code.clone(),
                weight: st.weight,
                puff: false,
                color_match_delta_e: 0.0,
                stitches: count,
                trims,
                cmd_range: (start, end),
                layer: i,
                stop: i,
                starts_stop: true,
                angle_deg: 0.0,
                wanted_thread: i,
            });
        }
    };
    for (i, c) in cmds.iter().enumerate() {
        match c {
            Cmd::Color(k) => {
                close(&mut stops, &mut objects, i, count, trims, start);
                let t = threads
                    .get(*k)
                    .cloned()
                    .unwrap_or_else(|| crate::thread::generic_chart()[k % 31].clone());
                let needle = match needles.iter().position(|n| *n == t) {
                    Some(n) => n + 1,
                    None => {
                        needles.push(t.clone());
                        needles.len()
                    }
                };
                stops.push(Stop {
                    thread: t,
                    weight: recipe.weight,
                    needle,
                    stitches: 0,
                    objects: vec![format!("Stop {}", k + 1)],
                    note: String::new(),
                    top_m: 0.0,
                    bobbin_m: 0.0,
                });
                start = i;
                count = 0;
                trims = 0;
            }
            Cmd::Stitch(_) => count += 1,
            Cmd::Trim => trims += 1,
            _ => {}
        }
    }
    close(&mut stops, &mut objects, cmds.len(), count, trims, start);
    let mut stats = compute_stats(&cmds, &bbox, profile);
    thread_usage(&cmds, &mut stops, &mut stats);
    Ok(Design {
        name: name.into(),
        profile,
        recipe: recipe.clone(),
        cmds,
        stops,
        objects,
        stats,
        bbox,
        warnings: Vec::new(),
        layer_needles: Default::default(),
    })
}

/// Fill in each stop's estimated thread use and the design total.
pub fn thread_usage(cmds: &[Cmd], stops: &mut [Stop], stats: &mut Stats) {
    let mut path = vec![0.0f64; stops.len()];
    let mut count = vec![0usize; stops.len()];
    let mut stop: Option<usize> = None;
    let mut prev: Option<Pt> = None;
    for c in cmds {
        match *c {
            Cmd::Color(i) => {
                stop = Some(i);
                prev = None;
            }
            Cmd::Jump(p) => prev = Some(p),
            Cmd::Trim => prev = None,
            Cmd::Stitch(p) => {
                if let Some(i) = stop.filter(|&i| i < path.len()) {
                    if let Some(q) = prev {
                        path[i] += q.dist(p);
                    }
                    count[i] += 1;
                }
                prev = Some(p);
            }
            Cmd::End => {}
        }
    }
    stats.top_m = 0.0;
    stats.bobbin_m = 0.0;
    for (i, st) in stops.iter_mut().enumerate() {
        let (t, b) = crate::thread::usage_m(path[i], count[i]);
        st.top_m = t;
        st.bobbin_m = b;
        stats.top_m += t;
        stats.bobbin_m += b;
    }
}

fn compute_stats(cmds: &[Cmd], bbox: &BBox, profile: Profile) -> Stats {
    let mut s = Stats {
        width_mm: bbox.width(),
        height_mm: bbox.height(),
        ..Default::default()
    };
    let mut prev: Option<Pt> = None;
    let mut prev_was_stitch = false;
    for c in cmds {
        match *c {
            Cmd::Stitch(p) => {
                s.stitches += 1;
                if let (Some(q), true) = (prev, prev_was_stitch) {
                    let d = q.dist(p);
                    if d > 0.0 && d < Limits::SHORT_STITCH {
                        s.short_stitches += 1;
                    }
                    s.longest_stitch_mm = s.longest_stitch_mm.max(d);
                }
                prev = Some(p);
                prev_was_stitch = true;
            }
            Cmd::Jump(p) => {
                s.jumps += 1;
                prev = Some(p);
                prev_was_stitch = false;
            }
            Cmd::Trim => s.trims += 1,
            Cmd::Color(_) => {}
            Cmd::End => {}
        }
    }
    s.color_changes = cmds
        .iter()
        .filter(|c| matches!(c, Cmd::Color(_)))
        .count()
        .saturating_sub(1);
    s.run_minutes =
        s.stitches as f64 / profile.spm() + s.color_changes as f64 * 0.12 + s.trims as f64 * 0.05;
    s
}

#[cfg(test)]
mod tests {
    fn art(svg: &str) -> Artwork {
        let (opt, _) = crate::import::font_options(None).unwrap();
        crate::import::load_svg_str(svg, &opt).unwrap()
    }
    fn stitches(d: &Design) -> usize {
        d.stats.stitches
    }

    #[test]
    fn underlay_kinds_change_the_stitching() {
        let chart = crate::thread::generic_chart();
        let rec = crate::settings::recipe("pique").unwrap();
        let svg = |tag: &str| format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='60mm' height='40mm' viewBox='0 0 60 40'>\
             <rect id='bar {tag}' x='5' y='15' width='50' height='6' fill='#123'/>\
             <rect id='panel sf-fill' x='5' y='25' width='50' height='12' fill='#a33'/></svg>");
        let build = |tag: &str, ul: crate::import::Underlay, fill_ul: crate::import::Underlay| {
            let mut a = art(&svg(tag));
            a.shapes[0].opts.stitch = crate::import::StitchChoice::Satin;
            a.shapes[0].opts.satin_underlay = ul;
            a.shapes[1].opts.fill_underlay = fill_ul;
            build("t", &a, &chart, &rec, Profile::Flat).unwrap()
        };
        use crate::import::Underlay as U;
        let center = stitches(&build("", U::CenterRun, U::Tatami));
        let edge_zz = stitches(&build("", U::EdgeZigzag, U::Tatami));
        let none = stitches(&build("", U::Off, U::Off));
        let double = stitches(&build("", U::CenterRun, U::DoubleTatami));
        assert!(edge_zz > center + 40, "{center} {edge_zz}");
        assert!(none < center, "{none} {center}");
        assert!(double > center + 40, "{double} {center}");
    }

    #[test]
    fn puff_gets_contour_foam_stop_and_sews_last() {
        let chart = crate::thread::generic_chart();
        let rec = crate::settings::recipe("cap").unwrap();
        let a = art("<svg xmlns='http://www.w3.org/2000/svg' width='80mm' height='40mm' viewBox='0 0 80 40'>\
             <rect id='bar sf-puff' x='10' y='5' width='60' height='8' fill='#eee'/>\
             <rect id='line' x='10' y='25' width='60' height='4' fill='#22a'/></svg>");
        let d = build("p", &a, &chart, &rec, Profile::Cap).unwrap();
        // flat line first, then puff contour, then the foam stop + puff satin
        let kinds: Vec<&str> = d.objects.iter().map(|o| o.stitch_type).collect();
        assert_eq!(kinds.last(), Some(&"3D puff satin"), "{kinds:?}");
        assert!(kinds.iter().position(|k| *k == "Puff contour").unwrap() > 0);
        let foam = d.stops.iter().filter(|s| s.note.contains("foam")).count();
        assert_eq!(foam, 1);
        assert!(d.objects.last().unwrap().puff);
        // puff satin is packed tight: many more stitches than flat satin
        let mut flat = a.clone();
        flat.shapes[0].opts.puff = false;
        flat.shapes[0].opts.split_width = Some(12.0);
        let f = build("f", &flat, &chart, &rec, Profile::Cap).unwrap();
        assert!(d.stats.stitches > f.stats.stitches + 300, "puff {} flat {} {:?}", d.stats.stitches, f.stats.stitches, d.objects.iter().map(|o| (o.stitch_type, o.stitches)).collect::<Vec<_>>());
    }

    #[test]
    fn cap_sizes_adult_and_youth() {
        let chart = crate::thread::generic_chart();
        let mut rec = crate::settings::recipe("cap").unwrap();
        // 2.1 in tall: fine on adult caps, too tall for youth
        let a = art("<svg xmlns='http://www.w3.org/2000/svg' width='90mm' height='60mm' viewBox='0 0 90 60'>\
             <rect id='box sf-fill' x='0' y='0' width='90' height='53.3' fill='#333'/></svg>");
        let adult = build("a", &a, &chart, &rec, Profile::Cap).unwrap();
        assert!(!adult.warnings.iter().any(|w| w.contains("cap front")), "{:?}", adult.warnings);
        rec.cap_size = crate::settings::CapSize::Youth;
        let youth = build("y", &a, &chart, &rec, Profile::Cap).unwrap();
        assert!(youth.warnings.iter().any(|w| w.contains("youth")), "{:?}", youth.warnings);
    }

    #[test]
    fn short_stitches_on_curves() {
        // a tight arc: the inside edge gets fewer full-depth penetrations
        let ribs: Vec<Pt> = (0..40)
            .map(|i| {
                let a = i as f64 * 0.05;
                let r = if i % 2 == 0 { 2.0 } else { 6.0 };
                Pt::new(r * a.cos(), r * a.sin())
            })
            .collect();
        let s = crate::stitch::short_stitches_for_test(&ribs, 0.4);
        let moved = ribs.iter().zip(&s).filter(|(a, b)| a.dist(**b) > 0.01).count();
        assert!(moved >= 5, "{moved}");
    }

    use super::*;
    use crate::geom::{FillRule, Pt};
    use crate::import::{Shape, ShapeKind, ShapeOpts};

    fn square(x: f64, rgb: [u8; 3], layer: usize, stop: Option<bool>) -> Shape {
        let r = vec![
            Pt::new(x, 0.0),
            Pt::new(x + 8.0, 0.0),
            Pt::new(x + 8.0, 8.0),
            Pt::new(x, 8.0),
        ];
        Shape {
            curves: Vec::new(),
            kind: ShapeKind::Area {
                rings: vec![r],
                rule: FillRule::NonZero,
            },
            rgb,
            label: format!("L{layer}"),
            group: layer,
            opts: ShapeOpts {
                layer,
                stop,
                ..Default::default()
            },
        }
    }

    #[test]
    fn stop_off_holds_for_every_piece_of_a_layer() {
        // Layer 0 green; layer 1 black with two separate pieces and its stop off.
        let mut s2 = square(20.0, [10, 10, 10], 1, Some(false));
        if let (ShapeKind::Area { rings, .. }, ShapeKind::Area { rings: r2, .. }) = (
            &mut s2.kind,
            square(40.0, [10, 10, 10], 1, Some(false)).kind,
        ) {
            rings.extend(r2);
        }
        let art = Artwork {
            fixed: Default::default(),
            shapes: vec![square(0.0, [20, 140, 70], 0, None), s2],
            warnings: vec![],
        };
        let chart = crate::thread::generic_chart();
        let r = crate::settings::recipe("pique").unwrap();
        let d = build_with(
            "t",
            &art,
            &chart,
            &r,
            Profile::Flat,
            BuildOptions {
                keep_order: true,
                auto_stops: true,
            },
        )
        .unwrap();
        assert_eq!(d.stops.len(), 1, "no stop anywhere in layer 1");
        assert!(d.objects.iter().all(|o| o.stop == 0));
    }

    #[test]
    fn forced_stop_with_same_thread() {
        let art = Artwork {
            fixed: Default::default(),
            shapes: vec![
                square(0.0, [20, 140, 70], 0, None),
                square(20.0, [20, 140, 70], 1, Some(true)),
            ],
            warnings: vec![],
        };
        let chart = crate::thread::generic_chart();
        let r = crate::settings::recipe("pique").unwrap();
        let d = build_with(
            "t",
            &art,
            &chart,
            &r,
            Profile::Flat,
            BuildOptions {
                keep_order: true,
                auto_stops: true,
            },
        )
        .unwrap();
        assert_eq!(d.stops.len(), 2);
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;
    use crate::geom::{FillRule, Pt};
    use crate::import::{Shape, ShapeKind, ShapeOpts};

    fn sq(x: f64, y: f64, s: f64, rgb: [u8; 3], layer: usize, opts: ShapeOpts) -> Shape {
        let r = vec![
            Pt::new(x, y),
            Pt::new(x + s, y),
            Pt::new(x + s, y + s),
            Pt::new(x, y + s),
        ];
        Shape {
            curves: Vec::new(),
            kind: ShapeKind::Area {
                rings: vec![r],
                rule: FillRule::NonZero,
            },
            rgb,
            label: format!("L{layer}"),
            group: layer,
            opts: ShapeOpts { layer, ..opts },
        }
    }

    fn stitches(shapes: Vec<Shape>) -> usize {
        let art = Artwork {
            fixed: Default::default(),
            shapes,
            warnings: vec![],
        };
        let r = crate::settings::recipe("pique").unwrap();
        build_with(
            "t",
            &art,
            &crate::thread::generic_chart(),
            &r,
            Profile::Flat,
            BuildOptions {
                keep_order: true,
                auto_stops: true,
            },
        )
        .unwrap()
        .objects[0]
            .stitches
    }

    #[test]
    fn keep_under_sews_more_than_cut_out() {
        let top = sq(10.0, 10.0, 20.0, [200, 20, 40], 1, ShapeOpts::default());
        let cut = stitches(vec![
            sq(0.0, 0.0, 40.0, [20, 140, 70], 0, ShapeOpts::default()),
            top.clone(),
        ]);
        let keep = stitches(vec![
            sq(
                0.0,
                0.0,
                40.0,
                [20, 140, 70],
                0,
                ShapeOpts {
                    keep_under: true,
                    ..Default::default()
                },
            ),
            top,
        ]);
        assert!(keep > cut + 150, "keep {keep} vs cut {cut}");
    }

    #[test]
    fn underlay_off_sews_fewer() {
        let with = stitches(vec![sq(
            0.0,
            0.0,
            30.0,
            [20, 140, 70],
            0,
            ShapeOpts::default(),
        )]);
        let without = stitches(vec![sq(
            0.0,
            0.0,
            30.0,
            [20, 140, 70],
            0,
            ShapeOpts {
                no_underlay: true,
                ..Default::default()
            },
        )]);
        assert!(without + 100 < with, "without {without} vs with {with}");
    }

    #[test]
    fn old_projects_without_new_options_still_load() {
        let json = r#"{"stitch":"Auto","angle_deg":null,"thread":null,"stop":null,"layer":0}"#;
        let o: ShapeOpts = serde_json::from_str(json).unwrap();
        assert!(!o.no_underlay && !o.keep_under && o.column_width.is_none());
    }
}
