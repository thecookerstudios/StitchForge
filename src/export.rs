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
        summary,
    })
}
