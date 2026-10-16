//! Name drops: personalisation runs on multi-head machines.
//!
//! A drop file sews the logo on every head, then stops; each name follows on
//! its own stop so the operator runs it on one head at a time:
//!   logo · STOP · name 1 (head 1) · STOP · name 2 (head 2) · STOP · …
//! Names come from an Excel or CSV sheet; you pick the columns, the logo
//! layers, the name (template) layer, the head count and names per drop.

use crate::design::{BuildOptions, Design, build_with};
use crate::formats::{self, Format};
use crate::geom::BBox;
use crate::project::{Layer, Project, compose, text_shapes};
use crate::sheet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A spreadsheet as plain text cells (row 0 may be the header row).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Table {
    pub source: String,
    pub raw: Vec<Vec<String>>,
}

impl Table {
    pub fn width(&self) -> usize {
        self.raw.iter().map(|r| r.len()).max().unwrap_or(0)
    }
    /// Column titles: the first row when it holds headers, else A, B, C…
    pub fn headers(&self, first_row_headers: bool) -> Vec<String> {
        (0..self.width())
            .map(|c| {
                let letter = col_letter(c);
                match self.raw.first().and_then(|r| r.get(c)).map(|v| v.trim()) {
                    Some(h) if first_row_headers && !h.is_empty() => format!("{h} ({letter})"),
                    _ => format!("Column {letter}"),
                }
            })
            .collect()
    }
    pub fn data_rows(&self, first_row_headers: bool) -> &[Vec<String>] {
        if first_row_headers && !self.raw.is_empty() {
            &self.raw[1..]
        } else {
            &self.raw[..]
        }
    }
}

pub fn col_letter(mut c: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s
}

/// Read .xlsx/.xlsm/.xls/.ods (first sheet with data) or .csv/.tsv/.txt.
pub fn read_table(path: &Path) -> Result<Table, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let source = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let raw = match ext.as_str() {
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => read_workbook(path)?,
        "csv" | "tsv" | "txt" => {
            let data = crate::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let text = String::from_utf8_lossy(&data)
                .trim_start_matches('\u{feff}')
                .to_string();
            let delim = if ext == "tsv"
                || (text
                    .lines()
                    .next()
                    .map(|l| l.matches('\t').count() > l.matches(',').count())
                    .unwrap_or(false))
            {
                '\t'
            } else {
                ','
            };
            parse_csv(&text, delim)
        }
        _ => return Err(format!("{source}: use an Excel file (.xlsx) or a CSV")),
    };
    let raw: Vec<Vec<String>> = raw
        .into_iter()
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .collect();
    if raw.is_empty() {
        return Err(format!("{source} has no data"));
    }
    Ok(Table { source, raw })
}

fn read_workbook(path: &Path) -> Result<Vec<Vec<String>>, String> {
    use calamine::{Data, Reader, open_workbook_auto_from_rs};
    let bytes = crate::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut wb = open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for name in wb.sheet_names().to_owned() {
        let Ok(range) = wb.worksheet_range(&name) else {
            continue;
        };
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|r| {
                r.iter()
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => {
                            format!("{}", *f as i64)
                        }
                        Data::Float(f) => format!("{f}"),
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => b.to_string(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect();
        if rows.iter().any(|r| r.iter().any(|c| !c.trim().is_empty())) {
            return Ok(rows);
        }
    }
    Err(format!("{}: no sheet with data", path.display()))
}

/// CSV with quoted fields ("a, b" and "" escapes).
pub fn parse_csv(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cell.push(ch);
            }
        } else if ch == '"' {
            quoted = true;
        } else if ch == delim {
            row.push(std::mem::take(&mut cell));
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else {
            cell.push(ch);
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Join {
    #[default]
    Space,
    NewLine,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Case {
    #[default]
    AsTyped,
    Upper,
    Title,
}

/// Everything about a name-drop run. Saved inside the project.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DropSetup {
    pub table: Option<Table>,
    pub first_row_headers: bool,
    /// Columns that make up each name, in order (e.g. First, Last).
    pub columns: Vec<usize>,
    pub join: Join,
    pub case: Case,
    /// Optional column with how many of each name to sew.
    pub qty_column: Option<usize>,
    pub skip_blank: bool,
    /// Layers sewn on every head before the names.
    pub logo_layers: Vec<u64>,
    /// The text layer whose font, size, colour and position every name uses.
    pub name_layer: Option<u64>,
    pub heads: usize,
    pub per_drop: usize,
    pub logo_every_drop: bool,
    /// Shrink names wider than this (mm), keeping their proportions.
    pub max_width_mm: Option<f64>,
    /// Hand fixes to individual names, by position in the list.
    pub edits: BTreeMap<usize, String>,
}

impl Default for DropSetup {
    fn default() -> Self {
        DropSetup {
            table: None,
            first_row_headers: true,
            columns: vec![],
            join: Join::Space,
            case: Case::AsTyped,
            qty_column: None,
            skip_blank: true,
            logo_layers: vec![],
            name_layer: None,
            heads: 6,
            per_drop: 6,
            logo_every_drop: true,
            max_width_mm: None,
            edits: BTreeMap::new(),
        }
    }
}

/// One name in the run and where it sews.
#[derive(Clone, Debug, Serialize)]
pub struct Slot {
    pub index: usize,
    pub text: String,
    /// Spreadsheet row it came from (1-based, as Excel numbers rows).
    pub row: usize,
    pub drop: usize,
    pub head: usize,
    /// Pass 2 = the second round on the same heads (when names per drop
    /// is more than the head count).
    pub pass: usize,
    pub edited: bool,
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl DropSetup {
    /// The name list after columns, joining, case, blanks and quantities.
    pub fn names(&self) -> Vec<Slot> {
        let Some(t) = &self.table else { return vec![] };
        let per = self.per_drop.max(1);
        let heads = self.heads.max(1);
        let skip = if self.first_row_headers { 2 } else { 1 };
        let mut out = Vec::new();
        for (ri, r) in t.data_rows(self.first_row_headers).iter().enumerate() {
            let parts: Vec<String> = self
                .columns
                .iter()
                .filter_map(|c| r.get(*c))
                .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|v| !v.is_empty())
                .collect();
            let joined = parts.join(match self.join {
                Join::Space => " ",
                Join::NewLine => "\n",
            });
            let text = match self.case {
                Case::AsTyped => joined,
                Case::Upper => joined.to_uppercase(),
                Case::Title => joined
                    .lines()
                    .map(title_case)
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            if text.is_empty() && self.skip_blank {
                continue;
            }
            let qty = self
                .qty_column
                .and_then(|c| r.get(c))
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|q| q.max(0.0).round() as usize)
                .unwrap_or(1);
            for _ in 0..qty {
                out.push(Slot {
                    index: 0,
                    text: text.clone(),
                    row: ri + skip,
                    drop: 0,
                    head: 0,
                    pass: 0,
                    edited: false,
                });
            }
        }
        for (k, s) in out.iter_mut().enumerate() {
            s.index = k;
            if let Some(e) = self.edits.get(&k) {
                s.text = e.clone();
                s.edited = true;
            }
            let slot = k % per;
            s.drop = k / per;
            s.head = slot % heads + 1;
            s.pass = slot / heads + 1;
        }
        out
    }

    pub fn drop_count(&self) -> usize {
        let n = self.names().len();
        n.div_ceil(self.per_drop.max(1))
    }

    /// What's missing before drops can be made, in plain words.
    pub fn problem(&self, layers: &[Layer]) -> Option<String> {
        if self.table.is_none() {
            return Some("Load an Excel or CSV file with the names".into());
        }
        if self.columns.is_empty() {
            return Some("Pick the column(s) that hold the names".into());
        }
        match self
            .name_layer
            .and_then(|id| layers.iter().find(|l| l.id == id))
        {
            None => {
                return Some("Pick the name layer (a text layer; add one with Add text)".into());
            }
            Some(l) if l.text.is_none() => {
                return Some("The name layer must be a text layer".into());
            }
            _ => {}
        }
        if self.names().is_empty() {
            return Some("No names found in the chosen columns".into());
        }
        None
    }
}

/// Layers for one drop: logo layers, then each name on its own stop.
/// Returns the layers and how many of them are logo layers.
pub fn drop_layers(
    project: &Project,
    opt: &usvg::Options,
    drop: usize,
) -> Result<(Vec<Layer>, usize, Vec<Slot>), String> {
    let s = &project.drops;
    if let Some(p) = s.problem(&project.layers) {
        return Err(p);
    }
    let slots: Vec<Slot> = s.names().into_iter().filter(|n| n.drop == drop).collect();
    if slots.is_empty() {
        return Err(format!("There is no drop {}", drop + 1));
    }
    let template = project
        .layers
        .iter()
        .find(|l| Some(l.id) == s.name_layer)
        .unwrap();
    let mut layers: Vec<Layer> = Vec::new();
    if drop == 0 || s.logo_every_drop {
        for l in &project.layers {
            if s.logo_layers.contains(&l.id) && l.id != template.id {
                let mut l = l.clone();
                l.visible = true;
                layers.push(l);
            }
        }
    }
    let n_logo = layers.len();
    let mut next_id = project.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
    for slot in &slots {
        let mut l = template.clone();
        l.id = next_id;
        next_id += 1;
        l.link = None;
        l.visible = true;
        let mut spec = l.text.clone().unwrap();
        spec.text = slot.text.clone();
        l.shapes = text_shapes(opt, &spec)?;
        l.text = Some(spec);
        l.opts.stop = Some(true); // every name gets its own stop
        l.opts.overlay = true; // names never cut holes in the logo or each other
        if let Some(max) = s.max_width_mm {
            let w = l.bbox().width();
            if w > max && w > 0.0 {
                let k = max / w;
                l.sx *= k;
                l.sy *= k;
            }
        }
        let pass = if slot.pass > 1 {
            format!(", pass {}", slot.pass)
        } else {
            String::new()
        };
        l.name = format!(
            "Name {}: {} · head {}{}",
            slot.index + 1,
            slot.text.replace('\n', " / "),
            slot.head,
            pass
        );
        layers.push(l);
    }
    Ok((layers, n_logo, slots))
}

/// Build one drop's design.
pub fn build_drop(
    project: &Project,
    opt: &usvg::Options,
    drop: usize,
) -> Result<(Design, usize, Vec<Slot>), String> {
    let (layers, n_logo, slots) = drop_layers(project, opt, drop)?;
    let (art, chart) = compose(&layers, &project.chart);
    let name = format!("{}-drop{}", project.name, drop + 1);
    let d = build_with(
        &name,
        &art,
        &chart,
        &project.recipe,
        project.profile,
        BuildOptions {
            keep_order: true,
            auto_stops: project.auto_stops,
        },
    )?;
    Ok((d, n_logo, slots))
}

/// Stop-by-stop instructions for the operator: what sews and which heads run.
pub fn stop_plan(
    d: &Design,
    n_logo: usize,
    slots: &[Slot],
) -> Vec<(usize, String, String, String)> {
    let mut rows = Vec::new();
    for (si, st) in d.stops.iter().enumerate() {
        let layers: Vec<usize> = d
            .objects
            .iter()
            .filter(|o| o.stop == si)
            .map(|o| o.layer)
            .collect();
        let what;
        let heads;
        if layers.iter().all(|l| *l < n_logo) {
            what = "Logo".to_string();
            heads = "All heads".to_string();
        } else if let Some(l) = layers.iter().find(|l| **l >= n_logo) {
            let slot = &slots[*l - n_logo];
            what = format!(
                "Name {}: {}",
                slot.index + 1,
                slot.text.replace('\n', " / ")
            );
            heads = if slot.pass > 1 {
                format!("Head {} only (pass {})", slot.head, slot.pass)
            } else {
                format!("Head {} only", slot.head)
            };
        } else {
            what = "—".into();
            heads = "—".into();
        }
        rows.push((
            si + 1,
            format!("{} {}", st.thread.code, st.thread.name),
            what,
            heads,
        ));
    }
    rows
}

pub struct DropExport {
    pub files: Vec<PathBuf>,
    pub sheet: PathBuf,
}

/// Write every drop: machine files, an operator sheet per drop, and one
/// printable run sheet (plus CSV) listing every stop, head and name.
pub fn export_drops(
    project: &Project,
    opt: &usvg::Options,
    dir: &Path,
    fmts: &[Format],
    generated: &str,
) -> Result<DropExport, String> {
    crate::vfs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let n = project.drops.drop_count();
    let mut files = Vec::new();
    let mut html_drops = String::new();
    let mut csv = String::from("drop,file,stop,thread,sews,heads,spreadsheet_row\n");
    for drop in 0..n {
        let (d, n_logo, slots) = build_drop(project, opt, drop)?;
        let base = &d.name;
        for f in fmts {
            let p = dir.join(format!("{base}.{}", f.ext()));
            crate::vfs::write(&p, formats::write(&d, *f))
                .map_err(|e| format!("{}: {e}", p.display()))?;
            files.push(p);
        }
        let mut job = project.job.clone();
        let names: Vec<String> = slots
            .iter()
            .map(|s| format!("{} → head {}", s.text.replace('\n', " / "), s.head))
            .collect();
        job.notes = format!("Name drop {} of {}. {}", drop + 1, n, names.join("; "));
        let sheet_path = dir.join(format!("{base}.sheet.html"));
        crate::vfs::write(&sheet_path, sheet::operator_sheet(&d, &job, generated))
            .map_err(|e| format!("{}: {e}", sheet_path.display()))?;
        let file = format!("{base}.{}", fmts.first().map(|f| f.ext()).unwrap_or("dst"));
        let plan = stop_plan(&d, n_logo, &slots);
        let mut rows = String::new();
        for (stop, thread, what, heads) in &plan {
            let row = slots
                .iter()
                .find(|s| what.starts_with(&format!("Name {}:", s.index + 1)))
                .map(|s| s.row.to_string())
                .unwrap_or_default();
            rows.push_str(&format!(
                "<tr><td class='n'>{stop}</td><td>{}</td><td><b>{}</b></td><td>{}</td><td class='n'>{row}</td></tr>",
                esc(thread), esc(what), esc(heads)
            ));
            csv.push_str(&format!(
                "{},{},{},{},{},{},{}\n",
                drop + 1,
                file,
                stop,
                csv_cell(thread),
                csv_cell(what),
                csv_cell(heads),
                row
            ));
        }
        html_drops.push_str(&format!(
            "<section><h2>Drop {} of {} <span>{} · {} names · {} stitches · ~{:.0} min</span></h2>\
             <table><thead><tr><th class='n'>Stop</th><th>Thread</th><th>Sews</th><th>Heads running</th><th class='n'>Sheet row</th></tr></thead><tbody>{rows}</tbody></table></section>",
            drop + 1, n, esc(&file), slots.len(), d.stats.stitches, d.stats.run_minutes
        ));
    }
    let job = &project.job;
    let title = if job.job.is_empty() {
        project.name.clone()
    } else {
        format!("Job {} · {}", job.job, project.name)
    };
    let src = project
        .drops
        .table
        .as_ref()
        .map(|t| t.source.clone())
        .unwrap_or_default();
    let html = format!(
        "<!doctype html><html><head><meta charset='utf-8'><meta name='viewport' content='width=device-width, initial-scale=1'><title>{} · Name drops</title>{DROP_CSS}</head><body><main>\
         <header><div><p class='eyebrow'>Name drop run sheet</p><h1>{}</h1></div><button onclick='window.print()'>Print</button></header>\
         <p class='meta'>{} names from {} · {} heads · {} names per drop · {} drop file(s){}</p>\
         <p class='how'>At each <b>name</b> stop, turn on <b>only</b> the head listed, then start. Logo stops run on all heads.</p>\
         {html_drops}<footer>Generated {generated} by StitchForge</footer></main></body></html>",
        esc(&title),
        esc(&title),
        project.drops.names().len(),
        esc(&src),
        project.drops.heads,
        project.drops.per_drop,
        n,
        if project.drops.logo_every_drop {
            " · logo in every drop"
        } else {
            " · logo in drop 1 only"
        }
    );
    let sheet = dir.join(format!("{}-name-drops.html", project.name));
    crate::vfs::write(&sheet, html).map_err(|e| format!("{}: {e}", sheet.display()))?;
    let csv_path = dir.join(format!("{}-name-drops.csv", project.name));
    crate::vfs::write(&csv_path, csv).map_err(|e| format!("{}: {e}", csv_path.display()))?;
    Ok(DropExport { files, sheet })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn csv_cell(s: &str) -> String {
    if s.contains(',') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Width (mm) of a laid-out layer, for "this name will be shrunk" warnings.
pub fn layer_width(l: &Layer) -> f64 {
    let b: BBox = l.bbox();
    if b.is_valid() { b.width() } else { 0.0 }
}

const DROP_CSS: &str = r#"<style>
*{box-sizing:border-box}body{margin:0;background:#efede8;color:#1d1d1b;font:14px/1.45 -apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
main{max-width:860px;margin:24px auto;background:#fff;padding:28px 32px;border-radius:10px}
header{display:flex;justify-content:space-between;align-items:flex-start;border-bottom:2px solid #1d1d1b;padding-bottom:10px}
.eyebrow{margin:0;font-size:11px;letter-spacing:.08em;text-transform:uppercase;color:#6b6a65}h1{margin:2px 0 0;font-size:22px}
button{font:inherit;padding:8px 16px;border:1px solid #1d1d1b;background:#1d1d1b;color:#fff;border-radius:6px;cursor:pointer}
.meta{color:#6b6a65;margin:12px 0 4px}.how{background:#fff6e5;border-left:4px solid #9a5b00;padding:8px 12px;border-radius:4px}
h2{font-size:16px;margin:22px 0 8px}h2 span{font-weight:400;color:#6b6a65;font-size:13px;margin-left:8px}
table{width:100%;border-collapse:collapse}th{font-size:11px;text-align:left;color:#6b6a65;border-bottom:1.5px solid #1d1d1b;padding:6px 8px}
td{padding:6px 8px;border-bottom:1px solid #d9d6cf}.n{text-align:right;font-variant-numeric:tabular-nums}
section{break-inside:avoid}footer{margin-top:22px;font-size:11px;color:#6b6a65;border-top:1px solid #d9d6cf;padding-top:8px}
@media (max-width:640px){main{margin:0;padding:16px;border-radius:0}}
@media print{body{background:#fff}main{margin:0;max-width:none;padding:0}button{display:none}}
</style>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TextSpec;

    fn setup_with(names: &[&str], heads: usize, per: usize) -> DropSetup {
        let mut raw = vec![vec![
            "First".to_string(),
            "Last".to_string(),
            "Qty".to_string(),
        ]];
        for n in names {
            let mut p = n.split(' ');
            raw.push(vec![
                p.next().unwrap_or("").into(),
                p.next().unwrap_or("").into(),
                "1".into(),
            ]);
        }
        DropSetup {
            table: Some(Table {
                source: "t.csv".into(),
                raw,
            }),
            columns: vec![0, 1],
            heads,
            per_drop: per,
            ..Default::default()
        }
    }

    #[test]
    fn csv_quotes_and_commas() {
        let rows = parse_csv(
            "Name,Note\n\"Smith, Jr\",\"said \"\"hi\"\"\"\r\nLee,x\n",
            ',',
        );
        assert_eq!(rows[1], vec!["Smith, Jr", "said \"hi\""]);
        assert_eq!(rows[2], vec!["Lee", "x"]);
    }

    #[test]
    fn heads_and_passes() {
        let s = setup_with(
            &[
                "A a", "B b", "C c", "D d", "E e", "F f", "G g", "H h", "I i",
            ],
            4,
            8,
        );
        let n = s.names();
        assert_eq!(n.len(), 9);
        assert_eq!((n[0].drop, n[0].head, n[0].pass), (0, 1, 1));
        assert_eq!((n[5].drop, n[5].head, n[5].pass), (0, 2, 2)); // 6th name: 2nd pass on head 2
        assert_eq!((n[8].drop, n[8].head, n[8].pass), (1, 1, 1));
        assert_eq!(n[0].row, 2);
        assert_eq!(s.drop_count(), 2);
    }

    #[test]
    fn case_quantity_blank_and_edits() {
        let mut s = setup_with(&["ann lee", "", "bo diaz"], 6, 6);
        s.case = Case::Upper;
        s.qty_column = Some(2);
        if let Some(t) = &mut s.table {
            t.raw[3][2] = "2".into();
        }
        s.edits.insert(0, "ANNE LEE".into());
        let n: Vec<String> = s.names().into_iter().map(|x| x.text).collect();
        assert_eq!(n, vec!["ANNE LEE", "BO DIAZ", "BO DIAZ"]);
    }

    #[test]
    fn drop_has_logo_then_a_stop_per_name() {
        let (opt, _) = crate::import::font_options(None).unwrap();
        let logo_art =
            crate::import::load_svg_str(include_str!("../examples/left-chest-logo.svg"), &opt)
                .unwrap();
        let logo = Layer::new(
            1,
            "Logo".into(),
            logo_art
                .shapes
                .into_iter()
                .filter(|s| s.group <= 2)
                .collect(),
            false,
        );
        let spec = TextSpec {
            text: "Name".into(),
            font: "DejaVu Sans".into(),
            height_mm: 8.0,
            rgb: [20, 20, 20],
            ..Default::default()
        };
        let mut name = Layer::new(2, "Name".into(), text_shapes(&opt, &spec).unwrap(), false);
        name.origin = crate::geom::Pt::new(0.0, 0.0);
        name.offset = crate::geom::Pt::new(40.0, 120.0);
        name.text = Some(spec);
        let mut p = Project::new("team", vec![logo, name]);
        p.drops = setup_with(&["Ann Lee", "Bo Diaz", "Cy Ng"], 2, 4);
        p.drops.logo_layers = vec![1];
        p.drops.name_layer = Some(2);
        p.drops.case = Case::Upper;
        let (d, n_logo, slots) = build_drop(&p, &opt, 0).unwrap();
        assert_eq!(n_logo, 1);
        assert_eq!(slots.len(), 3);
        let plan = stop_plan(&d, n_logo, &slots);
        let names: Vec<&(usize, String, String, String)> =
            plan.iter().filter(|r| r.2.starts_with("Name")).collect();
        assert_eq!(names.len(), 3, "{plan:?}");
        assert_eq!(names[2].3, "Head 1 only (pass 2)");
        assert!(plan[0].3 == "All heads");
        // same capital height for every name (no descenders in upper case)
        let (layers, _, _) = drop_layers(&p, &opt, 0).unwrap();
        let hs: Vec<f64> = layers[1..].iter().map(|l| l.bbox().height()).collect();
        assert!(hs.iter().all(|h| (*h - 8.0).abs() < 0.6), "{hs:?}");
    }

    #[test]
    fn identical_names_sew_identically() {
        // Names share one spot; none may cut holes in another.
        let (opt, _) = crate::import::font_options(None).unwrap();
        let spec = TextSpec {
            text: "Name".into(),
            font: "DejaVu Sans".into(),
            height_mm: 8.0,
            rgb: [20, 20, 20],
            ..Default::default()
        };
        let mut name = Layer::new(2, "Name".into(), text_shapes(&opt, &spec).unwrap(), false);
        name.origin = crate::geom::Pt::new(0.0, 0.0);
        name.text = Some(spec);
        let mut p = Project::new("team", vec![name]);
        p.drops = setup_with(&["SAM LEE", "SAM LEE", "SAM LEE"], 3, 3);
        p.drops.name_layer = Some(2);
        let (d, _, _) = build_drop(&p, &opt, 0).unwrap();
        let per_layer: Vec<usize> = (0..3)
            .map(|l| {
                d.objects
                    .iter()
                    .filter(|o| o.layer == l)
                    .map(|o| o.stitches)
                    .sum()
            })
            .collect();
        assert!(
            per_layer
                .iter()
                .all(|c| (*c as i64 - per_layer[2] as i64).abs() < 20),
            "{per_layer:?}"
        );
    }
}
