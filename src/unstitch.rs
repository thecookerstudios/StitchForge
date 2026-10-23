//! Stitch file → editable shapes.
//!
//! A stitch file only stores needle points. To get shapes back, each colour
//! stop's stitches are drawn onto a fine grid, the gaps between neighbouring
//! rows are closed, and the solid regions are outlined and fitted with
//! curves, the same way a traced picture is. Each region's own stitches then
//! tell what it was:
//!
//! * stitches that keep turning back across the region → satin;
//! * rows of stitches that mostly carry on in one direction → fill, with the
//!   angle the rows ran at;
//! * thin lines outside any region → run (or triple run when the stitches
//!   go back and forth over themselves).
//!
//! The result is ordinary artwork: one layer per region or line, in the
//! order it was sewn, with its thread colour, ready to edit and regenerate.

use crate::design::Cmd;
use crate::geom::{BBox, FillRule, Pt, VPath, fit_path};
use crate::import::{Artwork, Shape, ShapeKind, ShapeOpts, StitchChoice};

/// What came back, for the status line.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub fills: usize,
    pub satins: usize,
    pub runs: usize,
}

#[derive(Clone, Copy)]
struct Seg {
    a: Pt,
    b: Pt,
    /// Sewing order.
    idx: usize,
}

/// Grid used for one colour stop.
struct Grid {
    w: usize,
    h: usize,
    origin: Pt,
    res: f64,
}

impl Grid {
    fn cell(&self, p: Pt) -> Option<usize> {
        let x = ((p.x - self.origin.x) / self.res).floor();
        let y = ((p.y - self.origin.y) / self.res).floor();
        if x < 0.0 || y < 0.0 || x >= self.w as f64 || y >= self.h as f64 {
            return None;
        }
        Some(y as usize * self.w + x as usize)
    }
}

/// Chamfer (3-4) distance from every cell to the nearest set cell, in
/// thirds of a cell.
fn distance(mask: &[bool], w: usize, h: usize) -> Vec<u32> {
    const BIG: u32 = u32::MAX / 4;
    let mut d: Vec<u32> = mask.iter().map(|&m| if m { 0 } else { BIG }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + 3);
            }
            if y > 0 {
                v = v.min(d[i - w] + 3);
                if x > 0 {
                    v = v.min(d[i - w - 1] + 4);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + 4);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + 3);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + 3);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + 4);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + 4);
                }
            }
            d[i] = v;
        }
    }
    d
}

fn dilate(mask: &[bool], w: usize, h: usize, r: f64) -> Vec<bool> {
    let lim = (r * 3.0).round() as u32;
    distance(mask, w, h).into_iter().map(|v| v <= lim).collect()
}

fn erode(mask: &[bool], w: usize, h: usize, r: f64) -> Vec<bool> {
    let inv: Vec<bool> = mask.iter().map(|m| !m).collect();
    dilate(&inv, w, h, r).into_iter().map(|v| !v).collect()
}

/// Connected regions (4-neighbour), labelled from 1; 0 = empty.
fn regions(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, usize) {
    let mut lab = vec![0u32; mask.len()];
    let mut n = 0;
    let mut stack = Vec::new();
    for s in 0..mask.len() {
        if !mask[s] || lab[s] != 0 {
            continue;
        }
        n += 1;
        lab[s] = n as u32;
        stack.push(s);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let mut go = |j: usize| {
                if mask[j] && lab[j] == 0 {
                    lab[j] = n as u32;
                    stack.push(j);
                }
            };
            if x > 0 {
                go(i - 1);
            }
            if x + 1 < w {
                go(i + 1);
            }
            if y > 0 {
                go(i - w);
            }
            if y + 1 < h {
                go(i + w);
            }
        }
    }
    (lab, n)
}

/// Undo back-and-forth stitching (A→B→A→B…) so a triple or bean run
/// becomes its single path.
fn collapse_backtrack(pts: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::new();
    for &p in pts {
        if out.len() >= 2 && out[out.len() - 2].dist(p) < 0.25 {
            out.pop();
        } else if out.last().is_none_or(|q| q.dist(p) > 0.05) {
            out.push(p);
        }
    }
    out
}

fn poly_len(pts: &[Pt]) -> f64 {
    pts.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// Stitch direction (degrees, 0 = across, counter-clockwise on screen) that
/// most of these stitches run at, by length.
fn main_angle(segs: &[&Seg]) -> f64 {
    const BINS: usize = 72; // 2.5° each over 0–180°
    let mut hist = [0.0f64; BINS];
    let ang = |s: &Seg| {
        let a = (-(s.b.y - s.a.y)).atan2(s.b.x - s.a.x).to_degrees();
        a.rem_euclid(180.0)
    };
    for s in segs {
        let l = s.a.dist(s.b);
        if l < 0.5 {
            continue;
        }
        hist[(ang(s) / 180.0 * BINS as f64) as usize % BINS] += l;
    }
    // smooth over neighbouring bins (wrapping at 180°), then take the peak
    let sm: Vec<f64> = (0..BINS)
        .map(|i| hist[(i + BINS - 1) % BINS] + 2.0 * hist[i] + hist[(i + 1) % BINS])
        .collect();
    let peak = (0..BINS).max_by(|&a, &b| sm[a].total_cmp(&sm[b])).unwrap_or(0);
    let centre = (peak as f64 + 0.5) * 180.0 / BINS as f64;
    // refine: length-weighted mean of the stitches within 10° of the peak
    let (mut sum, mut wt) = (0.0, 0.0);
    for s in segs {
        let l = s.a.dist(s.b);
        if l < 0.5 {
            continue;
        }
        let mut d = ang(s) - centre;
        if d > 90.0 {
            d -= 180.0;
        }
        if d < -90.0 {
            d += 180.0;
        }
        if d.abs() <= 10.0 {
            sum += d * l;
            wt += l;
        }
    }
    let a = if wt > 0.0 { centre + sum / wt } else { centre };
    (a.rem_euclid(180.0) * 10.0).round() / 10.0
}

/// Share of consecutive stitches in `segs` (sewing order) that turn back on
/// the one before: close to 1 for satin, low for fill rows.
fn turn_back_ratio(segs: &[&Seg]) -> f64 {
    let (mut pairs, mut back) = (0usize, 0usize);
    for w in segs.windows(2) {
        let (s, t) = (w[0], w[1]);
        if t.idx != s.idx + 1 {
            continue;
        }
        let u = s.b.sub(s.a);
        let v = t.b.sub(t.a);
        let (lu, lv) = (u.len(), v.len());
        if lu < 0.3 || lv < 0.3 {
            continue;
        }
        pairs += 1;
        if (u.x * v.x + u.y * v.y) / (lu * lv) < -0.5 {
            back += 1;
        }
    }
    if pairs == 0 { 0.0 } else { back as f64 / pairs as f64 }
}

/// Turn a stitch file's commands (mm, y down) back into editable shapes.
/// `colors` gives each stop's thread colour.
pub fn unstitch(cmds: &[Cmd], colors: &[[u8; 3]]) -> (Artwork, Summary) {
    // ---- stitches per stop
    let mut stops: Vec<Vec<Seg>> = vec![Vec::new()];
    let mut stop = 0usize;
    let mut last: Option<Pt> = None;
    let mut idx = 0usize;
    for c in cmds {
        match *c {
            Cmd::Color(i) => {
                stop = i;
                while stops.len() <= stop {
                    stops.push(Vec::new());
                }
            }
            Cmd::Jump(p) => last = Some(p),
            Cmd::Stitch(p) => {
                if let Some(q) = last
                    && q.dist(p) > 0.05
                {
                    // a "stitch" longer than any machine sews is a move
                    if q.dist(p) <= 15.0 {
                        stops[stop].push(Seg { a: q, b: p, idx });
                    }
                    idx += 1;
                }
                last = Some(p);
            }
            Cmd::Trim | Cmd::End => {}
        }
    }

    // pieces found, with the sewing position they start at
    struct Piece {
        first: usize,
        stop: usize,
        shape: Shape,
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut sum = Summary::default();

    for (si, segs) in stops.iter().enumerate() {
        if segs.is_empty() {
            continue;
        }
        let rgb = colors.get(si).copied().unwrap_or([40, 40, 40]);
        // ---- draw this stop's stitches
        let mut b = BBox::empty();
        for s in segs {
            b.add(s.a);
            b.add(s.b);
        }
        let margin = 1.5;
        let (bw, bh) = (b.width() + 2.0 * margin, b.height() + 2.0 * margin);
        let mut res = 0.1f64;
        while (bw / res) * (bh / res) > 6.0e6 {
            res *= 1.25;
        }
        let g = Grid {
            w: (bw / res).ceil() as usize + 1,
            h: (bh / res).ceil() as usize + 1,
            origin: Pt::new(b.min.x - margin, b.min.y - margin),
            res,
        };
        let (w, h) = (g.w, g.h);
        let mut line = vec![false; w * h];
        for s in segs {
            let n = (s.a.dist(s.b) / (res * 0.5)).ceil().max(1.0) as usize;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let p = Pt::new(s.a.x + (s.b.x - s.a.x) * t, s.a.y + (s.b.y - s.a.y) * t);
                if let Some(i) = g.cell(p) {
                    line[i] = true;
                }
            }
        }
        // close the gaps between rows (thread ~0.2 mm either side of the
        // needle line, rows up to ~0.8 mm apart, letters further apart
        // stay separate), then open to drop the
        // lone run lines, which are dealt with separately below
        let r_thread = 0.15 / res;
        let r_close = 0.25 / res;
        let r_open = 0.3 / res;
        let thick = dilate(&line, w, h, r_thread + r_close);
        let closed = erode(&thick, w, h, r_close);
        let area = dilate(&erode(&closed, w, h, r_open), w, h, r_open);
        let (lab, n_reg) = regions(&area, w, h);

        // ---- each stitch goes with the region its middle is in
        let mut by_reg: Vec<Vec<&Seg>> = vec![Vec::new(); n_reg + 1];
        let mut loose: Vec<&Seg> = Vec::new();
        for s in segs {
            let mid = Pt::new((s.a.x + s.b.x) / 2.0, (s.a.y + s.b.y) / 2.0);
            match g.cell(mid).map(|i| lab[i]) {
                Some(r) if r > 0 => by_reg[r as usize].push(s),
                _ => loose.push(s),
            }
        }

        // ---- regions → filled or satin areas
        let mut reg_box = vec![(usize::MAX, usize::MAX, 0usize, 0usize, 0usize); n_reg + 1];
        for (i, &l) in lab.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let (x, y) = (i % w, i / w);
            let e = &mut reg_box[l as usize];
            e.0 = e.0.min(x);
            e.1 = e.1.min(y);
            e.2 = e.2.max(x);
            e.3 = e.3.max(y);
            e.4 += 1;
        }
        for r in 1..=n_reg {
            let (x0, y0, x1, y1, count) = reg_box[r];
            let mm2 = count as f64 * res * res;
            if mm2 < 1.5 || by_reg[r].len() < 3 {
                continue;
            }
            // this region alone, cropped
            let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
            let mut sub = vec![false; cw * ch];
            for y in 0..ch {
                for x in 0..cw {
                    sub[y * cw + x] = lab[(y + y0) * w + x + x0] == r as u32;
                }
            }
            let mut curves = Vec::new();
            for ring in crate::trace::contours(&sub, cw, ch, 1.0) {
                let mm: Vec<Pt> = ring
                    .iter()
                    .map(|q| Pt::new(g.origin.x + (q.x + x0 as f64) * res, g.origin.y + (q.y + y0 as f64) * res))
                    .collect();
                let a2: f64 = mm
                    .iter()
                    .zip(mm.iter().cycle().skip(1))
                    .map(|(p, q)| p.x * q.y - q.x * p.y)
                    .sum::<f64>()
                    / 2.0;
                if a2.abs() < 0.4 {
                    continue;
                }
                let mut v = fit_path(&mm, true, 0.12);
                crate::trace::straighten(&mut v, 0.12);
                if v.nodes.len() >= 2 {
                    curves.push(v);
                }
            }
            if curves.is_empty() {
                continue;
            }
            let mine = &by_reg[r];
            let satin = turn_back_ratio(mine) > 0.55;
            let mut opts = ShapeOpts::default();
            let label;
            if satin {
                // satin, but let the engine split branching shapes (letters)
                // into columns the way it does for artwork
                opts.stitch = StitchChoice::Auto;
                sum.satins += 1;
                label = format!("Satin {}", sum.satins);
            } else {
                opts.stitch = StitchChoice::Fill;
                opts.angle_deg = Some(main_angle(mine));
                sum.fills += 1;
                label = format!("Fill {}", sum.fills);
            }
            let mut shape = Shape {
                kind: ShapeKind::Area {
                    rings: Vec::new(),
                    rule: FillRule::EvenOdd,
                },
                rgb,
                label,
                group: 0,
                opts,
                curves,
            };
            shape.reflatten();
            pieces.push(Piece {
                first: mine.iter().map(|s| s.idx).min().unwrap_or(0),
                stop: si,
                shape,
            });
        }

        // ---- loose stitches → run lines (chains of stitches sewn one
        // after another)
        let mut chains: Vec<(usize, Vec<Pt>)> = Vec::new();
        for s in loose {
            match chains.last_mut() {
                Some((_, pts)) if pts.last().is_some_and(|p| p.dist(s.a) < 0.05) => pts.push(s.b),
                _ => chains.push((s.idx, vec![s.a, s.b])),
            }
        }
        for (first, pts) in chains {
            let sewn = poly_len(&pts);
            let path = collapse_backtrack(&pts);
            let len = poly_len(&path);
            if len < 2.0 || path.len() < 2 {
                continue;
            }
            let triple = sewn > len * 1.8;
            let mut opts = ShapeOpts::default();
            opts.stitch = if triple { StitchChoice::TripleRun } else { StitchChoice::Run };
            // the stitch length the file used
            let n = (path.len() - 1).max(1) as f64;
            opts.stitch_len = Some(((len / n) * 10.0).round().clamp(10.0, 70.0) / 10.0);
            sum.runs += 1;
            let mut shape = Shape {
                kind: ShapeKind::Line {
                    paths: Vec::new(),
                    width: 0.4,
                },
                rgb,
                label: format!("{} {}", if triple { "Triple run" } else { "Run" }, sum.runs),
                group: 0,
                opts,
                // runs keep their needle points as corners: a stitched
                // line is already the shape it should be
                curves: vec![VPath::from_polyline(&path, false, 0.08)],
            };
            shape.reflatten();
            pieces.push(Piece { first, stop: si, shape });
        }
    }

    // ---- sewing order, one layer each; keep the file's stops even when
    // two stops share a colour
    // run lines that a later area sews over (travel runs, hidden
    // underlay) aren't part of the look: leave them out
    let covered = |run: &Piece| -> bool {
        let ShapeKind::Line { paths, .. } = &run.shape.kind else {
            return false;
        };
        let pts: Vec<Pt> = paths.iter().flatten().copied().collect();
        if pts.is_empty() {
            return true;
        }
        let under = pts
            .iter()
            .filter(|&&q| {
                pieces.iter().any(|a| {
                    a.first > run.first
                        && match &a.shape.kind {
                            ShapeKind::Area { rings, .. } => {
                                rings.iter().filter(|r| crate::cut::point_in_poly(r, q)).count() % 2 == 1
                            }
                            _ => false,
                        }
                })
            })
            .count();
        under as f64 >= 0.8 * pts.len() as f64
    };
    let hidden: Vec<bool> = pieces.iter().map(|p| covered(p)).collect();
    let mut keep = hidden.iter().map(|h| !h);
    pieces.retain(|_| keep.next().unwrap_or(true));
    sum.runs = pieces
        .iter()
        .filter(|p| matches!(p.shape.kind, ShapeKind::Line { .. }))
        .count();

    // pieces sewn one after another in the same stop with the same stitch
    // (the letters of a word, say) share a layer
    pieces.sort_by_key(|p| p.first);
    let mut shapes: Vec<Shape> = Vec::with_capacity(pieces.len());
    let mut prev: Option<(usize, [u8; 3])> = None;
    let mut group = 0usize;
    for mut p in pieces {
        let same_layer = shapes.last().is_some_and(|q| {
            prev.is_some_and(|(ps, _)| ps == p.stop)
                && q.opts.stitch == p.shape.opts.stitch
                && match (q.opts.angle_deg, p.shape.opts.angle_deg) {
                    (Some(a), Some(b)) => {
                        let d = (a - b).rem_euclid(180.0);
                        d.min(180.0 - d) < 5.0
                    }
                    (None, None) => true,
                    _ => false,
                }
                && matches!(
                    (&q.kind, &p.shape.kind),
                    (ShapeKind::Area { .. }, ShapeKind::Area { .. })
                )
        });
        if same_layer {
            let q = shapes.last().unwrap();
            p.shape.label = q.label.clone();
            p.shape.opts = q.opts.clone();
        } else if !shapes.is_empty() {
            group += 1;
        }
        p.shape.group = group;
        if !same_layer
            && let Some((ps, prgb)) = prev
            && ps != p.stop
            && prgb == p.shape.rgb
        {
            p.shape.opts.stop = Some(true);
        }
        prev = Some((p.stop, p.shape.rgb));
        shapes.push(p.shape);
    }
    let mut warnings = Vec::new();
    if shapes.is_empty() {
        warnings.push("No stitched areas or lines were found in this file".into());
    }
    (
        Artwork {
            shapes,
            warnings,
            fixed: Default::default(),
        },
        sum,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::build;
    use crate::import::load_svg_str;
    use crate::settings::{Profile, recipe};

    /// A filled square, a satin bar and an open line, sewn and read back.
    #[test]
    fn stitches_come_back_as_their_shapes() {
        let svg = r#"<svg xmlns='http://www.w3.org/2000/svg' width='100mm' height='60mm' viewBox='0 0 100 60'>
            <rect x='5' y='5' width='30' height='30' fill='#c00000'/>
            <rect x='45' y='10' width='40' height='4' fill='#0050c0'/>
            <path d='M 45 40 L 90 40 L 90 55' stroke='#008000' stroke-width='0.3' fill='none'/>
        </svg>"#;
        let (fopt, _) = crate::import::font_options(None).unwrap();
        let mut art = load_svg_str(svg, &fopt).unwrap();
        art.shapes[0].opts.angle_deg = Some(30.0);
        art.shapes[2].opts.stitch = StitchChoice::Run;
        let d = build(
            "t",
            &art,
            &crate::thread::generic_chart(),
            &recipe("pique").unwrap(),
            Profile::Flat,
        )
        .unwrap();
        let colors: Vec<[u8; 3]> = d.stops.iter().map(|s| s.thread.rgb).collect();
        let (back, sum) = unstitch(&d.cmds, &colors);
        assert_eq!((sum.fills, sum.satins), (1, 1), "{sum:?}");
        assert!(sum.runs >= 1, "{sum:?}");

        let area = |s: &Shape| -> f64 {
            match &s.kind {
                ShapeKind::Area { rings, .. } => rings
                    .iter()
                    .map(|r| {
                        r.iter()
                            .zip(r.iter().cycle().skip(1))
                            .map(|(p, q)| p.x * q.y - q.x * p.y)
                            .sum::<f64>()
                            .abs()
                            / 2.0
                    })
                    .sum(),
                _ => 0.0,
            }
        };
        let fill = back.shapes.iter().find(|s| s.opts.stitch == StitchChoice::Fill).unwrap();
        // 30×30 square: the outline comes back within a fraction of a mm
        let a = area(fill);
        assert!((a - 900.0).abs() < 900.0 * 0.06, "fill area {a}");
        let ang = fill.opts.angle_deg.unwrap();
        assert!((ang - 30.0).abs() < 4.0, "fill angle {ang}");
        assert_eq!(fill.rgb, colors[0]);

        let satin = back.shapes.iter().find(|s| s.label.starts_with("Satin")).unwrap();
        let b = BBox::of(satin.points());
        assert!((b.width() - 40.0).abs() < 1.5 && (b.height() - 4.0).abs() < 1.0, "satin box {b:?}");

        let run = back
            .shapes
            .iter()
            .filter(|s| s.opts.stitch == StitchChoice::Run)
            .max_by(|a, b| {
                let l = |s: &Shape| s.points().count();
                l(a).cmp(&l(b))
            })
            .unwrap();
        let rb = BBox::of(run.points());
        assert!(rb.width() > 40.0 && rb.height() > 12.0, "run box {rb:?}");

        // and they sew again
        let again = build(
            "t2",
            &back,
            &crate::thread::generic_chart(),
            &recipe("pique").unwrap(),
            Profile::Flat,
        )
        .unwrap();
        let n = |cmds: &[Cmd]| cmds.iter().filter(|c| matches!(c, Cmd::Stitch(_))).count();
        assert!(n(&again.cmds) > n(&d.cmds) / 2);
    }

    #[test]
    #[ignore]
    fn debug_file() {
        let path = std::env::var("SF_FILE").unwrap();
        let f = crate::formats::read_file(std::path::Path::new(&path)).unwrap();
        let colors: Vec<[u8; 3]> = (0..f.stop_colors.len()).map(|i| [i as u8 * 30, 0, 0]).collect();
        let t = std::time::Instant::now();
        let (art, sum) = unstitch(&f.cmds, &colors);
        eprintln!("{sum:?} in {:?}", t.elapsed());
        for s in &art.shapes {
            let b = BBox::of(s.points());
            eprintln!("g{} {} {:?} {:?} box {:.1}x{:.1} at {:.1},{:.1} curves {}", s.group, s.label, s.opts.stitch, s.opts.angle_deg, b.width(), b.height(), b.min.x, b.min.y, s.curves.len());
        }
    }

    #[test]
    fn triple_run_folds_back_to_one_path() {
        let a = Pt::new(0.0, 0.0);
        let b = Pt::new(2.0, 0.0);
        let c = Pt::new(4.0, 0.0);
        let p = collapse_backtrack(&[a, b, a, b, c, b, c]);
        assert_eq!(p.len(), 3);
        assert!(p[2].dist(c) < 1e-9);
    }
}
