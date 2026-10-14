//! Project model: layers built from artwork or text, each with its own
//! transform and stitch choices. Saved as a `.sfg` file (JSON) so the app,
//! the command line and AI agents all work from the same design.

use crate::design::{BuildOptions, Design, build_with};
use crate::geom::{BBox, Pt, spans_at};
use crate::import::{Artwork, Shape, ShapeKind, ShapeOpts};
use crate::settings::{Profile, Recipe};
use crate::sheet::JobInfo;
use crate::thread::{self, Thread};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextSpec {
    pub text: String,
    pub font: String,
    pub height_mm: f64,
    pub rgb: [u8; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    /// Geometry as imported (mm). The layer transform is applied on top.
    pub shapes: Vec<Shape>,
    /// Pivot for scale and rotation (centre of the original geometry).
    pub origin: Pt,
    pub offset: Pt,
    /// Horizontal and vertical scale in the layer's own frame.
    pub sx: f64,
    pub sy: f64,
    /// Keep width and height in proportion when one is typed.
    pub lock: bool,
    /// A thread the user typed in themselves (code, name, color).
    pub custom: Option<thread::Thread>,
    /// Degrees, counter-clockwise on screen.
    pub rotation: f64,
    pub opts: ShapeOpts,
    pub visible: bool,
    pub text: Option<TextSpec>,
    pub is_outline: bool,
    /// Layers sharing a link move, rotate and resize together (e.g. a shape's
    /// fill and its outline).
    pub link: Option<u64>,
}

impl Layer {
    pub fn new(id: u64, name: String, shapes: Vec<Shape>, is_outline: bool) -> Self {
        let b = BBox::of(shapes.iter().flat_map(|s| s.points()));
        Layer {
            id,
            name,
            origin: b.center(),
            shapes,
            offset: Pt::new(0.0, 0.0),
            sx: 1.0,
            sy: 1.0,
            lock: true,
            custom: None,
            rotation: 0.0,
            opts: ShapeOpts::default(),
            visible: true,
            text: None,
            is_outline,
            link: None,
        }
    }

    pub fn tf(&self, p: Pt) -> Pt {
        {
            let q = p.sub(self.origin);
            Pt::new(q.x * self.sx, q.y * self.sy)
        }
        .rot(-self.rotation.to_radians())
        .add(self.origin)
        .add(self.offset)
    }

    pub fn inv(&self, p: Pt) -> Pt {
        {
            let q = p
                .sub(self.offset)
                .sub(self.origin)
                .rot(self.rotation.to_radians());
            Pt::new(q.x / self.sx, q.y / self.sy)
        }
        .add(self.origin)
    }

    pub fn transformed(&self) -> Vec<Shape> {
        self.shapes
            .iter()
            .map(|s| {
                let mut s = s.clone();
                match &mut s.kind {
                    ShapeKind::Area { rings, .. } => {
                        rings.iter_mut().flatten().for_each(|p| *p = self.tf(*p))
                    }
                    ShapeKind::Line { paths, width } => {
                        paths.iter_mut().flatten().for_each(|p| *p = self.tf(*p));
                        *width *= (self.sx * self.sy).abs().sqrt();
                    }
                }
                s
            })
            .collect()
    }

    pub fn bbox(&self) -> BBox {
        let mut b = BBox::empty();
        for s in &self.shapes {
            for p in s.points() {
                b.add(self.tf(*p));
            }
        }
        b
    }

    /// Size in the layer's own frame (before rotation), mm.
    pub fn own_size(&self) -> (f64, f64) {
        let b = BBox::of(self.shapes.iter().flat_map(|s| s.points()));
        (b.width() * self.sx.abs(), b.height() * self.sy.abs())
    }

    pub fn center(&self) -> Pt {
        self.origin.add(self.offset)
    }

    /// Does the point (design mm) land on this layer?
    pub fn hit(&self, p: Pt, tol_mm: f64) -> bool {
        let q = self.inv(p);
        let tol = tol_mm / self.sx.abs().min(self.sy.abs()).max(1e-6);
        self.shapes.iter().any(|s| match &s.kind {
            ShapeKind::Area { rings, rule } => spans_at(rings, *rule, q.y)
                .iter()
                .any(|&(a, b)| q.x >= a - tol && q.x <= b + tol),
            ShapeKind::Line { paths, width } => paths.iter().any(|path| {
                path.windows(2)
                    .any(|w| seg_dist(q, w[0], w[1]) <= width / 2.0 + tol)
            }),
        })
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = ((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2;
    p.dist(a.lerp(b, t.clamp(0.0, 1.0)))
}

/// Turn visible layers into artwork for the engine. Custom threads are added
/// to a copy of the chart; returns the artwork and that working chart.
pub fn compose(layers: &[Layer], chart: &[Thread]) -> (Artwork, Vec<Thread>) {
    let mut chart = chart.to_vec();
    let mut shapes = Vec::new();
    for (li, l) in layers.iter().enumerate() {
        if !l.visible {
            continue;
        }
        let mut opts = ShapeOpts {
            layer: li,
            ..l.opts.clone()
        };
        if let Some(t) = &l.custom {
            chart.push(t.clone());
            opts.thread = Some(chart.len() - 1);
        }
        for mut s in l.transformed() {
            s.opts = opts.clone();
            s.label = l.name.clone();
            shapes.push(s);
        }
    }
    (
        Artwork {
            shapes,
            warnings: Vec::new(),
        },
        chart,
    )
}

pub const FORMAT: &str = "stitchforge-project";
pub const VERSION: u32 = 1;

/// Everything needed to rebuild a design exactly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub layers: Vec<Layer>,
    pub profile: Profile,
    pub fabric: String,
    pub recipe: Recipe,
    pub keep_order: bool,
    pub auto_stops: bool,
    pub chart_name: String,
    pub chart: Vec<Thread>,
    pub job: JobInfo,
}

impl Project {
    pub fn new(name: &str, layers: Vec<Layer>) -> Self {
        let profile = Profile::Flat;
        Project {
            format: FORMAT.into(),
            version: VERSION,
            name: name.into(),
            layers,
            profile,
            fabric: profile.default_recipe().into(),
            recipe: crate::settings::recipe(profile.default_recipe()).unwrap(),
            keep_order: true,
            auto_stops: true,
            chart_name: "Generic 40wt".into(),
            chart: thread::generic_chart(),
            job: JobInfo::default(),
        }
    }

    pub fn build(&self) -> Result<Design, String> {
        let (art, chart) = compose(&self.layers, &self.chart);
        if art.shapes.is_empty() {
            return Err("no visible layers to stitch".into());
        }
        let name = format!(
            "{}-{}",
            self.name,
            if self.profile == Profile::Cap {
                "cap"
            } else {
                "flat"
            }
        );
        build_with(
            &name,
            &art,
            &chart,
            &self.recipe,
            self.profile,
            BuildOptions {
                keep_order: self.keep_order,
                auto_stops: self.auto_stops,
            },
        )
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        // write to a temp file first so a crash never leaves a half-written project
        let tmp = path.with_extension("sfg.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let data = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let p: Project = serde_json::from_str(&data)
            .map_err(|e| format!("{} is not a StitchForge project: {e}", path.display()))?;
        if p.format != FORMAT {
            return Err(format!("{} is not a StitchForge project", path.display()));
        }
        if p.version > VERSION {
            return Err(format!(
                "{} was saved by a newer StitchForge (format {}); please update",
                path.display(),
                p.version
            ));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::FillRule;

    #[test]
    fn save_and_load_round_trip() {
        let ring = vec![
            Pt::new(0.0, 0.0),
            Pt::new(20.0, 0.0),
            Pt::new(20.0, 10.0),
            Pt::new(0.0, 10.0),
        ];
        let shape = Shape {
            kind: ShapeKind::Area {
                rings: vec![ring],
                rule: FillRule::NonZero,
            },
            rgb: [20, 140, 70],
            label: "box".into(),
            group: 1,
            opts: ShapeOpts::default(),
        };
        let mut l = Layer::new(1, "box".into(), vec![shape], false);
        l.rotation = 30.0;
        l.opts.angle_deg = Some(45.0);
        let p = Project::new("test", vec![l]);
        let before = p.build().unwrap();
        let path = std::env::temp_dir().join("sf-roundtrip.sfg");
        p.save(&path).unwrap();
        let q = Project::load(&path).unwrap();
        let after = q.build().unwrap();
        assert_eq!(before.stats.stitches, after.stats.stitches);
        assert_eq!(q.layers[0].rotation, 30.0);
        assert_eq!(q.layers[0].opts.angle_deg, Some(45.0));
    }
}
