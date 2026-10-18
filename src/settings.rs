//! Fabric recipes (stitch defaults per material) and garment profiles
//! (how a design sews: flat goods vs caps). Both are plain data so shops can
//! later edit and share them.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub name: String,
    /// Spacing between fill rows (mm).
    pub fill_spacing: f64,
    /// Fill stitch length (mm).
    pub fill_len: f64,
    /// Spacing between satin stitches on the same side (mm).
    pub satin_spacing: f64,
    /// Added to each end of fill and satin stitches (mm).
    pub pull_comp: f64,
    /// Spacing of underlay rows (mm).
    pub underlay_spacing: f64,
    /// How far underlay stays inside the edge (mm).
    pub underlay_inset: f64,
    pub backing: String,
    pub topping: String,
    /// The design's default thread weight (layers may override it). The
    /// spacings above are for 40 wt and are scaled for other weights.
    #[serde(default)]
    pub weight: crate::thread::ThreadWeight,
    /// Cap front size limits (adult or youth) when sewing on caps.
    #[serde(default)]
    pub cap_size: CapSize,
}

/// Cap front sizes: the most a standard structured cap front takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapSize {
    /// Adult caps: up to 4" wide × 2.25" high.
    #[default]
    Adult,
    /// Youth/kids caps: up to 4" wide × 2" high.
    Youth,
}

impl CapSize {
    pub fn label(&self) -> &'static str {
        match self {
            CapSize::Adult => "Adult (4 × 2.25 in)",
            CapSize::Youth => "Youth (4 × 2 in)",
        }
    }
    /// Max (width, height) in mm.
    pub fn limit(&self) -> (f64, f64) {
        match self {
            CapSize::Adult => (101.6, 57.15),
            CapSize::Youth => (101.6, 50.8),
        }
    }
}

pub fn recipe(name: &str) -> Option<Recipe> {
    let base = Recipe {
        name: "pique".into(),
        fill_spacing: 0.40,
        fill_len: 3.5,
        satin_spacing: 0.40,
        pull_comp: 0.25,
        underlay_spacing: 2.5,
        underlay_inset: 0.6,
        backing: "Cutaway 2.5 oz".into(),
        topping: "None".into(),
        weight: Default::default(),
        cap_size: Default::default(),
    };
    Some(match name {
        "pique" => base,
        "tshirt" => Recipe {
            name: "tshirt".into(),
            fill_spacing: 0.45,
            pull_comp: 0.30,
            underlay_spacing: 2.0,
            backing: "No-show mesh cutaway".into(),
            ..base
        },
        "fleece" => Recipe {
            name: "fleece".into(),
            fill_spacing: 0.42,
            underlay_spacing: 2.0,
            topping: "Water-soluble film".into(),
            ..base
        },
        "twill" => Recipe {
            name: "twill".into(),
            pull_comp: 0.20,
            backing: "Tearaway 2.0 oz".into(),
            ..base
        },
        "cap" => Recipe {
            name: "cap".into(),
            pull_comp: 0.30,
            underlay_spacing: 2.0,
            backing: "Cap tearaway 3.0 oz".into(),
            ..base
        },
        "towel" => Recipe {
            name: "towel".into(),
            fill_spacing: 0.38,
            underlay_spacing: 1.8,
            topping: "Water-soluble film".into(),
            backing: "Tearaway 2.0 oz".into(),
            ..base
        },
        _ => return None,
    })
}

pub const RECIPES: &[&str] = &["pique", "tshirt", "fleece", "twill", "cap", "towel"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    /// Flat goods: sew by stacking order, merging colours, shortest travel.
    Flat,
    /// Caps: start bottom-centre and work up and outward; fills sewn
    /// bottom-to-top with horizontal stitches; extra underlay.
    Cap,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "flat" => Some(Profile::Flat),
            "cap" | "hat" => Some(Profile::Cap),
            _ => None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Profile::Flat => "Flat",
            Profile::Cap => "Cap (center-out)",
        }
    }
    pub fn default_recipe(&self) -> &'static str {
        match self {
            Profile::Flat => "pique",
            Profile::Cap => "cap",
        }
    }
    /// Typical machine speed for run-time estimates (stitches per minute).
    pub fn spm(&self) -> f64 {
        match self {
            Profile::Flat => 850.0,
            Profile::Cap => 650.0,
        }
    }
    /// Max design size for the frame (width, height) in mm, used for warnings.
    pub fn frame_limit(&self) -> (f64, f64) {
        match self {
            Profile::Flat => (360.0, 360.0),
            Profile::Cap => CapSize::Adult.limit(),
        }
    }
}

/// Engine constants that are not fabric-specific.
pub struct Limits;
impl Limits {
    /// Widest satin before stitches are split (mm).
    pub const SATIN_MAX: f64 = 8.0;
    /// Shapes thinner than this (2·area/perimeter) are sewn as satin (mm).
    pub const SATIN_THICKNESS: f64 = 4.5;
    /// Strokes at least this wide become satin columns; thinner become runs (mm).
    pub const STROKE_SATIN_MIN: f64 = 1.2;
    /// Run stitch length (mm).
    pub const RUN_LEN: f64 = 2.5;
    /// Jumps longer than this get a trim (mm).
    pub const TRIM_AFTER: f64 = 3.0;
    /// Stitches shorter than this are flagged (mm).
    pub const SHORT_STITCH: f64 = 0.3;
    /// Longest single stitch a machine file can hold (mm).
    pub const MAX_STITCH: f64 = 12.0;
    /// Smallest area worth stitching (mm²).
    pub const MIN_AREA: f64 = 0.8;
}
