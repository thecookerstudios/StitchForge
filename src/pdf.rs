//! PDF and Illustrator (.ai, PDF-compatible) artwork: reads the vector paths
//! of the first page with their fill and stroke colours (RGB, CMYK, gray and
//! spot/Pantone colours) and turns them into SVG for the normal import.
//! When the page only holds a picture, that picture is handed back for the
//! auto-tracer instead.

use lopdf::{Dictionary, Document, Object, content::Content};

pub struct PdfArt {
    /// Vector art as SVG (mm), if the page has any.
    pub svg: Option<String>,
    /// The biggest picture on the page, if there is no vector art.
    pub picture: Option<crate::trace::Raster>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy)]
struct M([f64; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    /// `self` first, then `n`.
    fn then(&self, n: &M) -> M {
        let (a, b) = (self.0, n.0);
        M([
            a[0] * b[0] + a[1] * b[2],
            a[0] * b[1] + a[1] * b[3],
            a[2] * b[0] + a[3] * b[2],
            a[2] * b[1] + a[3] * b[3],
            a[4] * b[0] + a[5] * b[2] + b[4],
            a[4] * b[1] + a[5] * b[3] + b[5],
        ])
    }
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = self.0;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }
    fn scale(&self) -> f64 {
        let m = self.0;
        (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
    }
}

#[derive(Clone)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Spot colour: name and its colour at full tint.
    Spot(String, [f64; 3]),
    Other,
}

#[derive(Clone)]
struct Gs {
    ctm: M,
    fill: [f64; 3],
    stroke: [f64; 3],
    fill_name: Option<String>,
    stroke_name: Option<String>,
    fill_space: Space,
    stroke_space: Space,
    width: f64,
}

/// One painted path, in page points (y up).
struct Painted {
    d: Vec<Seg>,
    fill: Option<([u8; 3], bool, Option<String>)>,
    stroke: Option<([u8; 3], f64, Option<String>)>,
    bbox: [f64; 4],
}

#[derive(Clone, Copy)]
enum Seg {
    M(f64, f64),
    L(f64, f64),
    C(f64, f64, f64, f64, f64, f64),
    Z,
}

fn num(o: &Object) -> f64 {
    match o {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => *r as f64,
        _ => 0.0,
    }
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().map(num).collect()
}

fn rgb8(c: [f64; 3]) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn cmyk(c: f64, m: f64, y: f64, k: f64) -> [f64; 3] {
    [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

/// Colour from operands in a colour space.
fn color_in(space: &Space, v: &[f64]) -> [f64; 3] {
    match (space, v.len()) {
        (Space::Spot(_, full), _) => {
            let t = v.first().copied().unwrap_or(1.0);
            [0, 1, 2].map(|i| 1.0 - t * (1.0 - full[i]))
        }
        (_, 1) => [v[0]; 3],
        (_, 3) => [v[0], v[1], v[2]],
        (_, 4) => cmyk(v[0], v[1], v[2], v[3]),
        _ => [0.3; 3],
    }
}

/// Read a colour space by name from the page resources.
fn space_named(doc: &Document, res: &Dictionary, name: &[u8]) -> Space {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => return Space::Gray,
        b"DeviceRGB" | b"RGB" | b"CalRGB" => return Space::Rgb,
        b"DeviceCMYK" | b"CMYK" => return Space::Cmyk,
        _ => {}
    }
    let Some(cs) = res
        .get(b"ColorSpace")
        .ok()
        .map(|o| deref(doc, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(name).ok())
        .map(|o| deref(doc, o))
    else {
        return Space::Other;
    };
    space_of(doc, cs)
}

fn space_of(doc: &Document, cs: &Object) -> Space {
    if let Ok(n) = cs.as_name() {
        return match n {
            b"DeviceGray" => Space::Gray,
            b"DeviceRGB" => Space::Rgb,
            b"DeviceCMYK" => Space::Cmyk,
            _ => Space::Other,
        };
    }
    let Ok(arr) = cs.as_array() else { return Space::Other };
    let kind = arr.first().and_then(|o| o.as_name().ok()).unwrap_or(b"");
    match kind {
        b"ICCBased" => {
            let n = arr
                .get(1)
                .map(|o| deref(doc, o))
                .and_then(|o| o.as_stream().ok())
                .and_then(|s| s.dict.get(b"N").ok())
                .map(num)
                .unwrap_or(3.0) as usize;
            match n {
                1 => Space::Gray,
                4 => Space::Cmyk,
                _ => Space::Rgb,
            }
        }
        b"Separation" | b"DeviceN" => {
            let name = match arr.get(1).map(|o| deref(doc, o)) {
                Some(Object::Name(n)) => String::from_utf8_lossy(n).to_string(),
                Some(Object::Array(a)) => a
                    .iter()
                    .filter_map(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).to_string())
                    .collect::<Vec<_>>()
                    .join("+"),
                _ => "Spot".into(),
            };
            let alt = arr.get(2).map(|o| deref(doc, o));
            let alt_space = alt.map(|a| space_of(doc, a)).unwrap_or(Space::Cmyk);
            // tint transform at full tint: type 2 functions are exact; others
            // fall back to a mid gray and get flagged
            let full = arr
                .get(3)
                .map(|o| deref(doc, o))
                .and_then(|f| {
                    let d = match f {
                        Object::Dictionary(d) => d,
                        Object::Stream(s) => &s.dict,
                        _ => return None,
                    };
                    let ty = d.get(b"FunctionType").map(num).unwrap_or(-1.0) as i32;
                    if ty == 2 {
                        let c1 = d.get(b"C1").ok().and_then(|o| o.as_array().ok()).map(|a| nums(a)).unwrap_or(vec![1.0]);
                        Some(color_in(&alt_space, &c1))
                    } else if ty == 0 || ty == 4 {
                        sample_function(d, f, &alt_space)
                    } else {
                        None
                    }
                })
                .unwrap_or([0.45; 3]);
            let name = name.replace('#', " ");
            if name == "All" || name == "None" {
                return Space::Gray;
            }
            Space::Spot(name, full)
        }
        b"Indexed" | b"Pattern" => Space::Other,
        b"Lab" => Space::Rgb,
        _ => Space::Other,
    }
}

/// Sampled (type 0) tint functions: read the sample for full tint. Type 4
/// (PostScript) is not evaluated.
fn sample_function(d: &Dictionary, f: &Object, alt: &Space) -> Option<[f64; 3]> {
    let s = f.as_stream().ok()?;
    if d.get(b"FunctionType").map(num).ok()? as i32 != 0 {
        return None;
    }
    let data = s.decompressed_content().ok()?;
    let bps = d.get(b"BitsPerSample").map(num).ok()? as usize;
    let size = d.get(b"Size").ok()?.as_array().ok()?.first().map(num)? as usize;
    let n_out = d.get(b"Range").ok()?.as_array().ok()?.len() / 2;
    if bps != 8 || size == 0 || n_out == 0 {
        return None;
    }
    let off = (size - 1) * n_out;
    let v: Vec<f64> = (0..n_out).map(|i| *data.get(off + i).unwrap_or(&0) as f64 / 255.0).collect();
    Some(color_in(alt, &v))
}

struct Reader<'a> {
    doc: &'a Document,
    painted: Vec<Painted>,
    images: Vec<(f64, crate::trace::Raster)>,
    text_runs: usize,
    shadings: usize,
    unknown_colors: bool,
    depth: usize,
}

impl<'a> Reader<'a> {
    fn run(&mut self, content: &[u8], res: &Dictionary, gs0: Gs) {
        if self.depth > 8 {
            return;
        }
        let Ok(ops) = Content::decode(content) else { return };
        let mut gs = gs0;
        let mut stack: Vec<Gs> = Vec::new();
        let mut path: Vec<Seg> = Vec::new();
        let mut cur = (0.0f64, 0.0f64);
        let mut start = (0.0f64, 0.0f64);
        for op in &ops.operations {
            let v = nums(&op.operands);
            let get = |i: usize| v.get(i).copied().unwrap_or(0.0);
            let tf = |gs: &Gs, x: f64, y: f64| gs.ctm.apply(x, y);
            match op.operator.as_str() {
                "q" => stack.push(gs.clone()),
                "Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                "cm" if v.len() == 6 => {
                    gs.ctm = M([v[0], v[1], v[2], v[3], v[4], v[5]]).then(&gs.ctm);
                }
                "w" => gs.width = get(0),
                "m" => {
                    cur = (get(0), get(1));
                    start = cur;
                    let p = tf(&gs, cur.0, cur.1);
                    path.push(Seg::M(p.0, p.1));
                }
                "l" => {
                    cur = (get(0), get(1));
                    let p = tf(&gs, cur.0, cur.1);
                    path.push(Seg::L(p.0, p.1));
                }
                "c" | "v" | "y" => {
                    let (c1, c2, e) = match op.operator.as_str() {
                        "c" => ((get(0), get(1)), (get(2), get(3)), (get(4), get(5))),
                        "v" => (cur, (get(0), get(1)), (get(2), get(3))),
                        _ => ((get(0), get(1)), (get(2), get(3)), (get(2), get(3))),
                    };
                    let (a, b, c) = (tf(&gs, c1.0, c1.1), tf(&gs, c2.0, c2.1), tf(&gs, e.0, e.1));
                    path.push(Seg::C(a.0, a.1, b.0, b.1, c.0, c.1));
                    cur = e;
                }
                "h" => {
                    path.push(Seg::Z);
                    cur = start;
                }
                "re" => {
                    let (x, y, w, h) = (get(0), get(1), get(2), get(3));
                    let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
                    for (i, (px, py)) in pts.iter().enumerate() {
                        let p = tf(&gs, *px, *py);
                        path.push(if i == 0 { Seg::M(p.0, p.1) } else { Seg::L(p.0, p.1) });
                    }
                    path.push(Seg::Z);
                    cur = (x, y);
                    start = cur;
                }
                "f" | "F" | "f*" | "S" | "s" | "B" | "B*" | "b" | "b*" | "n" => {
                    let o = op.operator.as_str();
                    if o == "s" || o == "b" || o == "b*" {
                        path.push(Seg::Z);
                    }
                    let fill = matches!(o, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                    let stroke = matches!(o, "S" | "s" | "B" | "B*" | "b" | "b*");
                    let even = o.ends_with('*');
                    if (fill || stroke) && !path.is_empty() {
                        let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                        for s in &path {
                            let pts: &[(f64, f64)] = &match *s {
                                Seg::M(x, y) | Seg::L(x, y) => vec![(x, y)],
                                Seg::C(a, b, c, d, e, f) => vec![(a, b), (c, d), (e, f)],
                                Seg::Z => vec![],
                            };
                            for &(x, y) in pts {
                                bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x), bb[3].max(y)];
                            }
                        }
                        self.painted.push(Painted {
                            d: std::mem::take(&mut path),
                            fill: fill.then(|| (rgb8(gs.fill), even, gs.fill_name.clone())),
                            stroke: stroke.then(|| (rgb8(gs.stroke), gs.width * gs.ctm.scale(), gs.stroke_name.clone())),
                            bbox: bb,
                        });
                    }
                    path.clear();
                }
                "W" | "W*" => {}
                "g" => {
                    gs.fill = [get(0); 3];
                    gs.fill_space = Space::Gray;
                    gs.fill_name = None;
                }
                "G" => {
                    gs.stroke = [get(0); 3];
                    gs.stroke_space = Space::Gray;
                    gs.stroke_name = None;
                }
                "rg" => {
                    gs.fill = [get(0), get(1), get(2)];
                    gs.fill_space = Space::Rgb;
                    gs.fill_name = None;
                }
                "RG" => {
                    gs.stroke = [get(0), get(1), get(2)];
                    gs.stroke_space = Space::Rgb;
                    gs.stroke_name = None;
                }
                "k" => {
                    gs.fill = cmyk(get(0), get(1), get(2), get(3));
                    gs.fill_space = Space::Cmyk;
                    gs.fill_name = None;
                }
                "K" => {
                    gs.stroke = cmyk(get(0), get(1), get(2), get(3));
                    gs.stroke_space = Space::Cmyk;
                    gs.stroke_name = None;
                }
                "cs" | "CS" => {
                    let name = op.operands.first().and_then(|o| o.as_name().ok()).unwrap_or(b"");
                    let sp = space_named(self.doc, res, name);
                    let (name_slot, space_slot, col) = if op.operator == "cs" {
                        (&mut gs.fill_name, &mut gs.fill_space, &mut gs.fill)
                    } else {
                        (&mut gs.stroke_name, &mut gs.stroke_space, &mut gs.stroke)
                    };
                    *name_slot = match &sp {
                        Space::Spot(n, _) => Some(n.clone()),
                        _ => None,
                    };
                    *col = match &sp {
                        Space::Spot(_, full) => *full,
                        Space::Rgb | Space::Gray => [0.0; 3],
                        Space::Cmyk => [0.0; 3],
                        Space::Other => [0.3; 3],
                    };
                    *space_slot = sp;
                }
                "sc" | "scn" | "SC" | "SCN" => {
                    let fill = op.operator.starts_with('s');
                    let space = if fill { gs.fill_space.clone() } else { gs.stroke_space.clone() };
                    let c = match &space {
                        Space::Other => {
                            self.unknown_colors = true;
                            [0.4; 3]
                        }
                        sp => color_in(sp, &v),
                    };
                    if fill {
                        gs.fill = c;
                    } else {
                        gs.stroke = c;
                    }
                }
                "Tj" | "TJ" | "'" | "\"" => self.text_runs += 1,
                "sh" => self.shadings += 1,
                "Do" => {
                    let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else { continue };
                    let Some(xo) = res
                        .get(b"XObject")
                        .ok()
                        .map(|o| deref(self.doc, o))
                        .and_then(|o| o.as_dict().ok())
                        .and_then(|d| d.get(name).ok())
                        .map(|o| deref(self.doc, o))
                        .and_then(|o| o.as_stream().ok())
                    else {
                        continue;
                    };
                    let sub = xo.dict.get(b"Subtype").and_then(|o| o.as_name()).unwrap_or(b"");
                    if sub == b"Form" {
                        let m = xo
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|o| o.as_array().ok())
                            .map(|a| nums(a))
                            .filter(|a| a.len() == 6)
                            .map(|a| M([a[0], a[1], a[2], a[3], a[4], a[5]]))
                            .unwrap_or(M::ID);
                        let sub_res = xo
                            .dict
                            .get(b"Resources")
                            .ok()
                            .map(|o| deref(self.doc, o))
                            .and_then(|o| o.as_dict().ok())
                            .cloned()
                            .unwrap_or_else(|| res.clone());
                        if let Ok(data) = xo.decompressed_content() {
                            let mut g = gs.clone();
                            g.ctm = m.then(&gs.ctm);
                            self.depth += 1;
                            self.run(&data, &sub_res, g);
                            self.depth -= 1;
                        }
                    } else if sub == b"Image" {
                        let area = {
                            let (a, b, c) = (gs.ctm.apply(0.0, 0.0), gs.ctm.apply(1.0, 0.0), gs.ctm.apply(0.0, 1.0));
                            ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs()
                        };
                        if let Some(r) = decode_image(self.doc, xo) {
                            self.images.push((area, r));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn decode_image(doc: &Document, s: &lopdf::Stream) -> Option<crate::trace::Raster> {
    let d = &s.dict;
    let filters: Vec<Vec<u8>> = match d.get(b"Filter").ok().map(|o| deref(doc, o)) {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(|n| n.to_vec())).collect(),
        _ => Vec::new(),
    };
    if filters.iter().any(|f| f == b"DCTDecode" || f == b"JPXDecode") {
        // JPEG data as is
        return crate::trace::Raster::decode(&s.content).ok();
    }
    let w = d.get(b"Width").map(num).ok()? as u32;
    let h = d.get(b"Height").map(num).ok()? as u32;
    let bpc = d.get(b"BitsPerComponent").map(num).unwrap_or(8.0) as u32;
    if bpc != 8 || w < 8 || h < 8 {
        return None;
    }
    let space = d.get(b"ColorSpace").ok().map(|o| space_of(doc, deref(doc, o))).unwrap_or(Space::Rgb);
    let nc = match space {
        Space::Gray | Space::Spot(..) => 1,
        Space::Cmyk => 4,
        Space::Rgb => 3,
        Space::Other => return None,
    };
    let data = s.decompressed_content().ok()?;
    if data.len() < (w * h * nc) as usize {
        return None;
    }
    let mut px = Vec::with_capacity((w * h) as usize);
    for i in 0..(w * h) as usize {
        let p = &data[i * nc as usize..(i + 1) * nc as usize];
        let c = match nc {
            1 => [p[0]; 3],
            3 => [p[0], p[1], p[2]],
            _ => rgb8(cmyk(p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0, p[3] as f64 / 255.0)),
        };
        px.push([c[0], c[1], c[2], 255]);
    }
    // soft mask (transparency)
    if let Some(sm) = d.get(b"SMask").ok().map(|o| deref(doc, o)).and_then(|o| o.as_stream().ok()) {
        let sw = sm.dict.get(b"Width").map(num).unwrap_or(0.0) as u32;
        let sh = sm.dict.get(b"Height").map(num).unwrap_or(0.0) as u32;
        if sw == w && sh == h {
            if let Ok(a) = sm.decompressed_content() {
                if a.len() >= (w * h) as usize {
                    for (i, p) in px.iter_mut().enumerate() {
                        p[3] = a[i];
                    }
                }
            }
        }
    }
    Some(crate::trace::Raster { w, h, px })
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Read the first page of a PDF (or PDF-compatible .ai file).
pub fn read(bytes: &[u8]) -> Result<PdfArt, String> {
    let doc = Document::load_mem(bytes).map_err(|e| format!("can't read the PDF: {e}"))?;
    let pages = doc.get_pages();
    let (_, &page) = pages.iter().next().ok_or("the PDF has no pages")?;
    let page_dict = doc.get_dictionary(page).map_err(|e| e.to_string())?;
    // page box (may be inherited from the page tree)
    let mut boxv = vec![0.0, 0.0, 612.0, 792.0];
    let mut node = Some(page_dict);
    while let Some(d) = node {
        let found = d
            .get(b"CropBox")
            .or_else(|_| d.get(b"MediaBox"))
            .ok()
            .map(|o| deref(&doc, o))
            .and_then(|o| o.as_array().ok())
            .map(|a| nums(a));
        if let Some(b) = found.filter(|b| b.len() == 4) {
            boxv = b;
            break;
        }
        node = d.get(b"Parent").ok().and_then(|p| p.as_reference().ok()).and_then(|id| doc.get_dictionary(id).ok());
    }
    let (x0, y0, x1, y1) = (boxv[0].min(boxv[2]), boxv[1].min(boxv[3]), boxv[0].max(boxv[2]), boxv[1].max(boxv[3]));
    let res = match doc.get_page_resources(page) {
        Ok((Some(d), _)) => d.clone(),
        Ok((None, ids)) => ids.first().and_then(|id| doc.get_dictionary(*id).ok()).cloned().unwrap_or_default(),
        _ => Dictionary::new(),
    };
    let content = doc.get_page_content(page);
    let mut r = Reader {
        doc: &doc,
        painted: Vec::new(),
        images: Vec::new(),
        text_runs: 0,
        shadings: 0,
        unknown_colors: false,
        depth: 0,
    };
    let gs = Gs {
        ctm: M::ID,
        fill: [0.0; 3],
        stroke: [0.0; 3],
        fill_name: None,
        stroke_name: None,
        fill_space: Space::Gray,
        stroke_space: Space::Gray,
        width: 1.0,
    };
    r.run(&content, &res, gs);

    let mut notes = Vec::new();
    // page-sized backgrounds aren't part of the logo
    let (pw, ph) = (x1 - x0, y1 - y0);
    let before = r.painted.len();
    let mut dropped_bg = false;
    let mut idx = 0;
    r.painted.retain(|p| {
        idx += 1;
        let big = (p.bbox[2] - p.bbox[0]) >= pw * 0.9 && (p.bbox[3] - p.bbox[1]) >= ph * 0.9;
        let keep = !(big && p.stroke.is_none() && idx <= 3);
        if !keep {
            dropped_bg = true;
        }
        keep
    });
    if dropped_bg {
        notes.push("A page-sized background rectangle was left out".to_string());
    }
    let _ = before;
    if r.text_runs > 0 {
        notes.push(
            "The PDF has live (editable) text that isn't included: convert text to outlines in the art app, or retype it with the Text tool"
                .to_string(),
        );
    }
    if r.shadings > 0 {
        notes.push("Gradients in the PDF were skipped: pick a thread colour for those areas".into());
    }
    if r.unknown_colors {
        notes.push("Some colours use patterns or a colour space StitchForge can't read; they show as gray".into());
    }
    let has_vector = r.painted.iter().any(|p| p.bbox[2] - p.bbox[0] > 0.5 || p.bbox[3] - p.bbox[1] > 0.5);
    if !has_vector {
        let picture = r
            .images
            .into_iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(_, img)| img);
        return Ok(PdfArt { svg: None, picture, notes });
    }
    if !r.images.is_empty() {
        notes.push("The PDF also holds pictures; only its vector art was used".into());
    }
    // to SVG in mm, y down; art bounding box only
    let k = 25.4 / 72.0;
    let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in &r.painted {
        let pad = p.stroke.as_ref().map(|s| s.1 / 2.0).unwrap_or(0.0);
        bb = [bb[0].min(p.bbox[0] - pad), bb[1].min(p.bbox[1] - pad), bb[2].max(p.bbox[2] + pad), bb[3].max(p.bbox[3] + pad)];
    }
    let (ox, oy) = (bb[0], bb[3]);
    let tx = |x: f64| (x - ox) * k;
    let ty = |y: f64| (oy - y) * k;
    let (w, h) = ((bb[2] - bb[0]) * k, (bb[3] - bb[1]) * k);
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w:.3}mm' height='{h:.3}mm' viewBox='0 0 {w:.3} {h:.3}'>\n"
    );
    // consecutive paths with the same paint become one element (one layer)
    let mut groups: Vec<(String, String, String)> = Vec::new(); // (style key, attrs, d)
    let mut used: std::collections::HashMap<String, usize> = Default::default();
    for p in &r.painted {
        let mut d = String::new();
        for s in &p.d {
            match *s {
                Seg::M(x, y) => d.push_str(&format!("M{:.3} {:.3}", tx(x), ty(y))),
                Seg::L(x, y) => d.push_str(&format!("L{:.3} {:.3}", tx(x), ty(y))),
                Seg::C(a, b, c, e, f, g) => {
                    d.push_str(&format!("C{:.3} {:.3} {:.3} {:.3} {:.3} {:.3}", tx(a), ty(b), tx(c), ty(e), tx(f), ty(g)))
                }
                Seg::Z => d.push('Z'),
            }
        }
        let fill = p.fill.as_ref();
        let stroke = p.stroke.as_ref();
        let key = format!(
            "{:?}|{:?}",
            fill.map(|f| (f.0, f.1)),
            stroke.map(|s| (s.0, (s.1 * 100.0).round() as i64))
        );
        let base = fill
            .and_then(|f| f.2.clone())
            .or_else(|| stroke.and_then(|s| s.2.clone()))
            .unwrap_or_else(|| {
                let c = fill.map(|f| f.0).or(stroke.map(|s| s.0)).unwrap_or([0; 3]);
                let chart = crate::thread::generic_chart();
                chart[crate::thread::nearest(&chart, c).0].name.clone()
            });
        let mut attrs = String::new();
        match fill {
            Some((c, even, _)) => attrs.push_str(&format!(
                " fill='{}' fill-rule='{}'",
                hex(*c),
                if *even { "evenodd" } else { "nonzero" }
            )),
            None => attrs.push_str(" fill='none'"),
        }
        if let Some((c, wd, _)) = stroke {
            attrs.push_str(&format!(
                " stroke='{}' stroke-width='{:.3}' stroke-linejoin='round'",
                hex(*c),
                (wd * k).max(0.1)
            ));
        }
        match groups.last_mut() {
            Some(g) if g.0 == key => {
                g.2.push(' ');
                g.2.push_str(&d);
            }
            _ => {
                let n = used.entry(base.clone()).or_insert(0);
                *n += 1;
                let label = if *n > 1 { format!("{base} {n}") } else { base };
                groups.push((key, format!("{label}\u{1}{attrs}"), d));
            }
        }
    }
    let mut ids = std::collections::HashSet::new();
    for (_, la, d) in &groups {
        let (label, attrs) = la.split_once('\u{1}').unwrap();
        let mut id: String = label.chars().map(|c| if c.is_alphanumeric() || " -()".contains(c) { c } else { '-' }).collect();
        while !ids.insert(id.clone()) {
            id.push('-');
        }
        svg.push_str(&format!("  <path id='{id}'{attrs} d='{d}'/>\n"));
    }
    svg.push_str("</svg>\n");
    Ok(PdfArt { svg: Some(svg), picture: None, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny hand-made PDF: white page background, a navy CMYK square, a
    /// Pantone (spot) circle-ish curve shape and a stroked line.
    pub fn sample_pdf() -> Vec<u8> {
        use lopdf::dictionary;
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let tint = doc.add_object(dictionary! {
            "FunctionType" => 2, "Domain" => vec![0.into(), 1.into()],
            "C0" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "C1" => vec![Object::Real(1.0), Object::Real(0.6), Object::Real(0.0), Object::Real(0.1)],
            "N" => 1,
        });
        let spot = Object::Array(vec![
            Object::Name(b"Separation".to_vec()),
            Object::Name(b"PANTONE 286 C".to_vec()),
            Object::Name(b"DeviceCMYK".to_vec()),
            Object::Reference(tint),
        ]);
        let resources = doc.add_object(dictionary! { "ColorSpace" => dictionary! { "CS0" => spot } });
        let content = b"1 g 0 0 300 200 re f\n\
            1 0.8 0 0.6 k 20 20 100 100 re f\n\
            /CS0 cs 1 scn 200 70 m 200 98 178 120 150 120 c 122 120 100 98 100 70 c 100 42 122 20 150 20 c 178 20 200 42 200 70 c h f\n\
            0 G 4 w 20 150 m 280 150 l S\n";
        let cid = doc.add_object(lopdf::Stream::new(dictionary! {}, content.to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => cid,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
            "Resources" => resources,
        });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
        }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn reads_vector_pdf() {
        let art = read(&sample_pdf()).unwrap();
        let svg = art.svg.expect("vector art");
        // background rect dropped, three elements kept
        assert!(art.notes.iter().any(|n| n.contains("background")), "{:?}", art.notes);
        assert_eq!(svg.matches("<path").count(), 3, "{svg}");
        assert!(svg.contains("id='PANTONE 286 C'"), "{svg}");
        // CMYK navy
        assert!(svg.contains("fill='#000a1a'") || svg.contains("#00"), "{svg}");
        let (opt, _) = crate::import::font_options(None).unwrap();
        let a = crate::import::load_svg_str(&svg, &opt).unwrap();
        assert_eq!(a.shapes.len(), 3);
        let b = crate::geom::BBox::of(a.shapes.iter().flat_map(|s| s.points()));
        // art spans x 20..280 pt = 260 pt ≈ 91.7 mm
        assert!((b.width() - 91.7).abs() < 2.5, "{}", b.width());
    }
}
