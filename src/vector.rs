//! Vector drawing: shapes made from scratch, path operations (union,
//! subtract, intersect, exclude), offsets for borders, outline-stroke, and
//! writing the art back out as SVG.
//!
//! Polygon booleans and offsets use the open-source i_overlay crate
//! (MIT/Apache). Curves are flattened for those operations; the results come
//! back as editable paths with an anchor wherever the outline turns.

use crate::geom::{FillRule, Node, Pt, Ring, VPath};
use i_overlay::core::fill_rule::FillRule as OFill;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};

/// Bézier handle length for a quarter circle.
const KAPPA: f64 = 0.552_284_749_8;

/// Rectangle with optional rounded corners (radius in mm).
pub fn rect(x: f64, y: f64, w: f64, h: f64, radius: f64) -> VPath {
    let (x0, x1) = (x.min(x + w), x.max(x + w));
    let (y0, y1) = (y.min(y + h), y.max(y + h));
    let r = radius.max(0.0).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    if r < 1e-6 {
        return VPath {
            nodes: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                .iter()
                .map(|&(a, b)| Node::corner(Pt::new(a, b)))
                .collect(),
            closed: true,
        };
    }
    let k = r * KAPPA;
    let n = |p: (f64, f64), cin: (f64, f64), cout: (f64, f64)| Node {
        p: Pt::new(p.0, p.1),
        cin: Pt::new(cin.0, cin.1),
        cout: Pt::new(cout.0, cout.1),
    };
    VPath {
        nodes: vec![
            n((x0 + r, y0), (x0 + r - k, y0), (x0 + r, y0)),
            n((x1 - r, y0), (x1 - r, y0), (x1 - r + k, y0)),
            n((x1, y0 + r), (x1, y0 + r - k), (x1, y0 + r)),
            n((x1, y1 - r), (x1, y1 - r), (x1, y1 - r + k)),
            n((x1 - r, y1), (x1 - r + k, y1), (x1 - r, y1)),
            n((x0 + r, y1), (x0 + r, y1), (x0 + r - k, y1)),
            n((x0, y1 - r), (x0, y1 - r + k), (x0, y1 - r)),
            n((x0, y0 + r), (x0, y0 + r), (x0, y0 + r - k)),
        ],
        closed: true,
    }
}

/// Ellipse from its centre and radii (four smooth anchors).
pub fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> VPath {
    let (rx, ry) = (rx.abs(), ry.abs());
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let n = |x: f64, y: f64, ix: f64, iy: f64, ox: f64, oy: f64| Node {
        p: Pt::new(x, y),
        cin: Pt::new(x + ix, y + iy),
        cout: Pt::new(x + ox, y + oy),
    };
    VPath {
        nodes: vec![
            n(cx + rx, cy, 0.0, -ky, 0.0, ky),
            n(cx, cy + ry, kx, 0.0, -kx, 0.0),
            n(cx - rx, cy, 0.0, ky, 0.0, -ky),
            n(cx, cy - ry, -kx, 0.0, kx, 0.0),
        ],
        closed: true,
    }
}

/// Regular polygon, or a star when `inner` (0..1, inner radius as a share of
/// the outer) is given. `rot_deg` turns it; 0 puts a point straight up.
pub fn polygon(cx: f64, cy: f64, r: f64, sides: usize, inner: Option<f64>, rot_deg: f64) -> VPath {
    let sides = sides.max(3);
    let count = if inner.is_some() { sides * 2 } else { sides };
    let nodes = (0..count)
        .map(|i| {
            let rr = match inner {
                Some(f) if i % 2 == 1 => r * f.clamp(0.05, 0.95),
                _ => r,
            };
            let a = (rot_deg - 90.0).to_radians() + std::f64::consts::TAU * i as f64 / count as f64;
            Node::corner(Pt::new(cx + rr * a.cos(), cy + rr * a.sin()))
        })
        .collect();
    VPath { nodes, closed: true }
}

/// Straight open line.
pub fn line(a: Pt, b: Pt) -> VPath {
    VPath {
        nodes: vec![Node::corner(a), Node::corner(b)],
        closed: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    /// First minus the rest.
    Subtract,
    Intersect,
    /// Where exactly one of them covers.
    Exclude,
}

impl BoolOp {
    pub fn label(&self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Subtract => "Subtract",
            BoolOp::Intersect => "Intersect",
            BoolOp::Exclude => "Exclude",
        }
    }
}

type Contours = Vec<Vec<[f64; 2]>>;

fn to_contours(rings: &[Ring]) -> Contours {
    rings
        .iter()
        .filter(|r| r.len() >= 3)
        .map(|r| r.iter().map(|p| [p.x, p.y]).collect())
        .collect()
}

/// Shapes from i_overlay (outer contour, then holes) as rings for even-odd.
fn from_shapes(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Vec<Ring> {
    shapes
        .into_iter()
        .flatten()
        .filter(|c| c.len() >= 3)
        .map(|c| c.into_iter().map(|p| Pt::new(p[0], p[1])).collect())
        .collect()
}

fn ofill(rule: FillRule) -> OFill {
    match rule {
        FillRule::NonZero => OFill::NonZero,
        FillRule::EvenOdd => OFill::EvenOdd,
    }
}

/// Merge one shape's own rings into clean outer outlines and holes.
pub fn normalize(rings: &[Ring], rule: FillRule) -> Vec<Ring> {
    let c = to_contours(rings);
    let empty: Contours = Vec::new();
    from_shapes(c.overlay(&empty, OverlayRule::Subject, ofill(rule)))
}

/// Boolean of several areas, in order (subtract = the first minus the rest).
/// Each input is (rings, fill rule). The result uses the even-odd rule.
pub fn boolean(inputs: &[(Vec<Ring>, FillRule)], op: BoolOp) -> Vec<Ring> {
    let mut iter = inputs.iter();
    let Some((r0, f0)) = iter.next() else {
        return Vec::new();
    };
    let mut acc = normalize(r0, *f0);
    for (r, f) in iter {
        let b = to_contours(&normalize(r, *f));
        let rule = match op {
            BoolOp::Union => OverlayRule::Union,
            BoolOp::Subtract => OverlayRule::Difference,
            BoolOp::Intersect => OverlayRule::Intersect,
            BoolOp::Exclude => OverlayRule::Xor,
        };
        acc = from_shapes(to_contours(&acc).overlay(&b, rule, OFill::EvenOdd));
    }
    acc
}

/// Grow (+) or shrink (−) an area by `d` mm, with rounded or sharp corners.
pub fn offset(rings: &[Ring], rule: FillRule, d: f64, round: bool) -> Vec<Ring> {
    if d.abs() < 1e-6 {
        return normalize(rings, rule);
    }
    let shapes: Vec<Contours> = vec![to_contours(&normalize(rings, rule))];
    let join = if round {
        LineJoin::Round(0.12)
    } else {
        LineJoin::Miter(0.3)
    };
    let style = OutlineStyle::new(d).line_join(join);
    from_shapes(shapes.outline(&style))
}

/// The area a line of `width` mm covers (outline stroke).
pub fn stroke_area(paths: &[Vec<Pt>], width: f64, round: bool) -> Vec<Ring> {
    let mut all: Vec<Ring> = Vec::new();
    for p in paths {
        if p.len() < 2 {
            continue;
        }
        let closed = p.len() > 2 && p[0].dist(*p.last().unwrap()) < 1e-6;
        let pts: Vec<[f64; 2]> = p
            .iter()
            .take(if closed { p.len() - 1 } else { p.len() })
            .map(|q| [q.x, q.y])
            .collect();
        let style = if round {
            StrokeStyle::new(width)
                .line_join(LineJoin::Round(0.12))
                .start_cap(LineCap::Round(0.12))
                .end_cap(LineCap::Round(0.12))
        } else {
            StrokeStyle::new(width).line_join(LineJoin::Miter(0.3))
        };
        all.extend(from_shapes(pts.stroke(style, closed)));
    }
    normalize(&all, FillRule::NonZero)
}

/// Editable paths from rings: an anchor wherever the outline really turns.
pub fn rings_to_curves(rings: &[Ring]) -> Vec<VPath> {
    rings
        .iter()
        .map(|r| crate::geom::fit_path(r, true, 0.04))
        .filter(|v| v.nodes.len() >= 3)
        .collect()
}

/// SVG path data for a vector path, with every point passed through `tf`.
pub fn path_d(v: &VPath, tf: &dyn Fn(Pt) -> Pt) -> String {
    use std::fmt::Write;
    let mut d = String::new();
    let Some(first) = v.nodes.first() else {
        return d;
    };
    let f = |p: Pt| {
        let q = tf(p);
        format!("{:.3},{:.3}", q.x, q.y)
    };
    let _ = write!(d, "M{}", f(first.p));
    let n = v.nodes.len();
    let segs = if v.closed { n } else { n.saturating_sub(1) };
    for s in 0..segs {
        let (a, b) = (v.nodes[s], v.nodes[(s + 1) % n]);
        if a.cout.dist(a.p) < 1e-9 && b.cin.dist(b.p) < 1e-9 {
            let _ = write!(d, " L{}", f(b.p));
        } else {
            let _ = write!(d, " C{} {} {}", f(a.cout), f(b.cin), f(b.p));
        }
    }
    if v.closed {
        d.push_str(" Z");
    }
    d
}

/// Separate the bands of a shape (rings, borders, thin lines: anything no
/// wider than `max_band` mm) from its solid parts (a silhouette, a disc),
/// even where they touch and are the same colour. Bands suit satin, solids
/// suit fill.
///
/// Works by "opening": shrink the shape by half the band width and grow it
/// back. Bands vanish when shrunk; solid parts come back. Bands are what the
/// opening lost. Scraps too small to be a real band (corners the opening
/// rounds off) stay with the solid part next to them.
///
/// Returns (bands, solids), both even-odd rings.
pub fn split_bands(rings: &[Ring], rule: FillRule, max_band: f64) -> (Vec<Ring>, Vec<Ring>) {
    let whole = normalize(rings, rule);
    if whole.is_empty() || max_band <= 0.0 {
        return (Vec::new(), whole);
    }
    let r = max_band / 2.0 + 0.05;
    let shrunk = offset(&whole, FillRule::EvenOdd, -r, true);
    if shrunk.is_empty() {
        return (whole, Vec::new()); // all of it is band
    }
    let opened = offset(&shrunk, FillRule::EvenOdd, r, true);
    let solids = boolean(&[(opened, FillRule::EvenOdd), (whole.clone(), FillRule::EvenOdd)], BoolOp::Intersect);
    let rest = boolean(&[(whole.clone(), FillRule::EvenOdd), (solids.clone(), FillRule::EvenOdd)], BoolOp::Subtract);
    // keep only pieces of `rest` long enough to be bands; scraps go back to
    // the solids
    let area = |rs: &[Ring]| -> f64 {
        rs.iter()
            .map(|r| {
                (0..r.len())
                    .map(|i| {
                        let (p, q) = (r[i], r[(i + 1) % r.len()]);
                        p.x * q.y - q.x * p.y
                    })
                    .sum::<f64>()
                    / 2.0
            })
            .sum::<f64>()
            .abs()
    };
    let mut bands: Vec<Ring> = Vec::new();
    let mut scraps: Vec<Ring> = Vec::new();
    for piece in crate::geom::split_islands(&rest, FillRule::EvenOdd) {
        let a = area(&piece);
        let b = crate::geom::BBox::of(piece.iter().flatten());
        let span = b.width().max(b.height());
        // a band is long for its width: at least ~3 widths long
        if a > 0.05 && span >= 3.0 * (a / span.max(1e-6)).max(0.2) && span > max_band {
            bands.extend(piece);
        } else {
            scraps.extend(piece);
        }
    }
    let solids = if scraps.is_empty() {
        solids
    } else {
        boolean(&[(solids, FillRule::EvenOdd), (scraps, FillRule::EvenOdd)], BoolOp::Union)
    };
    (bands, solids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{rings_area, spans_at};

    fn area(rings: &[Ring]) -> f64 {
        rings_area(rings, FillRule::EvenOdd)
    }

    fn sq(x: f64, y: f64, s: f64) -> Ring {
        rect(x, y, s, s, 0.0).flatten()[..4].to_vec()
    }

    #[test]
    fn shapes_have_the_right_size() {
        let c = ellipse(0.0, 0.0, 10.0, 10.0).flatten();
        let a = area(&[c]);
        assert!((a - std::f64::consts::PI * 100.0).abs() < 1.5, "{a}");
        let r = rect(0.0, 0.0, 20.0, 10.0, 3.0).flatten();
        let expect = 200.0 - (4.0 - std::f64::consts::PI) * 9.0;
        assert!((area(&[r]) - expect).abs() < 0.5);
        let star = polygon(0.0, 0.0, 10.0, 5, Some(0.5), 0.0);
        assert_eq!(star.nodes.len(), 10);
    }

    #[test]
    fn booleans() {
        let a = (vec![sq(0.0, 0.0, 10.0)], FillRule::NonZero);
        let b = (vec![sq(5.0, 0.0, 10.0)], FillRule::NonZero);
        let u = boolean(&[a.clone(), b.clone()], BoolOp::Union);
        assert!((area(&u) - 150.0).abs() < 0.01);
        let s = boolean(&[a.clone(), b.clone()], BoolOp::Subtract);
        assert!((area(&s) - 50.0).abs() < 0.01);
        let i = boolean(&[a.clone(), b.clone()], BoolOp::Intersect);
        assert!((area(&i) - 50.0).abs() < 0.01);
        let x = boolean(&[a, b], BoolOp::Exclude);
        assert!((area(&x) - 100.0).abs() < 0.01);
    }

    #[test]
    fn offset_and_stroke() {
        let a = vec![sq(0.0, 0.0, 10.0)];
        let grown = offset(&a, FillRule::NonZero, 1.0, false);
        assert!((area(&grown) - 144.0).abs() < 0.5, "{}", area(&grown));
        let shrunk = offset(&a, FillRule::NonZero, -1.0, false);
        assert!((area(&shrunk) - 64.0).abs() < 0.5);
        // a 20 mm line, 2 mm wide, square ends
        let s = stroke_area(&[vec![Pt::new(0.0, 0.0), Pt::new(20.0, 0.0)]], 2.0, false);
        assert!((area(&s) - 40.0).abs() < 0.5, "{}", area(&s));
        assert!(!spans_at(&s, FillRule::EvenOdd, 0.5).is_empty());
    }

    #[test]
    fn svg_path_round_trip() {
        let e = ellipse(10.0, 10.0, 5.0, 3.0);
        let d = path_d(&e, &|p| p);
        assert!(d.starts_with("M15.000,10.000 C"));
        assert!(d.ends_with('Z'));
    }

    #[test]
    fn split_bands_separates_a_ring_from_a_disc_it_touches() {
        use crate::geom::Pt;
        // a ring 2 mm wide (radius 20 to 22) and a solid disc of radius 6
        // sitting on it at the top: one connected shape
        let circle = |cx: f64, cy: f64, r: f64, ccw: bool| -> Ring {
            let n = 180;
            let mut v: Vec<Pt> = (0..n)
                .map(|i| {
                    let t = i as f64 / n as f64 * std::f64::consts::TAU;
                    Pt::new(cx + r * t.cos(), cy + r * t.sin())
                })
                .collect();
            if !ccw {
                v.reverse();
            }
            v
        };
        let ring = vec![circle(0.0, 0.0, 22.0, true), circle(0.0, 0.0, 20.0, false)];
        let disc = vec![circle(0.0, -21.0, 6.0, true)];
        let shape = boolean(&[(ring, FillRule::EvenOdd), (disc, FillRule::EvenOdd)], BoolOp::Union);
        assert_eq!(crate::geom::split_islands(&shape, FillRule::EvenOdd).len(), 1, "one connected shape");
        let (bands, solids) = split_bands(&shape, FillRule::EvenOdd, 4.0);
        let area = |rs: &[Ring]| rs.iter().map(|r| (0..r.len()).map(|i| { let (p, q) = (r[i], r[(i + 1) % r.len()]); p.x * q.y - q.x * p.y }).sum::<f64>() / 2.0).sum::<f64>().abs();
        let disc_area = std::f64::consts::PI * 36.0;
        let ring_area = std::f64::consts::PI * (22.0f64.powi(2) - 20.0f64.powi(2));
        // the solid part is about the disc; the band is about the ring
        assert!((area(&solids) - disc_area).abs() < disc_area * 0.25, "solids {} vs disc {}", area(&solids), disc_area);
        assert!(area(&bands) > ring_area * 0.75, "bands {} vs ring {}", area(&bands), ring_area);
    }
}
