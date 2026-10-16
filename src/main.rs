//! StitchForge — clean-room embroidery engine (v0.1, Phase 1 core).
//!
//! Built only from the StitchForge spec, public format documentation and
//! general embroidery practice. No competitor code, files or UI were used.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use stitchforge::settings::Profile;
use stitchforge::{design, export, import, settings, sheet, thread};

#[derive(Parser)]
#[command(
    name = "stitchforge",
    version,
    about = "Vector art and any-font text to machine-ready embroidery, with operator spec sheets"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert an SVG file (from Illustrator, Inkscape, CorelDRAW, Affinity...) to stitches
    Convert {
        /// Input SVG file
        input: PathBuf,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Stitch a line of text in any installed font (or a .ttf/.otf file)
    Text {
        /// The words to stitch
        text: String,
        /// Font family name as installed, e.g. "Poppins"
        #[arg(long)]
        font: Option<String>,
        /// Use this .ttf or .otf file directly
        #[arg(long)]
        font_file: Option<String>,
        /// Thread color as hex, e.g. "#1d3a8a"
        #[arg(long, default_value = "#18243f")]
        color: String,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Write the Gate 1 calibration sampler (a test design to sew)
    Sampler {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Name drops: logo on every head, then one name per head, from a sheet
    Drops {
        /// StitchForge project (.sfg) with the logo and a text layer for the name
        project: PathBuf,
        /// Excel (.xlsx) or CSV with the names
        #[arg(long)]
        data: PathBuf,
        /// Name columns: letters (A,B) or header text (First,Last)
        #[arg(long, default_value = "A")]
        columns: String,
        /// Put each column on its own line instead of joining with spaces
        #[arg(long)]
        lines: bool,
        /// upper, title or as-is
        #[arg(long, default_value = "as-is")]
        case: String,
        /// Number of heads on the machine
        #[arg(long, default_value_t = 6)]
        heads: usize,
        /// Names per drop file (default = heads; up to 2x heads)
        #[arg(long)]
        per_drop: Option<usize>,
        /// Name of the text layer to use for names (default: first text layer)
        #[arg(long)]
        name_layer: Option<String>,
        /// Shrink names wider than this many inches
        #[arg(long)]
        max_width_in: Option<f64>,
        /// Quantity column (letter or header): a name with Qty 3 sews 3 times
        #[arg(long)]
        qty: Option<String>,
        /// Machine formats, comma separated
        #[arg(long, default_value = "dst")]
        formats: String,
        #[arg(long, short, default_value = "out")]
        out_dir: PathBuf,
    },
    /// List installed font families
    Fonts,
    /// Read a DST file and report what is in it
    Inspect { file: PathBuf },
}

#[derive(Args)]
struct OutputArgs {
    /// Sewing order: flat, cap, or both (exports two versions)
    #[arg(long, default_value = "flat")]
    profile: String,
    /// Fabric recipe: pique, tshirt, fleece, twill, cap, towel (default depends on profile)
    #[arg(long)]
    fabric: Option<String>,
    /// Final design width in mm (keeps proportions)
    #[arg(long)]
    width_mm: Option<f64>,
    /// Final design height in mm (keeps proportions)
    #[arg(long)]
    height_mm: Option<f64>,
    /// Final width in inches (alternative to --width-mm)
    #[arg(long)]
    width_in: Option<f64>,
    /// Final height in inches (alternative to --height-mm)
    #[arg(long)]
    height_in: Option<f64>,
    /// Thread chart CSV (code,name,r,g,b); default is the built-in generic chart
    #[arg(long)]
    threads: Option<String>,
    /// Output folder
    #[arg(long, short, default_value = "out")]
    out_dir: PathBuf,
    /// Base file name (defaults to the input name)
    #[arg(long)]
    name: Option<String>,
    /// Machine formats to write, comma separated: dst, pes, exp
    #[arg(long, default_value = "dst")]
    formats: String,
    /// Print a JSON summary (for scripts and AI agents)
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = "")]
    job: String,
    #[arg(long, default_value = "")]
    customer: String,
    #[arg(long, default_value = "")]
    due: String,
    #[arg(long, default_value = "")]
    qty: String,
    #[arg(long, default_value = "")]
    machine: String,
    #[arg(long, default_value = "")]
    placement: String,
    #[arg(long, default_value = "")]
    garment: String,
    #[arg(long, default_value = "")]
    notes: String,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Command::Convert { input, out } => {
            let name = out.name.clone().unwrap_or_else(|| stem(&input));
            import::load_svg_file(&input.to_string_lossy(), None)
                .and_then(|art| produce(art, &name, &out))
        }
        Command::Text {
            text,
            font,
            font_file,
            color,
            out,
        } => {
            let rgb = parse_hex(&color);
            let name = out.name.clone().unwrap_or_else(|| slug(&text));
            let out = OutputArgs {
                height_mm: out.height_mm.or(
                    if out.width_mm.is_none() && out.width_in.is_none() && out.height_in.is_none() {
                        Some(20.0)
                    } else {
                        None
                    },
                ),
                ..out
            };
            rgb.and_then(|rgb| import::load_text(&text, font.as_deref(), font_file.as_deref(), rgb))
                .and_then(|art| produce(art, &name, &out))
        }
        Command::Sampler { out } => {
            let name = out
                .name
                .clone()
                .unwrap_or_else(|| "calibration-sampler".into());
            let out = OutputArgs {
                notes: if out.notes.is_empty() {
                    stitchforge::sampler::NOTES.into()
                } else {
                    out.notes.clone()
                },
                ..out
            };
            import::font_options(None)
                .and_then(|(opt, _)| import::load_svg_str(&stitchforge::sampler::svg(), &opt))
                .and_then(|art| produce(art, &name, &out))
        }
        Command::Drops {
            project,
            data,
            columns,
            lines,
            case,
            heads,
            per_drop,
            name_layer,
            max_width_in,
            qty,
            formats,
            out_dir,
        } => run_drops(
            &project,
            &data,
            &columns,
            lines,
            &case,
            heads,
            per_drop,
            name_layer,
            max_width_in,
            qty,
            &formats,
            &out_dir,
        ),
        Command::Fonts => {
            for f in import::list_fonts() {
                println!("{f}");
            }
            Ok(())
        }
        Command::Inspect { file } => inspect(&file),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn produce(mut art: import::Artwork, name: &str, o: &OutputArgs) -> Result<(), String> {
    let w = o.width_mm.or(o.width_in.map(|v| v * 25.4));
    let h = o.height_mm.or(o.height_in.map(|v| v * 25.4));
    import::fit(&mut art, w, h);
    let chart = match &o.threads {
        Some(p) => thread::load_chart_csv(p)?,
        None => thread::generic_chart(),
    };
    let profiles: Vec<Profile> = match o.profile.as_str() {
        "both" => vec![Profile::Flat, Profile::Cap],
        p => vec![
            Profile::parse(p).ok_or(format!("unknown profile '{p}' (use flat, cap or both)"))?,
        ],
    };
    std::fs::create_dir_all(&o.out_dir).map_err(|e| e.to_string())?;
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let mut summaries = Vec::new();
    for profile in profiles {
        let fabric = o
            .fabric
            .clone()
            .unwrap_or_else(|| profile.default_recipe().into());
        let recipe = settings::recipe(&fabric).ok_or(format!(
            "unknown fabric '{fabric}' (use {})",
            settings::RECIPES.join(", ")
        ))?;
        let base = if o.profile == "both" {
            format!(
                "{name}-{}",
                if profile == Profile::Cap {
                    "cap"
                } else {
                    "flat"
                }
            )
        } else {
            name.to_string()
        };
        let d = design::build(&base, &art, &chart, &recipe, profile)?;
        let job = sheet::JobInfo {
            job: o.job.clone(),
            customer: o.customer.clone(),
            due: o.due.clone(),
            quantity: o.qty.clone(),
            machine: o.machine.clone(),
            placement: o.placement.clone(),
            garment: if o.garment.is_empty() && profile == Profile::Cap {
                "Cap".into()
            } else {
                o.garment.clone()
            },
            notes: o.notes.clone(),
        };
        let fmts: Vec<stitchforge::formats::Format> = o
            .formats
            .split(',')
            .map(|f| {
                stitchforge::formats::Format::from_ext(f.trim())
                    .ok_or(format!("unknown format '{f}' (use dst, pes, exp)"))
            })
            .collect::<Result<_, _>>()?;
        let summary = export::write_formats(&d, &job, &o.out_dir, &generated, &fmts)?.summary;
        if !o.json {
            print_human(&d, &base, &o.out_dir);
        }
        summaries.push(summary);
    }
    if o.json {
        println!("{}", serde_json::to_string_pretty(&summaries).unwrap());
    }
    Ok(())
}

fn print_human(d: &design::Design, base: &str, dir: &std::path::Path) {
    let s = &d.stats;
    println!(
        "{base}: {:.2}\" x {:.2}\" ({:.1} x {:.1} mm) · {} stitches · {} colors · {} trims · ~{:.1} min · {}",
        s.width_mm / 25.4,
        s.height_mm / 25.4,
        s.width_mm,
        s.height_mm,
        s.stitches,
        d.stops.len(),
        s.trims,
        s.run_minutes,
        d.profile.label()
    );
    for (i, st) in d.stops.iter().enumerate() {
        println!(
            "  stop {} · needle {} · {} {} · {} stitches",
            i + 1,
            st.needle,
            st.thread.code,
            st.thread.name,
            st.stitches
        );
    }
    for w in &d.warnings {
        println!("  ! {w}");
    }
    println!(
        "  -> {}/{base}.dst, .sheet.html, .stops.csv, .preview.png",
        dir.display()
    );
}

fn inspect(file: &PathBuf) -> Result<(), String> {
    use stitchforge::design::Cmd;
    let f = stitchforge::formats::read_file(file)?;
    let (mut st, mut jumps, mut trims) = (0, 0, 0);
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for c in &f.cmds {
        match c {
            Cmd::Stitch(p) => {
                st += 1;
                minx = minx.min(p.x);
                maxx = maxx.max(p.x);
                miny = miny.min(p.y);
                maxy = maxy.max(p.y);
            }
            Cmd::Jump(_) => jumps += 1,
            Cmd::Trim => trims += 1,
            _ => {}
        }
    }
    println!(
        "{} ({}): label '{}' · {} stitches · {} jumps · {} trims · {} color stops · size {:.1} x {:.1} mm",
        file.display(),
        f.format,
        f.label,
        st,
        jumps,
        trims,
        f.stop_colors.len(),
        maxx - minx,
        maxy - miny
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_drops(
    project: &std::path::Path,
    data: &std::path::Path,
    columns: &str,
    lines: bool,
    case: &str,
    heads: usize,
    per_drop: Option<usize>,
    name_layer: Option<String>,
    max_width_in: Option<f64>,
    qty: Option<String>,
    formats: &str,
    out_dir: &std::path::Path,
) -> Result<(), String> {
    use stitchforge::drops::{self, Case, Join};
    let mut p = stitchforge::project::Project::load(project)?;
    let table = drops::read_table(data)?;
    let headers = table.headers(true);
    let find_col = |c: &str| -> Result<usize, String> {
        let by_letter = (0..table.width()).find(|i| drops::col_letter(*i).eq_ignore_ascii_case(c));
        let by_name = headers.iter().position(|h| {
            h.to_lowercase()
                .starts_with(&format!("{} (", c.to_lowercase()))
        });
        by_letter.or(by_name).ok_or(format!(
            "no column '{c}' in {} (columns: {})",
            table.source,
            headers.join(", ")
        ))
    };
    let mut cols = Vec::new();
    for c in columns.split(',').map(|c| c.trim()) {
        cols.push(find_col(c)?);
    }
    let qty_col = qty.as_deref().map(|q| find_col(q.trim())).transpose()?;
    let first_row_headers = cols.iter().all(|c| {
        table.raw[0]
            .get(*c)
            .map(|v| v.parse::<f64>().is_err())
            .unwrap_or(true)
    }) && columns.split(',').any(|c| c.trim().len() > 2);
    let template = match &name_layer {
        Some(n) => p
            .layers
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(n) && l.text.is_some()),
        None => p.layers.iter().find(|l| l.text.is_some()),
    }
    .ok_or("the project needs a text layer for the names (add one in the app with Add text)")?
    .id;
    let s = &mut p.drops;
    s.table = Some(table);
    s.first_row_headers = first_row_headers;
    s.columns = cols;
    s.join = if lines { Join::NewLine } else { Join::Space };
    s.case = match case {
        "upper" => Case::Upper,
        "title" => Case::Title,
        _ => Case::AsTyped,
    };
    s.heads = heads.max(1);
    s.per_drop = per_drop.unwrap_or(heads).clamp(1, heads.max(1) * 2);
    s.name_layer = Some(template);
    s.logo_layers = p
        .layers
        .iter()
        .filter(|l| l.id != template && l.visible)
        .map(|l| l.id)
        .collect();
    s.max_width_mm = max_width_in.map(|w| w * 25.4);
    s.qty_column = qty_col;
    let fmts: Vec<stitchforge::formats::Format> = formats
        .split(',')
        .map(|f| {
            stitchforge::formats::Format::from_ext(f.trim()).ok_or(format!("unknown format '{f}'"))
        })
        .collect::<Result<_, _>>()?;
    let (opt, _) = import::font_options(None)?;
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let ex = drops::export_drops(&p, &opt, out_dir, &fmts, &generated)?;
    println!(
        "{} names · {} drop file(s) · {} heads · {} per drop -> {}",
        p.drops.names().len(),
        p.drops.drop_count(),
        p.drops.heads,
        p.drops.per_drop,
        ex.sheet.display()
    );
    Ok(())
}

fn parse_hex(s: &str) -> Result<[u8; 3], String> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 {
        return Err(format!("color '{s}' must be 6-digit hex like #1d3a8a"));
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("bad color '{s}'"));
    Ok([p(0)?, p(2)?, p(4)?])
}

fn stem(p: &std::path::Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "design".into())
}

fn slug(t: &str) -> String {
    let s: String = t
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s
        .split('-')
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "text".into()
    } else {
        s.chars().take(40).collect()
    }
}
