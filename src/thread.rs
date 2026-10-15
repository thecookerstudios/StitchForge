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

pub fn load_chart_csv(path: &str) -> Result<Vec<Thread>, String> {
    let data = crate::vfs::read_to_string(std::path::Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))?;
    let brand = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (i, line) in data.lines().enumerate() {
        let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if f.len() < 5 || f[0].eq_ignore_ascii_case("code") || line.trim().is_empty() {
            continue;
        }
        let num = |s: &str| {
            s.parse::<u8>()
                .map_err(|_| format!("{path} line {}: bad color value '{s}'", i + 1))
        };
        out.push(Thread {
            brand: brand.clone(),
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
    Ok(out)
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

/// Perceptual colour difference (CIE76 ΔE).
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (p, q) = (lab(a), lab(b));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

pub fn nearest(chart: &[Thread], rgb: [u8; 3]) -> (usize, f64) {
    chart
        .iter()
        .enumerate()
        .map(|(i, t)| (i, delta_e(t.rgb, rgb)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_navy_not_black() {
        let chart = generic_chart();
        let (i, _) = nearest(&chart, [20, 40, 90]);
        assert_eq!(chart[i].name, "Navy");
    }
}
