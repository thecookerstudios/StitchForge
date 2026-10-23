//! The operator spec sheet as a PDF: thread colours print exactly as shown
//! (no browser "background graphics" setting to forget), page 1 is what the
//! operator needs at the machine, and the object-by-object stitch list is
//! on page 2 when it is wanted.
//!
//! Written directly (no PDF library): US Letter pages, the built-in
//! Helvetica fonts, and the stitch preview as a compressed image.

use crate::design::Design;
use crate::sheet::JobInfo;

const W: f64 = 612.0; // US Letter, points
const H: f64 = 792.0;
const M: f64 = 36.0; // margin (0.5")
const PT_PER_MM: f64 = 72.0 / 25.4;

#[derive(Clone, Copy, PartialEq)]
enum Font {
    Regular,
    Bold,
}

/// Advance widths (1/1000 em) of Helvetica and Helvetica-Bold, ASCII 32–126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELV_B: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Text as WinAnsi bytes (what the built-in fonts understand).
fn win_ansi(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.chars() {
        match c {
            ' '..='~' => out.push(c as u8),
            '\u{a0}'..='\u{ff}' => out.push(c as u32 as u8),
            '—' => out.push(0x97),
            '–' => out.push(0x96),
            '•' => out.push(0x95),
            '‘' => out.push(0x91),
            '’' => out.push(0x92),
            '“' => out.push(0x93),
            '”' => out.push(0x94),
            '″' => out.push(b'"'),
            '′' => out.push(b'\''),
            '→' => out.extend_from_slice(b"->"),
            '←' => out.extend_from_slice(b"<-"),
            '…' => out.push(0x85),
            'Δ' => out.push(b'd'),
            '\n' | '\t' => out.push(b' '),
            _ => out.push(b'?'),
        }
    }
    out
}

fn char_w(b: u8, f: Font) -> f64 {
    let t = if f == Font::Bold { &HELV_B } else { &HELV };
    match b {
        32..=126 => t[(b - 32) as usize] as f64,
        0x97 => 1000.0,
        0x96 | 0x95 => 556.0,
        0xd7 => 584.0,
        0xb0 => 400.0,
        0xb7 => 278.0,
        _ => 556.0,
    }
}

fn text_w(s: &str, size: f64, f: Font) -> f64 {
    win_ansi(s).iter().map(|b| char_w(*b, f)).sum::<f64>() * size / 1000.0
}

/// Break text into lines no wider than `width`.
fn wrap(s: &str, size: f64, f: Font, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if text_w(&trial, size, f) <= width || line.is_empty() {
                line = trial;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        lines.push(line);
    }
    lines
}

fn esc_pdf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len() + 2);
    for &c in b {
        if c == b'(' || c == b')' || c == b'\\' {
            out.push(b'\\');
        }
        out.push(c);
    }
    out
}

struct Page {
    ops: Vec<u8>,
    image: bool,
}

/// A page being drawn, with y measured down from the top edge.
struct Canvas {
    pages: Vec<Page>,
    y: f64,
}

type Rgb = [u8; 3];
const INK: Rgb = [29, 29, 27];
const MUTED: Rgb = [107, 106, 101];
const RULE: Rgb = [217, 214, 207];
const WARN: Rgb = [154, 91, 0];
const RED: Rgb = [180, 35, 26];

fn c(v: u8) -> f64 {
    v as f64 / 255.0
}

impl Canvas {
    fn new() -> Self {
        Canvas { pages: vec![Page { ops: Vec::new(), image: false }], y: M }
    }
    fn ops(&mut self) -> &mut Vec<u8> {
        &mut self.pages.last_mut().unwrap().ops
    }
    fn new_page(&mut self) {
        self.pages.push(Page { ops: Vec::new(), image: false });
        self.y = M;
    }
    /// Start a new page if `need` points won't fit.
    fn room(&mut self, need: f64) -> bool {
        if self.y + need > H - M - 18.0 {
            self.new_page();
            true
        } else {
            false
        }
    }
    fn text(&mut self, x: f64, top: f64, size: f64, f: Font, rgb: Rgb, s: &str) {
        let font = if f == Font::Bold { "F2" } else { "F1" };
        let base = H - top - size * 0.8;
        let mut o = format!(
            "BT /{font} {size:.2} Tf {:.3} {:.3} {:.3} rg {x:.2} {base:.2} Td (",
            c(rgb[0]), c(rgb[1]), c(rgb[2])
        )
        .into_bytes();
        o.extend(esc_pdf(&win_ansi(s)));
        o.extend_from_slice(b") Tj ET\n");
        self.ops().extend(o);
    }
    fn text_right(&mut self, right: f64, top: f64, size: f64, f: Font, rgb: Rgb, s: &str) {
        let w = text_w(s, size, f);
        self.text(right - w, top, size, f, rgb, s);
    }
    fn rect(&mut self, x: f64, top: f64, w: f64, h: f64, fill: Option<Rgb>, stroke: Option<(Rgb, f64)>) {
        let y = H - top - h;
        let mut o = String::new();
        if let Some(f) = fill {
            o += &format!("{:.3} {:.3} {:.3} rg ", c(f[0]), c(f[1]), c(f[2]));
        }
        if let Some((s, lw)) = stroke {
            o += &format!("{:.3} {:.3} {:.3} RG {lw:.2} w ", c(s[0]), c(s[1]), c(s[2]));
        }
        let op = match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B",
            (true, false) => "f",
            _ => "S",
        };
        o += &format!("{x:.2} {y:.2} {w:.2} {h:.2} re {op}\n");
        self.ops().extend(o.into_bytes());
    }
    fn hline(&mut self, x0: f64, x1: f64, top: f64, rgb: Rgb, lw: f64) {
        let y = H - top;
        let o = format!(
            "{:.3} {:.3} {:.3} RG {lw:.2} w {x0:.2} {y:.2} m {x1:.2} {y:.2} l S\n",
            c(rgb[0]), c(rgb[1]), c(rgb[2])
        );
        self.ops().extend(o.into_bytes());
    }
    fn image(&mut self, x: f64, top: f64, w: f64, h: f64) {
        let y = H - top - h;
        let o = format!("q {w:.2} 0 0 {h:.2} {x:.2} {y:.2} cm /Im1 Do Q\n");
        self.ops().extend(o.into_bytes());
        self.pages.last_mut().unwrap().image = true;
    }
    fn heading(&mut self, s: &str) {
        self.room(40.0);
        self.y += 10.0;
        self.text(M, self.y, 8.5, Font::Bold, MUTED, &s.to_uppercase());
        self.y += 16.0;
    }
}

/// One table column: title, width (points), right-aligned?
struct Col<'a>(&'a str, f64, bool);

fn table_header(cv: &mut Canvas, cols: &[Col]) {
    let mut x = M;
    for Col(t, w, right) in cols {
        if *right {
            cv.text_right(x + w - 4.0, cv.y, 7.5, Font::Bold, MUTED, t);
        } else {
            cv.text(x + 4.0, cv.y, 7.5, Font::Bold, MUTED, t);
        }
        x += w;
    }
    cv.y += 12.0;
    cv.hline(M, W - M, cv.y, INK, 1.0);
    cv.y += 4.0;
}

pub struct SheetPdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
}

/// Build the PDF operator sheet. `sequence_page`: add the stitch-by-object
/// list on its own page.
pub fn operator_sheet_pdf(d: &Design, job: &JobInfo, generated: &str, sequence_page: bool) -> Result<SheetPdf, String> {
    let st = &d.stats;
    let mut cv = Canvas::new();
    let inner = W - 2.0 * M;

    // ---- title
    let title = if job.job.is_empty() { d.name.clone() } else { format!("Job {} · {}", job.job, d.name) };
    cv.text(M, cv.y, 8.0, Font::Bold, MUTED, "OPERATOR SPEC SHEET");
    cv.y += 12.0;
    cv.text(M, cv.y, 18.0, Font::Bold, INK, &title);
    cv.y += 26.0;
    cv.hline(M, W - M, cv.y, INK, 1.5);
    cv.y += 10.0;

    // ---- job fields, 3 × 2
    let fields = [
        ("Customer", &job.customer),
        ("Due", &job.due),
        ("Quantity", &job.quantity),
        ("Machine", &job.machine),
        ("Placement", &job.placement),
        ("Garment", &job.garment),
    ];
    let colw = inner / 3.0;
    for row in 0..2 {
        for col in 0..3 {
            let (label, v) = fields[row * 3 + col];
            let x = M + col as f64 * colw;
            cv.text(x, cv.y, 7.0, Font::Regular, MUTED, &label.to_uppercase());
            let shown = if v.trim().is_empty() { String::new() } else { v.clone() };
            let mut s = shown;
            while !s.is_empty() && text_w(&s, 10.5, Font::Bold) > colw - 14.0 {
                s.pop();
            }
            cv.text(x, cv.y + 10.0, 10.5, Font::Bold, INK, &s);
            cv.hline(x, x + colw - 12.0, cv.y + 25.0, RULE, 0.7);
        }
        cv.y += 34.0;
    }

    // ---- preview (left) and specs (right)
    cv.y += 6.0;
    let spec_w = 190.0;
    let box_w = inner - spec_w - 16.0;
    // leave room on page 1 for the stops table, warnings and notes
    let stops_h = d.stops.len() as f64 * 19.0;
    let warn_h = d.warnings.len() as f64 * 11.5 + if d.warnings.is_empty() { 0.0 } else { 38.0 };
    let box_h = (H - 2.0 * M - 18.0 - 190.0 - 60.0 - stops_h - warn_h - 90.0).clamp(170.0, 250.0);
    let dw = d.bbox.width().max(1.0) + 4.0;
    let dh = d.bbox.height().max(1.0) + 4.0;
    let true_fit = dw * PT_PER_MM <= box_w - 12.0 && dh * PT_PER_MM <= box_h - 24.0;
    let scale = ((box_w - 12.0) / (dw * PT_PER_MM)).min((box_h - 24.0) / (dh * PT_PER_MM)).min(1.0);
    let (iw, ih) = (dw * PT_PER_MM * scale, dh * PT_PER_MM * scale);
    let top0 = cv.y;
    cv.rect(M, top0, box_w, box_h, None, Some((RULE, 0.8)));
    // preview pixels: ~200 dpi at the printed size
    let px_per_mm = ((200.0 / 25.4) * scale).clamp(3.0, 12.0) as f32;
    let png = crate::sheet::preview_png(d, px_per_mm, &job.fabric_hex())?;
    let img = image::load_from_memory(&png).map_err(|e| format!("preview: {e}"))?.to_rgb8();
    cv.image(M + (box_w - iw) / 2.0, top0 + 6.0 + (box_h - 24.0 - ih) / 2.0, iw, ih);
    let note = if true_fit && scale >= 0.999 {
        "Shown at actual size (print at 100%, no fit-to-page)".to_string()
    } else {
        format!("Shown at {:.0}% of actual size", scale * 100.0)
    };
    cv.text(M + 8.0, top0 + box_h - 14.0, 7.0, Font::Regular, MUTED, &note);

    let sx = M + box_w + 16.0;
    let mut y = top0;
    cv.text(sx, y, 7.0, Font::Regular, MUTED, "SIZE");
    y += 10.0;
    cv.text(sx, y, 17.0, Font::Bold, INK, &format!("{:.2}\" × {:.2}\"", st.width_mm / 25.4, st.height_mm / 25.4));
    y += 21.0;
    cv.text(sx, y, 8.0, Font::Regular, MUTED, &format!("{:.1} × {:.1} mm (W × H)", st.width_mm, st.height_mm));
    y += 16.0;
    let kv: Vec<(&str, String)> = vec![
        ("Stitches", fmt_int(st.stitches)),
        ("Color stops", d.stops.len().to_string()),
        ("Trims", st.trims.to_string()),
        ("Est. run time", format!("{:.1} min", st.run_minutes)),
        ("Thread (each)", format!("{:.0} m top · {:.0} m bobbin", st.top_m, st.bobbin_m)),
        ("Thread weight", weights(d)),
        ("Sewing order", d.profile.label().to_string()),
        ("Fabric recipe", d.recipe.name.clone()),
        ("Backing", d.recipe.backing.clone()),
        ("Topping", d.recipe.topping.clone()),
        ("Start point", "Design center".into()),
    ];
    for (k, v) in kv {
        cv.text(sx, y, 7.5, Font::Regular, MUTED, k);
        let mut v = v;
        while text_w(&v, 8.5, Font::Bold) > spec_w - 62.0 && v.len() > 3 {
            v.pop();
        }
        cv.text_right(W - M, y, 8.5, Font::Bold, INK, &v);
        y += 12.0;
        cv.hline(sx, W - M, y, RULE, 0.6);
        y += 5.0;
    }
    cv.y = top0 + box_h.max(y - top0) + 4.0;

    // ---- colour stops
    cv.heading("Color stops");
    let cols = [
        Col("STOP", 34.0, true),
        Col("NEEDLE", 40.0, true),
        Col("", 40.0, false),
        Col("THREAD", 196.0, false),
        Col("BRAND", 70.0, false),
        Col("WEIGHT", 54.0, false),
        Col("STITCHES", 52.0, true),
        Col("THREAD", inner - 34.0 - 40.0 - 40.0 - 196.0 - 70.0 - 54.0 - 52.0, true),
    ];
    table_header(&mut cv, &cols);
    for (i, s) in d.stops.iter().enumerate() {
        let note_lines = if s.note.is_empty() { vec![] } else { wrap(&s.note, 7.5, Font::Bold, 190.0) };
        let row_h = 18.0 + note_lines.len() as f64 * 9.5;
        if cv.room(row_h) {
            table_header(&mut cv, &cols);
        }
        let ty = cv.y + 4.0;
        let mut x = M;
        cv.text_right(x + 34.0 - 4.0, ty, 9.0, Font::Regular, INK, &(i + 1).to_string());
        x += 34.0;
        cv.text_right(x + 40.0 - 4.0, ty, 9.0, Font::Regular, INK, &s.needle.to_string());
        x += 40.0;
        cv.rect(x + 6.0, cv.y + 2.0, 28.0, 13.0, Some(s.thread.rgb), Some(([90, 90, 90], 0.6)));
        x += 40.0;
        cv.text(x + 4.0, ty, 9.0, Font::Bold, INK, &s.thread.code);
        let cw = text_w(&s.thread.code, 9.0, Font::Bold);
        let mut name = s.thread.name.clone();
        while text_w(&name, 9.0, Font::Regular) > 196.0 - cw - 14.0 && name.len() > 2 {
            name.pop();
        }
        cv.text(x + 8.0 + cw, ty, 9.0, Font::Regular, INK, &name);
        for (k, l) in note_lines.iter().enumerate() {
            cv.text(x + 4.0, ty + 11.0 + k as f64 * 9.5, 7.5, Font::Bold, RED, l);
        }
        x += 196.0;
        cv.text(x + 4.0, ty, 9.0, Font::Regular, INK, &s.thread.brand);
        x += 70.0;
        let wl = if s.weight == crate::thread::ThreadWeight::W40 {
            s.weight.label().to_string()
        } else {
            format!("{} · {}", s.weight.label(), s.weight.needle())
        };
        cv.text(x + 4.0, ty, 9.0, Font::Regular, INK, &wl);
        x += 54.0;
        cv.text_right(x + 52.0 - 4.0, ty, 9.0, Font::Regular, INK, &fmt_int(s.stitches));
        cv.text_right(W - M - 4.0, ty, 9.0, Font::Regular, INK, &format!("{:.1} m", s.top_m));
        cv.y += row_h;
        cv.hline(M, W - M, cv.y, RULE, 0.6);
    }

    // ---- warnings
    if !d.warnings.is_empty() {
        let lines: Vec<String> = d.warnings.iter().flat_map(|w| {
            let mut v = wrap(w, 8.5, Font::Regular, inner - 34.0);
            if let Some(f) = v.first_mut() {
                *f = format!("• {f}");
            }
            v
        }).collect();
        // split across pages if it doesn't fit, a box per page
        let mut rest: &[String] = &lines;
        let mut first = true;
        while !rest.is_empty() {
            let free = H - M - 18.0 - cv.y - 12.0 - 26.0;
            let mut fit = ((free / 11.0).floor().max(0.0) as usize).min(rest.len());
            if fit < 3.min(rest.len()) {
                cv.new_page();
                continue;
            }
            if fit == 0 {
                fit = rest.len();
            }
            let (now, later) = rest.split_at(fit);
            let h = 26.0 + now.len() as f64 * 11.0;
            cv.y += 12.0;
            cv.rect(M, cv.y, inner, h, Some([255, 246, 229]), None);
            cv.rect(M, cv.y, 3.5, h, Some(WARN), None);
            let head = if first { "CHECK BEFORE RUNNING" } else { "CHECK BEFORE RUNNING (CONTINUED)" };
            cv.text(M + 12.0, cv.y + 8.0, 8.5, Font::Bold, WARN, head);
            for (k, l) in now.iter().enumerate() {
                cv.text(M + 12.0, cv.y + 22.0 + k as f64 * 11.0, 8.5, Font::Regular, INK, l);
            }
            cv.y += h;
            rest = later;
            first = false;
        }
    }

    // ---- notes
    let note_lines = if job.notes.trim().is_empty() { vec![] } else { wrap(&job.notes, 9.0, Font::Regular, inner - 16.0) };
    let nh = (16.0 + note_lines.len() as f64 * 11.5).max(46.0);
    cv.room(nh + 30.0);
    cv.heading("Notes");
    cv.rect(M, cv.y, inner, nh, None, Some((RULE, 0.8)));
    for (k, l) in note_lines.iter().enumerate() {
        cv.text(M + 8.0, cv.y + 8.0 + k as f64 * 11.5, 9.0, Font::Regular, INK, l);
    }
    cv.y += nh;

    // ---- page 2: sewing sequence
    if sequence_page && !d.objects.is_empty() {
        // its own page; if page 1 already ran over, carry on after the notes
        if cv.pages.len() == 1 {
            cv.new_page();
        } else {
            cv.y += 24.0;
            cv.room(80.0);
        }
        cv.text(M, cv.y, 8.0, Font::Bold, MUTED, "OPERATOR SPEC SHEET · SEWING SEQUENCE");
        cv.y += 12.0;
        cv.text(M, cv.y, 14.0, Font::Bold, INK, &format!("{title} — sewing sequence"));
        cv.y += 22.0;
        let cols = [
            Col("#", 26.0, true),
            Col("", 20.0, false),
            Col("OBJECT", 132.0, false),
            Col("STITCH", 84.0, false),
            Col("UNDERLAY", 160.0, false),
            Col("THREAD", 64.0, false),
            Col("STITCHES", inner - 26.0 - 20.0 - 132.0 - 84.0 - 160.0 - 64.0, true),
        ];
        table_header(&mut cv, &cols);
        for o in &d.objects {
            if cv.room(15.0) {
                table_header(&mut cv, &cols);
            }
            let ty = cv.y + 3.0;
            let rgb = d.stops.get(o.stop).map(|s| s.thread.rgb).unwrap_or([0, 0, 0]);
            cv.text_right(M + 22.0, ty, 8.0, Font::Regular, INK, &o.sequence.to_string());
            cv.rect(M + 30.0, cv.y + 2.5, 12.0, 8.0, Some(rgb), Some(([90, 90, 90], 0.5)));
            let clip = |s: &str, w: f64, size: f64| {
                let mut t = s.to_string();
                while text_w(&t, size, Font::Regular) > w && t.len() > 2 {
                    t.pop();
                }
                t
            };
            let x0 = M + 46.0;
            cv.text(x0 + 4.0, ty, 8.0, Font::Regular, INK, &clip(&o.label, 124.0, 8.0));
            cv.text(x0 + 136.0, ty, 8.0, Font::Regular, INK, &clip(o.stitch_type, 78.0, 8.0));
            let ul = o.underlay.trim_start_matches("Auto: ").split(" · ").take(2).collect::<Vec<_>>().join(" · ");
            let mut chs = ul.chars();
            let ul = match chs.next() { Some(f) => f.to_uppercase().collect::<String>() + chs.as_str(), None => ul };
            let ul = if o.underlay_stitches > 0 { format!("{ul} ({})", fmt_int(o.underlay_stitches)) } else { ul };
            cv.text(x0 + 220.0, ty, 7.0, Font::Regular, INK, &clip(&ul, 154.0, 7.0));
            cv.text(x0 + 380.0, ty, 8.0, Font::Regular, INK, &clip(&o.thread_code, 60.0, 8.0));
            cv.text_right(W - M - 4.0, ty, 8.0, Font::Regular, INK, &fmt_int(o.stitches));
            cv.y += 14.0;
            cv.hline(M, W - M, cv.y, RULE, 0.5);
        }
    }

    // ---- footers
    let n = cv.pages.len();
    for (i, p) in cv.pages.iter_mut().enumerate() {
        let foot = format!(
            "Generated {generated} by StitchForge · file {}.dst · page {} of {n}",
            d.name,
            i + 1
        );
        let mut o = format!("BT /F1 7 Tf {:.3} {:.3} {:.3} rg {M:.2} {:.2} Td (", c(MUTED[0]), c(MUTED[1]), c(MUTED[2]), M - 10.0).into_bytes();
        o.extend(esc_pdf(&win_ansi(&foot)));
        o.extend_from_slice(b") Tj ET\n");
        p.ops.extend(o);
    }

    let bytes = assemble(&cv.pages, &img);
    Ok(SheetPdf { bytes, pages: n })
}

fn weights(d: &Design) -> String {
    let mut ws: Vec<crate::thread::ThreadWeight> = d.stops.iter().map(|s| s.weight).collect();
    ws.sort();
    ws.dedup();
    if ws.is_empty() {
        ws.push(d.recipe.weight);
    }
    ws.iter().map(|w| format!("{} · {}", w.label(), w.needle())).collect::<Vec<_>>().join(" + ")
}

fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Write the PDF file structure around the drawn pages.
fn assemble(pages: &[Page], img: &image::RgbImage) -> Vec<u8> {
    // object numbers: 1 catalog, 2 pages, 3 font, 4 bold font, 5 image,
    // then a (content, page) pair per page
    let mut objs: Vec<Vec<u8>> = Vec::new();
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 8 + 2 * i)).collect();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec());
    let raw = miniz_oxide::deflate::compress_to_vec_zlib(img.as_raw(), 6);
    let mut im = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
        img.width(),
        img.height(),
        raw.len()
    )
    .into_bytes();
    im.extend(raw);
    im.extend_from_slice(b"\nendstream");
    objs.push(im);
    // object 6 is unused padding so page pairs start at 6/7 cleanly
    objs.push(b"null".to_vec());
    for p in pages {
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&p.ops, 6);
        let mut s = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", z.len()).into_bytes();
        s.extend(z);
        s.extend_from_slice(b"\nendstream");
        let content_no = objs.len() + 2; // the content object we're about to push is objs.len()+1
        let _ = content_no;
        objs.push(s);
        let content_ref = objs.len(); // 1-based number of the content just pushed
        let xobj = if p.image { " /XObject << /Im1 5 0 R >>" } else { "" };
        objs.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >>{xobj} >> /Contents {content_ref} 0 R >>"
            )
            .into_bytes(),
        );
    }
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).into_bytes());
        out.extend(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).into_bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").into_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).into_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_pdf_reads_back_with_colors_and_page_two() {
        let (opt, _) = crate::import::font_options(None).unwrap();
        let art = crate::import::load_svg_str(include_str!("../examples/left-chest-logo.svg"), &opt).unwrap();
        let chart = crate::thread::generic_chart();
        let recipe = crate::settings::recipe("pique").unwrap();
        let d = crate::design::build_with("logo", &art, &chart, &recipe, crate::settings::Profile::Flat, Default::default()).unwrap();
        let job = JobInfo { job: "1042".into(), customer: "Sample customer".into(), ..Default::default() };
        let two = operator_sheet_pdf(&d, &job, "today", true).unwrap();
        let one = operator_sheet_pdf(&d, &job, "today", false).unwrap();
        assert_eq!(two.pages, one.pages + 1);
        // a real PDF that a PDF reader can open
        let doc = lopdf::Document::load_mem(&two.bytes).expect("valid pdf");
        assert_eq!(doc.get_pages().len(), two.pages);
        // the colour swatches are filled rectangles in each stop's colour
        let page1 = doc.get_page_content(*doc.get_pages().get(&1).unwrap());
        let text = String::from_utf8_lossy(&page1).to_string();
        let rgb = d.stops[0].thread.rgb;
        let want = format!("{:.3} {:.3} {:.3} rg", c(rgb[0]), c(rgb[1]), c(rgb[2]));
        assert!(text.contains(&want), "swatch colour {want}");
        assert!(text.contains("Color stops") || text.contains("COLOR STOPS"));
    }
}
