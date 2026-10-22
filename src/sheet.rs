//! Outputs for people: stitch preview (SVG/PNG), the printable operator spec
//! sheet (HTML, prints to PDF from any browser), and the colour-stop CSV.

use crate::design::{Cmd, Design};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub job: String,
    pub customer: String,
    pub due: String,
    pub quantity: String,
    pub machine: String,
    pub placement: String,
    pub garment: String,
    pub notes: String,
    /// Garment colour the previews are drawn on (None = natural canvas).
    #[serde(default)]
    pub garment_rgb: Option<[u8; 3]>,
    /// Put the object-by-object stitch list on its own page (page 2) of the
    /// sheet; off leaves it out.
    #[serde(default = "yes")]
    pub sequence_page: bool,
}

fn yes() -> bool {
    true
}

impl Default for JobInfo {
    fn default() -> Self {
        JobInfo {
            job: String::new(),
            customer: String::new(),
            due: String::new(),
            quantity: String::new(),
            machine: String::new(),
            placement: String::new(),
            garment: String::new(),
            notes: String::new(),
            garment_rgb: None,
            sequence_page: true,
        }
    }
}

impl JobInfo {
    /// Background colour for previews, as #rrggbb.
    pub fn fabric_hex(&self) -> String {
        self.garment_rgb.map(hex).unwrap_or_else(|| "#f4f1ea".into())
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn shade(rgb: [u8; 3], k: f64) -> String {
    let f = |c: u8| ((c as f64) * k).clamp(0.0, 255.0) as u8;
    hex([f(rgb[0]), f(rgb[1]), f(rgb[2])])
}

/// Thread-like render of the stitches. Units are mm; `scale` sets the
/// physical size (1.0 = true size).
pub fn preview_svg(d: &Design, scale: f64, fabric: &str) -> String {
    let m = 3.0;
    let (w, h) = (d.bbox.width() + 2.0 * m, d.bbox.height() + 2.0 * m);
    let (ox, oy) = (d.bbox.min.x - m, d.bbox.min.y - m);
    // Draw in true sewing order, in short chunks, so later stitches cover
    // earlier ones (underlay disappears under the top stitching, as on fabric).
    let mut chunks: Vec<(usize, String)> = Vec::new();
    let mut stop = 0usize;
    let mut last: Option<crate::geom::Pt> = None;
    let mut cur = String::new();
    let mut n = 0usize;
    let flush = |chunks: &mut Vec<(usize, String)>, cur: &mut String, stop: usize| {
        if cur.contains('L') {
            chunks.push((stop, std::mem::take(cur)));
        } else {
            cur.clear();
        }
    };
    for c in &d.cmds {
        match *c {
            Cmd::Color(i) => {
                flush(&mut chunks, &mut cur, stop);
                stop = i;
                last = None;
            }
            Cmd::Jump(_) | Cmd::Trim => {
                flush(&mut chunks, &mut cur, stop);
                last = None;
            }
            Cmd::Stitch(p) => {
                if cur.is_empty() {
                    if let Some(l) = last {
                        cur.push_str(&format!("M{:.2} {:.2}", l.x - ox, l.y - oy));
                    }
                }
                let cmd = if cur.is_empty() { 'M' } else { 'L' };
                cur.push_str(&format!("{cmd}{:.2} {:.2}", p.x - ox, p.y - oy));
                last = Some(p);
                n += 1;
                if n % 24 == 0 {
                    flush(&mut chunks, &mut cur, stop);
                }
            }
            Cmd::End => {}
        }
    }
    flush(&mut chunks, &mut cur, stop);
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{:.2}mm' height='{:.2}mm' viewBox='0 0 {:.2} {:.2}'>\
         <rect width='100%' height='100%' fill='{fabric}'/><g fill='none' stroke-linejoin='round' stroke-linecap='round'>",
        w * scale,
        h * scale,
        w,
        h
    );
    for (i, p) in &chunks {
        let rgb = d.stops[*i].thread.rgb;
        // strand width follows the thread weight (40 wt ≈ 0.33 mm)
        let k = d.stops[*i].weight.strand_mm() / 0.33;
        s.push_str(&format!(
            "<path d='{p}' stroke='{}' stroke-width='{:.3}'/><path d='{p}' stroke='{}' stroke-width='{:.3}'/>\
             <path d='{p}' stroke='{}' stroke-width='{:.3}' stroke-opacity='0.5' transform='translate(-0.05 -0.05)'/>",
            shade(rgb, 0.6),
            0.44 * k,
            hex(rgb),
            0.30 * k,
            shade(rgb, 1.35),
            0.07 * k
        ));
    }
    s.push_str("</g>");
    s.push_str("</svg>");
    s
}

pub fn preview_png(d: &Design, px_per_mm: f32, fabric: &str) -> Result<Vec<u8>, String> {
    let svg = preview_svg(d, 1.0, fabric);
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    // tree size is in px at 96 dpi; scale to the requested mm resolution
    let k = px_per_mm * 25.4 / 96.0;
    let (w, h) = (
        (size.width() * k).ceil() as u32,
        (size.height() * k).ceil() as u32,
    );
    let mut pix = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("image too large")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(k, k),
        &mut pix.as_mut(),
    );
    pix.encode_png().map_err(|e| e.to_string())
}

pub fn stops_csv(d: &Design) -> String {
    let mut s = String::from(
        "stop,needle,brand,code,name,hex,weight,needle_size,stitches,top_thread_m,bobbin_m,note\n",
    );
    for (i, st) in d.stops.iter().enumerate() {
        s.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{:.1},{:.1},{}\n",
            i + 1,
            st.needle,
            st.thread.brand,
            st.thread.code,
            st.thread.name,
            hex(st.thread.rgb),
            st.weight.number(),
            st.weight.needle(),
            st.stitches,
            st.top_m,
            st.bobbin_m,
            st.note.replace(',', ";")
        ));
    }
    s
}

pub fn operator_sheet(d: &Design, job: &JobInfo, generated: &str) -> String {
    let st = &d.stats;
    let in_ = |mm: f64| mm / 25.4;
    // True size when it fits the page; otherwise scale down and say so.
    let max_w = 170.0;
    let max_h = 120.0;
    let w = d.bbox.width() + 6.0;
    let h = d.bbox.height() + 6.0;
    let scale = (max_w / w).min(max_h / h).min(1.0);
    let scale_note = if scale < 0.999 {
        format!("Shown at {:.0}% of actual size", scale * 100.0)
    } else {
        "Shown at actual size (print at 100%, no fit-to-page)".into()
    };
    let preview = preview_svg(d, scale, &job.fabric_hex());
    let field = |label: &str, v: &str| -> String {
        if v.trim().is_empty() {
            format!("<div class='f'><span>{label}</span><b class='blank'>&nbsp;</b></div>")
        } else {
            format!("<div class='f'><span>{label}</span><b>{}</b></div>", esc(v))
        }
    };
    let mut rows = String::new();
    for (i, s) in d.stops.iter().enumerate() {
        rows.push_str(&format!(
            "<tr><td class='n'>{}</td><td class='n'>{}</td><td><span class='sw' style='background:{}'></span></td>\
             <td><b>{}</b> {}{}</td><td>{}</td><td>{}{}</td><td class='n'>{}</td><td class='n'>{:.1} m</td></tr>",
            i + 1, s.needle, hex(s.thread.rgb), esc(&s.thread.code), esc(&s.thread.name),
            if s.note.is_empty() { String::new() } else { format!("<br><b style='color:#b4231a'>{}</b>", esc(&s.note)) },
            esc(&s.thread.brand),
            s.weight.label(),
            if s.weight == crate::thread::ThreadWeight::W40 { String::new() } else { format!(" · needle {}", s.weight.needle()) },
            fmt_int(s.stitches), s.top_m
        ));
    }
    let mut seq = String::new();
    for o in d.objects.iter().filter(|_| job.sequence_page) {
        seq.push_str(&format!(
            "<tr><td class='n'>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class='n'>{}</td></tr>",
            o.sequence,
            esc(&o.label),
            o.stitch_type,
            esc(&if o.underlay_stitches > 0 { format!("{} ({})", o.underlay, fmt_int(o.underlay_stitches)) } else { o.underlay.clone() }),
            esc(&format!("{} · {}", o.thread_code, o.weight.label())),
            fmt_int(o.stitches)
        ));
    }
    let seq = if seq.is_empty() {
        String::new()
    } else {
        format!(
            "<section class='seq'><h3>Sewing sequence (page 2)</h3><table><thead><tr><th class='n'>#</th><th>Object</th><th>Stitch</th><th>Underlay</th>\
             <th>Thread</th><th class='n'>Stitches</th></tr></thead><tbody>{seq}</tbody></table></section>"
        )
    };
    let warnings = if d.warnings.is_empty() {
        String::new()
    } else {
        let items: String = d
            .warnings
            .iter()
            .map(|w| format!("<li>{}</li>", esc(w)))
            .collect();
        format!("<section class='warn'><h3>Check before running</h3><ul>{items}</ul></section>")
    };
    let notes = if job.notes.trim().is_empty() {
        "&nbsp;".to_string()
    } else {
        esc(&job.notes)
    };
    let title = if job.job.is_empty() {
        esc(&d.name)
    } else {
        format!("Job {} · {}", esc(&job.job), esc(&d.name))
    };

    let mut html = String::new();
    html.push_str("<!doctype html><html lang='en'><head><meta charset='utf-8'><meta name='viewport' content='width=device-width, initial-scale=1'>");
    html.push_str(&format!("<title>{title} · Operator sheet</title>"));
    html.push_str(CSS);
    html.push_str("</head><body><main>");
    html.push_str(&format!(
        "<header><div><p class='eyebrow'>Operator spec sheet</p><h1>{title}</h1></div>\
         <button onclick='window.print()'>Print</button></header>"
    ));
    html.push_str("<section class='grid'>");
    html.push_str(&field("Customer", &job.customer));
    html.push_str(&field("Due", &job.due));
    html.push_str(&field("Quantity", &job.quantity));
    html.push_str(&field("Machine", &job.machine));
    html.push_str(&field("Placement", &job.placement));
    html.push_str(&field("Garment", &job.garment));
    html.push_str("</section>");

    html.push_str(&format!(
        "<section class='art'><div class='pv'>{preview}<p class='note'>{scale_note}</p></div>\
         <div class='specs'>\
           <div class='big'><span>Size</span><b>{:.2}″ × {:.2}″</b><em>{:.1} × {:.1} mm (W × H)</em></div>\
           <div class='kv'><span>Stitches</span><b>{}</b></div>\
           <div class='kv'><span>Color stops</span><b>{}</b></div>\
           <div class='kv'><span>Trims</span><b>{}</b></div>\
           <div class='kv'><span>Est. run time</span><b>{:.1} min</b></div>\
           <div class='kv'><span>Thread (each)</span><b>{:.0} m top · {:.0} m bobbin</b></div>\
           <div class='kv'><span>Thread weight</span><b>{}</b></div>\
           <div class='kv'><span>Sewing order</span><b>{}</b></div>\
           <div class='kv'><span>Fabric recipe</span><b>{}</b></div>\
           <div class='kv'><span>Backing</span><b>{}</b></div>\
           <div class='kv'><span>Topping</span><b>{}</b></div>\
           <div class='kv'><span>Start point</span><b>Design center</b></div>\
         </div></section>",
        in_(st.width_mm), in_(st.height_mm), st.width_mm, st.height_mm,
        fmt_int(st.stitches), d.stops.len(), st.trims, st.run_minutes,
        st.top_m, st.bobbin_m, weights_line(d),
        d.profile.label(), d.recipe.name, d.recipe.backing, d.recipe.topping
    ));
    html.push_str(&format!(
        "<section><h3>Color stops</h3><table><thead><tr><th class='n'>Stop</th><th class='n'>Needle</th><th></th>\
         <th>Thread</th><th>Brand</th><th>Weight</th><th class='n'>Stitches</th><th class='n'>Thread</th></tr></thead><tbody>{rows}</tbody></table></section>"
    ));
    html.push_str(&warnings);
    html.push_str(&format!(
        "<section><h3>Notes</h3><div class='notes'>{notes}</div></section>\
         {seq}\
         <footer>Generated {generated} by StitchForge · file {}.dst · sheet regenerates whenever the design changes</footer>",
        esc(&d.name)
    ));
    html.push_str("</main></body></html>");
    html
}

/// "40 wt · needle 75/11", or a list when the design mixes weights.
fn weights_line(d: &Design) -> String {
    let mut ws: Vec<crate::thread::ThreadWeight> = d.stops.iter().map(|s| s.weight).collect();
    ws.sort();
    ws.dedup();
    if ws.is_empty() {
        ws.push(d.recipe.weight);
    }
    ws.iter()
        .map(|w| format!("{} · needle {}", w.label(), w.needle()))
        .collect::<Vec<_>>()
        .join(" + ")
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

const CSS: &str = r#"<style>
:root{--ink:#1d1d1b;--muted:#6b6a65;--rule:#d9d6cf;--paper:#fff;--bg:#efede8;--accent:#1f5fbf;--warn:#9a5b00;--warnbg:#fff6e5}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.45 -apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
main{max-width:820px;margin:24px auto;background:var(--paper);padding:28px 32px;border-radius:10px;box-shadow:0 1px 3px rgba(0,0,0,.08)}
header{display:flex;justify-content:space-between;align-items:flex-start;gap:16px;border-bottom:2px solid var(--ink);padding-bottom:12px}
.eyebrow{margin:0;font-size:11px;letter-spacing:.08em;text-transform:uppercase;color:var(--muted)}
h1{margin:2px 0 0;font-size:22px}
h3{font-size:12px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);margin:22px 0 8px}
button{font:inherit;padding:8px 16px;border:1px solid var(--ink);background:var(--ink);color:#fff;border-radius:6px;cursor:pointer}
.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:10px 20px;margin-top:16px}
.f span,.kv span,.big span{display:block;font-size:11px;color:var(--muted);text-transform:uppercase;letter-spacing:.05em}
.f b{display:block;font-size:15px;border-bottom:1px solid var(--rule);min-height:24px;padding-bottom:2px}
.art{display:grid;grid-template-columns:minmax(0,1fr) 230px;gap:24px;margin-top:20px;align-items:start}
.pv{border:1px solid var(--rule);border-radius:8px;padding:10px;text-align:center;overflow:auto}
.pv svg{max-width:100%;height:auto}
.note{margin:6px 0 0;font-size:11px;color:var(--muted)}
.big b{font-size:24px;display:block}.big em{font-style:normal;color:var(--muted);font-size:12px}
.specs{display:flex;flex-direction:column;gap:9px}
.kv{display:flex;justify-content:space-between;border-bottom:1px solid var(--rule);padding-bottom:4px}
.kv span{display:inline}.kv b{font-weight:600;text-align:right}
table{width:100%;border-collapse:collapse}
th{font-size:11px;text-align:left;color:var(--muted);font-weight:600;border-bottom:1.5px solid var(--ink);padding:6px 8px}
td{padding:7px 8px;border-bottom:1px solid var(--rule)}
.n{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}
.sw{display:inline-block;width:26px;height:16px;border-radius:3px;border:1px solid rgba(0,0,0,.25);vertical-align:middle}
.warn{background:var(--warnbg);border-left:4px solid var(--warn);padding:2px 14px 8px;margin-top:18px;border-radius:4px}
.warn h3{color:var(--warn)}.warn ul{margin:0;padding-left:18px}
.notes{min-height:54px;border:1px solid var(--rule);border-radius:6px;padding:8px 10px}
.seq td,.seq th{font-size:12px;padding:4px 8px}
footer{margin-top:22px;font-size:11px;color:var(--muted);border-top:1px solid var(--rule);padding-top:8px}
@media (max-width:640px){main{margin:0;border-radius:0;padding:16px}.grid{grid-template-columns:1fr 1fr}.art{grid-template-columns:1fr}}
*{-webkit-print-color-adjust:exact;print-color-adjust:exact}
@media print{body{background:#fff}main{box-shadow:none;margin:0;max-width:none;padding:0}button{display:none}.seq{break-before:page}@page{size:letter;margin:12mm}}
</style>"#;
