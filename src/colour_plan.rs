//! User-assisted tracing: the user names each colour, gives it a sew order,
//! a stitch type and a fidelity (blanket, exact or mixed). Each colour is
//! traced on its own, over the same canvas, so the pieces line up, and the
//! result is one SVG in sew order (first colour at the bottom).
//!
//! Colours are chosen by the user, not matched to a thread. A close thread
//! can be shown as a hint by the caller; nothing here depends on it.

use crate::trace::{self, Detail, Raster, Source, TraceOptions};
use serde::{Deserialize, Serialize};

/// How closely a colour is traced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Fidelity {
    /// Simplified, smoothed shapes: large areas where exact edges don't matter.
    Blanket,
    /// Fine tracing: small specks and thin lines kept, edges followed closely.
    Exact,
    /// Between the two.
    #[default]
    Mixed,
}

impl Fidelity {
    /// Trace settings (smoothing, speck area, thinnest line, curve tolerance), all in mm.
    pub fn options(self) -> (f64, f64, f64, f64) {
        match self {
            Fidelity::Blanket => (0.6, 0.8, 0.6, 0.15),
            Fidelity::Mixed => (0.25, 0.3, 0.3, 0.05),
            Fidelity::Exact => (0.05, 0.05, 0.1, 0.02),
        }
    }
}

/// Stitch type for a colour's shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StitchKind {
    #[default]
    Auto,
    Fill,
    Satin,
    Run,
    TripleRun,
    /// Satin with a run stitch down the middle first (for gold and fine lines).
    SatinRun,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanColour {
    /// What the user calls it, e.g. "Dark outline".
    pub name: String,
    /// The colour as picked in the artwork.
    pub rgb: [u8; 3],
    /// Sew order: lower sews first (the base).
    pub order: u32,
    #[serde(default)]
    pub stitch: StitchKind,
    #[serde(default)]
    pub fidelity: Fidelity,
    /// Pieces too small to fill: "run" (default), "fill" or "skip".
    #[serde(default)]
    pub small: crate::import::SmallPieces,
}

/// A single area (one shape in the vector file) with its own setting,
/// e.g. "just blanket-fill this one". `id` is the shape id in the vector file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AreaOverride {
    pub id: String,
    #[serde(default)]
    pub stitch: StitchKind,
    #[serde(default)]
    pub small: Option<crate::import::SmallPieces>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Plan {
    pub colours: Vec<PlanColour>,
    #[serde(default)]
    pub areas: Vec<AreaOverride>,
}

impl Plan {
    pub fn from_json(s: &str) -> Result<Plan, String> {
        let p: Plan = serde_json::from_str(s).map_err(|e| format!("plan file: {e}"))?;
        if p.colours.is_empty() {
            return Err("plan has no colours".into());
        }
        Ok(p)
    }

    /// Colours in sew order (base first).
    pub fn sorted(&self) -> Vec<&PlanColour> {
        let mut v: Vec<&PlanColour> = self.colours.iter().collect();
        v.sort_by_key(|c| c.order);
        v
    }
}

/// The colour in the plan nearest to a pixel (squared RGB distance).
fn nearest(plan: &Plan, p: [u8; 4]) -> usize {
    let mut best = (f64::MAX, 0usize);
    for (i, c) in plan.colours.iter().enumerate() {
        let d: f64 = (0..3).map(|k| (p[k] as f64 - c.rgb[k] as f64).powi(2)).sum();
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

/// Background: transparent, or nearer to white paper than to any picked
/// colour. (Without this the white around the art goes to whichever picked
/// colour is palest, and that colour swallows the background.)
fn is_background(plan: &Plan, p: [u8; 4]) -> bool {
    if p[3] < 128 {
        return true;
    }
    let d = |c: [u8; 3]| -> f64 { (0..3).map(|k| (p[k] as f64 - c[k] as f64).powi(2)).sum() };
    let white = d([255, 255, 255]);
    plan.colours.iter().all(|c| white < d(c.rgb))
}

/// One traced colour, ready to write out.
pub struct PlanLayer {
    pub name: String,
    pub rgb: [u8; 3],
    pub order: u32,
    pub stitch: StitchKind,
    pub paths: Vec<crate::geom::VPath>,
}

/// Trace every colour in the plan over the same canvas, in sew order.
pub fn trace_plan(pic: &Raster, plan: &Plan, width_mm: f64) -> Result<Vec<PlanLayer>, String> {
    let (w, h) = (pic.w, pic.h);
    let mut out = Vec::new();
    for c in plan.sorted() {
        // Keep only this colour's pixels; everything else becomes white
        // background, which the tracer removes.
        let px: Vec<[u8; 4]> = pic
            .px
            .iter()
            .map(|&p| {
                if !is_background(plan, p) && nearest(plan, p) == plan.colours.iter().position(|x| x.rgb == c.rgb).unwrap_or(usize::MAX) {
                    [c.rgb[0], c.rgb[1], c.rgb[2], 255]
                } else {
                    [255, 255, 255, 255]
                }
            })
            .collect();
        let mask = Raster { w, h, px };
        let (smooth, speck, min_line, fit) = c.fidelity.options();
        let o = TraceOptions {
            width_mm,
            detail: Detail::Balanced,
            // a clean colour mask, never a garment photo: don't let the
            // tracer 'remove the garment' (it took the star for one)
            source: Source::Logo,
            remove_background: true,
            crop: Some([0, 0, w, h]),
            // white is the background: give it its own entry so it is removed
            fixed_palette: vec![c.rgb, [255, 255, 255]],
            smooth_mm: Some(smooth),
            speck_mm2: Some(speck),
            min_line_mm: Some(min_line),
            fit_tol_mm: Some(fit),
            ..Default::default()
        };
        let t = trace::trace(&mask, &o)?;
        // keep only this colour's layers. The tracer adjusts colours slightly,
        // so the white (everything else) layer is not exactly white: judge by
        // which of the two it is nearer, or the rest of the picture would be
        // stitched in this colour.
        let d2 = |a: [u8; 3], b: [u8; 3]| (0..3).map(|k| (a[k] as f64 - b[k] as f64).powi(2)).sum::<f64>();
        let paths: Vec<_> = t
            .layers
            .iter()
            .filter(|l| d2(l.rgb, c.rgb) < d2(l.rgb, [255, 255, 255]))
            .flat_map(|l| l.paths.clone())
            .collect();
        out.push(PlanLayer { name: c.name.clone(), rgb: c.rgb, order: c.order, stitch: c.stitch, paths });
    }
    Ok(out)
}

/// Write the traced layers as one SVG, in sew order. Fills are the picked
/// colours, so a later `convert` can match each shape to its colour.
pub fn to_svg(layers: &[PlanLayer], pic: &Raster, mm_per_px: f64) -> String {
    let (wm, hm) = (pic.w as f64 * mm_per_px, pic.h as f64 * mm_per_px);
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{wm:.2}mm' height='{hm:.2}mm' viewBox='0 0 {wm:.3} {hm:.3}'>\n"
    );
    for l in layers {
        // one path per colour, even-odd: an outline inside another is a hole
        // in it (written as separate paths, a hole would become a filled
        // shape sitting on top). Separate pieces are still separate parts.
        let d: String = l.paths.iter().map(|v| crate::vector::path_d(v, &|p| p)).collect::<Vec<_>>().join(" ");
        if d.trim().is_empty() {
            continue;
        }
        let id: String = l.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '-' }).collect();
        s.push_str(&format!(
            "  <path id='{id}' fill='#{:02x}{:02x}{:02x}' fill-rule='evenodd' d='{d}'/>\n",
            l.rgb[0], l.rgb[1], l.rgb[2]
        ));
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_parses_and_sorts_by_order() {
        let p = Plan::from_json(
            r#"{"colours":[
                {"name":"Gold","rgb":[155,151,106],"order":3,"stitch":"satin-run","fidelity":"exact"},
                {"name":"Base","rgb":[56,117,46],"order":1,"stitch":"fill","fidelity":"blanket"}
            ]}"#,
        )
        .unwrap();
        let s = p.sorted();
        assert_eq!(s[0].name, "Base");
        assert_eq!(s[1].name, "Gold");
        assert_eq!(s[1].stitch, StitchKind::SatinRun);
        assert_eq!(s[1].fidelity, Fidelity::Exact);
    }

    #[test]
    fn empty_plan_is_rejected() {
        assert!(Plan::from_json(r#"{"colours":[]}"#).is_err());
    }

    #[test]
    fn nearest_picks_closest_colour() {
        let p = Plan {
            areas: vec![],
            colours: vec![
                PlanColour { name: "a".into(), rgb: [0, 0, 0], order: 1, stitch: StitchKind::Auto, fidelity: Fidelity::Mixed, small: Default::default() },
                PlanColour { name: "b".into(), rgb: [255, 255, 255], order: 2, stitch: StitchKind::Auto, fidelity: Fidelity::Mixed, small: Default::default() },
            ],
        };
        assert_eq!(nearest(&p, [20, 10, 5, 255]), 0);
        assert_eq!(nearest(&p, [240, 250, 255, 255]), 1);
    }

    #[test]
    fn white_paper_is_background_unless_white_is_picked() {
        let col = |rgb| PlanColour { name: "c".into(), rgb, order: 1, stitch: StitchKind::Auto, fidelity: Fidelity::Mixed, small: Default::default() };
        let greens = Plan { areas: vec![], colours: vec![col([56, 117, 46]), col([155, 151, 106])] };
        assert!(is_background(&greens, [250, 250, 248, 255]));
        assert!(!is_background(&greens, [150, 150, 100, 255]));
        assert!(is_background(&greens, [150, 150, 100, 0]));
        let with_white = Plan { areas: vec![], colours: vec![col([56, 117, 46]), col([250, 250, 250])] };
        assert!(!is_background(&with_white, [252, 252, 252, 255]));
    }
}
