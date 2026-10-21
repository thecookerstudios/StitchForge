//! Artwork import: SVG files and live text in any installed or supplied font.
//! Text is converted to outlines by the SVG engine using the font itself,
//! so no special embroidery font format is ever needed.

use crate::geom::{BBox, FillRule, Node, Pt, Ring, VPath};
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
            StitchChoice::Fill => "Fill",
            StitchChoice::Satin => "Satin",
            StitchChoice::Run => "Run (outline)",
            StitchChoice::TripleRun => "Triple run (outline)",
        }
    }
}

/// How a satin column turns a sharp corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    /// Each side runs past the corner and overlaps (square, heavier corner).
    Square,
    /// Stitches fan to a diagonal seam through the corner.
    #[default]
    Miter,
    /// The outside of the corner is rounded; stitches fan around it.
    Round,
}

impl Corner {
    pub const ALL: [Corner; 3] = [Corner::Miter, Corner::Round, Corner::Square];
    pub fn label(&self) -> &'static str {
        match self {
            Corner::Square => "Square (overlap)",
            Corner::Miter => "Mitered",
            Corner::Round => "Rounded",
        }
    }
}

/// How an open satin column ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cap {
    #[default]
    Flat,
    Square,
    Round,
    Point,
}

impl Cap {
    pub const ALL: [Cap; 4] = [Cap::Flat, Cap::Square, Cap::Round, Cap::Point];
    pub fn label(&self) -> &'static str {
        match self {
            Cap::Flat => "Flat",
            Cap::Square => "Square (extended)",
            Cap::Round => "Round",
            Cap::Point => "Point",
        }
    }
}

/// The stitch pattern used to fill an area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillPattern {
    /// Straight back-and-forth rows with a staggered (brick) needle pattern.
    #[default]
    Tatami,
    /// Rows that follow the outline inward, ring by ring.
    Contour,
    /// One continuous line spiralling in from the outline.
    Spiral,
    /// Rows radiating from the middle of the shape.
    Radial,
}

impl FillPattern {
    pub const ALL: [FillPattern; 4] = [
        FillPattern::Tatami,
        FillPattern::Contour,
        FillPattern::Spiral,
        FillPattern::Radial,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            FillPattern::Tatami => "Tatami (rows)",
            FillPattern::Contour => "Contour (follows the outline)",
            FillPattern::Spiral => "Spiral",
            FillPattern::Radial => "Radial (from the middle)",
        }
    }
    pub fn short(&self) -> &'static str {
        match self {
            FillPattern::Tatami => "Tatami fill",
            FillPattern::Contour => "Contour fill",
            FillPattern::Spiral => "Spiral fill",
            FillPattern::Radial => "Radial fill",
        }
    }
}

/// Per-layer choices made by the user. Anything left `None`/`Auto` is decided
/// by the engine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
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
    /// Skip underlay for this layer.
    pub no_underlay: bool,
    /// Keep sewing this layer underneath layers above it (no cut-out).
    pub keep_under: bool,
    /// Satin column (outline) width in mm; None = the artwork's stroke width.
    pub column_width: Option<f64>,
    /// Satin areas (e.g. lettering): widen (+) or narrow (−) by this many mm.
    pub widen: f64,
    /// Satin outline position on its path (mm): 0 = centred on the line
    /// (50/50); + moves it outward (right of an open path), − inward.
    #[serde(skip_serializing_if = "is_zero")]
    pub satin_offset: f64,
    pub corner: Corner,
    pub cap: Cap,
    /// Sews on top without cutting holes in the layers under it (used for
    /// name drops, where each name runs on a different head).
    pub overlay: bool,

    // ---- this layer's stitch settings (None = the fabric's default) ----
    /// Fill stitch pattern.
    pub fill: FillPattern,
    /// Spacing between fill rows or satin stitches, mm.
    pub density: Option<f64>,
    /// Fill (and contour/radial run) stitch length, mm.
    pub stitch_len: Option<f64>,
    /// Pull compensation, mm.
    pub pull_comp: Option<f64>,
    /// Underlay row spacing, mm.
    pub underlay_spacing: Option<f64>,
    /// How far underlay stays inside the edge, mm.
    pub underlay_inset: Option<f64>,
    /// Tatami needle pattern repeats every N rows (2 = half offset, 3 = thirds…).
    pub stagger: Option<u8>,
    /// Thread weight for this layer (None = the design's weight, 40 wt by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<crate::thread::ThreadWeight>,

    // ---- satin and fill specifics ----
    /// Underlay under satin (Auto picks by width; Off = none).
    #[serde(default, skip_serializing_if = "is_default")]
    pub satin_underlay: Underlay,
    /// Underlay under fills.
    #[serde(default, skip_serializing_if = "is_default")]
    pub fill_underlay: Underlay,
    /// Longest satin stitch before it is split (mm); None = 8 mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_width: Option<f64>,
    /// Short stitches on the inside of curves (None = on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_stitches: Option<bool>,
    /// 3D puff: sewn over foam (satin only), with a stop to place the foam.
    #[serde(default, skip_serializing_if = "is_default")]
    pub puff: bool,
    /// Foam thickness for puff (mm); None = 3 mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foam_mm: Option<f64>,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// What sews under the top stitching to hold the fabric and give loft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Underlay {
    /// Chosen by stitch type, width and garment.
    #[default]
    Auto,
    Off,
    /// One run down the middle (narrow satin, small lettering).
    CenterRun,
    /// A run just inside the edges (crisp edges, stops the fabric creeping).
    EdgeRun,
    /// A zigzag inside the column (loft for wider satin).
    Zigzag,
    /// Edge run, then zigzag (wide satin, caps, raised look).
    EdgeZigzag,
    /// Two zigzag passes (very wide satin, thick fabric).
    DoubleZigzag,
    /// Sparse rows across the fill direction (standard fill underlay).
    Tatami,
    /// Edge run, then tatami rows (fills on caps, knits, fleece).
    EdgeTatami,
    /// Tatami rows both ways (heavy fabric, big fills).
    DoubleTatami,
}

impl Underlay {
    pub const SATIN: [Underlay; 7] = [
        Underlay::Auto,
        Underlay::CenterRun,
        Underlay::EdgeRun,
        Underlay::Zigzag,
        Underlay::EdgeZigzag,
        Underlay::DoubleZigzag,
        Underlay::Off,
    ];
    pub const FILL: [Underlay; 6] = [
        Underlay::Auto,
        Underlay::Tatami,
        Underlay::EdgeRun,
        Underlay::EdgeTatami,
        Underlay::DoubleTatami,
        Underlay::Off,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            Underlay::Auto => "Auto",
            Underlay::Off => "None",
            Underlay::CenterRun => "Center run",
            Underlay::EdgeRun => "Edge run",
            Underlay::Zigzag => "Zigzag",
            Underlay::EdgeZigzag => "Edge run + zigzag",
            Underlay::DoubleZigzag => "Double zigzag",
            Underlay::Tatami => "Tatami (across)",
            Underlay::EdgeTatami => "Edge run + tatami",
            Underlay::DoubleTatami => "Double tatami",
        }
    }
    pub fn hint(&self) -> &'static str {
        match self {
            Underlay::Auto => "Center run for narrow columns, zigzag for wide; edge run added on caps",
            Underlay::Off => "No underlay (very small text on stretchy fabric only)",
            Underlay::CenterRun => "Small lettering and narrow columns (under about 2 mm)",
            Underlay::EdgeRun => "Crisp edges; holds the fabric at the outline",
            Underlay::Zigzag => "Loft and support for columns 2–5 mm",
            Underlay::EdgeZigzag => "Wide satin, caps and borders",
            Underlay::DoubleZigzag => "Extra loft for very wide satin or thick fabric",
            Underlay::Tatami => "Standard support under fills",
            Underlay::EdgeTatami => "Fills on caps, knits and fleece",
            Underlay::DoubleTatami => "Heavy fabric or very large fills",
        }
    }
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl ShapeOpts {
    /// Does this layer change any stitch setting from the fabric defaults?
    pub fn has_own_settings(&self) -> bool {
        self.satin_underlay != Underlay::Auto
            || self.fill_underlay != Underlay::Auto
            || self.split_width.is_some()
            || self.short_stitches.is_some()
            || self.density.is_some()
            || self.stitch_len.is_some()
            || self.pull_comp.is_some()
            || self.underlay_spacing.is_some()
            || self.underlay_inset.is_some()
            || self.stagger.is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    pub rgb: [u8; 3],
    pub label: String,
    /// Shapes from the same SVG element (e.g. one text block) share a group.
    pub group: usize,
    pub opts: ShapeOpts,
    /// The editable vector paths (anchors and Bézier handles) this shape's
    /// rings or lines are flattened from. Empty for shapes from older files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<VPath>,
}

impl Shape {
    /// Rebuild the stitchable polylines after the vector paths changed.
    pub fn reflatten(&mut self) {
        if self.curves.is_empty() {
            return;
        }
        match &mut self.kind {
            ShapeKind::Area { rings, .. } => {
                *rings = self
                    .curves
                    .iter()
                    .map(|c| {
                        let mut r = c.flatten();
                        if r.len() > 1 && r[0].dist(*r.last().unwrap()) < 1e-6 {
                            r.pop();
                        }
                        r
                    })
                    .filter(|r| r.len() >= 3)
                    .collect();
            }
            ShapeKind::Line { paths, .. } => {
                *paths = self
                    .curves
                    .iter()
                    .map(|c| c.flatten())
                    .filter(|p| p.len() >= 2)
                    .collect();
            }
        }
    }

    /// Make sure there are editable paths (older files and text keep only
    /// polylines; those become paths of simplified corner anchors).
    pub fn ensure_curves(&mut self) {
        if !self.curves.is_empty() {
            return;
        }
        self.curves = match &self.kind {
            ShapeKind::Area { rings, .. } => rings
                .iter()
                .map(|r| crate::geom::fit_path(r, true, 0.03))
                .collect(),
            ShapeKind::Line { paths, .. } => paths
                .iter()
                .map(|p| {
                    let closed = p.len() > 2 && p[0].dist(*p.last().unwrap()) < 1e-6;
                    crate::geom::fit_path(p, closed, 0.03)
                })
                .collect(),
        };
    }

    pub fn points(&self) -> Box<dyn Iterator<Item = &Pt> + '_> {
        match &self.kind {
            ShapeKind::Area { rings, .. } => Box::new(rings.iter().flatten()),
            ShapeKind::Line { paths, .. } => Box::new(paths.iter().flatten()),
        }
    }
}

#[derive(Clone, Default)]
pub struct Artwork {
    pub shapes: Vec<Shape>,
    pub warnings: Vec<String>,
    /// Layers whose stitches were edited by hand: their needle points
    /// (design mm), sewn as they are instead of being generated.
    pub fixed: std::collections::BTreeMap<usize, Vec<crate::stitch::Needle>>,
}

const PX_TO_MM: f64 = 25.4 / 96.0;

thread_local! {
    /// Fonts added at run time (bundled in the web app, or loaded from a
    /// font file the user picked). Used alongside the installed fonts.
    static EXTRA_FONTS: std::cell::RefCell<Vec<std::sync::Arc<Vec<u8>>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Make a font file's faces available to text layers. Returns the first
/// family name in it.
pub fn add_font_data(data: Vec<u8>) -> Result<String, String> {
    let mut db = usvg::fontdb::Database::new();
    db.load_font_data(data.clone());
    let fam = db
        .faces()
        .next()
        .and_then(|f| f.families.first().map(|x| x.0.clone()))
        .ok_or("no usable font faces in that file")?;
    EXTRA_FONTS.with(|e| e.borrow_mut().push(std::sync::Arc::new(data)));
    Ok(fam)
}

fn load_fonts(db: &mut usvg::fontdb::Database) {
    db.load_system_fonts();
    EXTRA_FONTS.with(|e| {
        for f in e.borrow().iter() {
            db.load_font_source(usvg::fontdb::Source::Binary(f.clone()));
        }
    });
    // Generic families point at a font that is really there, so text in a
    // missing font still comes out (in a plain sans) instead of vanishing.
    let has = |db: &usvg::fontdb::Database, f: &str| {
        db.faces()
            .any(|x| x.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(f)))
    };
    for f in ["Arial", "Helvetica", "Liberation Sans", "DejaVu Sans", "Noto Sans", "Segoe UI"] {
        if has(db, f) {
            db.set_sans_serif_family(f);
            break;
        }
    }
    for f in ["Times New Roman", "Times", "Liberation Serif", "DejaVu Serif", "Georgia"] {
        if has(db, f) {
            db.set_serif_family(f);
            break;
        }
    }
}

/// Fonts named in an SVG that aren't installed; their text gets a fallback.
fn missing_fonts(svg: &str, opt: &usvg::Options) -> (String, Vec<String>) {
    let generic = ["serif", "sans-serif", "monospace", "cursive", "fantasy"];
    let mut missing: Vec<String> = Vec::new();
    let mut out = String::with_capacity(svg.len() + 64);
    let mut rest = svg;
    while let Some(i) = rest.find("font-family") {
        out.push_str(&rest[..i + 11]);
        rest = &rest[i + 11..];
        // value: ="..." / ='...' (attribute) or :...; (style)
        let t = rest.trim_start();
        let skip = rest.len() - t.len();
        let (open, close) = match t.chars().next() {
            Some('=') => {
                let q = t[1..].trim_start().chars().next().unwrap_or('"');
                (t.find(q).map(|k| k + 1), Some(q))
            }
            Some(':') => (Some(1), None),
            _ => (None, None),
        };
        let Some(open) = open else { continue };
        let body = &t[open..];
        let end = match close {
            Some(q) => body.find(q),
            None => body.find([';', '"', '\'', '}']),
        }
        .unwrap_or(body.len());
        let value = &body[..end];
        let fams: Vec<String> = value
            .split(',')
            .map(|f| f.trim().trim_matches(['\'', '"']).replace("&quot;", "").trim().to_string())
            .filter(|f| !f.is_empty())
            .collect();
        let any_present = fams.iter().any(|f| {
            generic.contains(&f.to_ascii_lowercase().as_str())
                || opt.fontdb.faces().any(|x| x.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(f)))
        });
        out.push_str(&rest[..skip + open]);
        out.push_str(value);
        if !any_present {
            if let Some(f) = fams.first() {
                if !missing.contains(f) {
                    missing.push(f.clone());
                }
            }
            out.push_str(", sans-serif");
        }
        rest = &body[end..];
    }
    out.push_str(rest);
    (out, missing)
}

pub fn font_options(
    font_file: Option<&str>,
) -> Result<(usvg::Options<'static>, Option<String>), String> {
    let mut opt = usvg::Options::default();
    let db = opt.fontdb_mut();
    load_fonts(db);
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
    load_fonts(&mut db);
    let mut v: Vec<String> = db
        .faces()
        .filter_map(|f| f.families.first().map(|x| x.0.clone()))
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn load_svg_str(svg: &str, opt: &usvg::Options) -> Result<Artwork, String> {
    let (svg, missing) = missing_fonts(svg, opt);
    let tree = usvg::Tree::from_str(&svg, opt).map_err(|e| format!("SVG parse error: {e}"))?;
    let mut art = Artwork {
        fixed: Default::default(),
        shapes: Vec::new(),
        warnings: missing
            .iter()
            .map(|f| {
                format!(
                    "Font \"{f}\" isn't installed on this computer, so its text uses a plain sans font. Install the font, or convert the text to outlines in your art app."
                )
            })
            .collect(),
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
    let data = crate::vfs::read_to_string(std::path::Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))?;
    let (opt, _) = font_options(font_file)?;
    load_svg_str(&data, &opt)
}

/// Open any supported artwork: SVG, a picture (PNG/JPG… auto-traced) or a
/// PDF/AI file (its vector paths, or its picture traced). `width_mm` is the
/// size the art will sew at, which tells the tracer how much detail to keep.
pub fn load_file(path: &str, width_mm: Option<f64>) -> Result<Artwork, String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".svg") {
        return load_svg_file(path, None);
    }
    let bytes = crate::vfs::read(std::path::Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))?;
    load_bytes(&lower, &bytes, width_mm)
}

/// As `load_file`, from bytes already read (`name` gives the file type).
pub fn load_bytes(name: &str, bytes: &[u8], width_mm: Option<f64>) -> Result<Artwork, String> {
    let (opt, _) = font_options(None)?;
    let lower = name.to_lowercase();
    if lower.ends_with(".pdf") || lower.ends_with(".ai") || bytes.starts_with(b"%PDF") {
        let pdf = crate::pdf::read(bytes)?;
        if let Some(svg) = pdf.svg {
            let mut art = load_svg_str(&svg, &opt)?;
            art.warnings.extend(pdf.notes);
            return Ok(art);
        }
        if let Some(pic) = pdf.picture {
            let o = crate::trace::TraceOptions {
                width_mm: width_mm.unwrap_or(101.6),
                ..Default::default()
            };
            let t = crate::trace::trace(&pic, &o)?;
            let mut art = load_svg_str(&t.to_svg(), &opt)?;
            art.warnings.extend(pdf.notes);
            art.warnings.push("The PDF holds a picture, not vector art: it was auto-traced".into());
            art.warnings.extend(t.notes);
            return Ok(art);
        }
        return Err("no vector art or picture found in the PDF".into());
    }
    if crate::trace::is_raster(&lower) || image_magic(bytes) {
        let o = crate::trace::TraceOptions {
            width_mm: width_mm.unwrap_or(101.6),
            ..Default::default()
        };
        let (svg, notes) = crate::trace::trace_bytes_to_svg(bytes, &o)?;
        let mut art = load_svg_str(&svg, &opt)?;
        art.warnings.extend(notes);
        return Ok(art);
    }
    let text = String::from_utf8_lossy(bytes);
    load_svg_str(&text, &opt)
}

fn image_magic(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G']) || b.starts_with(&[0xff, 0xd8]) || b.starts_with(b"GIF8") || b.starts_with(b"BM")
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

/// How lines of text line up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// Spacing for text, in the font's units at 200 px size.
#[derive(Clone, Debug)]
pub struct TextLayout {
    pub letter_px: f64,
    pub word_px: f64,
    pub line: f64,
    pub kern_px: Vec<f64>,
    pub align: TextAlign,
}

/// Text to artwork with letter, word and line spacing, per-letter kerning
/// and alignment (the font engine lays it out, so it stays real text).
pub fn load_text_laid_out(
    opt: &usvg::Options,
    text: &str,
    fam: &str,
    rgb: [u8; 3],
    lay: &TextLayout,
) -> Result<Artwork, String> {
    let esc = |c: char| match c {
        '&' => "&amp;".to_string(),
        '<' => "&lt;".to_string(),
        '>' => "&gt;".to_string(),
        c => c.to_string(),
    };
    let fam_esc = fam.replace('"', "").replace('\'', "");
    let (x, anchor) = match lay.align {
        TextAlign::Left => (10.0, "start"),
        TextAlign::Center => (3000.0, "middle"),
        TextAlign::Right => (5990.0, "end"),
    };
    let mut idx = 0usize;
    let mut lines = String::new();
    let n_lines = text.lines().count().max(1);
    for (i, l) in text.lines().enumerate() {
        let chars: Vec<char> = l.chars().collect();
        let dx: Vec<String> = chars
            .iter()
            .enumerate()
            .map(|(k, _)| {
                let v = if k == 0 { 0.0 } else { lay.kern_px.get(idx + k).copied().unwrap_or(0.0) };
                format!("{v:.2}")
            })
            .collect();
        idx += chars.len();
        let body: String = if chars.is_empty() { " ".into() } else { chars.iter().map(|c| esc(*c)).collect() };
        lines.push_str(&format!(
            "<tspan x='{x}' y='{:.1}' dx='{}'>{}</tspan>",
            300.0 + i as f64 * 240.0 * lay.line,
            dx.join(" "),
            body
        ));
    }
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='6000' height='{:.0}'>\
         <text xml:space='preserve' font-family=\"'{fam_esc}'\" font-size='200' text-anchor='{anchor}' \
         letter-spacing='{:.2}' word-spacing='{:.2}' fill='#{:02x}{:02x}{:02x}'>{lines}</text></svg>",
        400.0 + 240.0 * lay.line * n_lines as f64,
        lay.letter_px,
        lay.word_px,
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
    if id.contains("sf-miter") {
        opts.corner = Corner::Miter;
    } else if id.contains("sf-round-corner") {
        opts.corner = Corner::Round;
    } else if id.contains("sf-square-corner") {
        opts.corner = Corner::Square;
    }
    for (tag, cap) in [
        ("sf-cap-flat", Cap::Flat),
        ("sf-cap-square", Cap::Square),
        ("sf-cap-round", Cap::Round),
        ("sf-cap-point", Cap::Point),
    ] {
        if id.contains(tag) {
            opts.cap = cap;
        }
    }
    for (tag, f) in [
        ("sf-contour", FillPattern::Contour),
        ("sf-spiral", FillPattern::Spiral),
        ("sf-radial", FillPattern::Radial),
        ("sf-tatami", FillPattern::Tatami),
    ] {
        if id.contains(tag) {
            opts.fill = f;
            if opts.stitch == StitchChoice::Auto {
                opts.stitch = StitchChoice::Fill;
            }
        }
    }
    for w in crate::thread::ThreadWeight::ALL {
        if id.contains(&format!("sf-{}wt", w.number())) {
            opts.weight = Some(w);
        }
    }
    if id.contains("sf-puff") {
        opts.puff = true;
        if opts.stitch == StitchChoice::Auto {
            opts.stitch = StitchChoice::Satin;
        }
    }
    if id.contains("sf-no-underlay") {
        opts.no_underlay = true;
    }
    if id.contains("sf-keep-under") {
        opts.keep_under = true;
    }
    let num_after = |key: &str| -> Option<f64> {
        let i = id.find(key)?;
        let num: String = id[i + key.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        num.trim_end_matches('.').parse::<f64>().ok()
    };
    if let Some(w) = num_after("sf-width-") {
        opts.column_width = Some(w);
    }
    if let Some(v) = num_after("sf-density-") {
        opts.density = Some(v);
    }
    if let Some(v) = num_after("sf-len-") {
        opts.stitch_len = Some(v);
    }
    if let Some(v) = num_after("sf-pull-") {
        opts.pull_comp = Some(v);
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
                let words: String = t
                    .chunks()
                    .iter()
                    .map(|c| c.text())
                    .collect::<Vec<_>>()
                    .join(" ");
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

    // Keep every subpath as an editable vector path (anchors + handles),
    // then flatten each into a polyline for stitching.
    let mut vps: Vec<VPath> = Vec::new();
    let mut cur: Option<VPath> = None;
    let flush = |cur: &mut Option<VPath>, vps: &mut Vec<VPath>| {
        if let Some(v) = cur.take() {
            if v.nodes.len() >= 2 || (v.closed && v.nodes.len() >= 2) {
                vps.push(v);
            }
        }
    };
    for seg in p.data().segments() {
        match seg {
            PathSegment::MoveTo(a) => {
                flush(&mut cur, &mut vps);
                cur = Some(VPath {
                    nodes: vec![Node::corner(map(a.x, a.y))],
                    closed: false,
                });
            }
            PathSegment::LineTo(a) => {
                if let Some(v) = cur.as_mut() {
                    v.nodes.push(Node::corner(map(a.x, a.y)));
                }
            }
            PathSegment::QuadTo(c, a) => {
                if let Some(v) = cur.as_mut() {
                    let (c, a) = (map(c.x, c.y), map(a.x, a.y));
                    let last = v.nodes.last_mut().unwrap();
                    last.cout = last.p.lerp(c, 2.0 / 3.0);
                    v.nodes.push(Node {
                        p: a,
                        cin: a.lerp(c, 2.0 / 3.0),
                        cout: a,
                    });
                }
            }
            PathSegment::CubicTo(c1, c2, a) => {
                if let Some(v) = cur.as_mut() {
                    let a = map(a.x, a.y);
                    v.nodes.last_mut().unwrap().cout = map(c1.x, c1.y);
                    v.nodes.push(Node {
                        p: a,
                        cin: map(c2.x, c2.y),
                        cout: a,
                    });
                }
            }
            PathSegment::Close => {
                if let Some(v) = cur.as_mut() {
                    v.closed = true;
                    // an explicit line back to the start is the closing edge
                    if v.nodes.len() > 1 && v.nodes[0].p.dist(v.nodes.last().unwrap().p) < 1e-6 {
                        let last = v.nodes.pop().unwrap();
                        v.nodes[0].cin = last.cin;
                    }
                    let start = v.nodes[0].p;
                    flush(&mut cur, &mut vps);
                    // a path may carry on from the start point after closing
                    cur = Some(VPath {
                        nodes: vec![Node::corner(start)],
                        closed: false,
                    });
                }
            }
        }
    }
    flush(&mut cur, &mut vps);
    let mut subs: Vec<(Vec<Pt>, bool)> = vps.iter().map(|v| (v.flatten(), v.closed)).collect();

    // Drop duplicate consecutive points.
    for (s, _) in subs.iter_mut() {
        s.dedup_by(|a, b| a.dist(*b) < 1e-6);
    }

    if let Some(fill) = p.fill() {
        let rule = match fill.rule() {
            usvg::FillRule::NonZero => FillRule::NonZero,
            usvg::FillRule::EvenOdd => FillRule::EvenOdd,
        };
        let mut shape = Shape {
            kind: ShapeKind::Area {
                rings: Vec::new(),
                rule,
            },
            rgb: paint_rgb(fill.paint()),
            label: p.id().to_string(),
            group,
            opts: ShapeOpts::default(),
            curves: vps.clone(),
        };
        shape.reflatten();
        // keep only the paths that became rings, so the two stay paired
        shape.curves.retain(|c| c.flatten().len() >= 3 + usize::from(c.closed));
        shape.reflatten();
        if matches!(&shape.kind, ShapeKind::Area { rings, .. } if !rings.is_empty()) {
            art.shapes.push(shape);
        }
    }
    if let Some(stroke) = p.stroke() {
        let width = stroke.width().get() as f64 * scale * PX_TO_MM;
        let paths: Vec<Vec<Pt>> = subs.iter().map(|(s, _)| s.clone()).collect();
        if !paths.is_empty() {
            art.shapes.push(Shape {
                curves: vps.clone(),
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
