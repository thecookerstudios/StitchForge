//! StitchForge desktop app (v0.3 review build).
//!
//! Layers you can select, move, rotate, resize, recolor and re-angle on the
//! canvas; per-layer stop toggles; flat and 3D stitch views; sew-out playback.

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Shape as EShape, Stroke, Vec2};
use std::collections::HashSet;
use std::path::PathBuf;
use stitchforge::design::{BuildOptions, Cmd, Design};
use stitchforge::geom::{spans_at, BBox, Pt};
use stitchforge::import::{self, Artwork, Shape, ShapeKind, ShapeOpts, StitchChoice};
use stitchforge::settings::{self, Profile, Recipe};
use stitchforge::sheet::JobInfo;
use stitchforge::{export, thread};

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
    eframe::run_native("StitchForge", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

// ---------------------------------------------------------------- layers

#[derive(Clone)]
struct TextSpec {
    text: String,
    font: String,
    height_mm: f64,
    rgb: [u8; 3],
}

#[derive(Clone)]
struct Layer {
    id: u64,
    name: String,
    /// Geometry as imported (mm). The layer transform is applied on top.
    shapes: Vec<Shape>,
    /// Pivot for scale and rotation (centre of the original geometry).
    origin: Pt,
    offset: Pt,
    scale: f64,
    /// Degrees, counter-clockwise on screen.
    rotation: f64,
    opts: ShapeOpts,
    visible: bool,
    text: Option<TextSpec>,
    #[allow(dead_code)]
    is_outline: bool,
    /// Layers sharing a link move, rotate and resize together (e.g. a shape's
    /// fill and its outline).
    link: Option<u64>,
}

impl Layer {
    fn new(id: u64, name: String, shapes: Vec<Shape>, is_outline: bool) -> Self {
        let b = BBox::of(shapes.iter().flat_map(|s| s.points()));
        Layer {
            id,
            name,
            origin: b.center(),
            shapes,
            offset: Pt::new(0.0, 0.0),
            scale: 1.0,
            rotation: 0.0,
            opts: ShapeOpts::default(),
            visible: true,
            text: None,
            is_outline,
            link: None,
        }
    }

    fn tf(&self, p: Pt) -> Pt {
        p.sub(self.origin).mul(self.scale).rot(-self.rotation.to_radians()).add(self.origin).add(self.offset)
    }

    fn inv(&self, p: Pt) -> Pt {
        p.sub(self.offset).sub(self.origin).rot(self.rotation.to_radians()).mul(1.0 / self.scale).add(self.origin)
    }

    fn transformed(&self) -> Vec<Shape> {
        self.shapes
            .iter()
            .map(|s| {
                let mut s = s.clone();
                match &mut s.kind {
                    ShapeKind::Area { rings, .. } => rings.iter_mut().flatten().for_each(|p| *p = self.tf(*p)),
                    ShapeKind::Line { paths, width } => {
                        paths.iter_mut().flatten().for_each(|p| *p = self.tf(*p));
                        *width *= self.scale;
                    }
                }
                s
            })
            .collect()
    }

    fn bbox(&self) -> BBox {
        let mut b = BBox::empty();
        for s in &self.shapes {
            for p in s.points() {
                b.add(self.tf(*p));
            }
        }
        b
    }

    fn center(&self) -> Pt {
        self.origin.add(self.offset)
    }

    /// Does the point (design mm) land on this layer?
    fn hit(&self, p: Pt, tol_mm: f64) -> bool {
        let q = self.inv(p);
        let tol = tol_mm / self.scale;
        self.shapes.iter().any(|s| match &s.kind {
            ShapeKind::Area { rings, rule } => {
                spans_at(rings, *rule, q.y).iter().any(|&(a, b)| q.x >= a - tol && q.x <= b + tol)
            }
            ShapeKind::Line { paths, width } => paths.iter().any(|path| {
                path.windows(2).any(|w| seg_dist(q, w[0], w[1]) <= width / 2.0 + tol)
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

// ---------------------------------------------------------------- app

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

    status: String,
    last_dir: Option<PathBuf>,
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
            status: "Open an SVG, drop one on the window, or add text.".into(),
            last_dir: None,
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
                    if let (Some(dx), Some(dy), Some(l)) = (num(i + 1), num(i + 2), self.sel_mut()) {
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
                "--stop-off" => {
                    if let Some(l) = self.sel_mut() {
                        l.opts.stop = Some(false);
                    }
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
                a if a.ends_with(".svg") => self.open_svg_path(PathBuf::from(a), false),
                _ => {}
            }
            i += 1;
        }
        self
    }

    // ---------- layer helpers ----------

    fn sel_idx(&self) -> Option<usize> {
        self.selected.and_then(|id| self.layers.iter().position(|l| l.id == id))
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
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("design".into());
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
            let target = self.design.as_ref().map(|d| d.bbox.center()).unwrap_or(self.view_center);
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
                        if outline { format!("Outline {n}") } else { format!("Shape {n}") }
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
            Some(k) => (0..self.layers.len()).filter(|&j| self.layers[j].link == Some(k)).collect(),
            None => vec![li],
        }
    }

    /// Copy position, size and rotation from layer `li` to its linked layers.
    fn sync_links(&mut self, li: usize) {
        let (off, sc, rot) = (self.layers[li].offset, self.layers[li].scale, self.layers[li].rotation);
        for j in self.linked(li) {
            let l = &mut self.layers[j];
            l.offset = off;
            l.scale = sc;
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
                    ShapeKind::Area { rings, .. } => rings.iter_mut().flatten().for_each(|p| *p = p.sub(c).mul(k)),
                    ShapeKind::Line { paths, width } => {
                        paths.iter_mut().flatten().for_each(|p| *p = p.sub(c).mul(k));
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
                let mut l = Layer::new(id, spec.text.lines().next().unwrap_or("Text").to_string(), shapes, false);
                l.origin = Pt::new(0.0, 0.0);
                // Place below the current design, or at the view centre.
                l.offset = match &self.design {
                    Some(d) => Pt::new(d.bbox.center().x, d.bbox.max.y + spec.height_mm * 0.9),
                    None => self.view_center,
                };
                l.text = Some(spec);
                if self.layers.is_empty() {
                    self.design_name = l.name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
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
        let Some(spec) = self.layers[idx].text.clone() else { return };
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
        if self.layers.iter().all(|l| !l.visible) {
            self.design = None;
            return;
        }
        let mut shapes = Vec::new();
        for (li, l) in self.layers.iter().enumerate() {
            if !l.visible {
                continue;
            }
            for mut s in l.transformed() {
                s.opts = ShapeOpts { layer: li, ..l.opts.clone() };
                s.label = l.name.clone();
                shapes.push(s);
            }
        }
        let art = Artwork { shapes, warnings: Vec::new() };
        let name = format!("{}-{}", self.design_name, if self.profile == Profile::Cap { "cap" } else { "flat" });
        let t = std::time::Instant::now();
        let opts = BuildOptions { keep_order: self.keep_order, auto_stops: self.auto_stops };
        match stitchforge::design::build_with(&name, &art, &self.chart, &self.recipe, self.profile, opts) {
            Ok(d) => {
                self.progress = match self.pending_progress.take() {
                    Some(f) => (d.cmds.len() as f32 * f) as usize,
                    None => d.cmds.len(),
                };
                self.status = format!("Stitches generated in {} ms", t.elapsed().as_millis());
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
        let mut dlg = rfd::FileDialog::new().set_title("Choose a folder for the DST and operator sheet");
        if let Some(dir) = &self.last_dir {
            dlg = dlg.set_directory(dir);
        }
        let Some(dir) = dlg.pick_folder() else { return };
        let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let mut job = self.job.clone();
        if job.garment.is_empty() && self.profile == Profile::Cap {
            job.garment = "Cap".into();
        }
        match export::write_all(d, &job, &dir, &generated) {
            Ok(w) => {
                self.status = format!("Saved {} and the operator sheet to {}", w.dst.file_name().unwrap().to_string_lossy(), dir.display());
                open_path(&w.sheet);
                self.last_dir = Some(dir);
            }
            Err(e) => self.error = Some(e),
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
        d.objects.iter().find(|o| o.layer == li).map(|o| o.starts_stop)
    }

    fn layer_thread(&self, li: usize) -> Option<&thread::Thread> {
        let d = self.design.as_ref()?;
        d.objects.iter().find(|o| o.layer == li).map(|o| &d.stops[o.stop].thread)
    }

    // ---------- top bar ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("StitchForge").strong().size(17.0));
            ui.separator();
            if ui.button("Open SVG…").on_hover_text("Start a new design from vector art (Cmd/Ctrl+O)").clicked() {
                self.pick_svg(false);
            }
            if ui.add_enabled(!self.layers.is_empty(), egui::Button::new("Add art…")).on_hover_text("Add another SVG as new layers").clicked() {
                self.pick_svg(true);
            }
            if ui.button("Add text…").on_hover_text("Lettering in any installed font").clicked() {
                self.show_text = true;
            }
            if ui.button("Sample").clicked() {
                self.open_svg_data(SAMPLE_SVG, "sample-logo", false);
            }
            ui.separator();
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Cmd/Ctrl+Z").clicked() {
                self.do_undo();
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).on_hover_text("Shift+Cmd/Ctrl+Z").clicked() {
                self.do_redo();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(self.design.is_some(), egui::Button::new(RichText::new("Export DST + sheet…").strong())).clicked() {
                    self.export();
                }
                ui.separator();
                ui.selectable_value(&mut self.pro, true, "Pro");
                ui.selectable_value(&mut self.pro, false, "Simple");
            });
        });
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
        ui.label(RichText::new("Sews top to bottom. Click to select · STOP = machine stop before that layer.").weak().small());
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
            self.selected = if self.selected == Some(id) { None } else { Some(id) };
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
            .map(|d| d.objects.iter().filter(|o| o.layer == li && has_angle(o.stitch_type)).map(|o| o.angle_deg).collect())
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
                changed |= ui.add(egui::TextEdit::singleline(&mut l.name).desired_width(200.0)).changed();
            });
            if let Some(spec) = &mut l.text {
                ui.label("Words");
                text_changed |= ui.add(egui::TextEdit::multiline(&mut spec.text).desired_rows(2).desired_width(280.0)).lost_focus();
                ui.horizontal(|ui| {
                    ui.label("Font");
                    let fonts = self.fonts.get_or_insert_with(import::list_fonts);
                    egui::ComboBox::from_id_salt("layer-font").width(200.0).selected_text(&spec.font).show_ui(ui, |ui| {
                        for f in fonts.iter() {
                            if ui.selectable_value(&mut spec.font, f.clone(), f).clicked() {
                                text_changed = true;
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("Letter height");
                    let mut h = spec.height_mm / 25.4;
                    if ui.add(egui::DragValue::new(&mut h).speed(0.01).range(0.15..=8.0).suffix(" in").max_decimals(2)).changed() {
                        spec.height_mm = h * 25.4;
                        text_changed = true;
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("Width");
                let b = l.bbox();
                let mut w = b.width() / 25.4;
                if ui.add(egui::DragValue::new(&mut w).speed(0.01).range(0.1..=16.0).suffix(" in").max_decimals(2)).changed() && b.width() > 0.0 {
                    l.scale *= w * 25.4 / b.width();
                    changed = true;
                }
                ui.label(RichText::new(format!("× {:.2} in", b.height() / 25.4)).weak());
            });
            ui.horizontal(|ui| {
                ui.label("Rotate");
                changed |= ui.add(egui::DragValue::new(&mut l.rotation).speed(0.5).range(-360.0..=360.0).suffix("°").max_decimals(1)).changed();
                for (lbl, d) in [("−15°", -15.0), ("+15°", 15.0)] {
                    if ui.small_button(lbl).clicked() {
                        l.rotation += d;
                        changed = true;
                    }
                }
            });

            section(ui, "Thread");
            let current = l.opts.thread;
            let shown = match current {
                Some(t) if t < chart.len() => format!("{} {}", chart[t].code, chart[t].name),
                _ => "Match artwork color".into(),
            };
            ui.horizontal(|ui| {
                if let Some(t) = current.filter(|t| *t < chart.len()) {
                    swatch(ui, chart[t].rgb);
                }
                egui::ComboBox::from_id_salt("thread").width(230.0).selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "Match artwork color").clicked() {
                        l.opts.thread = None;
                        changed = true;
                    }
                    for (i, t) in chart.iter().enumerate() {
                        ui.horizontal(|ui| {
                            swatch(ui, t.rgb);
                            if ui.selectable_label(current == Some(i), format!("{} {}", t.code, t.name)).clicked() {
                                l.opts.thread = Some(i);
                                changed = true;
                            }
                        });
                    }
                });
            });

            section(ui, "Stitching");
            ui.horizontal(|ui| {
                ui.label("Type");
                egui::ComboBox::from_id_salt("stitch").selected_text(l.opts.stitch.label()).show_ui(ui, |ui| {
                    for c in StitchChoice::ALL {
                        if ui.selectable_value(&mut l.opts.stitch, c, c.label()).clicked() {
                            changed = true;
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Angle");
                let mut auto = l.opts.angle_deg.is_none();
                if ui.checkbox(&mut auto, "Auto").changed() {
                    l.opts.angle_deg = if auto { None } else { Some(auto_angle.unwrap_or(45.0)) };
                    changed = true;
                }
                match &mut l.opts.angle_deg {
                    Some(a) => {
                        changed |= ui.add(egui::DragValue::new(a).speed(1.0).range(0.0..=180.0).suffix("°").max_decimals(0)).changed();
                        for d in [0.0, 45.0, 90.0, 135.0] {
                            if ui.small_button(format!("{d:.0}")).clicked() {
                                *a = d;
                                changed = true;
                            }
                        }
                    }
                    None => {
                        if varies {
                            ui.label(RichText::new("varies per piece").weak());
                        } else if let Some(a) = auto_angle {
                            ui.label(RichText::new(format!("{a:.0}°")).weak());
                        }
                    }
                }
            });
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
            if ui.button("⬆ Earlier").on_hover_text("Sew sooner / move down in the stack").clicked() {
                action = Some("up");
            }
            if ui.button("⬇ Later").on_hover_text("Sew later / move up in the stack").clicked() {
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
                if ui.small_button("Unlink").on_hover_text("Move and size this layer on its own").clicked() {
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
                let rest: Vec<usize> = (0..self.layers.len()).filter(|&j| self.layers[j].link == k && k.is_some()).collect();
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

            section(ui, "Design size");
            if let Some(d) = &self.design {
                let b = d.bbox;
                let mut w = b.width() / 25.4;
                ui.horizontal(|ui| {
                    ui.label("Width");
                    let r = ui.add(egui::DragValue::new(&mut w).speed(0.01).range(0.3..=16.0).suffix(" in").max_decimals(2));
                    ui.label(RichText::new(format!("× {:.2} in", b.height() / 25.4)).weak());
                    if r.changed() && b.width() > 0.0 {
                        self.checkpoint(now);
                        scale_layers(&mut self.layers, b.center(), w * 25.4 / b.width());
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

            if let Some(d) = &self.design {
                section(ui, "Production");
                let s = &d.stats;
                egui::Grid::new("stats").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    kv(ui, "Size", &format!("{:.2} × {:.2} in  ({:.0} × {:.0} mm)", s.width_mm / 25.4, s.height_mm / 25.4, s.width_mm, s.height_mm));
                    kv(ui, "Stitches", &group(s.stitches));
                    kv(ui, "Stops", &d.stops.len().to_string());
                    kv(ui, "Trims", &s.trims.to_string());
                    kv(ui, "Est. run time", &format!("{:.1} min", s.run_minutes));
                    kv(ui, "Backing", d.recipe.backing);
                    kv(ui, "Topping", d.recipe.topping);
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
            ui.add_space(12.0);
        });
    }

    // ---------- bottom bar ----------

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let total = self.design.as_ref().map(|d| d.cmds.len()).unwrap_or(0);
            let enabled = total > 0;
            let play_label = if self.playing { "⏸ Pause" } else { "▶ Play sew-out" };
            if ui.add_enabled(enabled, egui::Button::new(play_label)).clicked() {
                if !self.playing && self.progress >= total {
                    self.progress = 0;
                }
                self.playing = !self.playing;
            }
            if ui.add_enabled(enabled, egui::Button::new("⏮")).on_hover_text("Back to start").clicked() {
                self.progress = 0;
                self.playing = false;
            }
            if ui.add_enabled(enabled, egui::Button::new("⏭")).on_hover_text("Show everything").clicked() {
                self.progress = total;
                self.playing = false;
            }
            ui.add_enabled(enabled, egui::Slider::new(&mut self.progress, 0..=total.max(1)).show_value(false));
            if let Some(d) = &self.design {
                let n = d.cmds[..self.progress.min(d.cmds.len())].iter().filter(|c| matches!(c, Cmd::Stitch(_))).count();
                ui.label(format!("{} / {}", group(n), group(d.stats.stitches)));
            }
            ui.label("Speed");
            ui.add(egui::Slider::new(&mut self.speed, 50.0..=8000.0).logarithmic(true).show_value(false));
            ui.separator();
            ui.selectable_value(&mut self.view, View::ThreeD, "3D").on_hover_text("Shaded thread view");
            ui.selectable_value(&mut self.view, View::Flat, "Flat").on_hover_text("Plain lines");
            ui.toggle_value(&mut self.show_jumps, "Jumps");
            if ui.button("Fit").on_hover_text("Fit design to view (F)").clicked() {
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
            painter.text(c - Vec2::new(0.0, 30.0), egui::Align2::CENTER_CENTER, "Drop an SVG here", egui::FontId::proportional(26.0), ink);
            painter.text(c + Vec2::new(0.0, 8.0), egui::Align2::CENTER_CENTER, "or use Open SVG…, Add text…, or Sample above", egui::FontId::proportional(15.0), Color32::from_rgb(120, 116, 108));
            if let Some(e) = &self.error {
                painter.text(c + Vec2::new(0.0, 44.0), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(14.0), Color32::from_rgb(200, 60, 50));
            }
            return;
        };

        let b = d.bbox;
        if self.fit {
            self.zoom = ((rect.width() - 100.0) / b.width() as f32).min((rect.height() - 100.0) / b.height() as f32).max(0.5);
            self.view_center = b.center();
            self.fit = false;
        }
        let zoom = self.zoom;
        let vc = self.view_center;
        let rc = rect.center();
        let to_screen = move |p: Pt| -> Pos2 { rc + Vec2::new((p.x - vc.x) as f32, (p.y - vc.y) as f32) * zoom };
        let to_mm = move |s: Pos2| -> Pt { Pt::new(vc.x + ((s.x - rc.x) / zoom) as f64, vc.y + ((s.y - rc.y) / zoom) as f64) };

        // ---- interaction: select, move, pan, zoom
        let hit_layer = |pos: Pos2, layers: &[Layer], zoom: f32| -> Option<u64> {
            let p = to_mm(pos);
            let tol = (4.0 / zoom) as f64;
            layers.iter().rev().find(|l| l.visible && l.hit(p, tol)).map(|l| l.id)
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
                None => self.view_center = self.view_center.sub(Pt::new((dlt.x / zoom) as f64, (dlt.y / zoom) as f64)),
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
            let (scroll, pinch, cursor) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let factor = pinch * (scroll / 400.0).exp();
            if (factor - 1.0).abs() > 1e-4 {
                if let Some(cur) = cursor {
                    let before = to_mm(cur);
                    self.zoom = (self.zoom * factor).clamp(0.3, 200.0);
                    // keep the point under the cursor fixed
                    let z = self.zoom;
                    self.view_center = Pt::new(before.x - ((cur.x - rc.x) / z) as f64, before.y - ((cur.y - rc.y) / z) as f64);
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
                painter.line_segment([Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())], Stroke::new(1.0_f32, grid));
                x += 10.0;
            }
            let mut y = (tl.y / 10.0).floor() * 10.0;
            while y <= br.y {
                let sy = to_screen(Pt::new(0.0, y)).y;
                painter.line_segment([Pos2::new(rect.left(), sy), Pos2::new(rect.right(), sy)], Stroke::new(1.0_f32, grid));
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
            painter.extend(stitch_shapes(d, a, z.min(end), rgb, &to, zoom, view, show_jumps));
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
                let r = Rect::from_min_max(to_screen(lb.min.add(shift)), to_screen(lb.max.add(shift))).expand(6.0);
                let accent = Color32::from_rgb(30, 110, 230);
                for seg in EShape::dashed_line(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()], Stroke::new(1.5_f32, accent), 6.0, 4.0) {
                    painter.add(seg);
                }
                for c in [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()] {
                    painter.rect_filled(Rect::from_center_size(c, Vec2::splat(7.0)), 1.0, accent);
                }
                let pieces: Vec<&stitchforge::design::ObjectInfo> = d.objects.iter().filter(|o| o.layer == li && has_angle(o.stitch_type)).collect();
                let same = pieces.iter().all(|o| (o.angle_deg - pieces[0].angle_deg).abs() < 0.5);
                if let Some(o) = pieces.first().filter(|_| same) {
                    {
                        let a = o.angle_deg.to_radians() as f32;
                        let c = r.center();
                        let len = (r.width().min(r.height()) * 0.35).clamp(20.0, 70.0);
                        let dir = Vec2::new(a.cos(), -a.sin()) * len;
                        let col = Color32::from_rgb(255, 255, 255);
                        painter.line_segment([c - dir, c + dir], Stroke::new(5.0_f32, Color32::from_black_alpha(120)));
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
            Pos2::new(to_screen(Pt::new(b.center().x, b.max.y)).x, to_screen(b.max).y + 12.0),
            egui::Align2::CENTER_TOP,
            format!("{:.2}\" × {:.2}\"  ·  {}", b.width() / 25.4, b.height() / 25.4, d.profile.label()),
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
        egui::Window::new("Add text").open(&mut open).resizable(false).collapsible(false).default_width(360.0).show(ctx, |ui| {
            ui.label("Words (Enter for a new line)");
            ui.add(egui::TextEdit::multiline(&mut self.text).desired_rows(2).desired_width(330.0));
            ui.horizontal(|ui| {
                ui.label("Letter height");
                ui.add(egui::DragValue::new(&mut self.text_height_in).speed(0.01).range(0.15..=8.0).suffix(" in").max_decimals(2));
                ui.label("Color");
                ui.color_edit_button_srgb(&mut self.text_color);
            });
            ui.label("Font (any installed font)");
            ui.add(egui::TextEdit::singleline(&mut self.font_filter).hint_text("Search fonts…").desired_width(330.0));
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let q = self.font_filter.to_lowercase();
                for f in self.fonts.as_ref().unwrap().iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q)).take(400) {
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
        // drag and drop: new design if empty, otherwise add as layers
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for p in dropped {
            let add = !self.layers.is_empty();
            self.open_svg_path(p, add);
        }
        // shortcuts (ignored while typing in a field)
        let typing = ctx.wants_keyboard_input();
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        if cmd && pressed(egui::Key::O) {
            self.pick_svg(false);
        }
        if !typing {
            if cmd && pressed(egui::Key::Z) {
                if shift { self.do_redo() } else { self.do_undo() }
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
                if pressed(egui::Key::ArrowLeft) { nudge.x -= step; }
                if pressed(egui::Key::ArrowRight) { nudge.x += step; }
                if pressed(egui::Key::ArrowUp) { nudge.y -= step; }
                if pressed(egui::Key::ArrowDown) { nudge.y += step; }
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
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("bottom")
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ctx, |ui| self.bottom_bar(ui));
        egui::SidePanel::left("layers").resizable(true).default_width(320.0).show(ctx, |ui| self.layers_panel(ui));
        egui::SidePanel::right("props").resizable(true).default_width(350.0).show(ctx, |ui| self.right_panel(ui, now));
        if let Some(e) = self.error.clone() {
            if self.design.is_some() && !self.show_text {
                egui::TopBottomPanel::top("err").show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(210, 70, 60)));
                        if ui.small_button("Dismiss").clicked() {
                            self.error = None;
                        }
                    });
                });
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| self.canvas(ui, now));
        self.text_window(ctx);
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
                        shapes.extend(EShape::dashed_line(&[l, q], Stroke::new(1.0_f32, jump_col), 4.0, 3.0));
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
            let mut mesh = egui::Mesh::default();
            // brightness across the thread (edge, middle, edge) and along it
            // (needle hole, middle, needle hole): a simple overlay gradient.
            let across = [0.62f32, 1.0, 0.5];
            let along = [0.62f32, 1.18, 0.62];
            for r in runs.iter().filter(|r| r.len() >= 2) {
                for s in r.windows(2) {
                    let (a, b) = (s[0], s[1]);
                    let v = b - a;
                    let len = v.length();
                    if len < 0.4 {
                        continue;
                    }
                    let dir = v / len;
                    let n = Vec2::new(-dir.y, dir.x) * (w * 0.62);
                    let base = mesh.vertices.len() as u32;
                    for (ti, t) in [0.0f32, 0.5, 1.0].iter().enumerate() {
                        let c = a + v * *t;
                        for (si, sgn) in [-1.0f32, 0.0, 1.0].iter().enumerate() {
                            let k = across[si] * along[ti];
                            mesh.colored_vertex(c + n * *sgn, scale_col(col, k));
                        }
                    }
                    for ti in 0..2u32 {
                        for si in 0..2u32 {
                            let i0 = base + ti * 3 + si;
                            mesh.add_triangle(i0, i0 + 1, i0 + 3);
                            mesh.add_triangle(i0 + 1, i0 + 4, i0 + 3);
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

fn has_angle(stitch_type: &str) -> bool {
    matches!(stitch_type, "Tatami fill" | "Satin")
}

/// A small expand/collapse triangle.
fn disclosure(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::click());
    let c = rect.center();
    let col = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let pts = if open {
        vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0), c + Vec2::new(0.0, 3.0)]
    } else {
        vec![c + Vec2::new(-2.0, -4.0), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.0, 4.0)]
    };
    ui.painter().add(EShape::convex_polygon(pts, col, Stroke::NONE));
    resp
}

fn scale_col(c: Color32, k: f32) -> Color32 {
    // Lighten toward white when k > 1 so dark threads still get a highlight.
    let f = |v: u8| -> u8 {
        let v = v as f32;
        if k <= 1.0 { (v * k) as u8 } else { (v + (255.0 - v) * (k - 1.0) * 1.6).min(255.0) as u8 }
    };
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn scale_layers(layers: &mut [Layer], c: Pt, k: f64) {
    for l in layers {
        let center = l.center();
        let new_center = c.add(center.sub(c).mul(k));
        l.offset = new_center.sub(l.origin);
        l.scale *= k;
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
    p.rect_stroke(r.rect, 3.0, Stroke::new(1.0_f32, Color32::from_black_alpha(70)), egui::StrokeKind::Inside);
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
    let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}
