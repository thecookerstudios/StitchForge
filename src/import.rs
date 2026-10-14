//! Artwork import: SVG files and live text in any installed or supplied font.
//! Text is converted to outlines by the SVG engine using the font itself,
//! so no special embroidery font format is ever needed.

use crate::geom::{BBox, FillRule, Pt, Ring};
use serde::{Deserialize, Serialize};

use usvg::tiny_skia_path::PathSegment;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShapeKind {
    /// Closed area to be filled (fill or satin, decided by width).
    Area { rings: Vec<Ring>, rule: FillRule },
    /// Stroked path; width in mm. Open or closed polylines.
    Line { paths: Vec<Vec<Pt>>, width: f64 },
}

/// How a layer should stitch. `Auto` lets the engine decide by shape width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StitchChoice {
    #[default]
    Auto,
    Fill,
    Satin,
    /// Areas: run stitch along the outline. Lines: single run.
    Run,
    /// Areas: triple run along the outline. Lines: triple run.
    TripleRun,
}

impl StitchChoice {
    pub const ALL: [StitchChoice; 5] = [
        StitchChoice::Auto,
        StitchChoice::Fill,
        StitchChoice::Satin,
        StitchChoice::Run,
        StitchChoice::TripleRun,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            StitchChoice::Auto => "Auto (by width)",
            StitchChoice::Fill => "Tatami fill",
            StitchChoice::Satin => "Satin",
            StitchChoice::Run => "Run (outline)",
            StitchChoice::TripleRun => "Triple run (outline)",
        }
    }
}

/// Per-layer choices made by the user. Anything left `None`/`Auto` is decided
/// by the engine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ShapeOpts {
    pub stitch: StitchChoice,
    /// Stitch direction in degrees (0 = horizontal, counter-clockwise).
    pub angle_deg: Option<f64>,
    /// Index into the thread chart.
    pub thread: Option<usize>,
    /// Machine stop (colour change) before this layer: Some(true) forces one,
    /// Some(false) suppresses one, None follows the automatic rule.
    pub stop: Option<bool>,
    /// Which layer this shape belongs to (its position in the layer list).
    pub layer: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    pub rgb: [u8; 3],
    pub label: String,
    /// Shapes from the same SVG element (e.g. one text block) share a group.
    pub group: usize,
    pub opts: ShapeOpts,
}

impl Shape {
    pub fn points(&self) -> Box<dyn Iterator<Item = &Pt> + '_> {
        match &self.kind {
            ShapeKind::Area { rings, .. } => Box::new(rings.iter().flatten()),
            ShapeKind::Line { paths, .. } => Box::new(paths.iter().flatten()),
        }
    }
}

#[derive(Clone)]
pub struct Artwork {
    pub shapes: Vec<Shape>,
    pub warnings: Vec<String>,
}

const PX_TO_MM: f64 = 25.4 / 96.0;

pub fn font_options(
    font_file: Option<&str>,
) -> Result<(usvg::Options<'static>, Option<String>), String> {
    let mut opt = usvg::Options::default();
    let db = opt.fontdb_mut();
    db.load_system_fonts();
    let mut family = None;
    if let Some(f) = font_file {
        let before = db.len();
        db.load_font_file(f)
            .map_err(|e| format!("cannot load font {f}: {e}"))?;
        if db.len() == before {
            return Err(format!("no usable font faces in {f}"));
        }
        family = db
            .faces()
            .last()
            .and_then(|face| face.families.first().map(|x| x.0.clone()));
    }
    Ok((opt, family))
}

pub fn list_fonts() -> Vec<String> {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    let mut v: Vec<String> = db
        .faces()
        .filter_map(|f| f.families.first().map(|x| x.0.clone()))
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn load_svg_str(svg: &str, opt: &usvg::Options) -> Result<Artwork, String> {
    let tree = usvg::Tree::from_str(svg, opt).map_err(|e| format!("SVG parse error: {e}"))?;
    let mut art = Artwork {
        shapes: Vec::new(),
        warnings: Vec::new(),
    };
    let mut groups = 0usize;
    walk(tree.root(), &mut art, None, &mut groups);
    // Each source element is its own layer unless the caller regroups them.
    for (i, sh) in art.shapes.iter_mut().enumerate() {
        sh.opts.layer = i;
    }
    // Stitch instructions in element ids/names: sf-satin, sf-fill, sf-run,
    // sf-triple, sf-angle-45. Lets designers set stitching in their art app.
    for sh in &mut art.shapes {
        apply_id_tags(&mut sh.opts, &sh.label);
    }
    if art.shapes.is_empty() {
        return Err("no stitchable shapes found in the artwork".into());
    }
    Ok(art)
}

pub fn load_svg_file(path: &str, font_file: Option<&str>) -> Result<Artwork, String> {
    let data = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let (opt, _) = font_options(font_file)?;
    load_svg_str(&data, &opt)
}

/// Build artwork from a line of text in any font.
pub fn load_text(
    text: &str,
    family: Option<&str>,
    font_file: Option<&str>,
    rgb: [u8; 3],
) -> Result<Artwork, String> {
    let (opt, file_family) = font_options(font_file)?;
    let fam = file_family
        .or(family.map(String::from))
        .unwrap_or_else(|| "sans-serif".into());
    if !opt
        .fontdb
        .faces()
        .any(|f| f.families.iter().any(|x| x.0.eq_ignore_ascii_case(&fam)))
        && !["sans-serif", "serif", "monospace"].contains(&fam.as_str())
    {
        return Err(format!(
            "font \"{fam}\" is not installed; run `stitchforge fonts` to list fonts or pass --font-file"
        ));
    }
    load_text_with(&opt, text, &fam, rgb)
}

/// Text to artwork with an already-loaded font database (fast for the app).
pub fn load_text_with(
    opt: &usvg::Options,
    text: &str,
    fam: &str,
    rgb: [u8; 3],
) -> Result<Artwork, String> {
    let esc = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let fam_esc = fam.replace('"', "").replace('\'', "");
    // One <tspan> per line so multi-line text keeps its layout.
    let lines: String = esc
        .lines()
        .enumerate()
        .map(|(i, l)| {
            format!(
                "<tspan x='10' y='{}'>{}</tspan>",
                300 + i * 240,
                if l.is_empty() { " " } else { l }
            )
        })
        .collect();
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='6000' height='{}'>\
         <text xml:space='preserve' font-family=\"'{fam_esc}'\" font-size='200' fill='#{:02x}{:02x}{:02x}'>{lines}</text></svg>",
        400 + 240 * esc.lines().count().max(1),
        rgb[0],
        rgb[1],
        rgb[2]
    );
    let mut art = load_svg_str(&svg, opt)?;
    for s in &mut art.shapes {
        s.label = text.lines().next().unwrap_or("Text").to_string();
        s.group = 0;
    }
    Ok(art)
}

/// Read `sf-…` instructions from an element id (see `load_svg_str`).
pub fn apply_id_tags(opts: &mut ShapeOpts, id: &str) {
    let id = id.to_ascii_lowercase();
    if id.contains("sf-satin") {
        opts.stitch = StitchChoice::Satin;
    } else if id.contains("sf-fill") {
        opts.stitch = StitchChoice::Fill;
    } else if id.contains("sf-triple") {
        opts.stitch = StitchChoice::TripleRun;
    } else if id.contains("sf-run") {
        opts.stitch = StitchChoice::Run;
    }
    if let Some(i) = id.find("sf-angle-") {
        let num: String = id[i + 9..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(a) = num.parse::<f64>() {
            opts.angle_deg = Some(a.rem_euclid(180.0));
        }
    }
}

/// `outer` is an extra transform applied after each path's own: text
/// outlines live in their text element's space, which usvg keeps separately.
fn walk(g: &usvg::Group, art: &mut Artwork, outer: Option<usvg::Transform>, groups: &mut usize) {
    for node in g.children() {
        match node {
            usvg::Node::Group(c) => walk(c, art, outer, groups),
            usvg::Node::Path(p) => {
                *groups += 1;
                add_path(p, art, outer, *groups)
            }
            usvg::Node::Text(t) => {
                let first = art.shapes.len();
                *groups += 1;
                let g = *groups;
                walk(t.flattened(), art, Some(t.abs_transform()), groups);
                for s in &mut art.shapes[first..] {
                    s.group = g;
                }
                *groups = g;
                // Name text by its words (or its id when it has one).
                let words: String = t.chunks().iter().map(|c| c.text()).collect::<Vec<_>>().join(" ");
                let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
                let label = if !t.id().is_empty() {
                    t.id().to_string()
                } else if !words.is_empty() {
                    words.chars().take(28).collect()
                } else {
                    "Lettering".to_string()
                };
                for s in &mut art.shapes[first..] {
                    if s.label.is_empty() {
                        s.label = label.clone();
                    }
                }
            }
            usvg::Node::Image(_) => art
                .warnings
                .push("embedded bitmap image skipped; trace it to vectors first".into()),
        }
    }
}

fn paint_rgb(p: &usvg::Paint) -> [u8; 3] {
    match p {
        usvg::Paint::Color(c) => [c.red, c.green, c.blue],
        usvg::Paint::LinearGradient(g) => stop_rgb(g.stops()),
        usvg::Paint::RadialGradient(g) => stop_rgb(g.stops()),
        usvg::Paint::Pattern(_) => [128, 128, 128],
    }
}

fn stop_rgb(stops: &[usvg::Stop]) -> [u8; 3] {
    stops
        .first()
        .map(|s| [s.color().red, s.color().green, s.color().blue])
        .unwrap_or([128, 128, 128])
}

fn add_path(p: &usvg::Path, art: &mut Artwork, outer: Option<usvg::Transform>, group: usize) {
    if !p.is_visible() {
        return;
    }
    let t = match outer {
        Some(o) => o.pre_concat(p.abs_transform()),
        None => p.abs_transform(),
    };
    let map = |x: f32, y: f32| -> Pt {
        let (x, y) = (x as f64, y as f64);
        Pt::new(
            (t.sx as f64 * x + t.kx as f64 * y + t.tx as f64) * PX_TO_MM,
            (t.ky as f64 * x + t.sy as f64 * y + t.ty as f64) * PX_TO_MM,
        )
    };
    let scale = ((t.sx as f64 * t.sy as f64 - t.kx as f64 * t.ky as f64).abs()).sqrt();

    // Flatten curves into polylines (one per subpath).
    let mut subs: Vec<(Vec<Pt>, bool)> = Vec::new();
    let mut cur: Vec<Pt> = Vec::new();
    let mut last = Pt::new(0.0, 0.0);
    let flush = |cur: &mut Vec<Pt>, closed: bool, subs: &mut Vec<(Vec<Pt>, bool)>| {
        if cur.len() >= 2 {
            subs.push((std::mem::take(cur), closed));
        } else {
            cur.clear();
        }
    };
    for seg in p.data().segments() {
        match seg {
            PathSegment::MoveTo(a) => {
                flush(&mut cur, false, &mut subs);
                last = map(a.x, a.y);
                cur.push(last);
            }
            PathSegment::LineTo(a) => {
                last = map(a.x, a.y);
                cur.push(last);
            }
            PathSegment::QuadTo(c, a) => {
                let (p0, p1, p2) = (last, map(c.x, c.y), map(a.x, a.y));
                let n = ((p0.dist(p1) + p1.dist(p2)) / 0.25).ceil().clamp(2.0, 64.0) as usize;
                for k in 1..=n {
                    let u = k as f64 / n as f64;
                    cur.push(p0.lerp(p1, u).lerp(p1.lerp(p2, u), u));
                }
                last = p2;
            }
            PathSegment::CubicTo(c1, c2, a) => {
                let (p0, p1, p2, p3) = (last, map(c1.x, c1.y), map(c2.x, c2.y), map(a.x, a.y));
                let n = ((p0.dist(p1) + p1.dist(p2) + p2.dist(p3)) / 0.25)
                    .ceil()
                    .clamp(2.0, 96.0) as usize;
                for k in 1..=n {
                    let u = k as f64 / n as f64;
                    let a1 = p0.lerp(p1, u);
                    let b1 = p1.lerp(p2, u);
                    let c1 = p2.lerp(p3, u);
                    cur.push(a1.lerp(b1, u).lerp(b1.lerp(c1, u), u));
                }
                last = p3;
            }
            PathSegment::Close => {
                if let Some(f) = cur.first().copied() {
                    // Close the loop so outlines include the last edge.
                    if cur.last().map(|l| l.dist(f) > 1e-6).unwrap_or(false) {
                        cur.push(f);
                    }
                    flush(&mut cur, true, &mut subs);
                    last = f;
                    cur.push(f);
                }
            }
        }
    }
    flush(&mut cur, false, &mut subs);

    // Drop duplicate consecutive points.
    for (s, _) in subs.iter_mut() {
        s.dedup_by(|a, b| a.dist(*b) < 1e-6);
    }

    if let Some(fill) = p.fill() {
        let rule = match fill.rule() {
            usvg::FillRule::NonZero => FillRule::NonZero,
            usvg::FillRule::EvenOdd => FillRule::EvenOdd,
        };
        let rings: Vec<Ring> = subs
            .iter()
            .filter(|(s, _)| s.len() >= 3)
            .map(|(s, _)| {
                let mut r = s.clone();
                if r.len() > 1 && r[0].dist(*r.last().unwrap()) < 1e-6 {
                    r.pop();
                }
                r
            })
            .collect();
        if !rings.is_empty() {
            art.shapes.push(Shape {
                kind: ShapeKind::Area { rings, rule },
                rgb: paint_rgb(fill.paint()),
                label: p.id().to_string(),
                group,
                opts: ShapeOpts::default(),
            });
        }
    }
    if let Some(stroke) = p.stroke() {
        let width = stroke.width().get() as f64 * scale * PX_TO_MM;
        let paths: Vec<Vec<Pt>> = subs.iter().map(|(s, _)| s.clone()).collect();
        if !paths.is_empty() {
            art.shapes.push(Shape {
                kind: ShapeKind::Line { paths, width },
                rgb: paint_rgb(stroke.paint()),
                label: if p.id().is_empty() {
                    String::new()
                } else {
                    format!("{} outline", p.id())
                },
                group,
                opts: ShapeOpts::default(),
            });
        }
    }
}

/// Scale the artwork uniformly to a target width or height (mm) and move its
/// top-left corner to the origin.
pub fn fit(art: &mut Artwork, width_mm: Option<f64>, height_mm: Option<f64>) {
    let b = BBox::of(art.shapes.iter().flat_map(|s| s.points()));
    if !b.is_valid() {
        return;
    }
    let k = match (width_mm, height_mm) {
        (Some(w), _) if b.width() > 0.0 => w / b.width(),
        (None, Some(h)) if b.height() > 0.0 => h / b.height(),
        _ => 1.0,
    };
    let tf = |p: &mut Pt| *p = p.sub(b.min).mul(k);
    for s in &mut art.shapes {
        match &mut s.kind {
            ShapeKind::Area { rings, .. } => rings.iter_mut().flatten().for_each(tf),
            ShapeKind::Line { paths, width } => {
                paths.iter_mut().flatten().for_each(tf);
                *width *= k;
            }
        }
    }
}
