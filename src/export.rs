//! Write every output for a design into a folder. Shared by the command
//! line and the desktop app so both produce identical files.

use crate::design::Design;
use crate::formats::{self, Format};
use crate::sheet;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub struct Written {
    /// The machine files written, first one first.
    pub machine: Vec<PathBuf>,
    pub sheet: PathBuf,
    /// The same sheet as a PDF (colours print exactly).
    pub sheet_pdf: PathBuf,
    /// Cut files for appliqué pieces (SVG / DXF / PLT).
    pub cut_files: Vec<PathBuf>,
    pub summary: Value,
}

fn round(v: f64, k: f64) -> f64 {
    (v * k).round() / k
}

pub fn write_all(
    d: &Design,
    job: &sheet::JobInfo,
    dir: &Path,
    generated: &str,
) -> Result<Written, String> {
    write_formats(d, job, dir, generated, &[Format::Dst])
}

pub fn write_formats(
    d: &Design,
    job: &sheet::JobInfo,
    dir: &Path,
    generated: &str,
    fmts: &[Format],
) -> Result<Written, String> {
    crate::vfs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let base = &d.name;
    let p = |ext: &str| dir.join(format!("{base}.{ext}"));
    let write = |path: PathBuf, data: &[u8]| {
        crate::vfs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))
    };
    let mut machine = Vec::new();
    for f in fmts {
        write(p(f.ext()), &formats::write(d, *f))?;
        machine.push(p(f.ext()));
    }
    write(
        p("sheet.html"),
        sheet::operator_sheet(d, job, generated).as_bytes(),
    )?;
    // the PDF keeps the thread colours exactly when printed
    let pdf = crate::pdfsheet::operator_sheet_pdf(d, job, generated, job.sequence_page)?;
    write(p("sheet.pdf"), &pdf.bytes)?;
    write(p("stops.csv"), sheet::stops_csv(d).as_bytes())?;
    // SF_PREVIEW_SCALE (pixels per mm) is a development aid for close-ups.
    let scale = std::env::var("SF_PREVIEW_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(12.0);
    write(p("preview.png"), &sheet::preview_png(d, scale, &job.fabric_hex())?)?;
    write(
        p("preview.svg"),
        sheet::preview_svg(d, 1.0, &job.fabric_hex()).as_bytes(),
    )?;
    // appliqué / twill cut files: one set per material colour
    let mut cut_files: Vec<PathBuf> = Vec::new();
    if !d.pieces.is_empty() {
        let mut groups: Vec<(String, Vec<&crate::applique::Piece>)> = Vec::new();
        for pc in &d.pieces {
            let tag = crate::applique::file_tag(&pc.material, pc.rgb);
            match groups.iter_mut().find(|(t, _)| *t == tag) {
                Some((_, v)) => v.push(pc),
                None => groups.push((tag, vec![pc])),
            }
        }
        let o = &job.cut;
        for (tag, ps) in groups {
            let rings = crate::applique::cut_paths(&ps, o);
            let title = format!("{base} {tag}{}", if o.mirror { " (mirrored)" } else { "" });
            if o.svg {
                write(p(&format!("cut-{tag}.svg")), crate::applique::cut_svg(&rings, &title).as_bytes())?;
                cut_files.push(p(&format!("cut-{tag}.svg")));
            }
            if o.dxf {
                write(p(&format!("cut-{tag}.dxf")), crate::applique::cut_dxf(&rings).as_bytes())?;
                cut_files.push(p(&format!("cut-{tag}.dxf")));
            }
            if o.plt {
                write(p(&format!("cut-{tag}.plt")), crate::applique::cut_plt(&rings).as_bytes())?;
                cut_files.push(p(&format!("cut-{tag}.plt")));
            }
        }
    }
    let files: Vec<String> = machine
        .iter()
        .cloned()
        .chain(
            [
                "sheet.html",
                "sheet.pdf",
                "stops.csv",
                "preview.png",
                "preview.svg",
                "job.json",
            ]
            .iter()
            .map(|e| p(e)),
        )
        .chain(cut_files.iter().cloned())
        .map(|f| f.display().to_string())
        .collect();
    let summary = json!({
        "name": base,
        "profile": d.profile,
        "fabric": d.recipe.name,
        "size_mm": [round(d.stats.width_mm, 10.0), round(d.stats.height_mm, 10.0)],
        "size_in": [round(d.stats.width_mm / 25.4, 100.0), round(d.stats.height_mm / 25.4, 100.0)],
        "stats": d.stats,
        "stops": d.stops,
        "objects": d.objects,
        "warnings": d.warnings,
        "pieces": d.pieces.iter().map(|pc| json!({"label": pc.label, "material": pc.material, "rgb": pc.rgb})).collect::<Vec<_>>(),
        "files": files,
    });
    write(
        p("job.json"),
        serde_json::to_string_pretty(&summary).unwrap().as_bytes(),
    )?;
    Ok(Written {
        machine,
        sheet: p("sheet.html"),
        sheet_pdf: p("sheet.pdf"),
        cut_files,
        summary,
    })
}
