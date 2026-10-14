//! Write every output for a design into a folder. Shared by the command
//! line and the desktop app so both produce identical files.

use crate::design::Design;
use crate::{dst, sheet};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Written {
    pub dst: PathBuf,
    pub sheet: PathBuf,
    pub summary: Value,
}

fn round(v: f64, k: f64) -> f64 {
    (v * k).round() / k
}

pub fn write_all(d: &Design, job: &sheet::JobInfo, dir: &Path, generated: &str) -> Result<Written, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let base = &d.name;
    let p = |ext: &str| dir.join(format!("{base}.{ext}"));
    let write = |path: PathBuf, data: &[u8]| std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()));
    write(p("dst"), &dst::write(d))?;
    write(p("sheet.html"), sheet::operator_sheet(d, job, generated).as_bytes())?;
    write(p("stops.csv"), sheet::stops_csv(d).as_bytes())?;
    write(p("preview.png"), &sheet::preview_png(d, 12.0)?)?;
    write(p("preview.svg"), sheet::preview_svg(d, 1.0, "#f4f1ea").as_bytes())?;
    let files: Vec<String> = ["dst", "sheet.html", "stops.csv", "preview.png", "preview.svg", "job.json"]
        .iter()
        .map(|e| p(e).display().to_string())
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
    write(p("job.json"), serde_json::to_string_pretty(&summary).unwrap().as_bytes())?;
    Ok(Written { dst: p("dst"), sheet: p("sheet.html"), summary })
}
