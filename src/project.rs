//! Project model: layers built from artwork or text, each with its own
//! transform and stitch choices. Saved as a `.sfg` file (JSON) so the app,
//! the command line and AI agents all work from the same design.

use crate::design::{BuildOptions, Design, build_with};
use crate::geom::{BBox, Pt, spans_at};
use crate::import::{Artwork, Shape, ShapeKind, ShapeOpts};
use crate::settings::{Profile, Recipe};
use crate::stitch::Needle;
use crate::sheet::JobInfo;
use crate::thread::{self, Thread};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextSpec {
    pub text: String,
    pub font: String,
    pub height_mm: f64,
    pub rgb: [u8; 3],
}

/// The embroidery side of a design object: its needle points, kept in the
/// project next to the vector shape. Generated from the shape and settings
/// until someone edits stitches by hand; after that they are sewn as they
/// are, and a later change to the shape is flagged instead of silently
/// throwing the hand edits away.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Embroidery {
    /// Needle points in the layer's own frame (mm, before the layer's
    /// move, resize and rotate, so those apply to stitches and shape alike).
    pub needles: Vec<Needle>,
    /// Stitches were edited by hand: sew these, don't regenerate.
    pub edited: bool,
    /// Fingerprint of the shape and stitch settings the stitches match.
    pub source: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    /// Geometry as imported (mm). The layer transform is applied on top.
    pub shapes: Vec<Shape>,
    /// Pivot for scale and rotation (centre of the original geometry).
    pub origin: Pt,
    pub offset: Pt,
    /// Horizontal and vertical scale in the layer's own frame.
    pub sx: f64,
    pub sy: f64,
    /// Keep width and height in proportion when one is typed.
    pub lock: bool,
    /// A thread the user typed in themselves (code, name, color).
    pub custom: Option<thread::Thread>,
    /// Degrees, counter-clockwise on screen.
    pub rotation: f64,
    pub opts: ShapeOpts,
    pub visible: bool,
    pub text: Option<TextSpec>,
    pub is_outline: bool,
    /// Layers sharing a link move, rotate and resize together (e.g. a shape's
    /// fill and its outline).
    pub link: Option<u64>,
    #[serde(default)]
    pub embroidery: Embroidery,
}

impl Layer {
    pub fn new(id: u64, name: String, shapes: Vec<Shape>, is_outline: bool) -> Self {
        let b = BBox::of(shapes.iter().flat_map(|s| s.points()));
        Layer {
            id,
            name,
            origin: b.center(),
            shapes,
            offset: Pt::new(0.0, 0.0),
            sx: 1.0,
            sy: 1.0,
            lock: true,
            custom: None,
            rotation: 0.0,
            opts: ShapeOpts::default(),
            visible: true,
            text: None,
            is_outline,
            link: None,
            embroidery: Embroidery::default(),
        }
    }

    /// Fingerprint of everything the stitches are generated from: the
    /// vector shape and this layer's stitch settings (not its position,
    /// size, thread or stop, which don't change the stitches themselves).
    pub fn source_fingerprint(&self) -> u64 {
        let mut o = self.opts.clone();
        o.thread = None;
        o.stop = None;
        o.layer = 0;
        let mut h: u64 = 0xcbf29ce484222325;
        let mut feed = |b: &[u8]| {
            for x in b {
                h ^= *x as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        };
        for sh in &self.shapes {
            feed(serde_json::to_string(&sh.kind).unwrap_or_default().as_bytes());
        }
        feed(serde_json::to_string(&o).unwrap_or_default().as_bytes());
        h
    }

    /// Hand-edited stitches whose shape or settings changed since.
    pub fn out_of_sync(&self) -> bool {
        self.embroidery.edited && self.embroidery.source != self.source_fingerprint()
    }

    pub fn tf(&self, p: Pt) -> Pt {
        {
            let q = p.sub(self.origin);
            Pt::new(q.x * self.sx, q.y * self.sy)
        }
        .rot(-self.rotation.to_radians())
        .add(self.origin)
        .add(self.offset)
    }

    pub fn inv(&self, p: Pt) -> Pt {
        {
            let q = p
                .sub(self.offset)
                .sub(self.origin)
                .rot(self.rotation.to_radians());
            Pt::new(q.x / self.sx, q.y / self.sy)
        }
        .add(self.origin)
    }

    pub fn transformed(&self) -> Vec<Shape> {
        self.shapes
            .iter()
            .map(|s| {
                let mut s = s.clone();
                match &mut s.kind {
                    ShapeKind::Area { rings, .. } => {
                        rings.iter_mut().flatten().for_each(|p| *p = self.tf(*p))
                    }
                    ShapeKind::Line { paths, width } => {
                        paths.iter_mut().flatten().for_each(|p| *p = self.tf(*p));
                        *width *= (self.sx * self.sy).abs().sqrt();
                    }
                }
                s
            })
            .collect()
    }

    pub fn bbox(&self) -> BBox {
        let mut b = BBox::empty();
        for s in &self.shapes {
            for p in s.points() {
                b.add(self.tf(*p));
            }
        }
        b
    }

    /// Size in the layer's own frame (before rotation), mm.
    pub fn own_size(&self) -> (f64, f64) {
        let b = BBox::of(self.shapes.iter().flat_map(|s| s.points()));
        (b.width() * self.sx.abs(), b.height() * self.sy.abs())
    }

    pub fn center(&self) -> Pt {
        self.origin.add(self.offset)
    }

    /// Does the point (design mm) land on this layer?
    pub fn hit(&self, p: Pt, tol_mm: f64) -> bool {
        let q = self.inv(p);
        let tol = tol_mm / self.sx.abs().min(self.sy.abs()).max(1e-6);
        self.shapes.iter().any(|s| match &s.kind {
            ShapeKind::Area { rings, rule } => spans_at(rings, *rule, q.y)
                .iter()
                .any(|&(a, b)| q.x >= a - tol && q.x <= b + tol),
            ShapeKind::Line { paths, width } => paths.iter().any(|path| {
                path.windows(2)
                    .any(|w| seg_dist(q, w[0], w[1]) <= width / 2.0 + tol)
            }),
        })
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = ((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2;
    p.dist(a.lerp(b, t.clamp(0.0, 1.0)))
}

/// Turn visible layers into artwork for the engine. Custom threads are added
/// to a copy of the chart; returns the artwork and that working chart.
pub fn compose(layers: &[Layer], chart: &[Thread]) -> (Artwork, Vec<Thread>) {
    let mut chart = chart.to_vec();
    let mut shapes = Vec::new();
    let mut fixed = std::collections::BTreeMap::new();
    for (li, l) in layers.iter().enumerate() {
        if !l.visible {
            continue;
        }
        let mut opts = ShapeOpts {
            layer: li,
            ..l.opts.clone()
        };
        if let Some(t) = &l.custom {
            chart.push(t.clone());
            opts.thread = Some(chart.len() - 1);
        }
        for mut s in l.transformed() {
            s.opts = opts.clone();
            s.label = l.name.clone();
            shapes.push(s);
        }
        if l.embroidery.edited && !l.embroidery.needles.is_empty() {
            let pts = l
                .embroidery
                .needles
                .iter()
                .map(|n| Needle {
                    p: l.tf(n.p),
                    jump: n.jump,
                })
                .collect();
            fixed.insert(li, pts);
        }
    }
    (
        Artwork {
            fixed,
            shapes,
            warnings: Vec::new(),
        },
        chart,
    )
}

const PX_TO_MM: f64 = 25.4 / 96.0;

/// Outlines for a text layer, in mm, centred on the origin. `height_mm` is the
/// capital-letter height, so every name in a run comes out the same size and
/// sits on the same baseline whatever its letters.
pub fn text_shapes(opt: &usvg::Options, spec: &TextSpec) -> Result<Vec<Shape>, String> {
    let art = crate::import::load_text_with(opt, &spec.text, &spec.font, spec.rgb)?;
    let b = BBox::of(art.shapes.iter().flat_map(|s| s.points()));
    if !b.is_valid() || b.width() <= 0.0 {
        return Err(format!(
            "“{}” produced no outlines in {}",
            spec.text.trim(),
            spec.font
        ));
    }
    let h = crate::import::load_text_with(opt, "H", &spec.font, spec.rgb)?;
    let hb = BBox::of(h.shapes.iter().flat_map(|s| s.points()));
    let cap = if hb.is_valid() && hb.height() > 0.0 {
        hb.height()
    } else {
        b.height()
    };
    let lines = spec.text.lines().count().max(1) as f64;
    let last_base = hb.max.y + (lines - 1.0) * 240.0 * PX_TO_MM;
    let c = Pt::new(b.center().x, (hb.min.y + last_base) / 2.0);
    let k = spec.height_mm / cap;
    Ok(art
        .shapes
        .into_iter()
        .map(|mut s| {
            match &mut s.kind {
                ShapeKind::Area { rings, .. } => rings
                    .iter_mut()
                    .flatten()
                    .for_each(|p| *p = p.sub(c).mul(k)),
                ShapeKind::Line { paths, width } => {
                    paths
                        .iter_mut()
                        .flatten()
                        .for_each(|p| *p = p.sub(c).mul(k));
                    *width *= k;
                }
            }
            s
        })
        .collect())
}

pub const FORMAT: &str = "stitchforge-project";
pub const VERSION: u32 = 1;

/// Everything needed to rebuild a design exactly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub layers: Vec<Layer>,
    pub profile: Profile,
    pub fabric: String,
    pub recipe: Recipe,
    pub keep_order: bool,
    pub auto_stops: bool,
    pub chart_name: String,
    pub chart: Vec<Thread>,
    pub job: JobInfo,
    /// Name-drop setup (personalisation runs), if any.
    #[serde(default)]
    pub drops: crate::drops::DropSetup,
}

impl Project {
    pub fn new(name: &str, layers: Vec<Layer>) -> Self {
        let profile = Profile::Flat;
        Project {
            format: FORMAT.into(),
            version: VERSION,
            name: name.into(),
            layers,
            profile,
            fabric: profile.default_recipe().into(),
            recipe: crate::settings::recipe(profile.default_recipe()).unwrap(),
            keep_order: true,
            auto_stops: true,
            chart_name: "Generic 40wt".into(),
            chart: thread::generic_chart(),
            job: JobInfo::default(),
            drops: Default::default(),
        }
    }

    pub fn build(&self) -> Result<Design, String> {
        let (art, chart) = compose(&self.layers, &self.chart);
        if art.shapes.is_empty() {
            return Err("no visible layers to stitch".into());
        }
        let name = format!(
            "{}-{}",
            self.name,
            if self.profile == Profile::Cap {
                "cap"
            } else {
                "flat"
            }
        );
        build_with(
            &name,
            &art,
            &chart,
            &self.recipe,
            self.profile,
            BuildOptions {
                keep_order: self.keep_order,
                auto_stops: self.auto_stops,
            },
        )
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        // write to a temp file first so a crash never leaves a half-written project
        let tmp = path.with_extension("sfg.tmp");
        crate::vfs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
        crate::vfs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let data = crate::vfs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let p: Project = serde_json::from_str(&data)
            .map_err(|e| format!("{} is not a StitchForge project: {e}", path.display()))?;
        if p.format != FORMAT {
            return Err(format!("{} is not a StitchForge project", path.display()));
        }
        if p.version > VERSION {
            return Err(format!(
                "{} was saved by a newer StitchForge (format {}); please update",
                path.display(),
                p.version
            ));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::FillRule;

    #[test]
    fn save_and_load_round_trip() {
        let ring = vec![
            Pt::new(0.0, 0.0),
            Pt::new(20.0, 0.0),
            Pt::new(20.0, 10.0),
            Pt::new(0.0, 10.0),
        ];
        let shape = Shape {
            curves: Vec::new(),
            kind: ShapeKind::Area {
                rings: vec![ring],
                rule: FillRule::NonZero,
            },
            rgb: [20, 140, 70],
            label: "box".into(),
            group: 1,
            opts: ShapeOpts::default(),
        };
        let mut l = Layer::new(1, "box".into(), vec![shape], false);
        l.rotation = 30.0;
        l.opts.angle_deg = Some(45.0);
        let p = Project::new("test", vec![l]);
        let before = p.build().unwrap();
        let path = std::env::temp_dir().join("sf-roundtrip.sfg");
        p.save(&path).unwrap();
        let q = Project::load(&path).unwrap();
        let after = q.build().unwrap();
        assert_eq!(before.stats.stitches, after.stats.stitches);
        assert_eq!(q.layers[0].rotation, 30.0);
        assert_eq!(q.layers[0].opts.angle_deg, Some(45.0));
    }
}

#[cfg(test)]
mod object_tests {
    use super::*;
    use crate::geom::{FillRule, Pt};
    use crate::import::{FillPattern, StitchChoice};

    fn square_layer(id: u64, x: f64, s: f64) -> Layer {
        let shape = Shape {
            kind: ShapeKind::Area {
                rings: vec![vec![
                    Pt::new(x, 0.0),
                    Pt::new(x + s, 0.0),
                    Pt::new(x + s, s),
                    Pt::new(x, s),
                ]],
                rule: FillRule::NonZero,
            },
            rgb: [20, 40, 120],
            label: "sq".into(),
            group: 0,
            opts: ShapeOpts::default(),
            curves: Vec::new(),
        };
        Layer::new(id, format!("Square {id}"), vec![shape], false)
    }

    fn build(layers: Vec<Layer>) -> crate::design::Design {
        let mut p = Project::new("t", layers);
        p.keep_order = true;
        p.build().unwrap()
    }

    #[test]
    fn layer_settings_change_only_that_layer() {
        let a = square_layer(1, 0.0, 20.0);
        let mut b = square_layer(2, 30.0, 20.0);
        let base = build(vec![a.clone(), b.clone()]);
        b.opts.stitch = StitchChoice::Fill;
        b.opts.density = Some(0.8);
        let d = build(vec![a, b]);
        let n = |d: &crate::design::Design, i: usize| d.objects[i].stitches;
        assert_eq!(n(&base, 0), n(&d, 0), "first layer untouched");
        assert!(n(&d, 1) < n(&base, 1) * 7 / 10, "sparser rows on the second");
    }

    #[test]
    fn fill_patterns_sew_inside_the_shape() {
        for f in [FillPattern::Contour, FillPattern::Spiral, FillPattern::Radial] {
            let mut l = square_layer(1, 0.0, 20.0);
            l.opts.stitch = StitchChoice::Fill;
            l.opts.fill = f;
            let d = build(vec![l]);
            assert!(d.objects[0].stitches > 300, "{f:?}: {}", d.objects[0].stitches);
            let b = &d.bbox;
            for c in &d.cmds {
                if let crate::design::Cmd::Stitch(p) = c {
                    assert!(p.x > b.min.x - 1.0 && p.x < b.max.x + 1.0, "{f:?} {p:?}");
                }
            }
        }
    }

    #[test]
    fn art_exports_and_reimports() {
        let mut l = square_layer(1, 0.0, 20.0);
        l.rotation = 30.0;
        let svg = art_svg(&[l.clone()], &crate::thread::generic_chart());
        let (opt, _) = crate::import::font_options(None).unwrap();
        let art = crate::import::load_svg_str(&svg, &opt).unwrap();
        assert_eq!(art.shapes.len(), 1);
        let a = match &art.shapes[0].kind {
            ShapeKind::Area { rings, rule } => crate::geom::rings_area(rings, *rule),
            _ => 0.0,
        };
        assert!((a - 400.0).abs() < 2.0, "{a}");
    }

    #[test]
    fn hand_edited_stitches_are_kept_and_move_with_the_layer() {
        let mut l = square_layer(1, 0.0, 10.0);
        let d = build(vec![l.clone()]);
        let made = d.layer_needles[&0].clone();
        // edit: drop every other needle point, store in the layer's own frame
        l.embroidery = Embroidery {
            needles: made.iter().step_by(2).map(|n| Needle { p: l.inv(n.p), jump: n.jump }).collect(),
            edited: true,
            source: l.source_fingerprint(),
        };
        let d2 = build(vec![l.clone()]);
        assert_eq!(d2.layer_needles[&0].len(), l.embroidery.needles.len());
        // moving the object moves its stitches too, without regenerating
        l.offset = Pt::new(15.0, 0.0);
        let d3 = build(vec![l.clone()]);
        let (a, b) = (d2.layer_needles[&0][3].p, d3.layer_needles[&0][3].p);
        assert!((b.x - a.x - 15.0).abs() < 1e-6);
        assert!(!l.out_of_sync(), "moving doesn't change the source");
        // changing the stitch settings does flag it
        l.opts.density = Some(0.6);
        assert!(l.out_of_sync());
    }
}

/// The project's artwork as an SVG file (mm units): every visible layer's
/// shape with its thread colour, so the art can go back to Inkscape or
/// Illustrator, or to the customer as a proof.
pub fn art_svg(layers: &[Layer], chart: &[Thread]) -> String {
    use crate::geom::BBox;
    let mut b = BBox::empty();
    for l in layers.iter().filter(|l| l.visible) {
        let lb = l.bbox();
        if lb.is_valid() {
            b.add(lb.min);
            b.add(lb.max);
        }
    }
    if !b.is_valid() {
        return "<svg xmlns=\"http://www.w3.org/2000/svg\"/>".into();
    }
    let pad = 2.0;
    let (x0, y0) = (b.min.x - pad, b.min.y - pad);
    let (w, h) = (b.width() + 2.0 * pad, b.height() + 2.0 * pad);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.2}mm\" height=\"{h:.2}mm\" viewBox=\"0 0 {w:.3} {h:.3}\">\n"
    );
    let esc = |s: &str| s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    for l in layers.iter().filter(|l| l.visible) {
        let rgb = l
            .custom
            .as_ref()
            .map(|t| t.rgb)
            .or_else(|| l.opts.thread.and_then(|t| chart.get(t)).map(|t| t.rgb))
            .unwrap_or_else(|| l.shapes.first().map(|s| s.rgb).unwrap_or([0, 0, 0]));
        let col = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
        let tf = |p: Pt| {
            let q = l.tf(p);
            Pt::new(q.x - x0, q.y - y0)
        };
        for s in &l.shapes {
            let mut sh = s.clone();
            sh.ensure_curves();
            let d: Vec<String> = sh.curves.iter().map(|c| crate::vector::path_d(c, &tf)).collect();
            if d.is_empty() {
                continue;
            }
            match &s.kind {
                ShapeKind::Area { rule, .. } => {
                    let fr = match rule {
                        crate::geom::FillRule::EvenOdd => "evenodd",
                        crate::geom::FillRule::NonZero => "nonzero",
                    };
                    out.push_str(&format!(
                        "  <path id=\"{}\" d=\"{}\" fill=\"{col}\" fill-rule=\"{fr}\"/>\n",
                        esc(&l.name),
                        d.join(" ")
                    ));
                }
                ShapeKind::Line { width, .. } => {
                    let w = l.opts.column_width.unwrap_or(*width) * (l.sx * l.sy).abs().sqrt();
                    out.push_str(&format!(
                        "  <path id=\"{}\" d=\"{}\" fill=\"none\" stroke=\"{col}\" stroke-width=\"{w:.3}\"/>\n",
                        esc(&l.name),
                        d.join(" ")
                    ));
                }
            }
        }
    }
    out.push_str("</svg>\n");
    out
}
