//! StitchForge — clean-room embroidery engine (v0.1, Phase 1 core).
//!
//! Built only from the StitchForge spec, public format documentation and
//! general embroidery practice. No competitor code, files or UI were used.


use clap::{Args, Parser, Subcommand};
use stitchforge::{design, dst, export, import, settings, sheet, thread};
use stitchforge::settings::Profile;
use std::path::PathBuf;

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
        let summary = export::write_all(&d, &job, &o.out_dir, &generated)?.summary;
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
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let d = dst::read(&bytes)?;
    let (mut minx, mut maxx, mut miny, mut maxy) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, y) in &d.stitches {
        minx = minx.min(x);
        maxx = maxx.max(x);
        miny = miny.min(y);
        maxy = maxy.max(y);
    }
    println!(
        "{}: label '{}' · {} stitches (header {}) · {} jumps · {} color changes (header {}) · size {:.1} x {:.1} mm · end marker {}",
        file.display(),
        d.label,
        d.stitches.len(),
        d.header_stitches,
        d.jumps,
        d.color_changes,
        d.header_colors,
        (maxx - minx) as f64 / 10.0,
        (maxy - miny) as f64 / 10.0,
        if d.ended { "ok" } else { "MISSING" }
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
