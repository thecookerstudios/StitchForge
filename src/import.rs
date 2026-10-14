//! Artwork import: SVG files and live text in any installed or supplied font.
//! Text is converted to outlines by the SVG engine using the font itself,
//! so no special embroidery font format is ever needed.

use crate::geom::{BBox, FillRule, Pt, Ring};

use usvg::tiny_skia_path::PathSegment;

#[derive(Clone, Debug)]
pub enum ShapeKind {
    /// Closed area to be filled (fill or satin, decided by width).
    Area { rings: Vec<Ring>, rule: FillRule },
    /// Stroked path; width in mm. Open or closed polylines.
    Line { paths: Vec<Vec<Pt>>, width: f64 },
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub kind: ShapeKind,
    pub rgb: [u8; 3],
    pub label: String,
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
    walk(tree.root(), &mut art);
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
    let esc = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let fam_esc = fam.replace('"', "").replace('\'', "");
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='4000' height='400'>\
         <text x='10' y='300' font-family=\"'{fam_esc}'\" font-size='200' fill='#{:02x}{:02x}{:02x}'>{esc}</text></svg>",
        rgb[0], rgb[1], rgb[2]
    );
    let mut art = load_svg_str(&svg, &opt)?;
    for s in &mut art.shapes {
        s.label = format!("Text \"{text}\" ({fam})");
    }
    Ok(art)
}

fn walk(g: &usvg::Group, art: &mut Artwork) {
    for node in g.children() {
        match node {
            usvg::Node::Group(c) => walk(c, art),
            usvg::Node::Path(p) => add_path(p, art),
            usvg::Node::Text(t) => {
                let first = art.shapes.len();
                walk(t.flattened(), art);
                let label = if t.id().is_empty() {
                    "Lettering".to_string()
                } else {
                    t.id().to_string()
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

fn add_path(p: &usvg::Path, art: &mut Artwork) {
    if !p.is_visible() {
        return;
    }
    let t = p.abs_transform();
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
                label: p.id().to_string(),
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
