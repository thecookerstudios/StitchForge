//! Sequencing (garment profiles) and assembly into machine commands.

use crate::geom::{BBox, Pt};
use crate::import::Artwork;
use crate::settings::{Limits, Profile, Recipe};
use crate::stitch::{Planned, StitchType, generate, plan};
use crate::thread::Thread;
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
    pub needle: usize,
    pub stitches: usize,
    pub objects: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObjectInfo {
    pub sequence: usize,
    pub label: String,
    pub stitch_type: &'static str,
    pub thread_code: String,
    pub color_match_delta_e: f64,
    pub stitches: usize,
    pub trims: usize,
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
}

/// Sequence objects. Stacking order is respected wherever objects overlap
/// (a lower layer always sews before what covers it); within that rule:
/// Flat — keep the current colour as long as possible, then stack order.
/// Cap  — closest to the vertical centre line first, bottom first, and stay
///        on the current colour when objects are equally central.
pub fn sequence(objs: &[Planned], profile: Profile, design: &BBox) -> Vec<usize> {
    let n = objs.len();
    let mut preds = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for j in 0..n {
        for i in 0..j {
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
    let mut cur_thread: Option<usize> = None;
    for _ in 0..n {
        let pick = (0..n)
            .filter(|&i| !done[i] && preds[i] == 0)
            .min_by_key(|&i| {
                let o = &objs[i];
                let color_change = (Some(o.thread) != cur_thread) as i64;
                match profile {
                    Profile::Flat => (0, color_change, o.stack as i64, 0),
                    Profile::Cap => (
                        center_bucket(&o.bbox),
                        color_change,
                        -(o.bbox.max.y * 10.0) as i64,
                        o.stack as i64,
                    ),
                }
            })
            .expect("stacking constraints are acyclic");
        done[pick] = true;
        cur_thread = Some(objs[pick].thread);
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
    let mut warnings = art.warnings.clone();
    let objs = plan(art, chart, profile, &mut warnings);
    if objs.is_empty() {
        return Err("nothing left to stitch after skipping tiny shapes".into());
    }
    let mut bbox = BBox::empty();
    for o in &objs {
        bbox.add(o.bbox.min);
        bbox.add(o.bbox.max);
    }
    let order = sequence(&objs, profile, &bbox);
    let anchor = Pt::new(bbox.center().x, bbox.max.y);

    let mut cmds = Vec::new();
    let mut stops: Vec<Stop> = Vec::new();
    let mut infos = Vec::new();
    let mut needles_for_thread: Vec<usize> = Vec::new();
    let mut cur: Option<Pt> = None;
    let mut cur_thread: Option<usize> = None;
    let mut last_dir = Pt::new(1.0, 0.0);

    for (seq, &oi) in order.iter().enumerate() {
        let o = &objs[oi];
        let from = match (profile, cur) {
            (Profile::Cap, _) => anchor,
            (_, Some(p)) => p,
            (_, None) => bbox.min,
        };
        let needles = generate(o, recipe, profile, from);
        if needles.is_empty() {
            warnings.push(format!("{}: produced no stitches", o.label));
            continue;
        }
        if cur_thread != Some(o.thread) {
            if cur_thread.is_some() {
                tie_off(&mut cmds, cur, last_dir);
                cmds.push(Cmd::Trim);
            }
            let needle = match needles_for_thread.iter().position(|&t| t == o.thread) {
                Some(i) => i + 1,
                None => {
                    needles_for_thread.push(o.thread);
                    needles_for_thread.len()
                }
            };
            cmds.push(Cmd::Color(stops.len()));
            stops.push(Stop {
                thread: chart[o.thread].clone(),
                needle,
                stitches: 0,
                objects: Vec::new(),
            });
            cur_thread = Some(o.thread);
        }
        let stop = stops.last_mut().unwrap();
        stop.objects.push(o.label.clone());
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
            stitch_type: o.kind.label(),
            thread_code: chart[o.thread].code.clone(),
            color_match_delta_e: (o.color_error * 10.0).round() / 10.0,
            stitches: added,
            trims: cmds.iter().filter(|c| matches!(c, Cmd::Trim)).count() - trims_before,
        });
        if o.kind == StitchType::Satin && o.bbox.width().min(o.bbox.height()) < 1.0 {
            warnings.push(format!(
                "{}: very narrow column (under 1 mm), may not hold",
                o.label
            ));
        }
    }
    tie_off(&mut cmds, cur, last_dir);
    cmds.push(Cmd::Trim);
    cmds.push(Cmd::End);

    let stats = compute_stats(&cmds, &bbox, profile);
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
    if stats.short_stitches > 0 {
        warnings.push(format!(
            "{} stitches shorter than {} mm",
            stats.short_stitches,
            Limits::SHORT_STITCH
        ));
    }
    for i in &infos {
        if i.color_match_delta_e > 10.0 {
            warnings.push(format!(
                "{}: closest thread {} is a loose color match (ΔE {:.0}); check the chart",
                i.label, i.thread_code, i.color_match_delta_e
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
    })
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
