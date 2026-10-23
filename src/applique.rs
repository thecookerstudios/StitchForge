//! Appliqué and tackle twill: a fabric piece (twill, felt…) sewn on instead
//! of filling the shape with thread.
//!
//! The usual shop sequence, which StitchForge follows:
//!
//! 1. **Placement line** – a run on the garment where the piece goes.
//!    Machine stops.
//! 2. **Tack-down** – the piece is laid on the line and held with a zigzag,
//!    E-stitch or double run a little inside its edge.
//!    *Cut in place* only: the machine stops again so the operator trims the
//!    fabric close to the tack-down.
//! 3. **Cover** – the raw edge is sealed: satin border, E-stitch (blanket),
//!    open zigzag, or a run/bean line for pre-finished pieces.
//!
//! Pre-cut pieces (laser, plotter or die cut) come from the cut files written
//! with the export: one file per material colour, in SVG, DXF and HPGL
//! (.plt), at real size, optionally mirrored for cutting face-down and with a
//! kerf allowance for lasers.

use crate::geom::{Pt, Ring};
use serde::{Deserialize, Serialize};

/// How the piece gets its shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Cut beforehand (laser, plotter, die) from the cut file.
    #[default]
    PreCut,
    /// A larger piece is tacked down, then trimmed at the machine.
    CutInPlace,
}

impl Method {
    pub fn label(&self) -> &'static str {
        match self {
            Method::PreCut => "Pre-cut pieces",
            Method::CutInPlace => "Cut in place",
        }
    }
}

/// Stitch that holds the piece before the cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tack {
    #[default]
    Zigzag,
    EStitch,
    DoubleRun,
    None,
}

impl Tack {
    pub const ALL: [Tack; 4] = [Tack::Zigzag, Tack::EStitch, Tack::DoubleRun, Tack::None];
    pub fn label(&self) -> &'static str {
        match self {
            Tack::Zigzag => "Zigzag",
            Tack::EStitch => "E-stitch",
            Tack::DoubleRun => "Double run",
            Tack::None => "None",
        }
    }
}

/// Stitch that seals the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cover {
    #[default]
    Satin,
    EStitch,
    Zigzag,
    DoubleRun,
    Bean,
    None,
}

impl Cover {
    pub const ALL: [Cover; 6] = [Cover::Satin, Cover::EStitch, Cover::Zigzag, Cover::Bean, Cover::DoubleRun, Cover::None];
    pub fn label(&self) -> &'static str {
        match self {
            Cover::Satin => "Satin border",
            Cover::EStitch => "E-stitch (blanket)",
            Cover::Zigzag => "Open zigzag",
            Cover::DoubleRun => "Double run",
            Cover::Bean => "Bean (triple run)",
            Cover::None => "None",
        }
    }
    pub fn hint(&self) -> &'static str {
        match self {
            Cover::Satin => "Solid border that hides the cut edge; the classic tackle twill look",
            Cover::EStitch => "Blanket stitch: a line along the edge with prongs onto the piece; athletic and vintage looks",
            Cover::Zigzag => "Open zigzag over the edge; lets some of the twill show",
            Cover::DoubleRun => "Run inside the edge for laser-sealed or pre-finished pieces",
            Cover::Bean => "Heavier run (three passes) inside the edge",
            Cover::None => "Tack-down only",
        }
    }
    /// Typical settings for this cover: (width or prong length, outer edge
    /// position relative to the piece edge, spacing), mm.
    pub fn defaults(&self) -> (f64, f64, f64) {
        match self {
            Cover::Satin => (3.0, 0.8, 0.4),
            Cover::EStitch => (2.5, 0.3, 2.5),
            Cover::Zigzag => (3.5, 1.0, 1.2),
            Cover::DoubleRun | Cover::Bean => (0.0, -1.2, 2.5),
            Cover::None => (0.0, 0.0, 0.0),
        }
    }
}

/// A layer sewn as appliqué. Positions are relative to the piece's edge
/// (the artwork outline): + is outside the piece, − on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Applique {
    pub method: Method,
    /// Material name for the sheet ("Twill", "Felt", "Mesh"…).
    pub material: String,
    /// Material colour (drawn in previews, names the cut file).
    pub material_rgb: [u8; 3],
    /// Sew a placement line first (and stop).
    pub placement: bool,
    pub tack: Tack,
    /// Tack-down zigzag / E-stitch width (mm).
    pub tack_width: f64,
    /// Centre of the tack-down, inside the edge (mm).
    pub tack_inset: f64,
    pub cover: Cover,
    /// Satin / zigzag width, E-stitch prong length (mm).
    pub cover_width: f64,
    /// Where the cover's outer edge sits: + mm outside the piece edge.
    /// (Runs: the line itself; − = on the piece.)
    pub cover_outside: f64,
    /// Zigzag / E-stitch spacing along the edge, satin density (mm).
    pub cover_spacing: f64,
    /// Cut line relative to the edge (+ larger piece, − smaller), mm.
    pub cut_offset: f64,
}

impl Default for Applique {
    fn default() -> Self {
        let (w, o, s) = Cover::Satin.defaults();
        Applique {
            method: Method::PreCut,
            material: "Twill".into(),
            material_rgb: [200, 30, 40],
            placement: true,
            tack: Tack::Zigzag,
            tack_width: 1.2,
            tack_inset: 1.0,
            cover: Cover::Satin,
            cover_width: w,
            cover_outside: o,
            cover_spacing: s,
            cut_offset: 0.0,
        }
    }
}

impl Applique {
    /// Switch cover stitch, taking that stitch's usual sizes.
    pub fn set_cover(&mut self, c: Cover) {
        self.cover = c;
        let (w, o, s) = c.defaults();
        self.cover_width = w;
        self.cover_outside = o;
        self.cover_spacing = s;
    }

    /// A one-line description for sheets and lists.
    pub fn summary(&self) -> String {
        format!(
            "{} {} · {} · tack-down {} · {}",
            self.material,
            crate::thread::color_name(self.material_rgb),
            self.method.label().to_lowercase(),
            self.tack.label().to_lowercase(),
            self.cover.label().to_lowercase()
        )
    }
}

/// One fabric piece in a finished design (design mm, y down).
#[derive(Clone, Debug, Serialize)]
pub struct Piece {
    pub layer: usize,
    pub label: String,
    pub material: String,
    pub rgb: [u8; 3],
    /// The piece's outline as it lies on the garment.
    pub rings: Vec<Ring>,
    /// The cut line (outline moved by the cut offset).
    pub cut: Vec<Ring>,
    /// Index into `Design::cmds` from which the piece is on the garment
    /// (the start of its tack-down), for drawing during playback.
    #[serde(skip)]
    pub from_cmd: usize,
}

/// One piece as a small SVG picture with a twill weave look, for previews:
/// (svg, top-left in design mm, size in mm).
pub fn piece_svg(p: &Piece) -> (String, Pt, Pt) {
    let mut b = crate::geom::BBox::empty();
    for q in p.rings.iter().flatten() {
        b.add(*q);
    }
    let pad = 0.5;
    let (ox, oy) = (b.min.x - pad, b.min.y - pad);
    let (w, h) = (b.width() + 2.0 * pad, b.height() + 2.0 * pad);
    let d: String = p
        .rings
        .iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, q)| format!("{}{:.3} {:.3}", if i == 0 { 'M' } else { 'L' }, q.x - ox, q.y - oy))
                .collect::<String>()
                + "Z"
        })
        .collect();
    let shade = |k: f64| {
        let f = |c: u8| ((c as f64) * k).clamp(0.0, 255.0) as u8;
        format!("#{:02x}{:02x}{:02x}", f(p.rgb[0]), f(p.rgb[1]), f(p.rgb[2]))
    };
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w:.3}' height='{h:.3}' viewBox='0 0 {w:.3} {h:.3}'>\
         <defs><pattern id='tw' width='0.9' height='0.9' patternUnits='userSpaceOnUse' patternTransform='rotate(45)'>\
         <rect width='0.9' height='0.9' fill='{}'/><rect width='0.45' height='0.9' fill='{}'/></pattern></defs>\
         <path d='{d}' fill='url(#tw)' fill-rule='evenodd' stroke='{}' stroke-width='0.12'/></svg>",
        shade(1.0),
        shade(0.88),
        shade(0.7)
    );
    (svg, Pt::new(ox, oy), Pt::new(w, h))
}

// ------------------------------------------------------------------ cut files

/// Options for writing cut files.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutFiles {
    pub svg: bool,
    pub dxf: bool,
    pub plt: bool,
    /// Mirror left-right (cutting with the adhesive side up).
    pub mirror: bool,
    /// Extra outward allowance for the laser beam or blade (mm).
    pub kerf: f64,
}

impl Default for CutFiles {
    fn default() -> Self {
        CutFiles {
            svg: true,
            dxf: true,
            plt: true,
            mirror: false,
            kerf: 0.0,
        }
    }
}

/// Cut paths for these pieces: closed rings in mm, y up, origin at the
/// lower-left of the job, ready for a cutter.
pub fn cut_paths(pieces: &[&Piece], o: &CutFiles) -> Vec<Ring> {
    let mut rings: Vec<Ring> = Vec::new();
    for p in pieces {
        let rs = if o.kerf != 0.0 {
            crate::vector::offset(&p.cut, crate::geom::FillRule::EvenOdd, o.kerf, false)
        } else {
            p.cut.clone()
        };
        rings.extend(rs.into_iter().filter(|r| r.len() >= 3));
    }
    let mut b = crate::geom::BBox::empty();
    for q in rings.iter().flatten() {
        b.add(*q);
    }
    if !b.is_valid() {
        return rings;
    }
    // y up for cutters, a 5 mm margin, mirrored if asked
    rings
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|q| {
                    let x = if o.mirror { b.max.x - q.x } else { q.x - b.min.x };
                    Pt::new(x + 5.0, b.max.y - q.y + 5.0)
                })
                .collect()
        })
        .collect()
}

fn extent(rings: &[Ring]) -> (f64, f64) {
    let mut w: f64 = 0.0;
    let mut h: f64 = 0.0;
    for q in rings.iter().flatten() {
        w = w.max(q.x);
        h = h.max(q.y);
    }
    (w + 5.0, h + 5.0)
}

/// SVG at real size (mm), hairline red cut paths (the usual laser "cut" colour).
pub fn cut_svg(rings: &[Ring], title: &str) -> String {
    let (w, h) = extent(rings);
    let mut s = format!(
        "<?xml version='1.0' encoding='UTF-8'?>\n<svg xmlns='http://www.w3.org/2000/svg' width='{w:.3}mm' height='{h:.3}mm' viewBox='0 0 {w:.3} {h:.3}'>\n<title>{}</title>\n",
        title.replace('<', "").replace('&', "and")
    );
    for r in rings {
        let d: String = r
            .iter()
            .enumerate()
            // SVG y runs down
            .map(|(i, q)| format!("{}{:.3} {:.3}", if i == 0 { "M" } else { "L" }, q.x, h - q.y))
            .collect::<Vec<_>>()
            .join(" ");
        s.push_str(&format!(
            "<path d='{d} Z' fill='none' stroke='#ff0000' stroke-width='0.01'/>\n"
        ));
    }
    s.push_str("</svg>\n");
    s
}

/// DXF (R12, millimetres): one closed POLYLINE per path on layer CUT.
pub fn cut_dxf(rings: &[Ring]) -> String {
    let mut s = String::new();
    let mut g = |code: i32, v: &str| {
        s.push_str(&format!("{code}\n{v}\n"));
    };
    let (w, h) = extent(rings);
    g(0, "SECTION");
    g(2, "HEADER");
    g(9, "$ACADVER");
    g(1, "AC1009");
    g(9, "$INSUNITS");
    g(70, "4"); // millimetres
    g(9, "$EXTMIN");
    g(10, "0.0");
    g(20, "0.0");
    g(9, "$EXTMAX");
    g(10, &format!("{w:.3}"));
    g(20, &format!("{h:.3}"));
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "TABLES");
    g(0, "TABLE");
    g(2, "LAYER");
    g(70, "1");
    g(0, "LAYER");
    g(2, "CUT");
    g(70, "0");
    g(62, "1"); // red
    g(6, "CONTINUOUS");
    g(0, "ENDTAB");
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "ENTITIES");
    for r in rings {
        g(0, "POLYLINE");
        g(8, "CUT");
        g(66, "1");
        g(70, "1"); // closed
        for q in r {
            g(0, "VERTEX");
            g(8, "CUT");
            g(10, &format!("{:.4}", q.x));
            g(20, &format!("{:.4}", q.y));
        }
        g(0, "SEQEND");
        g(8, "CUT");
    }
    g(0, "ENDSEC");
    g(0, "EOF");
    s
}

/// HPGL for plotter/cutters (.plt): 40 plotter units per mm.
pub fn cut_plt(rings: &[Ring]) -> String {
    let u = |v: f64| (v * 40.0).round() as i64;
    let mut s = String::from("IN;SP1;");
    for r in rings {
        let Some(first) = r.first() else { continue };
        s.push_str(&format!("PU{},{};", u(first.x), u(first.y)));
        let pts: Vec<String> = r
            .iter()
            .skip(1)
            .chain(std::iter::once(first))
            .map(|q| format!("{},{}", u(q.x), u(q.y)))
            .collect();
        s.push_str(&format!("PD{};", pts.join(",")));
    }
    s.push_str("PU;SP0;\n");
    s
}

/// File-name part for a material colour ("red-twill").
pub fn file_tag(material: &str, rgb: [u8; 3]) -> String {
    let name = format!("{}-{}", crate::thread::color_name(rgb), material);
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Piece {
        let r = vec![
            Pt::new(10.0, 10.0),
            Pt::new(40.0, 10.0),
            Pt::new(40.0, 30.0),
            Pt::new(10.0, 30.0),
        ];
        Piece {
            layer: 0,
            label: "A".into(),
            material: "Twill".into(),
            rgb: [200, 30, 40],
            rings: vec![r.clone()],
            cut: vec![r],
            from_cmd: 0,
        }
    }

    #[test]
    fn cut_paths_are_real_size_and_mirror() {
        let p = square();
        let plain = cut_paths(&[&p], &CutFiles::default());
        let xs: Vec<f64> = plain[0].iter().map(|q| q.x).collect();
        let w = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!((w - 30.0).abs() < 1e-9);
        // mirrored: the left edge of the art is now on the right
        let m = cut_paths(&[&p], &CutFiles { mirror: true, ..Default::default() });
        assert!((m[0][0].x - (30.0 + 5.0)).abs() < 1e-9, "{:?}", m[0][0]);
        // kerf makes the piece bigger
        let k = cut_paths(&[&p], &CutFiles { kerf: 0.5, ..Default::default() });
        let kw = k[0].iter().map(|q| q.x).fold(f64::MIN, f64::max) - k[0].iter().map(|q| q.x).fold(f64::MAX, f64::min);
        assert!((kw - 31.0).abs() < 0.05, "kerf width {kw}");
    }

    #[test]
    fn plt_and_dxf_are_well_formed() {
        let p = square();
        let rings = cut_paths(&[&p], &CutFiles::default());
        let plt = cut_plt(&rings);
        assert!(plt.starts_with("IN;SP1;PU200,"));
        // 30 mm = 1200 plotter units across
        assert!(plt.contains("1400,"), "{plt}");
        let dxf = cut_dxf(&rings);
        assert_eq!(dxf.matches("VERTEX").count(), 4);
        assert!(dxf.trim_end().ends_with("EOF"));
        let svg = cut_svg(&rings, "t");
        assert!(svg.contains("width='40.000mm'"), "{svg}");
    }

    fn twill_design(a: Applique) -> crate::design::Design {
        // a 40 × 30 mm block with a 10 mm square hole (like a letter's counter)
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='60mm' height='50mm' viewBox='0 0 60 50'>\
            <path d='M10 10 H50 V40 H10 Z M25 20 H35 V30 H25 Z' fill='#c00000' fill-rule='evenodd'/></svg>";
        let (fopt, _) = crate::import::font_options(None).unwrap();
        let mut art = crate::import::load_svg_str(svg, &fopt).unwrap();
        art.shapes[0].opts.applique = Some(a);
        crate::design::build(
            "twill",
            &art,
            &crate::thread::generic_chart(),
            &crate::settings::recipe("pique").unwrap(),
            crate::settings::Profile::Flat,
        )
        .unwrap()
    }

    fn stitches_of(d: &crate::design::Design, label: &str) -> Vec<Pt> {
        let mut v = Vec::new();
        for o in d.objects.iter().filter(|o| o.stitch_type == label) {
            for c in &d.cmds[o.cmd_range.0..o.cmd_range.1] {
                if let crate::design::Cmd::Stitch(p) = c {
                    v.push(*p);
                }
            }
        }
        v
    }

    /// Signed distance from the block's outline: + outside the twill.
    fn edge_dist(p: Pt) -> f64 {
        let outer = (p.x - 30.0).abs().max(0.0) - 20.0;
        let outer = outer.max((p.y - 25.0).abs() - 15.0);
        let hole = 5.0 - (p.x - 30.0).abs().max((p.y - 25.0).abs());
        // outside the block, or inside the hole, is off the twill
        if outer > 0.0 { outer } else if hole > 0.0 { hole } else { outer.max(hole) }
    }

    #[test]
    fn twill_sews_placement_stop_tackdown_cover() {
        let d = twill_design(Applique::default());
        // one stop to start, one to lay the piece
        assert_eq!(d.stops.len(), 2, "{:?}", d.stops.iter().map(|s| &s.note).collect::<Vec<_>>());
        assert!(d.stops[1].note.contains("placement line"), "{}", d.stops[1].note);
        assert_eq!(d.pieces.len(), 1);
        assert_eq!(d.pieces[0].rings.len(), 2, "outline and hole");
        let order: Vec<&str> = d.objects.iter().map(|o| o.stitch_type).collect();
        assert_eq!(order, ["Appliqué placement", "Tack-down (zigzag)", "Appliqué satin border"]);
        // placement on the edge, tack-down on the twill inside it, satin over the edge
        for p in stitches_of(&d, "Appliqué placement") {
            assert!(edge_dist(p).abs() < 0.15, "placement at {p:?}: {}", edge_dist(p));
        }
        for p in stitches_of(&d, "Tack-down (zigzag)") {
            let e = edge_dist(p);
            assert!((-1.75..=-0.25).contains(&e), "tack at {p:?}: {e}");
        }
        let sat = stitches_of(&d, "Appliqué satin border");
        let (lo, hi) = sat.iter().map(|p| edge_dist(*p)).fold((f64::MAX, f64::MIN), |(a, b), e| (a.min(e), b.max(e)));
        assert!(hi > 0.6 && hi < 1.3, "satin reaches {hi} outside");
        assert!(lo < -1.9 && lo > -2.8, "satin reaches {lo} onto the twill");
    }

    #[test]
    fn cut_in_place_stops_to_trim() {
        let d = twill_design(Applique { method: Method::CutInPlace, ..Default::default() });
        assert_eq!(d.stops.len(), 3);
        assert!(d.stops[2].note.contains("trim"), "{}", d.stops[2].note);
    }

    #[test]
    fn every_cover_and_tack_sews() {
        for c in Cover::ALL {
            for t in Tack::ALL {
                let mut a = Applique { tack: t, ..Default::default() };
                a.set_cover(c);
                let d = twill_design(a);
                let n = d.cmds.iter().filter(|c| matches!(c, crate::design::Cmd::Stitch(_))).count();
                assert!(n > 60, "{c:?}/{t:?}: {n} stitches");
                // nothing strays far from the edge
                for cmd in &d.cmds {
                    if let crate::design::Cmd::Stitch(p) = cmd {
                        assert!(edge_dist(*p).abs() < 4.0, "{c:?}/{t:?}: stitch at {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn estitch_prongs_point_onto_the_piece() {
        let mut a = Applique::default();
        a.set_cover(Cover::EStitch);
        let d = twill_design(a);
        let e: Vec<f64> = stitches_of(&d, "Appliqué E-stitch").iter().map(|p| edge_dist(*p)).collect();
        let base = e.iter().filter(|v| (**v - 0.3).abs() < 0.2).count();
        let prong = e.iter().filter(|v| (**v + 2.2).abs() < 0.3).count();
        assert!(base > 20 && prong > 20, "base {base}, prongs {prong}");
    }

    #[test]
    fn file_tags_are_tidy() {
        let t = file_tag("Tackle Twill", [200, 30, 40]);
        assert!(t.ends_with("tackle-twill"), "{t}");
        assert!(!t.contains("--"));
    }
}
