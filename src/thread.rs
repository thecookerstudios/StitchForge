//! Thread charts and colour matching in CIELAB space (perceptual distance,
//! not raw RGB). The built-in chart is an original generic 40 wt palette;
//! brand charts load from CSV: `code,name,r,g,b`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub brand: String,
    pub code: String,
    pub name: String,
    pub rgb: [u8; 3],
}

const GENERIC: &[(&str, &str, [u8; 3])] = &[
    ("SF-001", "White", [250, 250, 248]),
    ("SF-002", "Natural", [236, 228, 208]),
    ("SF-003", "Silver Gray", [176, 178, 180]),
    ("SF-004", "Charcoal", [70, 72, 76]),
    ("SF-005", "Black", [18, 18, 20]),
    ("SF-010", "Lemon", [250, 226, 60]),
    ("SF-011", "Athletic Gold", [244, 178, 26]),
    ("SF-012", "Orange", [240, 110, 30]),
    ("SF-013", "Tangerine", [252, 150, 70]),
    ("SF-020", "Red", [200, 24, 40]),
    ("SF-021", "Cardinal", [150, 20, 36]),
    ("SF-022", "Maroon", [100, 22, 36]),
    ("SF-023", "Coral", [240, 110, 100]),
    ("SF-024", "Pink", [244, 160, 190]),
    ("SF-025", "Magenta", [200, 40, 130]),
    ("SF-030", "Lavender", [180, 160, 210]),
    ("SF-031", "Purple", [96, 50, 140]),
    ("SF-040", "Navy", [24, 36, 80]),
    ("SF-041", "Royal Blue", [30, 70, 170]),
    ("SF-042", "Columbia Blue", [130, 180, 230]),
    ("SF-043", "Sky Blue", [90, 150, 215]),
    ("SF-044", "Turquoise", [20, 170, 180]),
    ("SF-045", "Teal", [10, 110, 120]),
    ("SF-050", "Kelly Green", [20, 140, 70]),
    ("SF-051", "Forest Green", [20, 80, 50]),
    ("SF-052", "Lime", [150, 200, 60]),
    ("SF-053", "Olive", [100, 110, 50]),
    ("SF-060", "Tan", [200, 170, 120]),
    ("SF-061", "Vegas Gold", [196, 176, 110]),
    ("SF-062", "Brown", [100, 66, 40]),
    ("SF-063", "Copper", [180, 100, 50]),
];

/// Thread weight (wt): the higher the number, the finer the thread. Stitch
/// recipes are tuned for 40 wt; other weights scale spacing, limits and
/// needle size from there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ThreadWeight {
    W12,
    W30,
    #[default]
    W40,
    W60,
}

impl ThreadWeight {
    pub const ALL: [ThreadWeight; 4] = [Self::W12, Self::W30, Self::W40, Self::W60];
    pub fn label(&self) -> &'static str {
        match self {
            Self::W12 => "12 wt",
            Self::W30 => "30 wt",
            Self::W40 => "40 wt",
            Self::W60 => "60 wt",
        }
    }
    pub fn number(&self) -> u32 {
        match self {
            Self::W12 => 12,
            Self::W30 => 30,
            Self::W40 => 40,
            Self::W60 => 60,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().trim_end_matches("wt").trim() {
            "12" => Some(Self::W12),
            "30" => Some(Self::W30),
            "40" => Some(Self::W40),
            "60" => Some(Self::W60),
            _ => None,
        }
    }
    /// What it is typically used for.
    pub fn use_for(&self) -> &'static str {
        match self {
            Self::W12 => "bold, hand-stitched look; big simple shapes",
            Self::W30 => "fast coverage on large fills and jacket backs",
            Self::W40 => "standard for almost everything",
            Self::W60 => "small lettering and fine detail",
        }
    }
    /// Apparent width of one strand lying on fabric (mm), for the preview.
    pub fn strand_mm(&self) -> f64 {
        match self {
            Self::W12 => 0.62,
            Self::W30 => 0.40,
            Self::W40 => 0.33,
            Self::W60 => 0.24,
        }
    }
    /// Stitch spacing relative to 40 wt (thinner thread sews closer rows).
    pub fn spacing_factor(&self) -> f64 {
        match self {
            Self::W12 => 1.9,
            Self::W30 => 1.2,
            Self::W40 => 1.0,
            Self::W60 => 0.75,
        }
    }
    /// Recommended needle size (metric/US).
    pub fn needle(&self) -> &'static str {
        match self {
            Self::W12 => "100/16",
            Self::W30 => "90/14",
            Self::W40 => "75/11",
            Self::W60 => "65/9",
        }
    }
    /// Narrowest satin column that still covers and holds (mm).
    pub fn min_column(&self) -> f64 {
        match self {
            Self::W12 => 2.5,
            Self::W30 => 1.3,
            Self::W40 => 1.0,
            Self::W60 => 0.7,
        }
    }
    /// Smallest capital-letter height that reads cleanly (mm).
    pub fn min_text_height(&self) -> f64 {
        match self {
            Self::W12 => 15.0,
            Self::W30 => 6.5,
            Self::W40 => 5.0,
            Self::W60 => 3.5,
        }
    }
    /// Thread per standard cone (m), for usage estimates.
    pub fn cone_m(&self) -> f64 {
        match self {
            Self::W12 => 1000.0,
            Self::W30 => 2500.0,
            Self::W40 => 5000.0,
            Self::W60 => 5000.0,
        }
    }
}

/// Estimated thread use for a run of stitches: top (needle) thread and
/// bobbin, in metres. Top thread follows the stitch path plus what each
/// penetration pulls down into the fabric; bobbin is roughly a third.
pub fn usage_m(path_mm: f64, stitches: usize) -> (f64, f64) {
    let top = (path_mm * 1.10 + stitches as f64 * 0.75) / 1000.0;
    (top, top * 0.35)
}

pub fn generic_chart() -> Vec<Thread> {
    GENERIC
        .iter()
        .map(|(c, n, rgb)| Thread {
            brand: "Generic 40wt".into(),
            code: c.to_string(),
            name: n.to_string(),
            rgb: *rgb,
        })
        .collect()
}

/// A thread library: one manufacturer's (or your own) chart, loaded from CSV.
#[derive(Clone, Debug, Default)]
pub struct Library {
    /// Display name, from `# name:` or the file name.
    pub name: String,
    /// Who makes the threads, from `# manufacturer:`.
    pub manufacturer: String,
    /// Where the colour values came from and under what terms, from `# licence:`.
    pub licence: String,
    pub threads: Vec<Thread>,
}

/// Load a library CSV. Lines starting with `#` are notes:
/// `# name: Madeira Polyneon`, `# manufacturer: Madeira`, `# licence: ...`.
/// Data rows are `code,name,r,g,b`.
pub fn load_library(path: &str) -> Result<Library, String> {
    let data = crate::vfs::read_to_string(std::path::Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))?;
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    parse_library(&stem, &data, path)
}

/// Thread libraries that ship inside StitchForge (file name, contents).
pub const BUILT_IN: &[(&str, &str)] = &[
    ("madeira-polyneon.csv", include_str!("../thread-libraries/madeira/madeira-polyneon.csv")),
    ("madeira-classic-rayon.csv", include_str!("../thread-libraries/madeira/madeira-classic-rayon.csv")),
    ("madeira-frosted-matt.csv", include_str!("../thread-libraries/madeira/madeira-frosted-matt.csv")),
    ("madeira-fire-fighter.csv", include_str!("../thread-libraries/madeira/madeira-fire-fighter.csv")),
    ("madeira-sensa-green.csv", include_str!("../thread-libraries/madeira/madeira-sensa-green.csv")),
    ("madeira-burmilana.csv", include_str!("../thread-libraries/madeira/madeira-burmilana.csv")),
    ("madeira-burmilanaco.csv", include_str!("../thread-libraries/madeira/madeira-burmilanaco.csv")),
    ("madeira-supertwist.csv", include_str!("../thread-libraries/madeira/madeira-supertwist.csv")),
    ("madeira-fs-metallic.csv", include_str!("../thread-libraries/madeira/madeira-fs-metallic.csv")),
    ("madeira-cr-metallic.csv", include_str!("../thread-libraries/madeira/madeira-cr-metallic.csv")),
];

/// Read a library from CSV text (`source` is only used in error messages).
pub fn parse_library(stem: &str, data: &str, source: &str) -> Result<Library, String> {
    let path = source;
    let stem = stem.to_string();
    let mut lib = Library { name: stem.clone(), ..Default::default() };
    let mut out = Vec::new();
    for (i, line) in data.lines().enumerate() {
        if let Some(note) = line.trim().strip_prefix('#') {
            if let Some((k, v)) = note.split_once(':') {
                let v = v.trim().to_string();
                match k.trim().to_ascii_lowercase().as_str() {
                    "name" if !v.is_empty() => lib.name = v,
                    "manufacturer" => lib.manufacturer = v,
                    "licence" | "license" => lib.licence = v,
                    _ => {}
                }
            }
            continue;
        }
        let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if f.len() < 5 || f[0].eq_ignore_ascii_case("code") || line.trim().is_empty() {
            continue;
        }
        let num = |s: &str| {
            s.parse::<u8>()
                .map_err(|_| format!("{path} line {}: bad color value '{s}'", i + 1))
        };
        out.push(Thread {
            brand: lib.name.clone(),
            code: f[0].into(),
            name: f[1].into(),
            rgb: [num(f[2])?, num(f[3])?, num(f[4])?],
        });
    }
    if out.is_empty() {
        return Err(format!(
            "{path}: no threads found (expected code,name,r,g,b)"
        ));
    }
    lib.threads = out;
    Ok(lib)
}

pub fn load_chart_csv(path: &str) -> Result<Vec<Thread>, String> {
    Ok(load_library(path)?.threads)
}

/// Folder where thread libraries live: `StitchForge/Thread libraries` in the
/// user's home folder. Each `.csv` in it is one library (files starting with
/// `_` are ignored, so the template is not listed).
pub fn library_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join("StitchForge").join("Thread libraries")
}

/// Library files found in a folder, sorted by name.
pub fn scan_libraries(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let is_csv = p.extension().map(|e| e.eq_ignore_ascii_case("csv")).unwrap_or(false);
                    let hidden = p.file_name().map(|n| n.to_string_lossy().starts_with('_')).unwrap_or(true);
                    is_csv && !hidden
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The template a user copies to make their own library. Notes are `#` lines;
/// the example rows are placeholders and must be replaced with real values.
pub const LIBRARY_TEMPLATE: &str = "\
# name: My library
# manufacturer: Your name or brand
# licence: Where these colours came from and the terms you have to publish them
# Data rows: code,name,r,g,b   (r, g, b are 0-255 sRGB values)
code,name,r,g,b
EXAMPLE-1,Replace me,200,24,40
EXAMPLE-2,Replace me too,30,70,170
";

/// Write the template into the library folder (never overwrites a file).
pub fn write_template(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("_template.csv");
    if !path.exists() {
        std::fs::write(&path, LIBRARY_TEMPLATE)?;
    }
    Ok(path)
}

fn lab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIELAB of an sRGB colour.
pub fn lab_of(rgb: [u8; 3]) -> [f64; 3] {
    lab(rgb)
}

/// sRGB of a CIELAB colour (clamped to the displayable range).
pub fn rgb_of_lab(l: [f64; 3]) -> [u8; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let fx = fy + l[1] / 500.0;
    let fz = fy - l[2] / 200.0;
    let inv = |t: f64| {
        if t.powi(3) > 0.008856 {
            t.powi(3)
        } else {
            (t - 16.0 / 116.0) / 7.787
        }
    };
    let (x, y, z) = (inv(fx) * 0.95047, inv(fy), inv(fz) * 1.08883);
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    let enc = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        let v = if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (v * 255.0).round() as u8
    };
    [enc(r), enc(g), enc(b)]
}

/// Perceptual colour difference (CIE76 ΔE).
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (p, q) = (lab(a), lab(b));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

/// A plain colour name for an RGB colour ("Red", "Navy"), from the generic chart.
pub fn color_name(rgb: [u8; 3]) -> String {
    let chart = generic_chart();
    chart[nearest(&chart, rgb).0].name.clone()
}

pub fn nearest(chart: &[Thread], rgb: [u8; 3]) -> (usize, f64) {
    // variegated threads (ombre, multi, astro, melange) and ones marked as
    // unverified are never picked automatically, only by hand
    let solid = |t: &Thread| {
        let n = t.name.to_ascii_lowercase();
        !["ombre", "multi", "astro", "variegated", "melange", "unverified", "discontinued"].iter().any(|w| n.contains(w))
    };
    let best = |only_solid: bool| {
        chart
            .iter()
            .enumerate()
            .filter(|(_, t)| !only_solid || solid(t))
            .map(|(i, t)| (i, delta_e(t.rgb, rgb)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
    };
    best(true).or_else(|| best(false)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn built_in_libraries_load() {
        for (file, data) in BUILT_IN {
            let lib = parse_library(file, data, file).unwrap();
            assert!(lib.threads.len() >= 30, "{file}");
            assert!(lib.name.starts_with("Madeira"), "{file}");
        }
    }
    #[test]
    fn auto_match_skips_variegated() {
        let t = |c: &str, n: &str, rgb| Thread { brand: "x".into(), code: c.into(), name: n.into(), rgb };
        let chart = vec![t("1", "Orange Ombre", [240, 150, 20]), t("2", "Orange", [230, 130, 30])];
        assert_eq!(nearest(&chart, [240, 150, 20]).0, 1);
        let only = vec![t("1", "Orange Ombre", [240, 150, 20])];
        assert_eq!(nearest(&only, [240, 150, 20]).0, 0);
    }
    #[test]
    fn weights() {
        assert_eq!(ThreadWeight::default(), ThreadWeight::W40);
        assert_eq!(ThreadWeight::parse("60wt"), Some(ThreadWeight::W60));
        assert!(ThreadWeight::W60.spacing_factor() < 1.0);
        let (t, b) = usage_m(10_000.0, 2000);
        assert!(t > 10.0 && b < t);
    }
    #[test]
    fn library_notes_and_template() {
        let dir = std::env::temp_dir().join(format!("sf-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let t = write_template(&dir).unwrap();
        // the template is a valid library, with its notes read
        let lib = load_library(t.to_str().unwrap()).unwrap();
        assert_eq!(lib.name, "My library");
        assert_eq!(lib.manufacturer, "Your name or brand");
        assert!(lib.licence.starts_with("Where these colours"));
        assert_eq!(lib.threads.len(), 2);
        assert_eq!(lib.threads[0].code, "EXAMPLE-1");
        // underscore files are templates, not libraries, so they are not listed
        assert!(scan_libraries(&dir).is_empty());
        let real = dir.join("madeira.csv");
        std::fs::write(&real, "# name: Test chart\n# manufacturer: Test\ncode,name,r,g,b\n1,Red,200,24,40\n").unwrap();
        let found = scan_libraries(&dir);
        assert_eq!(found, vec![real.clone()]);
        let lib = load_library(real.to_str().unwrap()).unwrap();
        assert_eq!(lib.name, "Test chart");
        assert_eq!(lib.threads[0].brand, "Test chart");
        assert_eq!(load_chart_csv(real.to_str().unwrap()).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_navy_not_black() {
        let chart = generic_chart();
        let (i, _) = nearest(&chart, [20, 40, 90]);
        assert_eq!(chart[i].name, "Navy");
    }
}
