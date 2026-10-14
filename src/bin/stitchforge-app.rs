//! StitchForge desktop app (v0.3 review build).
//!
//! Layers you can select, move, rotate, resize, recolor and re-angle on the
//! canvas; per-layer stop toggles; flat and 3D stitch views; sew-out playback.

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Shape as EShape, Stroke, Vec2};
use std::collections::HashSet;
use std::path::PathBuf;
use stitchforge::design::{BuildOptions, Cmd, Design};
use stitchforge::formats::{self, Format, MachineFile};
use stitchforge::geom::{BBox, Pt};
use stitchforge::import::{self, Artwork, Shape, ShapeKind, ShapeOpts, StitchChoice};
use stitchforge::project::{Layer, Project, TextSpec};
use stitchforge::settings::{self, Profile, Recipe};
use stitchforge::sheet::JobInfo;
use stitchforge::{export, sampler, thread};

const SAMPLE_SVG: &str = include_str!("../../examples/left-chest-logo.svg");

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("StitchForge")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 620.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "StitchForge",
        opts,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

// ---------------------------------------------------------------- app

/// An opened DST/EXP/PES: stitches are fixed, threads per stop are chosen.
struct MachineDoc {
    file: MachineFile,
    threads: Vec<thread::Thread>,
    path: PathBuf,
}

/// Something waiting on "save changes?" before it can happen.
#[derive(Clone)]
enum Pending {
    Close,
    New,
    Open,
    OpenPath(PathBuf),
    Sampler,
    Sample,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Flat,
    ThreeD,
}

struct App {
    layers: Vec<Layer>,
    next_id: u64,
    design: Option<Design>,
    error: Option<String>,
    dirty: bool,
    design_name: String,

    undo: Vec<Vec<Layer>>,
    redo: Vec<Vec<Layer>>,
    last_checkpoint: f64,

    profile: Profile,
    fabric: String,
    recipe: Recipe,
    keep_order: bool,
    auto_stops: bool,
    pro: bool,
    chart: Vec<thread::Thread>,
    chart_name: String,

    // text tool
    show_text: bool,
    text: String,
    font: String,
    text_height_in: f64,
    text_color: [u8; 3],
    fonts: Option<Vec<String>>,
    font_opts: Option<usvg::Options<'static>>,
    font_filter: String,

    job: JobInfo,

    // canvas
    view: View,
    zoom: f32,
    view_center: Pt,
    fit: bool,
    show_jumps: bool,
    selected: Option<u64>,
    moving: Option<Pt>,
    expanded: HashSet<u64>,

    // playback (index into design.cmds)
    progress: usize,
    playing: bool,
    speed: f32,
    play_acc: f32,
    pending_progress: Option<f32>,
    pending_zoom: Option<f32>,

    status: String,
    last_dir: Option<PathBuf>,

    // files
    project_path: Option<PathBuf>,
    modified: bool,
    clean_after_regen: bool,
    allow_close: bool,
    confirm: Option<Pending>,
    title: String,
    machine: Option<MachineDoc>,
    show_export: bool,
    export_fmts: [bool; 3],
}

use stitchforge::import::font_options;

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        cc.egui_ctx.set_style(style);
        let fabric = Profile::Flat.default_recipe().to_string();
        App {
            layers: Vec::new(),
            next_id: 1,
            design: None,
            error: None,
            dirty: false,
            design_name: "design".into(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_checkpoint: -10.0,
            profile: Profile::Flat,
            recipe: settings::recipe(&fabric).unwrap(),
            fabric,
            keep_order: true,
            auto_stops: true,
            pro: false,
            chart: thread::generic_chart(),
            chart_name: "Generic 40wt".into(),
            show_text: false,
            text: "Sunny Hill".into(),
            font: "Arial".into(),
            text_height_in: 0.75,
            text_color: [24, 36, 80],
            fonts: None,
            font_opts: None,
            font_filter: String::new(),
            job: JobInfo::default(),
            view: View::ThreeD,
            zoom: 4.0,
            view_center: Pt::new(0.0, 0.0),
            fit: true,
            show_jumps: false,
            selected: None,
            moving: None,
            expanded: HashSet::new(),
            progress: 0,
            playing: false,
            speed: 600.0,
            play_acc: 0.0,
            pending_progress: None,
            pending_zoom: None,
            status: "Open an SVG, drop one on the window, or add text.".into(),
            last_dir: None,
            project_path: None,
            modified: false,
            clean_after_regen: false,
            allow_close: false,
            confirm: None,
            title: String::new(),
            machine: None,
            show_export: false,
            export_fmts: [true, false, false],
        }
        .with_launch_args()
    }

    /// `stitchforge-app [file.svg] [--sample] [--cap] [--pro] [--flat-view]
    /// [--select N] [--progress 0.5] [--text WORDS] [--move dx dy] [--angle DEG]`
    fn with_launch_args(mut self) -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        let num = |i: usize| args.get(i).and_then(|v| v.parse::<f64>().ok());
        while i < args.len() {
            match args[i].as_str() {
                "--cap" => {
                    self.profile = Profile::Cap;
                    self.fabric = "cap".into();
                    self.recipe = settings::recipe("cap").unwrap();
                }
                "--pro" => self.pro = true,
                "--flat-view" => self.view = View::Flat,
                "--sample" => self.open_svg_data(SAMPLE_SVG, "sample-logo", false),
                "--select" => {
                    if let Some(n) = num(i + 1) {
                        self.selected = self.layers.get(n as usize).map(|l| l.id);
                    }
                    i += 1;
                }
                "--expand" => {
                    self.expanded = self.layers.iter().map(|l| l.id).collect();
                }
                "--move" => {
                    if let (Some(dx), Some(dy), Some(l)) = (num(i + 1), num(i + 2), self.sel_mut())
                    {
                        l.offset = l.offset.add(Pt::new(dx, dy));
                    }
                    i += 2;
                }
                "--angle" => {
                    if let (Some(a), Some(l)) = (num(i + 1), self.sel_mut()) {
                        l.opts.angle_deg = Some(a);
                    }
                    i += 1;
                }
                "--custom" => {
                    let rgb = args.get(i + 1).and_then(|v| parse_hex(v));
                    if let (Some(rgb), Some(l)) = (rgb, self.sel_mut()) {
                        l.custom = Some(thread::Thread {
                            brand: "Custom".into(),
                            code: "1147".into(),
                            name: "Madeira Gold".into(),
                            rgb,
                        });
                    }
                    i += 1;
                }
                "--stop-off" => {
                    if let Some(l) = self.sel_mut() {
                        l.opts.stop = Some(false);
                    }
                }
                "--zoom" => {
                    self.pending_zoom = num(i + 1).map(|v| v as f32);
                    i += 1;
                }
                "--progress" => {
                    self.pending_progress = num(i + 1).map(|v| v as f32);
                    i += 1;
                }
                "--text" => {
                    if let Some(t) = args.get(i + 1) {
                        self.show_text = true;
                        self.text = t.clone();
                    }
                    i += 1;
                }
                "--sampler" => self.load_sampler(),
                "--save-to" => {
                    // test hook: save without a dialog
                    if let Some(p) = args.get(i + 1) {
                        self.regenerate();
                        self.save_to(PathBuf::from(p));
                    }
                    i += 1;
                }
                "--export-dialog" => self.show_export = true,
                "--modified" => {
                    self.modified = true;
                    self.confirm = Some(Pending::Close);
                }
                a if !a.starts_with("--") && std::path::Path::new(a).extension().is_some() => {
                    self.open_any(PathBuf::from(a))
                }
                _ => {}
            }
            i += 1;
        }
        self
    }

    // ---------- layer helpers ----------

    fn sel_idx(&self) -> Option<usize> {
        self.selected
            .and_then(|id| self.layers.iter().position(|l| l.id == id))
    }
    fn sel_mut(&mut self) -> Option<&mut Layer> {
        let i = self.sel_idx()?;
        self.layers.get_mut(i)
    }

    /// Save an undo point (coalesces rapid edits like slider drags).
    fn checkpoint(&mut self, now: f64) {
        if now - self.last_checkpoint > 0.6 {
            self.undo.push(self.layers.clone());
            if self.undo.len() > 80 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_checkpoint = now;
    }
    fn hard_checkpoint(&mut self) {
        self.last_checkpoint = -10.0;
        self.checkpoint(0.0);
        self.last_checkpoint = -10.0;
    }
    fn do_undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.layers, prev));
            self.dirty = true;
            self.status = "Undone".into();
        }
    }
    fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.layers, next));
            self.dirty = true;
            self.status = "Redone".into();
        }
    }

    fn fonts_db(&mut self) -> &usvg::Options<'static> {
        if self.font_opts.is_none() {
            self.font_opts = font_options(None).ok().map(|(o, _)| o);
        }
        self.font_opts.as_ref().unwrap()
    }

    // ---------- loading ----------

    fn open_svg_path(&mut self, path: PathBuf, add: bool) {
        match std::fs::read_to_string(&path) {
            Ok(data) => {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or("design".into());
                self.last_dir = path.parent().map(|p| p.to_path_buf());
                self.open_svg_data(&data, &name, add);
            }
            Err(e) => self.error = Some(format!("Couldn't read {}: {e}", path.display())),
        }
    }

    fn open_svg_data(&mut self, data: &str, name: &str, add: bool) {
        let opt = self.fonts_db();
        let art = match import::load_svg_str(data, &opt) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.hard_checkpoint();
        let mut new_layers = self.layers_from_art(art);
        // Real size from the file, unless it's clearly not embroidery-sized.
        let mut b = BBox::empty();
        for l in &new_layers {
            let lb = l.bbox();
            b.add(lb.min);
            b.add(lb.max);
        }
        if b.is_valid() && (b.width() > 400.0 || b.width() < 3.0) {
            let k = 88.9 / b.width();
            scale_layers(&mut new_layers, b.center(), k);
        }
        if add && !self.layers.is_empty() {
            // Drop the new art in the middle of the current design.
            let target = self
                .design
                .as_ref()
                .map(|d| d.bbox.center())
                .unwrap_or(self.view_center);
            let d = target.sub(b.center());
            for l in &mut new_layers {
                l.offset = l.offset.add(d);
            }
            self.layers.extend(new_layers);
        } else {
            self.layers = new_layers;
            self.design_name = name.to_string();
            self.selected = None;
            self.fit = true;
        }
        self.error = None;
        self.dirty = true;
    }

    fn layers_from_art(&mut self, art: Artwork) -> Vec<Layer> {
        // One layer per SVG element; fills and outlines of the same element
        // become separate layers so each can have its own thread and stitch.
        let mut out: Vec<Layer> = Vec::new();
        let mut key: Vec<(usize, bool)> = Vec::new();
        for s in art.shapes {
            let outline = matches!(s.kind, ShapeKind::Line { .. });
            let k = (s.group, outline);
            match key.iter().position(|x| *x == k) {
                Some(i) => out[i].shapes.push(s),
                None => {
                    key.push(k);
                    let n = out.len() + 1;
                    let name = if s.label.is_empty() {
                        if outline {
                            format!("Outline {n}")
                        } else {
                            format!("Shape {n}")
                        }
                    } else {
                        s.label.clone()
                    };
                    let id = self.next_id;
                    self.next_id += 1;
                    out.push(Layer::new(id, name, vec![s], outline));
                }
            }
        }
        // Fill + outline from the same element: same pivot, linked.
        for i in 0..out.len() {
            let gi = key[i].0;
            if let Some(j) = (0..out.len()).find(|&j| j != i && key[j].0 == gi) {
                let link = gi as u64 + 1_000_000 * self.next_id;
                out[i].link = Some(link);
                out[j].link = Some(link);
                let mut b = BBox::of(out[i].shapes.iter().flat_map(|s| s.points()));
                let b2 = BBox::of(out[j].shapes.iter().flat_map(|s| s.points()));
                b.add(b2.min);
                b.add(b2.max);
                out[i].origin = b.center();
                out[j].origin = b.center();
            }
        }
        out
    }

    /// Indices of the layer and everything linked to it.
    fn linked(&self, li: usize) -> Vec<usize> {
        match self.layers[li].link {
            Some(k) => (0..self.layers.len())
                .filter(|&j| self.layers[j].link == Some(k))
                .collect(),
            None => vec![li],
        }
    }

    /// Copy position, size and rotation from layer `li` to its linked layers.
    fn sync_links(&mut self, li: usize) {
        let (off, sx, sy, rot) = (
            self.layers[li].offset,
            self.layers[li].sx,
            self.layers[li].sy,
            self.layers[li].rotation,
        );
        for j in self.linked(li) {
            let l = &mut self.layers[j];
            l.offset = off;
            l.sx = sx;
            l.sy = sy;
            l.rotation = rot;
        }
    }

    fn text_shapes(&mut self, spec: &TextSpec) -> Result<Vec<Shape>, String> {
        let opt = self.fonts_db();
        let art = import::load_text_with(&opt, &spec.text, &spec.font, spec.rgb)?;
        let b = BBox::of(art.shapes.iter().flat_map(|s| s.points()));
        if !b.is_valid() || b.height() <= 0.0 {
            return Err("that text produced no outlines".into());
        }
        let k = spec.height_mm / b.height();
        let c = b.center();
        Ok(art
            .shapes
            .into_iter()
            .map(|mut s| {
                match &mut s.kind {
                    ShapeKind::Area { rings, .. } => rings
                        .iter_mut()
                        .flatten()
                        .for_each(|p| *p = p.sub(c).mul(k)),
                    ShapeKind::Line { paths, width } => {
                        paths
                            .iter_mut()
                            .flatten()
                            .for_each(|p| *p = p.sub(c).mul(k));
                        *width *= k;
                    }
                }
                s
            })
            .collect())
    }

    fn add_text_layer(&mut self) {
        let spec = TextSpec {
            text: self.text.clone(),
            font: self.font.clone(),
            height_mm: self.text_height_in * 25.4,
            rgb: self.text_color,
        };
        match self.text_shapes(&spec) {
            Ok(shapes) => {
                self.hard_checkpoint();
                let id = self.next_id;
                self.next_id += 1;
                let mut l = Layer::new(
                    id,
                    spec.text.lines().next().unwrap_or("Text").to_string(),
                    shapes,
                    false,
                );
                l.origin = Pt::new(0.0, 0.0);
                // Place below the current design, or at the view centre.
                l.offset = match &self.design {
                    Some(d) => Pt::new(d.bbox.center().x, d.bbox.max.y + spec.height_mm * 0.9),
                    None => self.view_center,
                };
                l.text = Some(spec);
                if self.layers.is_empty() {
                    self.design_name = l
                        .name
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .collect::<String>()
                        .to_lowercase();
                    self.fit = true;
                }
                self.layers.push(l);
                self.selected = Some(id);
                self.dirty = true;
                self.show_text = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn refresh_text(&mut self, idx: usize) {
        let Some(spec) = self.layers[idx].text.clone() else {
            return;
        };
        match self.text_shapes(&spec) {
            Ok(shapes) => {
                let l = &mut self.layers[idx];
                l.shapes = shapes;
                self.dirty = true;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn regenerate(&mut self) {
        self.dirty = false;
        if let Some(m) = &self.machine {
            match stitchforge::design::from_machine(
                &self.design_name,
                m.file.cmds.clone(),
                &m.threads,
                self.profile,
                &self.recipe,
            ) {
                Ok(d) => {
                    self.progress = match self.pending_progress.take() {
                        Some(f) => (d.cmds.len() as f32 * f) as usize,
                        None => d.cmds.len(),
                    };
                    self.design = Some(d);
                    self.error = None;
                }
                Err(e) => {
                    self.design = None;
                    self.error = Some(e);
                }
            }
            return;
        }
        if self.layers.iter().all(|l| !l.visible) {
            self.design = None;
            return;
        }
        let mut shapes = Vec::new();
        // Custom threads join the chart for this build.
        let mut chart = self.chart.clone();
        for (li, l) in self.layers.iter().enumerate() {
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
        let art = Artwork {
            shapes,
            warnings: Vec::new(),
        };
        let name = format!(
            "{}-{}",
            self.design_name,
            if self.profile == Profile::Cap {
                "cap"
            } else {
                "flat"
            }
        );
        let t = std::time::Instant::now();
        let opts = BuildOptions {
            keep_order: self.keep_order,
            auto_stops: self.auto_stops,
        };
        match stitchforge::design::build_with(&name, &art, &chart, &self.recipe, self.profile, opts)
        {
            Ok(d) => {
                self.progress = match self.pending_progress.take() {
                    Some(f) => (d.cmds.len() as f32 * f) as usize,
                    None => d.cmds.len(),
                };
                if !self.status.starts_with("Tip:") && !self.status.starts_with("Calibration") {
                    self.status = format!("Stitches generated in {} ms", t.elapsed().as_millis());
                }
                self.design = Some(d);
                self.error = None;
            }
            Err(e) => {
                self.design = None;
                self.error = Some(e);
            }
        }
    }

    /// Reorder layers the way the garment profile would (merge colours on
    /// flats, center-out on caps), then keep that order.
    fn auto_sequence(&mut self) {
        let keep = self.keep_order;
        self.keep_order = false;
        self.regenerate();
        self.keep_order = keep;
        let Some(d) = &self.design else { return };
        let mut first: Vec<(usize, usize)> = Vec::new();
        for (seq, o) in d.objects.iter().enumerate() {
            if !first.iter().any(|(l, _)| *l == o.layer) {
                first.push((o.layer, seq));
            }
        }
        self.hard_checkpoint();
        let old = std::mem::take(&mut self.layers);
        let mut placed = vec![false; old.len()];
        let mut new = Vec::with_capacity(old.len());
        for (l, _) in &first {
            new.push(old[*l].clone());
            placed[*l] = true;
        }
        for (i, l) in old.into_iter().enumerate() {
            if !placed[i] {
                new.push(l);
            }
        }
        self.layers = new;
        self.keep_order = true;
        self.dirty = true;
        self.status = "Layers re-sequenced".into();
    }

    fn export(&mut self) {
        let Some(d) = &self.design else { return };
        let fmts: Vec<Format> = Format::ALL
            .iter()
            .zip(self.export_fmts)
            .filter(|(_, on)| *on)
            .map(|(f, _)| *f)
            .collect();
        if fmts.is_empty() {
            self.error = Some("Pick at least one machine format".into());
            return;
        }
        let mut dlg = rfd::FileDialog::new()
            .set_title("Choose a folder for the machine files and operator sheet");
        if let Some(dir) = &self.last_dir {
            dlg = dlg.set_directory(dir);
        }
        let Some(dir) = dlg.pick_folder() else { return };
        let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let mut job = self.job.clone();
        if job.garment.is_empty() && self.profile == Profile::Cap {
            job.garment = "Cap".into();
        }
        match export::write_formats(d, &job, &dir, &generated, &fmts) {
            Ok(w) => {
                let names: Vec<String> = w
                    .machine
                    .iter()
                    .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                    .collect();
                self.status = format!(
                    "Saved {} and the operator sheet to {}",
                    names.join(", "),
                    dir.display()
                );
                open_path(&w.sheet);
                self.last_dir = Some(dir);
                self.show_export = false;
            }
            Err(e) => self.error = Some(e),
        }
    }

    // ---------- files: projects, stitch files, guard ----------

    fn display_name(&self) -> String {
        match (&self.project_path, &self.machine) {
            (_, Some(m)) => m
                .path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            (Some(p), _) => p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            _ if self.layers.is_empty() => "New design".into(),
            _ => format!("{} (not saved yet)", self.design_name),
        }
    }

    fn to_project(&self) -> Project {
        let mut p = Project::new(&self.design_name, self.layers.clone());
        p.profile = self.profile;
        p.fabric = self.fabric.clone();
        p.recipe = self.recipe.clone();
        p.keep_order = self.keep_order;
        p.auto_stops = self.auto_stops;
        p.chart_name = self.chart_name.clone();
        p.chart = self.chart.clone();
        p.job = self.job.clone();
        p
    }

    fn apply_project(&mut self, p: Project, path: Option<PathBuf>) {
        self.next_id = p.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        self.layers = p.layers;
        self.design_name = p.name;
        self.profile = p.profile;
        self.fabric = p.fabric;
        self.recipe = p.recipe;
        self.keep_order = p.keep_order;
        self.auto_stops = p.auto_stops;
        self.chart_name = p.chart_name;
        self.chart = p.chart;
        self.job = p.job;
        self.project_path = path;
        self.machine = None;
        self.selected = None;
        self.undo.clear();
        self.redo.clear();
        self.fit = true;
        self.dirty = true;
        self.clean_after_regen = true;
    }

    /// Save to the current file, or ask where. Returns true if saved.
    fn save(&mut self) -> bool {
        if self.machine.is_some() || self.layers.is_empty() {
            return false;
        }
        match self.project_path.clone() {
            Some(p) => self.save_to(p),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        if self.machine.is_some() || self.layers.is_empty() {
            return false;
        }
        let mut dlg = rfd::FileDialog::new()
            .set_title("Save StitchForge project")
            .add_filter("StitchForge project", &["sfg"])
            .set_file_name(format!("{}.sfg", self.design_name));
        if let Some(dir) = &self.last_dir {
            dlg = dlg.set_directory(dir);
        }
        match dlg.save_file() {
            Some(mut p) => {
                if p.extension().is_none() {
                    p.set_extension("sfg");
                }
                self.save_to(p)
            }
            None => false,
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        if let Some(stem) = path.file_stem() {
            self.design_name = stem.to_string_lossy().to_string();
        }
        match self.to_project().save(&path) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.last_dir = path.parent().map(|p| p.to_path_buf());
                self.project_path = Some(path);
                self.modified = false;
                true
            }
            Err(e) => {
                self.error = Some(format!("Couldn't save: {e}"));
                false
            }
        }
    }

    fn new_design(&mut self) {
        self.layers.clear();
        self.machine = None;
        self.design = None;
        self.project_path = None;
        self.design_name = "design".into();
        self.selected = None;
        self.undo.clear();
        self.redo.clear();
        self.job = JobInfo::default();
        self.modified = false;
        self.status = "New design".into();
    }

    fn open_dialog(&mut self) {
        let mut dlg = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter(
                "Everything StitchForge opens",
                &["sfg", "svg", "dst", "exp", "pes"],
            )
            .add_filter("StitchForge project", &["sfg"])
            .add_filter("SVG artwork", &["svg"])
            .add_filter("Stitch files", &["dst", "exp", "pes"]);
        if let Some(dir) = &self.last_dir {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.open_any(p);
        }
    }

    fn open_any(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        self.last_dir = path.parent().map(|p| p.to_path_buf());
        match ext.as_str() {
            "sfg" => match Project::load(&path) {
                Ok(p) => {
                    self.apply_project(p, Some(path.clone()));
                    self.status = format!("Opened {}", path.display());
                }
                Err(e) => self.error = Some(e),
            },
            "svg" => {
                self.machine = None;
                self.project_path = None;
                self.open_svg_path(path, false);
                self.clean_after_regen = true;
                self.sequence_tip();
            }
            "dst" | "exp" | "pes" => self.open_machine(path),
            _ => self.error = Some(format!("StitchForge can't open .{ext} files")),
        }
    }

    fn open_machine(&mut self, path: PathBuf) {
        match formats::read_file(&path) {
            Ok(file) => {
                // Threads per stop: the file's own colours when it has them
                // (PES), otherwise distinct starter colours to tell stops apart.
                let codes = [
                    "SF-040", "SF-020", "SF-011", "SF-050", "SF-001", "SF-005", "SF-031", "SF-012",
                    "SF-043", "SF-024",
                ];
                let threads: Vec<thread::Thread> = file
                    .stop_colors
                    .iter()
                    .enumerate()
                    .map(|(i, c)| match c {
                        Some(rgb) => self.chart[thread::nearest(&self.chart, *rgb).0].clone(),
                        None => self
                            .chart
                            .iter()
                            .find(|t| t.code == codes[i % codes.len()])
                            .cloned()
                            .unwrap_or_else(|| self.chart[i % self.chart.len()].clone()),
                    })
                    .collect();
                self.design_name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or("design".into());
                self.status = format!("Opened {} ({} stops)", path.display(), threads.len());
                self.machine = Some(MachineDoc {
                    file,
                    threads,
                    path,
                });
                self.layers.clear();
                self.project_path = None;
                self.selected = None;
                self.undo.clear();
                self.redo.clear();
                self.fit = true;
                self.dirty = true;
                self.clean_after_regen = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn load_sampler(&mut self) {
        self.machine = None;
        self.project_path = None;
        self.open_svg_data(&sampler::svg(), "calibration-sampler", false);
        self.job.notes = sampler::NOTES.into();
        self.auto_sequence();
        self.undo.clear();
        self.clean_after_regen = true;
        self.status = "Calibration sampler loaded — sew it on your usual fabric, then compare with the notes on the operator sheet".into();
    }

    /// After opening art, mention it if auto-sequencing would save stops.
    fn sequence_tip(&mut self) {
        let (art, chart) = stitchforge::project::compose(&self.layers, &self.chart);
        if art.shapes.is_empty() {
            return;
        }
        let build = |keep: bool| {
            stitchforge::design::build_with("tip", &art, &chart, &self.recipe, self.profile, BuildOptions { keep_order: keep, auto_stops: self.auto_stops })
                .map(|d| d.stops.len())
                .unwrap_or(0)
        };
        let (mine, auto) = (build(true), build(false));
        if auto + 1 < mine {
            self.status = format!("Tip: Auto-sequence layers (right panel) would cut stops from {mine} to {auto}.");
        }
    }

    /// Ask to save first if there are unsaved changes.
    fn guard(&mut self, p: Pending, ctx: &egui::Context) {
        if self.modified {
            self.confirm = Some(p);
        } else {
            self.run_pending(p, ctx);
        }
    }

    fn run_pending(&mut self, p: Pending, ctx: &egui::Context) {
        match p {
            Pending::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::New => self.new_design(),
            Pending::Open => self.open_dialog(),
            Pending::OpenPath(path) => self.open_any(path),
            Pending::Sampler => self.load_sampler(),
            Pending::Sample => {
                self.machine = None;
                self.project_path = None;
                self.open_svg_data(SAMPLE_SVG, "sample-logo", false);
                self.clean_after_regen = true;
            }
        }
    }

    /// Stitch-file mode: each stop with its thread.
    fn machine_panel(&mut self, ui: &mut egui::Ui) {
        let Some(m) = &mut self.machine else { return };
        ui.label(RichText::new(format!("Stitch file · {}", m.file.format)).strong());
        ui.label(RichText::new("Stitches are read-only. Pick the thread for each stop, play the sew-out, print the operator sheet, or export to another format.").weak().small());
        ui.separator();
        let counts: Vec<usize> = self
            .design
            .as_ref()
            .map(|d| d.stops.iter().map(|s| s.stitches).collect())
            .unwrap_or_default();
        let chart = self.chart.clone();
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, t) in m.threads.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        swatch(ui, t.rgb);
                        ui.label(RichText::new(format!("Stop {}", i + 1)).strong());
                        if let Some(n) = counts.get(i) {
                            ui.label(
                                RichText::new(format!("{} stitches", group(*n)))
                                    .weak()
                                    .small(),
                            );
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        egui::ComboBox::from_id_salt(("stop-thread", i))
                            .width(200.0)
                            .height(360.0)
                            .selected_text(format!("{} {}", t.code, t.name))
                            .show_ui(ui, |ui| {
                                for c in &chart {
                                    ui.horizontal(|ui| {
                                        swatch(ui, c.rgb);
                                        if ui
                                            .selectable_label(
                                                c.code == t.code,
                                                format!("{} {}", c.code, c.name),
                                            )
                                            .clicked()
                                        {
                                            *t = c.clone();
                                            changed = true;
                                        }
                                    });
                                }
                            });
                        let mut rgb = t.rgb;
                        if ui
                            .color_edit_button_srgb(&mut rgb)
                            .on_hover_text("Custom color")
                            .changed()
                        {
                            *t = thread::Thread {
                                brand: "Custom".into(),
                                code: "Custom".into(),
                                name: format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
                                rgb,
                            };
                            changed = true;
                        }
                    });
                    ui.add_space(4.0);
                }
            });
        if changed {
            self.dirty = true;
        }
    }

    fn pick_svg(&mut self, add: bool) {
        let mut dlg = rfd::FileDialog::new().add_filter("SVG artwork", &["svg"]);
        if let Some(dir) = &self.last_dir {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.open_svg_path(p, add);
        }
    }

    /// Stop state shown on a layer row: does a stop happen right before it?
    fn layer_stop(&self, li: usize) -> Option<bool> {
        let d = self.design.as_ref()?;
        d.objects
            .iter()
            .find(|o| o.layer == li)
            .map(|o| o.starts_stop)
    }

    fn layer_thread(&self, li: usize) -> Option<&thread::Thread> {
        let d = self.design.as_ref()?;
        d.objects
            .iter()
            .find(|o| o.layer == li)
            .map(|o| &d.stops[o.stop].thread)
    }

    // ---------- top bar ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mode_art = self.machine.is_none();
        ui.horizontal(|ui| {
            ui.label(RichText::new("StitchForge").strong().size(17.0));
            ui.separator();
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    ui.close();
                    self.guard(Pending::New, &ctx);
                }
                if ui.button("Open…  (project, SVG, DST/EXP/PES)").clicked() {
                    ui.close();
                    self.guard(Pending::Open, &ctx);
                }
                ui.separator();
                if ui
                    .add_enabled(
                        mode_art && !self.layers.is_empty(),
                        egui::Button::new("Save"),
                    )
                    .clicked()
                {
                    ui.close();
                    self.save();
                }
                if ui
                    .add_enabled(
                        mode_art && !self.layers.is_empty(),
                        egui::Button::new("Save As…"),
                    )
                    .clicked()
                {
                    ui.close();
                    self.save_as();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        mode_art && !self.layers.is_empty(),
                        egui::Button::new("Add art…"),
                    )
                    .clicked()
                {
                    ui.close();
                    self.pick_svg(true);
                }
                if ui
                    .add_enabled(mode_art, egui::Button::new("Add text…"))
                    .clicked()
                {
                    ui.close();
                    self.show_text = true;
                }
                ui.separator();
                if ui
                    .button("Calibration sampler (Gate 1 test sew-out)")
                    .clicked()
                {
                    ui.close();
                    self.guard(Pending::Sampler, &ctx);
                }
                if ui.button("Sample logo").clicked() {
                    ui.close();
                    self.guard(Pending::Sample, &ctx);
                }
                ui.separator();
                if ui
                    .add_enabled(self.design.is_some(), egui::Button::new("Export…"))
                    .clicked()
                {
                    ui.close();
                    self.show_export = true;
                }
            });
            if ui
                .button("Open…")
                .on_hover_text("Project, SVG art, or a DST/EXP/PES stitch file (Cmd/Ctrl+O)")
                .clicked()
            {
                self.guard(Pending::Open, &ctx);
            }
            if ui
                .add_enabled(
                    mode_art && !self.layers.is_empty(),
                    egui::Button::new("Save"),
                )
                .on_hover_text("Cmd/Ctrl+S")
                .clicked()
            {
                self.save();
            }
            if ui
                .add_enabled(
                    mode_art && !self.layers.is_empty(),
                    egui::Button::new("Add art…"),
                )
                .on_hover_text("Add another SVG as new layers")
                .clicked()
            {
                self.pick_svg(true);
            }
            if ui
                .add_enabled(mode_art, egui::Button::new("Add text…"))
                .on_hover_text("Lettering in any installed font")
                .clicked()
            {
                self.show_text = true;
            }
            ui.separator();
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                .on_hover_text("Cmd/Ctrl+Z")
                .clicked()
            {
                self.do_undo();
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                .on_hover_text("Shift+Cmd/Ctrl+Z")
                .clicked()
            {
                self.do_redo();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.design.is_some(),
                        egui::Button::new(RichText::new("Export…").strong()),
                    )
                    .on_hover_text("Machine files + operator sheet (Cmd/Ctrl+E)")
                    .clicked()
                {
                    self.show_export = true;
                }
                ui.separator();
                ui.selectable_value(&mut self.pro, true, "Pro");
                ui.selectable_value(&mut self.pro, false, "Simple");
                ui.separator();
                let name = self.display_name();
                let dot = if self.modified { "  •" } else { "" };
                ui.label(RichText::new(format!("{name}{dot}")).weak())
                    .on_hover_text(if self.modified {
                        "Unsaved changes"
                    } else {
                        "All changes saved"
                    });
            });
        });
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        if !self.show_export {
            return;
        }
        let mut open = true;
        let mut go = false;
        egui::Window::new("Export").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("Machine formats");
            for (i, f) in Format::ALL.iter().enumerate() {
                ui.checkbox(&mut self.export_fmts[i], f.label());
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Always included: operator sheet (prints to PDF), color-stop CSV, PNG preview, job summary.").weak().small());
            ui.add_space(6.0);
            if ui.button(RichText::new("Choose folder and export").strong()).clicked() {
                go = true;
            }
        });
        if go {
            self.export();
        }
        if !open {
            self.show_export = false;
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.confirm.clone() else {
            return;
        };
        let mut choice: Option<u8> = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Save changes to “{}” first?", self.design_name));
                ui.label(
                    RichText::new("If you don't save, changes since the last save are lost.")
                        .weak()
                        .small(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Save").strong()).clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Don't save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
        match choice {
            Some(0) => {
                if self.save() {
                    self.confirm = None;
                    self.run_pending(p, ctx);
                }
            }
            Some(1) => {
                self.confirm = None;
                self.modified = false;
                self.run_pending(p, ctx);
            }
            Some(2) => self.confirm = None,
            _ => {}
        }
    }

    // ---------- layers panel ----------

    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Layers").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Collapse all").clicked() {
                    self.expanded.clear();
                }
                if ui.small_button("Expand all").clicked() {
                    self.expanded = self.layers.iter().map(|l| l.id).collect();
                }
            });
        });
        ui.label(
            RichText::new(
                "Sews top to bottom. Click to select · STOP = machine stop before that layer.",
            )
            .weak()
            .small(),
        );
        ui.separator();
        if self.layers.is_empty() {
            ui.label(RichText::new("Nothing loaded yet.").weak());
            return;
        }
        let mut select: Option<u64> = None;
        let mut toggle_vis: Option<usize> = None;
        let mut toggle_stop: Option<(usize, Option<bool>)> = None;
        let mut toggle_exp: Option<u64> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (li, l) in self.layers.iter().enumerate() {
                let open = self.expanded.contains(&l.id);
                let sel = self.selected == Some(l.id);
                let objs: Vec<&stitchforge::design::ObjectInfo> =
                    self.design.as_ref().map(|d| d.objects.iter().filter(|o| o.layer == li).collect()).unwrap_or_default();
                let stitches: usize = objs.iter().map(|o| o.stitches).sum();
                let row = egui::Frame::NONE
                    .fill(if sel { ui.visuals().selection.bg_fill.gamma_multiply(0.45) } else { Color32::TRANSPARENT })
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(2, 1));
                row.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if disclosure(ui, open).on_hover_text("Show the pieces in this layer").clicked() {
                            toggle_exp = Some(l.id);
                        }
                        if ui.add(egui::Button::new(if l.visible { "👁" } else { "◌" }).frame(false)).on_hover_text("Show / hide (hidden layers don't sew)").clicked() {
                            toggle_vis = Some(li);
                        }
                        let rgb = self.layer_thread(li).map(|t| t.rgb).unwrap_or(l.shapes.first().map(|s| s.rgb).unwrap_or([128; 3]));
                        swatch(ui, rgb);
                        let name = if l.visible { RichText::new(&l.name) } else { RichText::new(&l.name).weak().strikethrough() };
                        if ui.add(egui::Label::new(name).sense(Sense::click()).truncate()).clicked() {
                            select = Some(l.id);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // STOP chip
                            let stop_now = self.layer_stop(li).unwrap_or(false);
                            let forced = l.opts.stop.is_some();
                            let (fill, text) = if stop_now {
                                (Color32::from_rgb(200, 60, 50), Color32::WHITE)
                            } else {
                                (Color32::TRANSPARENT, ui.visuals().weak_text_color())
                            };
                            let label = if forced { "STOP•" } else { "STOP" };
                            let chip = egui::Button::new(RichText::new(label).size(10.5).color(text))
                                .fill(fill)
                                .stroke(Stroke::new(1.0_f32, if stop_now { fill } else { ui.visuals().weak_text_color() }))
                                .corner_radius(9.0)
                                .min_size(Vec2::new(44.0, 18.0));
                            let r = ui.add_enabled(l.visible, chip).on_hover_text(
                                "Machine stop / colour change before this layer.\nClick to turn on or off. Right-click to go back to automatic.\n• = set by you",
                            );
                            if r.clicked() {
                                toggle_stop = Some((li, Some(!stop_now)));
                            }
                            if r.secondary_clicked() {
                                toggle_stop = Some((li, None));
                            }
                            if stitches > 0 {
                                ui.label(RichText::new(group(stitches)).weak().small());
                            }
                        });
                    });
                });
                if open {
                    ui.indent(l.id, |ui| {
                        if objs.is_empty() {
                            ui.label(RichText::new("(no stitches)").weak().small());
                        }
                        for o in objs {
                            let angle = if has_angle(o.stitch_type) { format!(" · {:.0}°", o.angle_deg) } else { String::new() };
                            ui.label(RichText::new(format!("#{}  {}{} · {} stitches", o.sequence, o.stitch_type, angle, group(o.stitches))).small());
                        }
                    });
                }
            }
        });
        if let Some(id) = toggle_exp {
            if !self.expanded.remove(&id) {
                self.expanded.insert(id);
            }
        }
        if let Some(id) = select {
            self.selected = if self.selected == Some(id) {
                None
            } else {
                Some(id)
            };
        }
        if let Some(li) = toggle_vis {
            self.hard_checkpoint();
            self.layers[li].visible = !self.layers[li].visible;
            self.dirty = true;
        }
        if let Some((li, v)) = toggle_stop {
            self.hard_checkpoint();
            self.layers[li].opts.stop = v;
            self.dirty = true;
        }
    }

    // ---------- properties ----------

    fn layer_props(&mut self, ui: &mut egui::Ui, now: f64) {
        let Some(li) = self.sel_idx() else { return };
        let angles: Vec<f64> = self
            .design
            .as_ref()
            .map(|d| {
                d.objects
                    .iter()
                    .filter(|o| o.layer == li && has_angle(o.stitch_type))
                    .map(|o| o.angle_deg)
                    .collect()
            })
            .unwrap_or_default();
        let auto_angle = angles.first().copied();
        let varies = angles.iter().any(|a| (a - angles[0]).abs() > 0.5);
        let stop_now = self.layer_stop(li);
        let chart = self.chart.clone();
        let mut changed = false;
        let mut text_changed = false;
        let mut action: Option<&str> = None;

        section(ui, "Selected layer");
        {
            let l = &mut self.layers[li];
            ui.horizontal(|ui| {
                ui.label("Name");
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut l.name).desired_width(200.0))
                    .changed();
            });
            if let Some(spec) = &mut l.text {
                ui.label("Words");
                text_changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut spec.text)
                            .desired_rows(2)
                            .desired_width(280.0),
                    )
                    .lost_focus();
                ui.horizontal(|ui| {
                    ui.label("Font");
                    let fonts = self.fonts.get_or_insert_with(import::list_fonts);
                    egui::ComboBox::from_id_salt("layer-font")
                        .width(200.0)
                        .selected_text(&spec.font)
                        .show_ui(ui, |ui| {
                            for f in fonts.iter() {
                                if ui.selectable_value(&mut spec.font, f.clone(), f).clicked() {
                                    text_changed = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Letter height");
                    if let Some(h) = num_field(ui, "letter-h", spec.height_mm / 25.4, 2, "in") {
                        if h > 0.05 {
                            spec.height_mm = h * 25.4;
                            text_changed = true;
                        }
                    }
                });
            }
            let (ow, oh) = l.own_size();
            ui.horizontal(|ui| {
                ui.label("W");
                if let Some(w) = num_field(ui, "layer-w", ow / 25.4, 2, "in") {
                    if w > 0.02 && ow > 0.0 {
                        let k = w * 25.4 / ow;
                        l.sx *= k;
                        if l.lock {
                            l.sy *= k;
                        }
                        changed = true;
                    }
                }
                let lock = if l.lock { "🔒" } else { "🔓" };
                if ui
                    .add(egui::Button::new(lock).frame(false))
                    .on_hover_text("Lock proportions: typing one size changes the other to match")
                    .clicked()
                {
                    l.lock = !l.lock;
                }
                ui.label("H");
                if let Some(h) = num_field(ui, "layer-h", oh / 25.4, 2, "in") {
                    if h > 0.02 && oh > 0.0 {
                        let k = h * 25.4 / oh;
                        l.sy *= k;
                        if l.lock {
                            l.sx *= k;
                        }
                        changed = true;
                    }
                }
            });
            if l.rotation.abs() > 0.01 {
                ui.label(RichText::new("Sizes are before rotation.").weak().small());
            }
            ui.horizontal(|ui| {
                ui.label("Rotate");
                if let Some(r) = num_field(ui, "layer-rot", l.rotation, 1, "°") {
                    l.rotation = r;
                    changed = true;
                }
                for (lbl, d) in [("−15°", -15.0), ("+15°", 15.0)] {
                    if ui.small_button(lbl).clicked() {
                        l.rotation += d;
                        changed = true;
                    }
                }
            });

            section(ui, "Thread");
            let current = l.opts.thread;
            let shown = if let Some(t) = &l.custom {
                format!("Custom: {} {}", t.code, t.name)
            } else {
                match current {
                    Some(t) if t < chart.len() => format!("{} {}", chart[t].code, chart[t].name),
                    _ => "Match artwork color".into(),
                }
            };
            ui.horizontal(|ui| {
                if let Some(t) = &l.custom {
                    swatch(ui, t.rgb);
                } else if let Some(t) = current.filter(|t| *t < chart.len()) {
                    swatch(ui, chart[t].rgb);
                }
                egui::ComboBox::from_id_salt("thread")
                    .width(240.0)
                    .height(380.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        let fid = egui::Id::new("thread-filter");
                        let mut q: String = ui.data_mut(|d| d.get_temp(fid)).unwrap_or_default();
                        ui.add(
                            egui::TextEdit::singleline(&mut q)
                                .hint_text("Search code or name…")
                                .desired_width(220.0),
                        );
                        ui.data_mut(|d| d.insert_temp(fid, q.clone()));
                        let ql = q.to_lowercase();
                        if ui
                            .selectable_label(
                                current.is_none() && l.custom.is_none(),
                                "Match artwork color",
                            )
                            .clicked()
                        {
                            l.opts.thread = None;
                            l.custom = None;
                            changed = true;
                        }
                        if ui
                            .selectable_label(
                                l.custom.is_some(),
                                "Custom thread (pick color, type code)…",
                            )
                            .clicked()
                        {
                            let rgb = current
                                .filter(|t| *t < chart.len())
                                .map(|t| chart[t].rgb)
                                .unwrap_or(l.shapes.first().map(|s| s.rgb).unwrap_or([128; 3]));
                            l.custom = Some(thread::Thread {
                                brand: "Custom".into(),
                                code: String::new(),
                                name: String::new(),
                                rgb,
                            });
                            changed = true;
                        }
                        ui.separator();
                        for (i, t) in chart.iter().enumerate() {
                            if !ql.is_empty()
                                && !t.code.to_lowercase().contains(&ql)
                                && !t.name.to_lowercase().contains(&ql)
                            {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                swatch(ui, t.rgb);
                                if ui
                                    .selectable_label(
                                        current == Some(i) && l.custom.is_none(),
                                        format!("{} {}", t.code, t.name),
                                    )
                                    .clicked()
                                {
                                    l.opts.thread = Some(i);
                                    l.custom = None;
                                    changed = true;
                                }
                            });
                        }
                    });
            });
            if let Some(t) = &mut l.custom {
                ui.horizontal(|ui| {
                    ui.label("Color");
                    changed |= ui.color_edit_button_srgb(&mut t.rgb).changed();
                    ui.label("Code");
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut t.code)
                                .hint_text("e.g. 1147")
                                .desired_width(70.0),
                        )
                        .lost_focus();
                });
                ui.horizontal(|ui| {
                    ui.label("Name");
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut t.name)
                                .hint_text("e.g. Madeira Gold")
                                .desired_width(170.0),
                        )
                        .lost_focus();
                    ui.label("Hex");
                    let hex = format!("#{:02x}{:02x}{:02x}", t.rgb[0], t.rgb[1], t.rgb[2]);
                    if let Some(v) = text_field(ui, "custom-hex", &hex, 70.0) {
                        if let Some(rgb) = parse_hex(&v) {
                            t.rgb = rgb;
                            changed = true;
                        }
                    }
                });
            }

            section(ui, "Stitching");
            ui.horizontal(|ui| {
                ui.label("Type");
                egui::ComboBox::from_id_salt("stitch")
                    .selected_text(l.opts.stitch.label())
                    .show_ui(ui, |ui| {
                        for c in StitchChoice::ALL {
                            if ui
                                .selectable_value(&mut l.opts.stitch, c, c.label())
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Angle");
                let mut auto = l.opts.angle_deg.is_none();
                if ui.checkbox(&mut auto, "Auto").changed() {
                    l.opts.angle_deg = if auto {
                        None
                    } else {
                        Some(auto_angle.unwrap_or(45.0))
                    };
                    changed = true;
                }
                // Type any angle; doing so switches Auto off.
                let shown = l.opts.angle_deg.or(auto_angle).unwrap_or(45.0);
                if let Some(v) = num_field(ui, "angle", shown, 0, "°") {
                    l.opts.angle_deg = Some(v.rem_euclid(180.0));
                    changed = true;
                }
                for d in [0.0, 45.0, 90.0, 135.0] {
                    if ui.small_button(format!("{d:.0}")).clicked() {
                        l.opts.angle_deg = Some(d);
                        changed = true;
                    }
                }
            });
            if l.opts.angle_deg.is_none() && varies {
                ui.label(RichText::new("Auto: angle varies per piece (each letter gets its own). Type a number to set one angle for all.").weak().small());
            }
            ui.horizontal(|ui| {
                ui.label("Stop before");
                let mut v = l.opts.stop;
                ui.selectable_value(&mut v, None, "Auto");
                ui.selectable_value(&mut v, Some(true), "On");
                ui.selectable_value(&mut v, Some(false), "Off");
                if v != l.opts.stop {
                    l.opts.stop = v;
                    changed = true;
                }
                if let (None, Some(s)) = (l.opts.stop, stop_now) {
                    ui.label(RichText::new(if s { "(stops)" } else { "(no stop)" }).weak());
                }
            });
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut l.visible, "Visible").changed();
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("⬆ Earlier")
                .on_hover_text("Sew sooner / move down in the stack")
                .clicked()
            {
                action = Some("up");
            }
            if ui
                .button("⬇ Later")
                .on_hover_text("Sew later / move up in the stack")
                .clicked()
            {
                action = Some("down");
            }
            if ui.button("Duplicate").clicked() {
                action = Some("dup");
            }
            if ui.button("Delete").clicked() {
                action = Some("del");
            }
        });
        if let Some(k) = self.layers[li].link {
            let others: Vec<String> = self
                .layers
                .iter()
                .enumerate()
                .filter(|(j, l)| *j != li && l.link == Some(k))
                .map(|(_, l)| l.name.clone())
                .collect();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("🔗 Moves with: {}", others.join(", "))).weak());
                if ui
                    .small_button("Unlink")
                    .on_hover_text("Move and size this layer on its own")
                    .clicked()
                {
                    action = Some("unlink");
                }
            });
        }
        if changed || text_changed {
            self.checkpoint(now);
            self.sync_links(li);
            self.dirty = true;
        }
        if text_changed {
            self.refresh_text(li);
        }
        if let Some(a) = action {
            self.layer_action(a, li);
        }
    }

    fn layer_action(&mut self, a: &str, li: usize) {
        self.hard_checkpoint();
        match a {
            "up" if li > 0 => self.layers.swap(li, li - 1),
            "down" if li + 1 < self.layers.len() => self.layers.swap(li, li + 1),
            "dup" => {
                let mut l = self.layers[li].clone();
                l.link = None;
                l.id = self.next_id;
                self.next_id += 1;
                l.name = format!("{} copy", l.name);
                l.offset = l.offset.add(Pt::new(5.0, 5.0));
                self.selected = Some(l.id);
                self.layers.insert(li + 1, l);
            }
            "del" => {
                self.layers.remove(li);
                self.selected = None;
            }
            "unlink" => {
                let k = self.layers[li].link;
                self.layers[li].link = None;
                let rest: Vec<usize> = (0..self.layers.len())
                    .filter(|&j| self.layers[j].link == k && k.is_some())
                    .collect();
                if rest.len() == 1 {
                    self.layers[rest[0]].link = None;
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, now: f64) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.layer_props(ui, now);
            if self.selected.is_some() {
                ui.add_space(6.0);
                ui.separator();
            }

            if self.machine.is_none() {
            section(ui, "Design size");
            if let Some(d) = &self.design {
                let b = d.bbox;
                ui.horizontal(|ui| {
                    ui.label("W");
                    let w = num_field(ui, "design-w", b.width() / 25.4, 2, "in");
                    ui.label("H");
                    let h = num_field(ui, "design-h", b.height() / 25.4, 2, "in");
                    ui.label(RichText::new("🔒").weak()).on_hover_text("The whole design scales in proportion; unlock a single layer to stretch it");
                    let k = match (w, h) {
                        (Some(w), _) if w > 0.05 && b.width() > 0.0 => Some(w * 25.4 / b.width()),
                        (_, Some(h)) if h > 0.05 && b.height() > 0.0 => Some(h * 25.4 / b.height()),
                        _ => None,
                    };
                    if let Some(k) = k {
                        self.hard_checkpoint();
                        scale_layers(&mut self.layers, b.center(), k);
                        self.dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    for (target, l) in [(1.0, "1\""), (2.0, "2\""), (3.5, "3.5\" chest"), (4.5, "4.5\" cap")] {
                        if ui.small_button(l).clicked() && b.width() > 0.0 {
                            self.hard_checkpoint();
                            scale_layers(&mut self.layers, b.center(), target * 25.4 / b.width());
                            self.dirty = true;
                        }
                    }
                });
            }

            }
            section(ui, "Sewing on");
            ui.horizontal(|ui| {
                let before = self.profile;
                ui.selectable_value(&mut self.profile, Profile::Flat, "Flat goods");
                ui.selectable_value(&mut self.profile, Profile::Cap, "Cap (center-out)");
                if self.profile != before {
                    self.fabric = self.profile.default_recipe().into();
                    self.recipe = settings::recipe(&self.fabric).unwrap();
                    self.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Fabric");
                let before = self.fabric.clone();
                egui::ComboBox::from_id_salt("fabric").selected_text(nice(&self.fabric)).show_ui(ui, |ui| {
                    for f in settings::RECIPES {
                        ui.selectable_value(&mut self.fabric, f.to_string(), nice(f));
                    }
                });
                if self.fabric != before {
                    self.recipe = settings::recipe(&self.fabric).unwrap();
                    self.dirty = true;
                }
            });

            if self.machine.is_none() {
            section(ui, "Sewing order and stops");
            if ui.checkbox(&mut self.keep_order, "Sew in my layer order").on_hover_text("Off = the garment profile reorders layers automatically").changed() {
                self.dirty = true;
            }
            if ui.button("Auto-sequence layers").on_hover_text(match self.profile {
                Profile::Flat => "Reorder layers to cut colour changes while keeping overlaps correct",
                Profile::Cap => "Reorder layers center-out, bottom first",
            }).clicked() {
                self.auto_sequence();
            }
            if ui.checkbox(&mut self.auto_stops, "Automatic stops at color changes").on_hover_text("Off = stops only where you turn STOP on").changed() {
                self.dirty = true;
            }

            if self.pro {
                section(ui, "Stitch settings (Pro)");
                let r = &mut self.recipe;
                let mut ch = false;
                egui::Grid::new("pro").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Fill density");
                    ch |= ui.add(egui::Slider::new(&mut r.fill_spacing, 0.25..=0.8).suffix(" mm").fixed_decimals(2)).changed();
                    ui.end_row();
                    ui.label("Fill stitch length");
                    ch |= ui.add(egui::Slider::new(&mut r.fill_len, 2.0..=6.0).suffix(" mm").fixed_decimals(1)).changed();
                    ui.end_row();
                    ui.label("Satin density");
                    ch |= ui.add(egui::Slider::new(&mut r.satin_spacing, 0.25..=0.8).suffix(" mm").fixed_decimals(2)).changed();
                    ui.end_row();
                    ui.label("Pull compensation");
                    ch |= ui.add(egui::Slider::new(&mut r.pull_comp, 0.0..=0.8).suffix(" mm").fixed_decimals(2)).changed();
                    ui.end_row();
                    ui.label("Underlay spacing");
                    ch |= ui.add(egui::Slider::new(&mut r.underlay_spacing, 1.0..=5.0).suffix(" mm").fixed_decimals(1)).changed();
                    ui.end_row();
                });
                if ch {
                    self.dirty = true;
                }
                if ui.small_button("Reset to fabric defaults").clicked() {
                    self.recipe = settings::recipe(&self.fabric).unwrap();
                    self.dirty = true;
                }
                ui.horizontal(|ui| {
                    ui.label(format!("Threads: {}", self.chart_name));
                    if ui.small_button("Load chart CSV…").on_hover_text("Columns: code,name,r,g,b").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Thread chart", &["csv"]).pick_file() {
                            match thread::load_chart_csv(&p.to_string_lossy()) {
                                Ok(c) => {
                                    self.chart_name = p.file_stem().unwrap().to_string_lossy().to_string();
                                    self.chart = c;
                                    for l in &mut self.layers {
                                        l.opts.thread = None;
                                    }
                                    self.dirty = true;
                                }
                                Err(e) => self.error = Some(e),
                            }
                        }
                    }
                });
            }

            }
            if let Some(d) = &self.design {
                section(ui, "Production");
                let s = &d.stats;
                egui::Grid::new("stats").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    kv(ui, "Size", &format!("{:.2} × {:.2} in  ({:.0} × {:.0} mm)", s.width_mm / 25.4, s.height_mm / 25.4, s.width_mm, s.height_mm));
                    kv(ui, "Stitches", &group(s.stitches));
                    kv(ui, "Stops", &d.stops.len().to_string());
                    kv(ui, "Trims", &s.trims.to_string());
                    kv(ui, "Est. run time", &format!("{:.1} min", s.run_minutes));
                    kv(ui, "Backing", &d.recipe.backing);
                    kv(ui, "Topping", &d.recipe.topping);
                });

                section(ui, "Color stops");
                for (i, st) in d.stops.iter().enumerate() {
                    ui.horizontal(|ui| {
                        swatch(ui, st.thread.rgb);
                        ui.label(format!("{} · needle {}", i + 1, st.needle));
                        ui.label(RichText::new(format!("{} {}", st.thread.code, st.thread.name)).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(group(st.stitches)).weak());
                        });
                    });
                }

                if !d.warnings.is_empty() {
                    section(ui, "Check before running");
                    for w in &d.warnings {
                        ui.label(RichText::new(format!("⚠ {w}")).color(Color32::from_rgb(196, 130, 20)));
                    }
                }
            }

            let job_before = self.job.clone();
            section(ui, "Job (for the operator sheet)");
            egui::Grid::new("job").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                for (label, v) in [
                    ("Job #", &mut self.job.job),
                    ("Customer", &mut self.job.customer),
                    ("Due", &mut self.job.due),
                    ("Quantity", &mut self.job.quantity),
                    ("Machine", &mut self.job.machine),
                    ("Placement", &mut self.job.placement),
                    ("Garment", &mut self.job.garment),
                ] {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(v).desired_width(180.0));
                    ui.end_row();
                }
                ui.label("Notes");
                ui.add(egui::TextEdit::multiline(&mut self.job.notes).desired_width(180.0).desired_rows(2));
                ui.end_row();
            });
            if self.job != job_before && self.machine.is_none() {
                self.modified = true;
            }
            ui.add_space(12.0);
        });
    }

    // ---------- bottom bar ----------

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let total = self.design.as_ref().map(|d| d.cmds.len()).unwrap_or(0);
            let enabled = total > 0;
            let play_label = if self.playing {
                "⏸ Pause"
            } else {
                "▶ Play sew-out"
            };
            if ui
                .add_enabled(enabled, egui::Button::new(play_label))
                .clicked()
            {
                if !self.playing && self.progress >= total {
                    self.progress = 0;
                }
                self.playing = !self.playing;
            }
            if ui
                .add_enabled(enabled, egui::Button::new("⏮"))
                .on_hover_text("Back to start")
                .clicked()
            {
                self.progress = 0;
                self.playing = false;
            }
            if ui
                .add_enabled(enabled, egui::Button::new("⏭"))
                .on_hover_text("Show everything")
                .clicked()
            {
                self.progress = total;
                self.playing = false;
            }
            ui.add_enabled(
                enabled,
                egui::Slider::new(&mut self.progress, 0..=total.max(1)).show_value(false),
            );
            if let Some(d) = &self.design {
                let n = d.cmds[..self.progress.min(d.cmds.len())]
                    .iter()
                    .filter(|c| matches!(c, Cmd::Stitch(_)))
                    .count();
                ui.label(format!("{} / {}", group(n), group(d.stats.stitches)));
            }
            ui.label("Speed");
            ui.add(
                egui::Slider::new(&mut self.speed, 50.0..=8000.0)
                    .logarithmic(true)
                    .show_value(false),
            );
            ui.separator();
            ui.selectable_value(&mut self.view, View::ThreeD, "3D")
                .on_hover_text("Shaded thread view");
            ui.selectable_value(&mut self.view, View::Flat, "Flat")
                .on_hover_text("Plain lines");
            ui.toggle_value(&mut self.show_jumps, "Jumps");
            if ui
                .button("Fit")
                .on_hover_text("Fit design to view (F)")
                .clicked()
            {
                self.fit = true;
            }
            ui.separator();
            ui.label(RichText::new(&self.status).weak());
        });
    }

    // ---------- canvas ----------

    fn canvas(&mut self, ui: &mut egui::Ui, now: f64) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let fabric = Color32::from_rgb(242, 239, 232);
        painter.rect_filled(rect, 0.0, fabric);

        let Some(d) = self.design.as_ref() else {
            let c = rect.center();
            let ink = Color32::from_rgb(70, 68, 64);
            painter.text(
                c - Vec2::new(0.0, 30.0),
                egui::Align2::CENTER_CENTER,
                "Drop an SVG here",
                egui::FontId::proportional(26.0),
                ink,
            );
            painter.text(
                c + Vec2::new(0.0, 8.0),
                egui::Align2::CENTER_CENTER,
                "or use Open SVG…, Add text…, or Sample above",
                egui::FontId::proportional(15.0),
                Color32::from_rgb(120, 116, 108),
            );
            if let Some(e) = &self.error {
                painter.text(
                    c + Vec2::new(0.0, 44.0),
                    egui::Align2::CENTER_CENTER,
                    e,
                    egui::FontId::proportional(14.0),
                    Color32::from_rgb(200, 60, 50),
                );
            }
            return;
        };

        let b = d.bbox;
        if self.fit {
            self.zoom = ((rect.width() - 100.0) / b.width() as f32)
                .min((rect.height() - 100.0) / b.height() as f32)
                .max(0.5);
            self.view_center = b.center();
            self.fit = false;
            if let Some(z) = self.pending_zoom.take() {
                self.zoom *= z;
            }
        }
        let zoom = self.zoom;
        let vc = self.view_center;
        let rc = rect.center();
        let to_screen = move |p: Pt| -> Pos2 {
            rc + Vec2::new((p.x - vc.x) as f32, (p.y - vc.y) as f32) * zoom
        };
        let to_mm = move |s: Pos2| -> Pt {
            Pt::new(
                vc.x + ((s.x - rc.x) / zoom) as f64,
                vc.y + ((s.y - rc.y) / zoom) as f64,
            )
        };

        // ---- interaction: select, move, pan, zoom
        let hit_layer = |pos: Pos2, layers: &[Layer], zoom: f32| -> Option<u64> {
            let p = to_mm(pos);
            let tol = (4.0 / zoom) as f64;
            layers
                .iter()
                .rev()
                .find(|l| l.visible && l.hit(p, tol))
                .map(|l| l.id)
        };
        if resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let hit = hit_layer(pos, &self.layers, zoom);
                if hit.is_some() && (hit == self.selected || ui.input(|i| !i.modifiers.shift)) {
                    self.selected = hit;
                    self.moving = Some(Pt::new(0.0, 0.0));
                } else {
                    self.moving = None;
                }
            }
        }
        if resp.dragged() {
            let dlt = resp.drag_delta();
            match &mut self.moving {
                Some(m) => *m = m.add(Pt::new((dlt.x / zoom) as f64, (dlt.y / zoom) as f64)),
                None => {
                    self.view_center = self
                        .view_center
                        .sub(Pt::new((dlt.x / zoom) as f64, (dlt.y / zoom) as f64))
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(m) = self.moving.take() {
                if m.len() > 0.01 {
                    self.hard_checkpoint();
                    if let Some(li) = self.sel_idx() {
                        for j in self.linked(li) {
                            self.layers[j].offset = self.layers[j].offset.add(m);
                        }
                    }
                    self.dirty = true;
                }
            }
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                self.selected = hit_layer(pos, &self.layers, zoom);
            }
        }
        if resp.double_clicked() && self.selected.is_none() {
            self.fit = true;
        }
        if resp.hovered() {
            let (scroll, pinch, cursor) = ui.input(|i| {
                (
                    i.smooth_scroll_delta.y,
                    i.zoom_delta(),
                    i.pointer.hover_pos(),
                )
            });
            let factor = pinch * (scroll / 400.0).exp();
            if (factor - 1.0).abs() > 1e-4 {
                if let Some(cur) = cursor {
                    let before = to_mm(cur);
                    self.zoom = (self.zoom * factor).clamp(0.3, 200.0);
                    // keep the point under the cursor fixed
                    let z = self.zoom;
                    self.view_center = Pt::new(
                        before.x - ((cur.x - rc.x) / z) as f64,
                        before.y - ((cur.y - rc.y) / z) as f64,
                    );
                }
            }
            if let Some(cur) = cursor {
                if !resp.dragged() && hit_layer(cur, &self.layers, zoom).is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }
        if self.moving.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let _ = now;

        let d = self.design.as_ref().unwrap();
        let painter = painter.with_clip_rect(rect);

        // grid every 10 mm, and a slightly darker hoop-ish area around the design
        let grid = Color32::from_rgba_unmultiplied(0, 0, 0, 12);
        let tl = to_mm(rect.min);
        let br = to_mm(rect.max);
        if zoom > 1.2 {
            let mut x = (tl.x / 10.0).floor() * 10.0;
            while x <= br.x {
                let sx = to_screen(Pt::new(x, 0.0)).x;
                painter.line_segment(
                    [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
                    Stroke::new(1.0_f32, grid),
                );
                x += 10.0;
            }
            let mut y = (tl.y / 10.0).floor() * 10.0;
            while y <= br.y {
                let sy = to_screen(Pt::new(0.0, y)).y;
                painter.line_segment(
                    [Pos2::new(rect.left(), sy), Pos2::new(rect.right(), sy)],
                    Stroke::new(1.0_f32, grid),
                );
                y += 10.0;
            }
        }

        // ---- stitches
        let end = self.progress.min(d.cmds.len());
        let sel_layer = self.sel_idx();
        let moving = self.moving.filter(|m| m.len() > 0.0);
        let moving_set: Vec<usize> = sel_layer.map(|li| self.linked(li)).unwrap_or_default();
        let view = self.view;
        let show_jumps = self.show_jumps;
        let draw_obj = |o: &stitchforge::design::ObjectInfo, shift: Pt, painter: &egui::Painter| {
            let (a, z) = o.cmd_range;
            if a >= end {
                return;
            }
            let to = |p: Pt| to_screen(p.add(shift));
            let rgb = d.stops[o.stop].thread.rgb;
            painter.extend(stitch_shapes(
                d,
                a,
                z.min(end),
                rgb,
                &to,
                zoom,
                view,
                show_jumps,
                rect.expand(20.0),
            ));
        };
        for o in &d.objects {
            if moving.is_some() && moving_set.contains(&o.layer) {
                continue;
            }
            draw_obj(o, Pt::new(0.0, 0.0), &painter);
        }
        if let Some(m) = moving {
            for o in d.objects.iter().filter(|o| moving_set.contains(&o.layer)) {
                draw_obj(o, m, &painter);
            }
        }

        // needle marker during playback
        if end < d.cmds.len() {
            if let Some(p) = d.cmds[..end].iter().rev().find_map(|c| match c {
                Cmd::Stitch(p) | Cmd::Jump(p) => Some(*p),
                _ => None,
            }) {
                let s = to_screen(p);
                painter.circle_filled(s, 4.0, Color32::from_rgb(230, 50, 40));
                painter.circle_stroke(s, 7.0, Stroke::new(1.5_f32, Color32::from_rgb(230, 50, 40)));
            }
        }

        // ---- selection box and stitch-direction arrow
        if let Some(li) = sel_layer {
            let l = &self.layers[li];
            let shift = moving.unwrap_or(Pt::new(0.0, 0.0));
            let lb = l.bbox();
            if lb.is_valid() {
                let r =
                    Rect::from_min_max(to_screen(lb.min.add(shift)), to_screen(lb.max.add(shift)))
                        .expand(6.0);
                let accent = Color32::from_rgb(30, 110, 230);
                for seg in EShape::dashed_line(
                    &[
                        r.left_top(),
                        r.right_top(),
                        r.right_bottom(),
                        r.left_bottom(),
                        r.left_top(),
                    ],
                    Stroke::new(1.5_f32, accent),
                    6.0,
                    4.0,
                ) {
                    painter.add(seg);
                }
                for c in [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ] {
                    painter.rect_filled(Rect::from_center_size(c, Vec2::splat(7.0)), 1.0, accent);
                }
                let pieces: Vec<&stitchforge::design::ObjectInfo> = d
                    .objects
                    .iter()
                    .filter(|o| o.layer == li && has_angle(o.stitch_type))
                    .collect();
                let same = pieces
                    .iter()
                    .all(|o| (o.angle_deg - pieces[0].angle_deg).abs() < 0.5);
                if let Some(o) = pieces.first().filter(|_| same) {
                    {
                        let a = o.angle_deg.to_radians() as f32;
                        let c = r.center();
                        let len = (r.width().min(r.height()) * 0.35).clamp(20.0, 70.0);
                        let dir = Vec2::new(a.cos(), -a.sin()) * len;
                        let col = Color32::from_rgb(255, 255, 255);
                        painter.line_segment(
                            [c - dir, c + dir],
                            Stroke::new(5.0_f32, Color32::from_black_alpha(120)),
                        );
                        painter.line_segment([c - dir, c + dir], Stroke::new(2.5_f32, col));
                        painter.circle_filled(c + dir, 4.5, col);
                        painter.circle_filled(c - dir, 4.5, col);
                        painter.text(
                            c + Vec2::new(0.0, -14.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("{:.0}°", o.angle_deg),
                            egui::FontId::proportional(13.0),
                            col,
                        );
                    }
                }
            }
        }

        // size callout
        painter.text(
            Pos2::new(
                to_screen(Pt::new(b.center().x, b.max.y)).x,
                to_screen(b.max).y + 12.0,
            ),
            egui::Align2::CENTER_TOP,
            format!(
                "{:.2}\" × {:.2}\"  ·  {}",
                b.width() / 25.4,
                b.height() / 25.4,
                d.profile.label()
            ),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(110, 106, 98),
        );
    }

    fn text_window(&mut self, ctx: &egui::Context) {
        if !self.show_text {
            return;
        }
        if self.fonts.is_none() {
            let fonts = import::list_fonts();
            if !fonts.iter().any(|f| *f == self.font) {
                if let Some(f) = fonts.iter().find(|f| f.contains("Sans")).or(fonts.first()) {
                    self.font = f.clone();
                }
            }
            self.fonts = Some(fonts);
        }
        let mut open = true;
        let mut go = false;
        egui::Window::new("Add text")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label("Words (Enter for a new line)");
                ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .desired_rows(2)
                        .desired_width(330.0),
                );
                ui.horizontal(|ui| {
                    ui.label("Letter height");
                    ui.add(
                        egui::DragValue::new(&mut self.text_height_in)
                            .speed(0.01)
                            .range(0.15..=8.0)
                            .suffix(" in")
                            .max_decimals(2),
                    );
                    ui.label("Color");
                    ui.color_edit_button_srgb(&mut self.text_color);
                });
                ui.label("Font (any installed font)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.font_filter)
                        .hint_text("Search fonts…")
                        .desired_width(330.0),
                );
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        let q = self.font_filter.to_lowercase();
                        for f in self
                            .fonts
                            .as_ref()
                            .unwrap()
                            .iter()
                            .filter(|f| q.is_empty() || f.to_lowercase().contains(&q))
                            .take(400)
                        {
                            if ui.selectable_label(*f == self.font, f).clicked() {
                                self.font = f.clone();
                            }
                        }
                    });
                ui.label(RichText::new(format!("Selected: {}", self.font)).weak());
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(Color32::from_rgb(210, 70, 60)));
                }
                if ui.button(RichText::new("Add to design").strong()).clicked() {
                    go = true;
                }
            });
        if go && !self.text.trim().is_empty() {
            self.add_text_layer();
        }
        if !open {
            self.show_text = false;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        // closing with unsaved work: ask first
        if ctx.input(|i| i.viewport().close_requested()) && self.modified && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Pending::Close);
        }
        // drag and drop: an SVG adds to the open design; anything else opens
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for p in dropped {
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if ext == "svg" && !self.layers.is_empty() && self.machine.is_none() {
                self.open_svg_path(p, true);
            } else {
                self.guard(Pending::OpenPath(p), ctx);
            }
        }
        // shortcuts (ignored while typing in a field)
        let typing = ctx.wants_keyboard_input();
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        if cmd && pressed(egui::Key::O) {
            self.guard(Pending::Open, ctx);
        }
        if cmd && pressed(egui::Key::S) {
            if shift {
                self.save_as();
            } else {
                self.save();
            }
        }
        if cmd && pressed(egui::Key::N) {
            self.guard(Pending::New, ctx);
        }
        if cmd && pressed(egui::Key::E) && self.design.is_some() {
            self.show_export = true;
        }
        if !typing {
            if cmd && pressed(egui::Key::Z) {
                if shift {
                    self.do_redo()
                } else {
                    self.do_undo()
                }
            }
            if pressed(egui::Key::F) && !cmd {
                self.fit = true;
            }
            if pressed(egui::Key::Space) && self.design.is_some() {
                self.playing = !self.playing;
            }
            if let Some(li) = self.sel_idx() {
                let step = if shift { 5.0 } else { 0.5 };
                let mut nudge = Pt::new(0.0, 0.0);
                if pressed(egui::Key::ArrowLeft) {
                    nudge.x -= step;
                }
                if pressed(egui::Key::ArrowRight) {
                    nudge.x += step;
                }
                if pressed(egui::Key::ArrowUp) {
                    nudge.y -= step;
                }
                if pressed(egui::Key::ArrowDown) {
                    nudge.y += step;
                }
                if nudge.len() > 0.0 {
                    self.checkpoint(now);
                    for j in self.linked(li) {
                        self.layers[j].offset = self.layers[j].offset.add(nudge);
                    }
                    self.dirty = true;
                }
                if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
                    self.layer_action("del", li);
                }
                if cmd && pressed(egui::Key::D) {
                    self.layer_action("dup", li);
                }
                if pressed(egui::Key::Escape) {
                    self.selected = None;
                }
            }
        }

        if self.dirty && self.moving.is_none() {
            if self.clean_after_regen {
                self.clean_after_regen = false;
                self.modified = false;
            } else if self.machine.is_none() {
                self.modified = true;
            }
            self.regenerate();
        }
        // playback
        if self.playing {
            if let Some(d) = &self.design {
                let dt = ctx.input(|i| i.stable_dt).min(0.1);
                self.play_acc += self.speed * dt;
                let mut steps = self.play_acc as usize;
                self.play_acc -= steps as f32;
                while steps > 0 && self.progress < d.cmds.len() {
                    if matches!(d.cmds[self.progress], Cmd::Stitch(_)) {
                        steps -= 1;
                    }
                    self.progress += 1;
                }
                if self.progress >= d.cmds.len() {
                    self.playing = false;
                }
                ctx.request_repaint();
            }
        }

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("bottom")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ctx, |ui| self.bottom_bar(ui));
        let title = format!(
            "StitchForge — {}{}",
            self.display_name(),
            if self.modified { " •" } else { "" }
        );
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        egui::SidePanel::left("layers")
            .resizable(true)
            .default_width(320.0)
            .show(ctx, |ui| {
                if self.machine.is_some() {
                    self.machine_panel(ui)
                } else {
                    self.layers_panel(ui)
                }
            });
        egui::SidePanel::right("props")
            .resizable(true)
            .default_width(350.0)
            .show(ctx, |ui| self.right_panel(ui, now));
        if let Some(e) = self.error.clone() {
            if self.design.is_some() && !self.show_text {
                egui::TopBottomPanel::top("err").show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(210, 70, 60)),
                        );
                        if ui.small_button("Dismiss").clicked() {
                            self.error = None;
                        }
                    });
                });
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.canvas(ui, now));
        self.text_window(ctx);
        self.export_window(ctx);
        self.confirm_window(ctx);
    }
}

// ---------------------------------------------------------------- drawing

/// Shapes for commands `from..to`. Flat = plain thread lines; 3D = each
/// stitch as a small mesh, dark at the needle holes and lit along its middle.
#[allow(clippy::too_many_arguments)]
fn stitch_shapes(
    d: &Design,
    from: usize,
    to_idx: usize,
    rgb: [u8; 3],
    to: &dyn Fn(Pt) -> Pos2,
    zoom: f32,
    view: View,
    show_jumps: bool,
    clip: Rect,
) -> Vec<EShape> {
    let col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let w = (0.38 * zoom).max(1.0);
    let mut shapes = Vec::new();
    let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
    let mut last: Option<Pos2> = None;
    let jump_col = Color32::from_rgba_unmultiplied(220, 40, 40, 170);
    for cmd in &d.cmds[from..to_idx] {
        match *cmd {
            Cmd::Trim | Cmd::Color(_) | Cmd::End => {
                runs.push(Vec::new());
                last = None;
            }
            Cmd::Jump(p) => {
                let q = to(p);
                if show_jumps {
                    if let Some(l) = last {
                        shapes.extend(EShape::dashed_line(
                            &[l, q],
                            Stroke::new(1.0_f32, jump_col),
                            4.0,
                            3.0,
                        ));
                    }
                }
                runs.push(Vec::new());
                last = Some(q);
            }
            Cmd::Stitch(p) => {
                let q = to(p);
                let r = runs.last_mut().unwrap();
                if r.is_empty() {
                    if let Some(l) = last {
                        r.push(l);
                    }
                }
                r.push(q);
                last = Some(q);
            }
        }
    }
    match view {
        View::Flat => {
            let shade = scale_col(col, 0.6);
            for r in runs.into_iter().filter(|r| r.len() >= 2) {
                // short overlapping pieces so later stitches paint over earlier ones
                let mut i = 0;
                while i + 1 < r.len() {
                    let piece = r[i..(i + 26).min(r.len())].to_vec();
                    shapes.push(EShape::line(piece.clone(), Stroke::new(w * 1.4, shade)));
                    shapes.push(EShape::line(piece, Stroke::new(w, col)));
                    i += 25;
                }
            }
        }
        View::ThreeD => {
            // Delicate thread look (after Tim's reference): the thread colour,
            // a soft gradient across its width (lit top edge, shaded bottom
            // edge, untouched middle) and, when zoomed in, faint diagonal
            // twist lines like the ply of real thread.
            let light = Vec2::new(0.0, -1.0); // light from the top of the screen
            let mut mesh = egui::Mesh::default();
            let across: [(f32, f32); 5] = [
                (-1.0, -0.36),
                (-0.55, -0.16),
                (0.0, 0.0),
                (0.55, 0.12),
                (1.0, 0.26),
            ];
            let half = w * 0.6;
            let twist = w >= 3.0; // only when a thread is wide enough to show it
            let pitch = (w * 1.1).max(3.0); // px between twist lines
            for r in runs.iter().filter(|r| r.len() >= 2) {
                for s in r.windows(2) {
                    let (a, b) = (s[0], s[1]);
                    let v = b - a;
                    let len = v.length();
                    if len < 0.3
                        || (!clip.contains(a)
                            && !clip.contains(b)
                            && !clip.intersects(Rect::from_two_pos(a, b)))
                    {
                        continue;
                    }
                    let dir = v / len;
                    let mut n = Vec2::new(-dir.y, dir.x);
                    if n.dot(light) < 0.0 {
                        n = -n; // +n always points toward the light
                    }
                    // stations along the stitch; twist lines are thin darker bands
                    let mut stations: Vec<(f32, f32)> = vec![(0.0, 0.0)];
                    if twist && len > pitch * 1.5 {
                        let k = (len / pitch).floor() as usize;
                        for i in 1..k {
                            let t = i as f32 * pitch;
                            stations.push((t - 0.18 * pitch, 0.0));
                            stations.push((t, -0.12));
                            stations.push((t + 0.18 * pitch, 0.0));
                        }
                    }
                    stations.push((len, 0.0));
                    // needle holes: the very ends sink slightly
                    stations[0].1 = -0.1;
                    let last = stations.len() - 1;
                    stations[last].1 = -0.1;
                    let slant = if twist { half * 0.9 } else { 0.0 };
                    let base = mesh.vertices.len() as u32;
                    let cols = across.len() as u32;
                    for &(t, dk) in &stations {
                        for &(sgn, tone) in &across {
                            // twist lines lean across the thread
                            let tt = (t + sgn * slant * if t > 0.0 && t < len { 1.0 } else { 0.0 })
                                .clamp(0.0, len);
                            let p = a + dir * tt + n * (sgn * half);
                            mesh.colored_vertex(p, tone_col(col, tone + dk));
                        }
                    }
                    for si in 0..(stations.len() as u32 - 1) {
                        for ci in 0..cols - 1 {
                            let i0 = base + si * cols + ci;
                            mesh.add_triangle(i0, i0 + 1, i0 + cols);
                            mesh.add_triangle(i0 + 1, i0 + cols + 1, i0 + cols);
                        }
                    }
                }
                if mesh.vertices.len() > 60_000 {
                    shapes.push(EShape::mesh(std::mem::take(&mut mesh)));
                }
            }
            if !mesh.vertices.is_empty() {
                shapes.push(EShape::mesh(mesh));
            }
        }
    }
    shapes
}

/// A typed number box. Shows the live value; returns the new value when the
/// user presses Enter or clicks away after typing.
fn num_field(
    ui: &mut egui::Ui,
    salt: &str,
    value: f64,
    decimals: usize,
    suffix: &str,
) -> Option<f64> {
    let shown = format!("{:.*}", decimals, value);
    let out = text_field(ui, salt, &shown, 58.0).and_then(|t| {
        let t = t.trim().trim_end_matches(suffix).trim().replace(',', ".");
        t.parse::<f64>().ok().filter(|v| v.is_finite())
    });
    ui.label(suffix);
    out
}

/// A text box that edits a copy and commits on Enter or focus loss.
fn text_field(ui: &mut egui::Ui, salt: &str, shown: &str, width: f32) -> Option<String> {
    let id = ui.id().with(salt);
    let mut buf: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| shown.to_string());
    let resp = ui.add(
        egui::TextEdit::singleline(&mut buf)
            .id(id.with("edit"))
            .desired_width(width),
    );
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf.clone()));
        None
    } else if resp.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        if buf != shown { Some(buf) } else { None }
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
        None
    }
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

fn has_angle(stitch_type: &str) -> bool {
    matches!(stitch_type, "Tatami fill" | "Satin")
}

/// A small expand/collapse triangle.
fn disclosure(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::click());
    let c = rect.center();
    let col = if resp.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let pts = if open {
        vec![
            c + Vec2::new(-4.0, -2.0),
            c + Vec2::new(4.0, -2.0),
            c + Vec2::new(0.0, 3.0),
        ]
    } else {
        vec![
            c + Vec2::new(-2.0, -4.0),
            c + Vec2::new(3.0, 0.0),
            c + Vec2::new(-2.0, 4.0),
        ]
    };
    ui.painter()
        .add(EShape::convex_polygon(pts, col, Stroke::NONE));
    resp
}

/// Shift a thread colour toward white (tone > 0) or black (tone < 0),
/// like a soft luminosity overlay.
fn tone_col(c: Color32, tone: f32) -> Color32 {
    let f = |v: u8| -> u8 {
        let v = v as f32;
        if tone >= 0.0 {
            v + (255.0 - v) * tone
        } else {
            v * (1.0 + tone)
        }
        .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn scale_col(c: Color32, k: f32) -> Color32 {
    // Lighten toward white when k > 1 so dark threads still get a highlight.
    let f = |v: u8| -> u8 {
        let v = v as f32;
        if k <= 1.0 {
            (v * k) as u8
        } else {
            (v + (255.0 - v) * (k - 1.0) * 1.6).min(255.0) as u8
        }
    };
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn scale_layers(layers: &mut [Layer], c: Pt, k: f64) {
    for l in layers {
        let center = l.center();
        let new_center = c.add(center.sub(c).mul(k));
        l.offset = new_center.sub(l.origin);
        l.sx *= k;
        l.sy *= k;
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(9.0);
    ui.label(RichText::new(title.to_uppercase()).small().strong().weak());
    ui.add_space(1.0);
}

fn kv(ui: &mut egui::Ui, k: &str, v: &str) {
    ui.label(RichText::new(k).weak());
    ui.label(RichText::new(v).strong());
    ui.end_row();
}

fn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {
    let (r, p) = ui.allocate_painter(Vec2::new(18.0, 12.0), Sense::hover());
    p.rect_filled(r.rect, 3.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    p.rect_stroke(
        r.rect,
        3.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha(70)),
        egui::StrokeKind::Inside,
    );
}

fn nice(f: &str) -> String {
    match f {
        "pique" => "Pique polo".into(),
        "tshirt" => "T-shirt knit".into(),
        "fleece" => "Fleece".into(),
        "twill" => "Twill / woven".into(),
        "cap" => "Structured cap".into(),
        "towel" => "Towel / terry".into(),
        o => o.into(),
    }
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn open_path(p: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(p)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}
