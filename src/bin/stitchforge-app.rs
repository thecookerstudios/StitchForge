//! StitchForge desktop app (v0.3 review build).
//!
//! Layers you can select, move, rotate, resize, recolor and re-angle on the
//! canvas; per-layer stop toggles; flat and 3D stitch views; sew-out playback.

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Shape as EShape, Stroke, Vec2};
use std::collections::HashSet;
use std::path::PathBuf;
use stitchforge::design::{BuildOptions, Cmd, Design};
use stitchforge::drops::{self as drops_mod, Case, DropSetup, Join, Slot};
use stitchforge::formats::{self, Format, MachineFile};
use stitchforge::geom::{BBox, Node, Pt};
use stitchforge::import::FillPattern;
use stitchforge::vector::{self, BoolOp};
use stitchforge::geom::{FillRule, VPath};
use stitchforge::project::Embroidery;
use stitchforge::stitch::Needle;
use stitchforge::trace;
use stitchforge::import::{self, Artwork, Shape, ShapeKind, ShapeOpts, StitchChoice};
use stitchforge::import::{Cap, Corner, Underlay};
use stitchforge::project::{Layer, Project, TextSpec};
use stitchforge::settings::{self, Limits, Profile, Recipe};
use stitchforge::sheet::JobInfo;
use stitchforge::{export, sampler, thread};

#[path = "../app_ui/theme.rs"]
mod theme;
use theme::ThemeKind;
use eframe::egui::CornerRadius;

/// Screen points per mm at 100% (96 per inch).
const MM_PX: f32 = 96.0 / 25.4;
/// Ruler thickness on the canvas.
const RULER: f32 = 18.0;
/// Flat hoops: name, width and height in mm (largest last).
const HOOPS: &[(&str, f64, f64)] = &[
    ("4 × 4 in", 100.0, 100.0),
    ("5 × 7 in", 130.0, 180.0),
    ("6 × 10 in", 150.0, 250.0),
    ("8 × 8 in", 200.0, 200.0),
    ("12 × 8 in", 300.0, 200.0),
    ("15 × 15 in", 380.0, 380.0),
];

/// A thread library as shown in the Threads panel.
struct LoadedLib {
    /// File name in the library folder, a full path, "generic", or
    /// "design:…" for the threads saved inside an opened design.
    key: String,
    name: String,
    note: String,
    threads: Vec<thread::Thread>,
    on: bool,
}

impl LoadedLib {
    fn from_library(key: String, lib: thread::Library, on: bool) -> Self {
        let note = match (lib.manufacturer.is_empty(), lib.licence.is_empty()) {
            (true, true) => format!("{} threads.", lib.threads.len()),
            (false, true) => format!("{} · {} threads", lib.manufacturer, lib.threads.len()),
            (true, false) => format!("{} threads · {}", lib.threads.len(), lib.licence),
            (false, false) => format!("{} · {} threads · {}", lib.manufacturer, lib.threads.len(), lib.licence),
        };
        LoadedLib { key, name: lib.name, note, threads: lib.threads, on }
    }
}

/// A workspace: which panels lead, and Simple or full controls.
#[derive(PartialEq, Clone, Copy, Debug)]
enum Workspace {
    Design,
    Lettering,
    NameDrops,
    Production,
    Simple,
}

impl Workspace {
    const ALL: [Workspace; 5] = [Workspace::Design, Workspace::Lettering, Workspace::NameDrops, Workspace::Production, Workspace::Simple];
    fn label(&self) -> &'static str {
        match self {
            Workspace::Design => "Design",
            Workspace::Lettering => "Lettering",
            Workspace::NameDrops => "Name drops",
            Workspace::Production => "Production",
            Workspace::Simple => "Simple",
        }
    }
    fn hint(&self) -> &'static str {
        match self {
            Workspace::Design => "Every tool and stitch setting",
            Workspace::Lettering => "Text tool ready, text settings first",
            Workspace::NameDrops => "Logo on every head, one name per head, from a sheet",
            Workspace::Production => "Stitch counts, run time, checks and the job sheet",
            Workspace::Simple => "Only the essentials: fewer settings on screen",
        }
    }
    fn key(&self) -> &'static str {
        match self {
            Workspace::Design => "design",
            Workspace::Lettering => "lettering",
            Workspace::NameDrops => "drops",
            Workspace::Production => "production",
            Workspace::Simple => "simple",
        }
    }
    fn from_key(s: &str) -> Option<Workspace> {
        Workspace::ALL.into_iter().find(|w| w.key() == s.trim())
    }
}

/// Panels in the right-hand dock.
#[derive(PartialEq, Clone, Copy, Debug)]
enum Dock {
    Properties,
    Layers,
    Threads,
    Production,
}

/// A shortcut as the platform writes it: ⌘S on a Mac, Ctrl+S elsewhere.
fn kb(k: &str) -> String {
    let shift = k.starts_with('⇧');
    let k = k.trim_start_matches('⇧');
    if cfg!(target_os = "macos") {
        format!("{}⌘{k}", if shift { "⇧" } else { "" })
    } else {
        format!("Ctrl+{}{k}", if shift { "Shift+" } else { "" })
    }
}

/// Line icons for the dock strip.
fn dock_icon(p: &egui::Painter, r: Rect, d: Dock, ink: Color32) {
    let s = Stroke::new(1.5_f32, ink);
    let (l, t, w, h) = (r.left(), r.top(), r.width(), r.height());
    let at = |x: f32, y: f32| Pos2::new(l + x * w, t + y * h);
    match d {
        Dock::Properties => {
            // three sliders
            for (i, k) in [0.25_f32, 0.7, 0.45].iter().enumerate() {
                let y = 0.2 + i as f32 * 0.3;
                p.line_segment([at(0.0, y), at(1.0, y)], s);
                p.circle_filled(at(*k, y), 2.6, ink);
            }
        }
        Dock::Layers => {
            for (i, y) in [0.25_f32, 0.5, 0.75].iter().enumerate() {
                let pts = vec![at(0.5, y - 0.2), at(1.0, *y), at(0.5, y + 0.2), at(0.0, *y)];
                if i == 0 {
                    p.add(EShape::convex_polygon(pts, Color32::TRANSPARENT, s));
                } else {
                    p.line_segment([at(0.0, *y), at(0.5, y + 0.2)], s);
                    p.line_segment([at(0.5, y + 0.2), at(1.0, *y)], s);
                }
            }
        }
        Dock::Threads => {
            // a thread spool
            p.line_segment([at(0.2, 0.08), at(0.8, 0.08)], Stroke::new(2.0_f32, ink));
            p.line_segment([at(0.2, 0.92), at(0.8, 0.92)], Stroke::new(2.0_f32, ink));
            p.rect_stroke(Rect::from_min_max(at(0.3, 0.14), at(0.7, 0.86)), 1.0, s, egui::StrokeKind::Middle);
            for y in [0.32_f32, 0.5, 0.68] {
                p.line_segment([at(0.3, y), at(0.7, y + 0.08)], Stroke::new(1.0_f32, ink));
            }
        }
        Dock::Production => {
            // a clipboard
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.12), at(0.88, 1.0)), 2.0, s, egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(0.35, 0.02), at(0.65, 0.2)), 1.0, ink);
            for y in [0.42_f32, 0.6, 0.78] {
                p.line_segment([at(0.28, y), at(0.72, y)], Stroke::new(1.0_f32, ink));
            }
        }
    }
}

const SAMPLE_SVG: &str = include_str!("../../examples/left-chest-logo.svg");

#[cfg(not(target_arch = "wasm32"))]
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
enum Tab {
    Design,
    Drops,
}

/// Which side of a design object is being edited on the canvas.
#[derive(PartialEq, Clone, Copy, Debug)]
enum Edit {
    None,
    /// Needle points of a layer (its embroidery representation).
    Stitches(u64),
    /// Anchors and handles of a layer's paths (its vector representation).
    Shape(u64),
}

/// Canvas tools (left strip).
#[derive(PartialEq, Clone, Copy, Debug)]
enum Tool {
    Select,
    Nodes,
    Stitches,
    Pen,
    Rect,
    Ellipse,
    Polygon,
    Line,
    Text,
    Measure,
    Knife,
    Eraser,
    Guides,
}

impl Tool {
    const ALL: [Tool; 13] = [
        Tool::Select,
        Tool::Nodes,
        Tool::Stitches,
        Tool::Pen,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Polygon,
        Tool::Line,
        Tool::Text,
        Tool::Measure,
        Tool::Knife,
        Tool::Eraser,
        Tool::Guides,
    ];
    fn hint(&self) -> &'static str {
        match self {
            Tool::Select => "Select and move (V)\nShift-click to select several",
            Tool::Nodes => "Edit shape points (N)\nDrag anchors and handles; double-click the outline to add one",
            Tool::Stitches => "Edit stitch points (M)\nMove, add or delete needle points by hand",
            Tool::Pen => "Pen (P)\nClick for corners, drag for curves. Click the first point to close, double-click or Enter to finish",
            Tool::Rect => "Rectangle (R)\nDrag; Shift for a square. Corner radius in the panel",
            Tool::Ellipse => "Ellipse (E)\nDrag; Shift for a circle",
            Tool::Polygon => "Polygon and star (G)\nDrag from the centre; Shift snaps the angle",
            Tool::Line => "Line (L)\nDrag; Shift snaps to 15°",
            Tool::Text => "Text (T)\nClick where the words go",
            Tool::Measure => "Measure (D)\nDrag across a column or gap to see its size and which thread weights it suits",
            Tool::Knife => "Knife (K)\nDrag a line across a shape to cut it into separate pieces; Shift snaps to 15°",
            Tool::Eraser => "Eraser (X)\nDraw around the part to remove: it is cut out of the shape and the stitches follow",
            Tool::Guides => "Direction lines (A)\nDrag across a fill to show which way the stitches run there; add more lines and the rows turn smoothly between them. Click a line and press Delete to remove it",
        }
    }
    fn key(&self) -> egui::Key {
        match self {
            Tool::Select => egui::Key::V,
            Tool::Nodes => egui::Key::N,
            Tool::Stitches => egui::Key::M,
            Tool::Pen => egui::Key::P,
            Tool::Rect => egui::Key::R,
            Tool::Ellipse => egui::Key::E,
            Tool::Polygon => egui::Key::G,
            Tool::Line => egui::Key::L,
            Tool::Text => egui::Key::T,
            Tool::Measure => egui::Key::D,
            Tool::Knife => egui::Key::K,
            Tool::Eraser => egui::Key::X,
            Tool::Guides => egui::Key::A,
        }
    }
    /// Tools that make new shapes on the canvas.
    fn draws(&self) -> bool {
        matches!(self, Tool::Pen | Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Line | Tool::Text)
    }
    fn name(&self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Nodes => "Edit shape points",
            Tool::Stitches => "Edit stitch points",
            Tool::Pen => "Pen",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Polygon => "Polygon and star",
            Tool::Line => "Line",
            Tool::Text => "Text",
            Tool::Measure => "Measure",
            Tool::Knife => "Knife",
            Tool::Eraser => "Eraser",
            Tool::Guides => "Direction lines",
        }
    }
    /// Keys in the vector-editor style preset.
    fn ai_key(&self) -> egui::Key {
        match self {
            Tool::Select => egui::Key::V,
            Tool::Nodes => egui::Key::A,
            Tool::Stitches => egui::Key::S,
            Tool::Pen => egui::Key::P,
            Tool::Rect => egui::Key::M,
            Tool::Ellipse => egui::Key::L,
            Tool::Polygon => egui::Key::G,
            Tool::Line => egui::Key::Backslash,
            Tool::Text => egui::Key::T,
            Tool::Measure => egui::Key::I,
            Tool::Knife => egui::Key::K,
            Tool::Eraser => egui::Key::E,
            Tool::Guides => egui::Key::J,
        }
    }
    fn key_for(&self, ai: bool) -> egui::Key {
        if ai { self.ai_key() } else { self.key() }
    }
    fn key_label(&self, ai: bool) -> &'static str {
        match self.key_for(ai) {
            egui::Key::Backslash => "\\",
            k => k.symbol_or_name(),
        }
    }
    /// Tooltip with the key of the preset in use.
    fn tip(&self, ai: bool) -> String {
        let rest = self.hint().split_once('\n').map(|x| x.1).unwrap_or("");
        format!("{} ({})\n{}", self.name(), self.key_label(ai), rest)
    }
}

/// Look of the next shape drawn.
#[derive(Clone)]
struct DrawStyle {
    fill_on: bool,
    fill: [u8; 3],
    stroke_on: bool,
    stroke: [u8; 3],
    stroke_w: f64,
    corner: f64,
    sides: usize,
    star: bool,
    inner: f64,
}

/// An anchor (part 0) or one of its handles (1 = in, 2 = out).
#[derive(PartialEq, Clone, Copy, Debug)]
struct NodeRef {
    shape: usize,
    path: usize,
    node: usize,
    part: u8,
}

enum EditDrag {
    /// Moving needle points: where the drag began, and their starting spots.
    Stitches(Pt, Vec<(usize, Pt)>),
    Node(NodeRef, Pt, Node),
    Box(Pos2),
    /// Free-hand selection outline (screen points).
    Lasso(Vec<Pos2>),
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    /// The artwork itself (filled vector shapes), for drawing and text.
    Vector,
    /// Stitches as thread on fabric.
    ThreeD,
    /// Stitches as plain lines.
    Flat,
}

impl View {
    const ALL: [View; 3] = [View::Vector, View::ThreeD, View::Flat];
    fn label(&self) -> &'static str {
        match self {
            View::Vector => "Vector",
            View::ThreeD => "Stitches 3D",
            View::Flat => "Flat lines",
        }
    }
    fn key(&self) -> egui::Key {
        match self {
            View::Vector => egui::Key::Num1,
            View::ThreeD => egui::Key::Num2,
            View::Flat => egui::Key::Num3,
        }
    }
}

/// The vector view's picture of the art, re-made when the art or zoom changes.
struct VecTex {
    key: (u64, i32),
    tex: egui::TextureHandle,
    origin: Pt,
    size: Pt,
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
    /// Manufacturer and licence notes of the loaded library.
    chart_note: String,
    /// Library files found in the library folder.
    libraries: Vec<PathBuf>,
    /// Every thread library known this session; the ticked ones make up `chart`.
    libs: Vec<LoadedLib>,
    /// Placeholder rope texture for the 3D view, made once on first use.
    rope_tex: Option<egui::TextureHandle>,

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
    /// Highlight the underlay (top stitches faded).
    show_underlay: bool,
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
    export_fmts: [bool; 9],

    // name drops
    tab: Tab,
    drops: DropSetup,
    drop_view: usize,
    drop_sel: usize,
    drop_info: Option<(usize, Vec<Slot>)>,
    /// How each name fits the width limit (by run position).
    drop_fit: Vec<(usize, drops_mod::FitInfo)>,
    quiet_regen: bool,

    // editing a design object's stitches or shape on the canvas
    edit: Edit,
    // drawing
    tool: Tool,
    multi: std::collections::BTreeSet<u64>,
    /// A part (one stitch object) picked in the layers list: (layer id,
    /// object sequence). Highlighted on the canvas.
    focus_part: Option<(u64, usize)>,
    style: DrawStyle,
    pen: Vec<Node>,
    pen_dragging: bool,
    /// Layer whose stitch angle is being dragged from its angle handle.
    angle_drag: Option<usize>,
    shape_drag: Option<(Pt, Pt)>,
    snap: bool,
    text_at: Option<Pt>,
    show_border: bool,
    show_weights: bool,
    measure: Option<(Pt, Pt)>,
    /// Knife line being dragged (design mm).
    cut: Option<(Pt, Pt)>,
    /// Eraser outline being drawn (design mm).
    lasso: Vec<Pt>,
    /// Direction line being dragged (design mm), and the picked line.
    guide_drag: Option<(Pt, Pt)>,
    guide_sel: Option<usize>,
    trace_dlg: Option<TraceDlg>,
    /// Bumped whenever the design is rebuilt (vector view cache key).
    layers_rev: u64,
    vec_tex: Option<VecTex>,
    /// Appliqué pieces drawn under the stitches: (cmd index they go on at, picture).
    piece_tex: Vec<(usize, VecTex)>,
    /// Notes about the opened file (live text in a PDF, garment removed…).
    file_notes: Vec<String>,
    border_dist: f64,
    border_round: bool,
    border_outline: bool,
    border_width: f64,
    st_sel: std::collections::BTreeSet<usize>,
    node_sel: Option<NodeRef>,
    edit_drag: Option<EditDrag>,

    // interface shell
    theme: ThemeKind,
    workspace: Workspace,
    dock: Dock,
    dock_open: bool,
    /// Vector-editor style tool keys instead of StitchForge's own.
    ai_keys: bool,
    show_rulers: bool,
    show_hoop: bool,
    /// Chosen flat hoop (index into HOOPS); None picks one to fit.
    hoop: Option<usize>,
    show_keys: bool,
    show_about: bool,
    /// Pointer position on the canvas, in mm.
    cursor_mm: Option<Pt>,
}

use stitchforge::import::font_options;

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        cc.egui_ctx.set_style(style);
        let prefs = theme::load_prefs();
        theme::apply(&cc.egui_ctx, prefs.theme);
        let workspace = match Workspace::from_key(&prefs.workspace).unwrap_or(Workspace::Design) {
            Workspace::NameDrops => Workspace::Design,
            w => w,
        };
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
            chart_note: "Built-in generic 40 wt palette, made for StitchForge.".into(),
            libraries: thread::scan_libraries(&thread::library_dir()),
            libs: Vec::new(),
            rope_tex: None,
            show_text: false,
            text: "Forge Co".into(),
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
            show_underlay: false,
            selected: None,
            moving: None,
            expanded: HashSet::new(),
            progress: 0,
            playing: false,
            speed: 600.0,
            play_acc: 0.0,
            pending_progress: None,
            pending_zoom: None,
            status: "Open art, a PDF or a picture (or drop one on the window), or add text.".into(),
            last_dir: None,
            project_path: None,
            modified: false,
            clean_after_regen: false,
            allow_close: false,
            confirm: None,
            title: String::new(),
            machine: None,
            show_export: false,
            export_fmts: [true, false, false, false, false, false, false, false, false],
            tab: Tab::Design,
            drops: DropSetup::default(),
            drop_view: 0,
            drop_sel: 0,
            drop_info: None,
            drop_fit: Vec::new(),
            quiet_regen: false,
            edit: Edit::None,
            tool: Tool::Select,
            multi: Default::default(),
            focus_part: None,
            style: DrawStyle {
                fill_on: true,
                fill: [28, 43, 74],
                stroke_on: false,
                stroke: [224, 165, 38],
                stroke_w: 2.0,
                corner: 0.0,
                sides: 5,
                star: true,
                inner: 0.45,
            },
            pen: Vec::new(),
            pen_dragging: false,
            angle_drag: None,
            shape_drag: None,
            snap: false,
            text_at: None,
            show_border: false,
            show_weights: false,
            measure: None,
            cut: None,
            lasso: Vec::new(),
            guide_drag: None,
            guide_sel: None,
            trace_dlg: None,
            layers_rev: 0,
            vec_tex: None,
            piece_tex: Vec::new(),
            file_notes: Vec::new(),
            border_dist: 1.5,
            border_round: true,
            border_outline: true,
            border_width: 2.5,
            st_sel: Default::default(),
            node_sel: None,
            edit_drag: None,
            theme: prefs.theme,
            workspace,
            dock: if workspace == Workspace::Production { Dock::Production } else { Dock::Properties },
            dock_open: true,
            ai_keys: prefs.keys == "illustrator",
            show_rulers: true,
            show_hoop: true,
            hoop: None,
            show_keys: false,
            show_about: false,
            cursor_mm: None,
        }
        .with_libraries(&prefs.libs)
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
                    self.recipe = {
                        let (w, cs) = (self.recipe.weight, self.recipe.cap_size);
                        let mut r = settings::recipe("cap").unwrap();
                        r.weight = w;
                        r.cap_size = cs;
                        r
                    };
                }
                "--pro" => self.workspace = Workspace::Design,
                "--simple" => self.workspace = Workspace::Simple,
                "--theme" => {
                    if let Some(k) = args.get(i + 1).and_then(|v| ThemeKind::from_key(v)) {
                        self.theme = k;
                    }
                    i += 1;
                }
                "--dock" => {
                    self.dock = match args.get(i + 1).map(|s| s.as_str()) {
                        Some("layers") => Dock::Layers,
                        Some("threads") => Dock::Threads,
                        Some("production") => Dock::Production,
                        _ => Dock::Properties,
                    };
                    i += 1;
                }
                "--garment" => {
                    if let Some(rgb) = args.get(i + 1).and_then(|v| parse_hex(v)) {
                        self.job.garment_rgb = Some(rgb);
                    }
                    i += 1;
                }
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
                "--focus" => {
                    if let (Some(n), Some(id)) = (num(i + 1), self.selected) {
                        self.focus_part = Some((id, n as usize));
                    }
                    i += 1;
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
                "--round" => {
                    if let Some(l) = self.sel_mut() {
                        l.opts.corner = Corner::Round;
                        l.opts.column_width = Some(3.0);
                        l.opts.no_underlay = true;
                    }
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
                "--add-text" => {
                    if let Some(t) = args.get(i + 1) {
                        self.text = t.clone();
                        self.font = "DejaVu Sans".into();
                        self.text_height_in = 0.35;
                        self.text_color = [245, 245, 245];
                        self.regenerate();
                        self.add_text_layer();
                    }
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
                "--drops-data" => {
                    if let Some(p) = args.get(i + 1) {
                        self.set_tab(Tab::Drops);
                        self.load_names_path(PathBuf::from(p));
                    }
                    i += 1;
                }
                "--drop" => {
                    if let Some(n) = num(i + 1) {
                        self.drop_view = (n as usize).saturating_sub(1);
                    }
                    i += 1;
                }
                "--drop-sel" => {
                    if let Some(n) = num(i + 1) {
                        self.drop_sel = n as usize;
                    }
                    i += 1;
                }
                "--save-to" => {
                    // test hook: save without a dialog
                    if let Some(p) = args.get(i + 1) {
                        self.regenerate();
                        self.save_to(PathBuf::from(p));
                    }
                    i += 1;
                }
                "--export-dialog" => self.show_export = true,
                // test hooks for editing design objects
                "--fill" => {
                    let f = match args.get(i + 1).map(|s| s.as_str()) {
                        Some("contour") => FillPattern::Contour,
                        Some("spiral") => FillPattern::Spiral,
                        Some("radial") => FillPattern::Radial,
                        _ => FillPattern::Tatami,
                    };
                    if let Some(l) = self.sel_mut() {
                        l.opts.stitch = StitchChoice::Fill;
                        l.opts.fill = f;
                    }
                    i += 1;
                }
                "--density" => {
                    if let (Some(v), Some(l)) = (num(i + 1), self.sel_mut()) {
                        l.opts.density = Some(v);
                    }
                    i += 1;
                }
                "--edit-stitches" => {
                    if let Some(li) = self.sel_idx() {
                        self.regenerate();
                        self.dirty = false;
                        self.start_stitch_edit(li);
                    }
                }
                "--sel-points" => {
                    if let (Some(a), Some(b)) = (num(i + 1), num(i + 2)) {
                        self.st_sel = (a as usize..b as usize).collect();
                    }
                    i += 2;
                }
                "--nudge-points" => {
                    if let (Some(dx), Some(dy), Some(li)) = (num(i + 1), num(i + 2), self.sel_idx()) {
                        let l = &mut self.layers[li];
                        for &k in &self.st_sel {
                            if let Some(n) = l.embroidery.needles.get(k).copied() {
                                l.embroidery.needles[k].p = l.inv(l.tf(n.p).add(Pt::new(dx, dy)));
                            }
                        }
                        self.touch_stitches(li);
                    }
                    i += 2;
                }
                "--edit-shape" => {
                    if let Some(li) = self.sel_idx() {
                        self.start_shape_edit(li);
                    }
                }
                "--move-node" => {
                    // --move-node NODE dx dy (first shape, first path)
                    if let (Some(k), Some(dx), Some(dy), Some(li)) = (num(i + 1), num(i + 2), num(i + 3), self.sel_idx()) {
                        let l = &mut self.layers[li];
                        if let Some(n) = l.shapes.get_mut(0).and_then(|s| s.curves.get_mut(0)).and_then(|c| c.nodes.get_mut(k as usize)) {
                            let d = Pt::new(dx, dy);
                            n.p = n.p.add(d);
                            n.cin = n.cin.add(d);
                            n.cout = n.cout.add(d);
                        }
                        if let Some(s) = l.shapes.get_mut(0) {
                            s.reflatten();
                        }
                        self.node_sel = Some(NodeRef { shape: 0, path: 0, node: k as usize, part: 0 });
                        self.dirty = true;
                    }
                    i += 3;
                }
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
        self.st_sel.clear();
        self.node_sel = None;
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

    /// Re-render the vector view's picture when the art or zoom changed.
    fn refresh_vector_texture(&mut self, ctx: &egui::Context) {
        let zb = (self.zoom.max(0.05).ln() / 1.25f32.ln()).round() as i32;
        let key = (self.layers_rev, zb);
        if self.vec_tex.as_ref().map(|v| v.key == key).unwrap_or(false) {
            return;
        }
        let vis: Vec<Layer> = self.layers.iter().filter(|l| l.visible).cloned().collect();
        let mut b = BBox::empty();
        for l in &vis {
            let lb = l.bbox();
            if lb.is_valid() {
                b.add(lb.min);
                b.add(lb.max);
            }
        }
        if !b.is_valid() {
            self.vec_tex = None;
            return;
        }
        let pad = 2.0; // art_svg's margin
        let (w, h) = (b.width() + 2.0 * pad, b.height() + 2.0 * pad);
        let ppp = ctx.pixels_per_point();
        let max_px = ((w.max(h) as f32) * self.zoom * ppp).clamp(64.0, 4096.0) as u32;
        let svg = stitchforge::project::art_svg(&vis, &self.chart);
        if let Some((pw, ph, px)) = render_svg(&svg, max_px) {
            let tex = ctx.load_texture("vector-view", egui::ColorImage::from_rgba_unmultiplied([pw, ph], &px), egui::TextureOptions::LINEAR);
            self.vec_tex = Some(VecTex {
                key,
                tex,
                origin: Pt::new(b.min.x - pad, b.min.y - pad),
                size: Pt::new(w, h),
            });
        }
    }

    /// Pictures of the appliqué pieces (twill look), re-made when the design
    /// or zoom changes.
    /// The rope thread texture (from photos), made on first use and repeated
    /// along the strand so it can be shrunk to thread size.
    fn rope_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.rope_tex.is_none() {
            let n = stitchforge::texture::TILE;
            let px = stitchforge::texture::rope_tile();
            let img = egui::ColorImage::from_rgba_unmultiplied([n, n], &px);
            let opts = egui::TextureOptions {
                wrap_mode: egui::TextureWrapMode::Repeat,
                mipmap_mode: Some(egui::TextureFilter::Linear),
                ..egui::TextureOptions::LINEAR
            };
            self.rope_tex = Some(ctx.load_texture("rope", img, opts));
        }
        self.rope_tex.as_ref().map(|t| t.id())
    }

    fn refresh_piece_textures(&mut self, ctx: &egui::Context) {
        use std::hash::{Hash, Hasher};
        let Some(d) = &self.design else {
            self.piece_tex.clear();
            return;
        };
        if d.pieces.is_empty() {
            self.piece_tex.clear();
            return;
        }
        let zb = (self.zoom.max(0.05).ln() / 1.25f32.ln()).round() as i32;
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        for p in &d.pieces {
            p.from_cmd.hash(&mut hs);
            p.rgb.hash(&mut hs);
            for q in p.rings.iter().flatten() {
                ((q.x * 100.0) as i64, (q.y * 100.0) as i64).hash(&mut hs);
            }
        }
        let key = (hs.finish(), zb);
        if self.piece_tex.first().map(|(_, v)| v.key == (key.0, key.1)).unwrap_or(false) {
            return;
        }
        let ppp = ctx.pixels_per_point();
        let mut out = Vec::new();
        for (i, p) in d.pieces.iter().enumerate() {
            let (svg, origin, size) = stitchforge::applique::piece_svg(p);
            let max_px = ((size.x.max(size.y) as f32) * self.zoom * ppp).clamp(32.0, 2048.0) as u32;
            if let Some((pw, ph, px)) = render_svg(&svg, max_px) {
                let tex = ctx.load_texture(
                    format!("piece-{i}"),
                    egui::ColorImage::from_rgba_unmultiplied([pw, ph], &px),
                    egui::TextureOptions::LINEAR,
                );
                out.push((p.from_cmd, VecTex { key: (key.0, key.1), tex, origin, size }));
            }
        }
        out.sort_by_key(|(c, _)| *c);
        self.piece_tex = out;
    }

    /// PDF/AI files and pictures: vector PDFs open like SVG; pictures (and
    /// PDFs that only hold a picture) go to the trace window.
    fn open_art_file(&mut self, path: PathBuf, add: bool) {
        self.last_dir = path.parent().map(|p| p.to_path_buf());
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("design".into());
        let lower = path.to_string_lossy().to_lowercase();
        let bytes = match stitchforge::vfs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(format!("Couldn't read {}: {e}", path.display()));
                return;
            }
        };
        if lower.ends_with(".pdf") || lower.ends_with(".ai") || bytes.starts_with(b"%PDF") {
            match stitchforge::pdf::read(&bytes) {
                Ok(art) => {
                    if let Some(svg) = art.svg {
                        if !add {
                            self.machine = None;
                            self.project_path = None;
                        }
                        self.open_svg_data(&svg, &name, add);
                        if !add {
                            self.clean_after_regen = true;
                        }
                        self.status = format!("Opened the vector art in {name}");
                        self.file_notes = art.notes;
                    } else if let Some(pic) = art.picture {
                        self.start_trace(pic, name, add, art.notes);
                    } else {
                        self.error = Some("No vector art or picture found in that PDF".into());
                    }
                }
                Err(e) => self.error = Some(e),
            }
            return;
        }
        match trace::Raster::decode(&bytes) {
            Ok(r) => self.start_trace(r, name, add, Vec::new()),
            Err(e) => self.error = Some(e),
        }
    }

    fn start_trace(&mut self, src: trace::Raster, name: String, add: bool, notes: Vec<String>) {
        let mut opts = trace::TraceOptions::default();
        // a sensible first size: left chest for flats, front for caps
        let width_in = match self.profile {
            Profile::Cap => 4.0,
            Profile::Flat => 3.5,
        };
        opts.width_mm = width_in * 25.4;
        self.trace_dlg = Some(TraceDlg {
            name,
            add,
            src,
            opts,
            width_in,
            result: None,
            changed_at: -1.0,
            src_tex: None,
            out_tex: None,
            stitch_tex: None,
            show_stitch: false,
            drag: None,
            notes,
        });
    }

    /// The trace window: picture on the left (drag to crop), the traced
    /// logo on the right, settings and colours below.
    fn trace_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.trace_dlg else { return };
        let now = ctx.input(|i| i.time);
        // trace when settings have been still for a moment
        if dlg.result.is_none() || (dlg.changed_at >= 0.0 && now - dlg.changed_at > 0.35) {
            dlg.changed_at = -1.0;
            let r = trace::trace(&dlg.src, &dlg.opts);
            dlg.out_tex = match &r {
                Ok(t) => {
                    let svg = t.to_svg();
                    render_svg(&svg, 520).map(|(w, h, px)| {
                        ctx.load_texture("trace-out", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR)
                    })
                }
                Err(_) => None,
            };
            // rough stitches from the same trace; the open design's recipe and profile
            dlg.stitch_tex = match &r {
                Ok(t) => stitch_preview(&t.to_svg(), t.w as f64 * t.mm_per_px, &self.chart, &self.recipe, self.profile)
                    .map(|(w, h, px)| ctx.load_texture("trace-stitch", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR)),
                Err(_) => None,
            };
            dlg.result = Some(r);
        } else if dlg.changed_at >= 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        if dlg.src_tex.is_none() {
            let k = (520.0 / dlg.src.w.max(dlg.src.h) as f64).min(1.0);
            let (w, h) = (((dlg.src.w as f64 * k) as usize).max(1), ((dlg.src.h as f64 * k) as usize).max(1));
            let mut px = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let sx = ((x as f64 / k) as u32).min(dlg.src.w - 1);
                    let sy = ((y as f64 / k) as u32).min(dlg.src.h - 1);
                    px.extend_from_slice(&dlg.src.px[(sy * dlg.src.w + sx) as usize]);
                }
            }
            dlg.src_tex = Some(ctx.load_texture("trace-src", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR));
        }
        let mut open = true;
        let mut go = false;
        let mut changed = false;
        let mut reset_colors = false;
        egui::Window::new("Trace picture")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(900.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("StitchForge finds the logo, its colours and its shapes, and builds stitch-ready layers. Drag on the picture to crop.").weak());
                ui.add_space(4.0);
                let box_sz = Vec2::new(420.0, 300.0);
                ui.horizontal(|ui| {
                    // ---- source with crop
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Picture").strong());
                        let tex = dlg.src_tex.as_ref().unwrap();
                        let ts = tex.size_vec2();
                        let k = (box_sz.x / ts.x).min(box_sz.y / ts.y);
                        let (r, resp) = ui.allocate_exact_size(box_sz, Sense::drag());
                        let img = Rect::from_center_size(r.center(), ts * k);
                        let p = ui.painter_at(r);
                        checker(&p, r);
                        p.image(tex.id(), img, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        let (sw, sh) = (dlg.src.w as f32, dlg.src.h as f32);
                        let to_img = |q: Pos2| -> (f32, f32) {
                            (((q.x - img.left()) / img.width() * sw).clamp(0.0, sw), ((q.y - img.top()) / img.height() * sh).clamp(0.0, sh))
                        };
                        if resp.drag_started() {
                            dlg.drag = resp.interact_pointer_pos().map(|q| (q, q));
                        }
                        if resp.dragged() {
                            if let (Some(d), Some(q)) = (&mut dlg.drag, resp.interact_pointer_pos()) {
                                d.1 = q;
                            }
                        }
                        if resp.drag_stopped() {
                            if let Some((a, b)) = dlg.drag.take() {
                                let (ax, ay) = to_img(a);
                                let (bx, by) = to_img(b);
                                let (x0, y0, x1, y1) = (ax.min(bx), ay.min(by), ax.max(bx), ay.max(by));
                                if x1 - x0 > 8.0 && y1 - y0 > 8.0 {
                                    dlg.opts.crop = Some([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]);
                                    reset_colors = true;
                                    changed = true;
                                }
                            }
                        }
                        // the crop in use
                        let crop = dlg
                            .drag
                            .map(|(a, b)| Rect::from_two_pos(a, b))
                            .or_else(|| {
                                let c = match &dlg.result {
                                    Some(Ok(t)) => t.crop,
                                    _ => dlg.opts.crop?,
                                };
                                Some(Rect::from_min_size(
                                    Pos2::new(img.left() + c[0] as f32 / sw * img.width(), img.top() + c[1] as f32 / sh * img.height()),
                                    Vec2::new(c[2] as f32 / sw * img.width(), c[3] as f32 / sh * img.height()),
                                ))
                            });
                        if let Some(c) = crop {
                            let shade = Color32::from_black_alpha(90);
                            for rr in [
                                Rect::from_min_max(img.min, Pos2::new(img.right(), c.top())),
                                Rect::from_min_max(Pos2::new(img.left(), c.bottom()), img.max),
                                Rect::from_min_max(Pos2::new(img.left(), c.top()), Pos2::new(c.left(), c.bottom())),
                                Rect::from_min_max(Pos2::new(c.right(), c.top()), Pos2::new(img.right(), c.bottom())),
                            ] {
                                if rr.is_positive() {
                                    p.rect_filled(rr, 0.0, shade);
                                }
                            }
                            p.rect_stroke(c, 0.0, Stroke::new(1.5_f32, Color32::from_rgb(214, 60, 120)), egui::StrokeKind::Outside);
                        }
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} × {} px", dlg.src.w, dlg.src.h)).weak().small());
                            if dlg.opts.crop.is_some() && ui.small_button("Auto crop").clicked() {
                                dlg.opts.crop = None;
                                reset_colors = true;
                                changed = true;
                            }
                        });
                    });
                    // ---- result
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut dlg.show_stitch, false, RichText::new("Traced art").strong())
                                .on_hover_text("The traced shapes and colours");
                            ui.selectable_value(&mut dlg.show_stitch, true, RichText::new("Rough stitches").strong())
                                .on_hover_text("A quick stitch preview of this trace, at the width and fabric you chose. Not a sew-out.");
                        });
                        let (r, _) = ui.allocate_exact_size(box_sz, Sense::hover());
                        let p = ui.painter_at(r);
                        p.rect_filled(r, 4.0, Color32::from_rgb(242, 239, 232));
                        let shown = if dlg.show_stitch { dlg.stitch_tex.as_ref() } else { dlg.out_tex.as_ref() };
                        match (&dlg.result, shown) {
                            (Some(Ok(_)), Some(tex)) => {
                                let ts = tex.size_vec2();
                                let k = (box_sz.x / ts.x).min(box_sz.y / ts.y) * 0.95;
                                p.image(tex.id(), Rect::from_center_size(r.center(), ts * k), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                            (Some(Err(e)), _) => {
                                p.text(r.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(13.0), Color32::from_rgb(170, 40, 40));
                            }
                            _ => {
                                p.text(r.center(), egui::Align2::CENTER_CENTER, "Tracing…", egui::FontId::proportional(14.0), Color32::GRAY);
                            }
                        }
                        if let Some(Ok(t)) = &dlg.result {
                            let pts: usize = t.layers.iter().flat_map(|l| l.paths.iter()).map(|p| p.nodes.len()).sum();
                            ui.label(
                                RichText::new(format!(
                                    "{:.2} × {:.2} in · {} colors · {} shapes · {} points{}",
                                    t.w as f64 * t.mm_per_px / 25.4,
                                    t.height_mm() / 25.4,
                                    {
                                        let mut c: Vec<usize> = t.layers.iter().map(|l| l.color).collect();
                                        c.sort();
                                        c.dedup();
                                        c.len()
                                    },
                                    t.layers.iter().map(|l| l.paths.len()).sum::<usize>(),
                                    pts,
                                    if t.mockup { " · garment removed" } else { "" }
                                ))
                                .weak()
                                .small(),
                            );
                        }
                    });
                });
                ui.separator();
                egui::Grid::new("trace-opts").num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                    ui.label("Width");
                    if ui.add(egui::DragValue::new(&mut dlg.width_in).speed(0.05).range(0.5..=16.0).suffix(" in").max_decimals(2)).changed() {
                        dlg.opts.width_mm = dlg.width_in * 25.4;
                        changed = true;
                    }
                    ui.label("Colors");
                    let shown = if dlg.opts.max_colors == 0 { "Automatic".to_string() } else { format!("Up to {}", dlg.opts.max_colors) };
                    egui::ComboBox::from_id_salt("trace-colors").selected_text(shown).show_ui(ui, |ui| {
                        for n in [0usize, 1, 2, 3, 4, 5, 6, 8, 10, 12] {
                            let label = if n == 0 { "Automatic".to_string() } else { format!("Up to {n}") };
                            if ui.selectable_value(&mut dlg.opts.max_colors, n, label).changed() {
                                reset_colors = true;
                                changed = true;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Detail");
                    ui.horizontal(|ui| {
                        for d in trace::Detail::ALL {
                            if ui.selectable_value(&mut dlg.opts.detail, d, d.label()).changed() {
                                reset_colors = true;
                                changed = true;
                            }
                        }
                    });
                    ui.label("Picture");
                    egui::ComboBox::from_id_salt("trace-src").selected_text(dlg.opts.source.label()).show_ui(ui, |ui| {
                        for s in trace::Source::ALL {
                            if ui.selectable_value(&mut dlg.opts.source, s, s.label()).changed() {
                                reset_colors = true;
                                changed = true;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    if ui.checkbox(&mut dlg.opts.remove_background, "Remove background").on_hover_text("The colour around the logo (and on a mockup, the garment) isn't stitched").changed() {
                        reset_colors = true;
                        changed = true;
                    }
                    ui.label("");
                    if ui.checkbox(&mut dlg.opts.overlap, "Colors run under each other").on_hover_text("Lower colours extend under the ones on top so no fabric shows between them").changed() {
                        changed = true;
                    }
                    ui.end_row();
                    ui.label("Parts");
                    ui.horizontal(|ui| {
                        let mut on = dlg.opts.split_bands_mm > 0.0;
                        if ui
                            .checkbox(&mut on, "Separate rings and lines")
                            .on_hover_text("Rings, borders and lines become their own parts (satin), apart from the solid shapes of the same colour they touch (fill)")
                            .changed()
                        {
                            dlg.opts.split_bands_mm = if on { 4.0 } else { 0.0 };
                            changed = true;
                        }
                        if on {
                            ui.label("up to");
                            if ui
                                .add(egui::DragValue::new(&mut dlg.opts.split_bands_mm).range(0.5..=10.0).speed(0.1).suffix(" mm"))
                                .on_hover_text("Bands up to this wide are separated. Wider ones stay with the solid shapes")
                                .changed()
                            {
                                changed = true;
                            }
                            ui.label("wide");
                        }
                    });
                    ui.end_row();
                });
                // ---- fine control: each starts at what the Detail level picks
                let tuned = dlg.opts.smooth_mm.is_some()
                    || dlg.opts.speck_mm2.is_some()
                    || dlg.opts.min_line_mm.is_some()
                    || dlg.opts.merge_de.is_some()
                    || dlg.opts.fit_tol_mm.is_some()
                    || !dlg.opts.snap_circles
                    || !dlg.opts.snap_corners;
                egui::CollapsingHeader::new(RichText::new(if tuned { "Fine tune •" } else { "Fine tune" }).strong())
                    .id_salt("trace-fine")
                    .default_open(tuned)
                    .show(ui, |ui| {
                        let o = &mut dlg.opts;
                        let (c_sm, c_fit, c_sp, c_line, c_de) = (o.eff_smooth(), o.eff_fit(), o.eff_speck(), o.eff_min_line(), o.eff_merge() as f64);
                        let slider = |ui: &mut egui::Ui, label: &str, tip: &str, v: &mut Option<f64>, cur: f64, range: std::ops::RangeInclusive<f64>, unit: &str, dec: usize| -> bool {
                            let mut x = v.unwrap_or(cur);
                            let mut ch = false;
                            ui.horizontal(|ui| {
                                ui.add_sized([150.0, 18.0], egui::Label::new(label)).on_hover_text(tip);
                                if ui.add(egui::Slider::new(&mut x, range).suffix(unit).fixed_decimals(dec)).changed() {
                                    *v = Some(x);
                                    ch = true;
                                }
                                if v.is_some() && ui.small_button("↺").on_hover_text("Back to the Detail level's setting").clicked() {
                                    *v = None;
                                    ch = true;
                                }
                            });
                            ch
                        };
                        changed |= slider(ui, "Edge smoothing", "Smooths jagged pixel edges. More = rounder, softer outlines; less = follows the picture exactly", &mut o.smooth_mm, c_sm, 0.0..=0.6, " mm", 2);
                        changed |= slider(ui, "Curve accuracy", "How closely the vector curves follow the edge. Smaller = more points and closer fit", &mut o.fit_tol_mm, c_fit, 0.01..=0.4, " mm", 2);
                        changed |= slider(ui, "Smallest piece", "Specks and islands smaller than this are dropped (too small to sew)", &mut o.speck_mm2, c_sp, 0.02..=10.0, " mm²", 2);
                        changed |= slider(ui, "Thinnest line", "Lines thinner than this are absorbed into the colour around them. Lower keeps finer strokes", &mut o.min_line_mm, c_line, 0.02..=2.0, " mm", 2);
                        let mut de = o.merge_de.map(|v| v as f64);
                        if slider(ui, "Merge similar colors", "Colours closer than this (ΔE) become one thread. Higher = fewer colours", &mut de, c_de, 3.0..=40.0, "", 0) {
                            o.merge_de = de.map(|v| v as f32);
                            reset_colors = true;
                            changed = true;
                        }
                        ui.horizontal(|ui| {
                            changed |= ui.checkbox(&mut o.snap_circles, "Snap circles").on_hover_text("Round shapes become perfect circles").changed();
                            changed |= ui.checkbox(&mut o.snap_corners, "Snap straight edges").on_hover_text("Bars, stars and block letters get crisp straight sides and sharp corners").changed();
                        });
                        if tuned && ui.small_button("All back to the Detail level").clicked() {
                            o.smooth_mm = None;
                            o.speck_mm2 = None;
                            o.min_line_mm = None;
                            o.merge_de = None;
                            o.fit_tol_mm = None;
                            o.snap_circles = true;
                            o.snap_corners = true;
                            reset_colors = true;
                            changed = true;
                        }
                    });
                if let Some(Ok(t)) = &dlg.result {
                    section(ui, "Thread colors (click to switch on or off)");
                    let mut toggle: Option<(usize, bool)> = None;
                    ui.horizontal_wrapped(|ui| {
                        for (i, c) in t.palette.iter().enumerate() {
                            if c.share <= 0.0 && !c.dropped {
                                continue;
                            }
                            let on = !c.dropped && t.layers.iter().any(|l| l.color == i);
                            let chart = &self.chart;
                            let th = &chart[thread::nearest(chart, c.rgb).0];
                            let text = format!(
                                "{} {}{}",
                                th.code,
                                th.name,
                                if c.like_background { " · background color" } else { "" }
                            );
                            let (rr, resp) = ui.allocate_exact_size(Vec2::new(200.0, 30.0), Sense::click());
                            let p = ui.painter();
                            p.rect_filled(rr, 5.0, if on { ui.visuals().widgets.inactive.bg_fill } else { Color32::TRANSPARENT });
                            p.rect_stroke(rr, 5.0, Stroke::new(1.0_f32, ui.visuals().widgets.inactive.bg_stroke.color), egui::StrokeKind::Inside);
                            let sw = Rect::from_min_size(rr.min + Vec2::new(6.0, 6.0), Vec2::splat(18.0));
                            p.rect_filled(sw, 3.0, Color32::from_rgb(c.rgb[0], c.rgb[1], c.rgb[2]));
                            if !on {
                                p.line_segment([sw.left_bottom(), sw.right_top()], Stroke::new(2.0_f32, Color32::from_rgb(200, 40, 40)));
                            }
                            p.text(
                                Pos2::new(sw.right() + 6.0, rr.center().y),
                                egui::Align2::LEFT_CENTER,
                                text,
                                egui::FontId::proportional(12.0),
                                if on { ui.visuals().text_color() } else { ui.visuals().weak_text_color() },
                            );
                            let tip = if c.like_background {
                                "Same colour as the background: where it's background showing through (letter insides, around the logo) it isn't stitched. Click to stitch it everywhere (e.g. for a dark garment)."
                            } else {
                                "Click to leave this colour out"
                            };
                            if resp.on_hover_text(tip).clicked() {
                                toggle = Some((i, on));
                            }
                        }
                    });
                    if let Some((i, on)) = toggle {
                        let o = &mut dlg.opts;
                        if on {
                            o.keep.retain(|k| *k != i);
                            if !o.drop.contains(&i) {
                                o.drop.push(i);
                            }
                        } else {
                            o.drop.retain(|k| *k != i);
                            if t.palette[i].like_background && !o.keep.contains(&i) {
                                o.keep.push(i);
                            }
                        }
                        changed = true;
                    }
                    for n in t.notes.iter().chain(dlg.notes.iter()) {
                        ui.label(RichText::new(format!("• {n}")).color(Color32::from_rgb(170, 110, 20)));
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ok = matches!(dlg.result, Some(Ok(_)));
                    if ui.add_enabled(ok, egui::Button::new(RichText::new(if dlg.add { "Add to design" } else { "Use this" }).strong())).clicked() {
                        go = true;
                    }
                    ui.label(RichText::new("Each color becomes a layer you can still edit, restitch or recolor.").weak().small());
                });
            });
        if reset_colors {
            dlg.opts.drop.clear();
            dlg.opts.keep.clear();
        }
        if changed {
            dlg.changed_at = now;
            ctx.request_repaint();
        }
        if go {
            let dlg = self.trace_dlg.take().unwrap();
            if let Some(Ok(t)) = dlg.result {
                let svg = t.to_svg();
                if !dlg.add {
                    self.machine = None;
                    self.project_path = None;
                }
                let before = self.layers.len();
                self.open_svg_data(&svg, &dlg.name, dlg.add);
                // exactly the width asked for (the trace keeps a little margin)
                let start = if dlg.add { before } else { 0 };
                let mut b = BBox::empty();
                for l in &self.layers[start..] {
                    let lb = l.bbox();
                    b.add(lb.min);
                    b.add(lb.max);
                }
                if b.is_valid() && b.width() > 1.0 {
                    let k = dlg.width_in * 25.4 / b.width();
                    scale_layers(&mut self.layers[start..], b.center(), k);
                    self.dirty = true;
                }
                // traced colours keep the exact traced shade in the name
                if !dlg.add {
                    self.clean_after_regen = true;
                    self.sequence_tip();
                }
                let n = self.layers.len() - if dlg.add { before } else { 0 };
                self.file_notes = t.notes.iter().chain(dlg.notes.iter()).cloned().collect();
                self.status = format!("Traced {} into {n} color layers — check them, then adjust stitches as usual", dlg.name);
            }
            return;
        }
        if !open {
            self.trace_dlg = None;
        }
    }

    fn open_svg_path(&mut self, path: PathBuf, add: bool) {
        match stitchforge::vfs::read_to_string(&path) {
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
            self.file_notes.clear();
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
                    // sf- stitch tags in the art's names are settings, not names
                    let label: String = s
                        .label
                        .split_whitespace()
                        .filter(|w| !w.starts_with("sf-"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let name = if label.is_empty() {
                        if outline {
                            format!("Outline {n}")
                        } else {
                            format!("Shape {n}")
                        }
                    } else {
                        label
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
        stitchforge::project::text_shapes(opt, spec)
    }

    fn add_text_layer(&mut self) {
        let spec = TextSpec {
            text: self.text.clone(),
            font: self.font.clone(),
            height_mm: self.text_height_in * 25.4,
            rgb: self.text_color,
            ..Default::default()
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
                if let Some(at) = self.text_at.take() {
                    l.offset = at;
                }
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
        self.layers_rev += 1;
        if self.tab == Tab::Drops && self.machine.is_none() {
            self.drop_info = None;
            let n = self.drops.drop_count();
            if n == 0 || self.drops.problem(&self.layers).is_some() {
                self.design = None;
                return;
            }
            self.drop_view = self.drop_view.min(n - 1);
            let proj = self.to_project();
            let view = self.drop_view;
            let (res, fit) = {
                let opt = self.fonts_db();
                (drops_mod::build_drop(&proj, opt, view), drops_mod::fit_report(&proj, opt))
            };
            self.drop_fit = fit;
            match res {
                Ok((d, n_logo, slots)) => {
                    self.drop_sel = self.drop_sel.min(slots.len().saturating_sub(1));
                    self.progress = d.cmds.len();
                    self.design = Some(d);
                    self.drop_info = Some((n_logo, slots));
                    self.error = None;
                }
                Err(e) => {
                    self.design = None;
                    self.error = Some(e);
                }
            }
            return;
        }
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
        // Hand-edited layers sew exactly the stitches the user left them with.
        let (art, chart) = stitchforge::project::compose(&self.layers, &self.chart);
        let name = format!(
            "{}-{}",
            self.design_name,
            if self.profile == Profile::Cap {
                "cap"
            } else {
                "flat"
            }
        );
        let t = now_ms();
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
                    self.status = format!("Stitches generated in {:.0} ms", now_ms() - t);
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
        if self.tab == Tab::Drops {
            let Some(dir) = export_dir(
                "Choose a folder for the drop files and run sheet",
                self.last_dir.as_ref(),
            ) else {
                return;
            };
            self.export_drops_to(&dir, &fmts);
            return;
        }
        let Some(dir) = export_dir(
            "Choose a folder for the machine files and operator sheet",
            self.last_dir.as_ref(),
        ) else {
            return;
        };
        let generated = stamp();
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
                deliver(&dir, &format!("{}-files", d.name), &w.sheet, &w.sheet_pdf);
                if cfg!(target_arch = "wasm32") {
                    self.status = format!("Downloaded {}-files.zip ({})", d.name, names.join(", "));
                }
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
        p.drops = self.drops.clone();
        p
    }

    fn apply_project(&mut self, p: Project, path: Option<PathBuf>) {
        self.file_notes.clear();
        self.next_id = p.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        self.layers = p.layers;
        self.design_name = p.name;
        self.profile = p.profile;
        self.fabric = p.fabric;
        self.recipe = p.recipe;
        self.keep_order = p.keep_order;
        self.auto_stops = p.auto_stops;
        self.chart_name = p.chart_name.clone();
        self.chart = p.chart.clone();
        // the threads saved with the design are shown as their own library
        // (ticked alone), so its colours stay exactly as saved
        self.libs.retain(|l| !l.key.starts_with("design:"));
        for l in self.libs.iter_mut() {
            l.on = false;
        }
        let name = if p.chart_name.is_empty() { "Threads in this design".to_string() } else { format!("{} (in this design)", p.chart_name) };
        let threads: Vec<thread::Thread> = p.chart.clone();
        let n = threads.len();
        self.libs.push(LoadedLib { key: format!("design:{}", self.design_name), name, note: format!("{n} threads saved with this design."), threads, on: true });
        self.chart_note = format!("{n} threads saved with this design.");
        self.job = p.job;
        self.drops = p.drops;
        self.tab = Tab::Design;
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
            Some(p) if !cfg!(target_arch = "wasm32") => self.save_to(p),
            _ => self.save_as(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn save_as(&mut self) -> bool {
        if self.machine.is_some() || self.layers.is_empty() {
            return false;
        }
        let p = PathBuf::from(format!("/out/{}.sfg", self.design_name));
        if !self.save_to(p.clone()) {
            return false;
        }
        if let Ok(data) = stitchforge::vfs::read(&p) {
            web::download(&format!("{}.sfg.json", self.design_name), data);
            self.status = format!("Downloaded {}.sfg.json — open it again with Open…", self.design_name);
        }
        true
    }

    #[cfg(not(target_arch = "wasm32"))]
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

    /// Store each layer's current stitches with it (the embroidery side of
    /// every design object travels in the project file).
    fn sync_embroidery(&mut self) {
        if self.dirty || self.machine.is_some() || self.tab != Tab::Design {
            return;
        }
        let Some(d) = &self.design else { return };
        for (li, l) in self.layers.iter_mut().enumerate() {
            if l.embroidery.edited {
                continue;
            }
            let needles = d
                .layer_needles
                .get(&li)
                .map(|v| {
                    v.iter()
                        .map(|n| Needle {
                            p: l.inv(n.p),
                            ..*n
                        })
                        .collect()
                })
                .unwrap_or_default();
            l.embroidery = Embroidery {
                needles,
                edited: false,
                source: l.source_fingerprint(),
            };
        }
    }

    // ---------- editing a design object ----------

    fn edit_layer(&self) -> Option<usize> {
        let id = match self.edit {
            Edit::Stitches(id) | Edit::Shape(id) => id,
            Edit::None => return None,
        };
        self.layers.iter().position(|l| l.id == id)
    }

    fn start_stitch_edit(&mut self, li: usize) {
        if self.dirty {
            self.regenerate();
            self.dirty = false;
        }
        self.sync_embroidery();
        self.edit = Edit::Stitches(self.layers[li].id);
        self.selected = Some(self.layers[li].id);
        self.st_sel.clear();
        self.edit_drag = None;
        self.playing = false;
        self.status = "Editing stitches: click or drag a box to select points, drag to move, Delete removes, double-click a stitch to add a point, Esc when done".into();
    }

    fn start_shape_edit(&mut self, li: usize) {
        self.hard_checkpoint();
        let l = &mut self.layers[li];
        if l.text.take().is_some() {
            self.status = "Text converted to outlines so its shape can be edited".into();
        } else {
            self.status = "Editing shape: drag anchors and handles, double-click the outline to add an anchor, Delete removes one, Esc when done".into();
        }
        for s in &mut l.shapes {
            s.ensure_curves();
        }
        self.edit = Edit::Shape(l.id);
        self.selected = Some(l.id);
        self.node_sel = None;
        self.edit_drag = None;
        self.playing = false;
    }

    fn end_edit(&mut self) {
        self.edit = Edit::None;
        if matches!(self.tool, Tool::Nodes | Tool::Stitches) {
            self.tool = Tool::Select;
        }
        self.st_sel.clear();
        self.node_sel = None;
        self.edit_drag = None;
        self.status = "Done editing".into();
    }

    /// Mark a layer's stitches as hand-edited (they now match its shape).
    fn touch_stitches(&mut self, li: usize) {
        let l = &mut self.layers[li];
        l.embroidery.edited = true;
        l.embroidery.source = l.source_fingerprint();
        self.dirty = true;
    }

    /// Keys while editing. Returns true if the key was used.
    fn edit_keys(&mut self, ctx: &egui::Context) -> bool {
        let Some(li) = self.edit_layer() else {
            if self.edit != Edit::None {
                self.end_edit();
            }
            return false;
        };
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        let (shift, cmd) = ctx.input(|i| (i.modifiers.shift, i.modifiers.command));
        if pressed(egui::Key::Escape) {
            self.end_edit();
            return true;
        }
        let del = pressed(egui::Key::Delete) || pressed(egui::Key::Backspace);
        match self.edit {
            Edit::Stitches(_) => {
                if cmd && pressed(egui::Key::A) {
                    self.st_sel = (0..self.layers[li].embroidery.needles.len()).collect();
                    return true;
                }
                if del && !self.st_sel.is_empty() {
                    self.hard_checkpoint();
                    let sel = std::mem::take(&mut self.st_sel);
                    let n = &mut self.layers[li].embroidery.needles;
                    *n = stitchforge::project::remove_needles(n, |i| sel.contains(&i));
                    self.touch_stitches(li);
                    return true;
                }
                let step = if shift { 0.5 } else { 0.1 };
                let mut d = Pt::new(0.0, 0.0);
                if pressed(egui::Key::ArrowLeft) { d.x -= step; }
                if pressed(egui::Key::ArrowRight) { d.x += step; }
                if pressed(egui::Key::ArrowUp) { d.y -= step; }
                if pressed(egui::Key::ArrowDown) { d.y += step; }
                if d.len() > 0.0 && !self.st_sel.is_empty() {
                    self.hard_checkpoint();
                    let l = &mut self.layers[li];
                    for &i in &self.st_sel {
                        if let Some(nd) = l.embroidery.needles.get(i).copied() {
                            let q = l.tf(nd.p).add(d);
                            l.embroidery.needles[i].p = l.inv(q);
                        }
                    }
                    self.touch_stitches(li);
                    return true;
                }
            }
            Edit::Shape(_) => {
                if del {
                    if let Some(r) = self.node_sel {
                        let c = &mut self.layers[li].shapes[r.shape].curves[r.path];
                        let min = if c.closed { 3 } else { 2 };
                        if c.nodes.len() > min {
                            self.hard_checkpoint();
                            let c = &mut self.layers[li].shapes[r.shape].curves[r.path];
                            c.nodes.remove(r.node);
                            self.layers[li].shapes[r.shape].reflatten();
                            self.node_sel = None;
                            self.dirty = true;
                        } else {
                            self.status = "A path needs at least that many anchors".into();
                        }
                    }
                    return true;
                }
            }
            Edit::None => {}
        }
        false
    }

    /// Panel shown while editing a design object.
    fn edit_panel(&mut self, ui: &mut egui::Ui) {
        let Some(li) = self.edit_layer() else { return };
        let accent = Color32::from_rgb(30, 110, 230);
        let mut clean: Option<u8> = None;
        egui::Frame::NONE
            .fill(accent.gamma_multiply(0.12))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let l = &self.layers[li];
                match self.edit {
                    Edit::Stitches(_) => {
                        ui.label(RichText::new(format!("Editing stitches · {}", l.name)).strong());
                        ui.label(format!(
                            "{} needle points · {} selected",
                            group(l.embroidery.needles.len()),
                            self.st_sel.len()
                        ));
                        ui.label(RichText::new("Click a point or drag a box to select (Shift adds) · Alt/Option-drag draws a free-hand selection · drag to move · arrows nudge 0.1 mm (Shift 0.5) · Delete removes (the thread jumps over the gap) · Eraser (X) wipes out the points you draw around · double-click a stitch to add a point · Ctrl/Cmd+A selects all · right-drag pans").weak().small());
                        ui.add_space(4.0);
                        ui.label(RichText::new("CLEAN UP").small().strong());
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Remove strays").on_hover_text("Drop needle points that are off the shape, and turn any stitch still crossing open fabric into a jump").clicked() {
                                clean = Some(0);
                            }
                            if ui.button("Remove tiny stitches").on_hover_text("Stitches shorter than 0.3 mm break thread and make knots").clicked() {
                                clean = Some(1);
                            }
                            if ui.button("Split long stitches").on_hover_text("Break stitches over 7 mm so they don't snag").clicked() {
                                clean = Some(2);
                            }
                        });
                    }
                    Edit::Shape(_) => {
                        let anchors: usize = l.shapes.iter().flat_map(|s| s.curves.iter()).map(|c| c.nodes.len()).sum();
                        ui.label(RichText::new(format!("Editing shape · {}", l.name)).strong());
                        ui.label(format!("{anchors} anchors"));
                        ui.label(RichText::new("Drag an anchor or handle · double-click the outline to add an anchor · Delete removes the selected anchor · stitches regenerate when you let go").weak().small());
                        if l.embroidery.edited {
                            ui.label(RichText::new("This layer's stitches were edited by hand: changing the shape will ask whether to regenerate them.").color(Color32::from_rgb(190, 120, 30)).small());
                        }
                    }
                    Edit::None => {}
                }
                if ui.button(RichText::new("Done").strong()).clicked() {
                    self.end_edit();
                }
            });
        if let Some(c) = clean {
            self.stitch_cleanup(li, c);
        }
        ui.add_space(6.0);
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        self.sync_embroidery();
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
        self.drops = DropSetup::default();
        self.tab = Tab::Design;
        self.modified = false;
        self.status = "New design".into();
        self.file_notes.clear();
    }

    fn open_dialog(&mut self) {
        pick(
            Want::Open,
            "Open",
            &[
                (
                    "Everything StitchForge opens",
                    &["sfg", "json", "svg", "pdf", "ai", "png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "dst", "exp", "pes", "pec", "jef", "vp3", "xxx", "u01", "tap"],
                ),
                ("StitchForge project", &["sfg", "json"]),
                ("Artwork (SVG, PDF, AI)", &["svg", "pdf", "ai"]),
                ("Pictures and mockups (auto-traced)", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"]),
                ("Stitch files (DST, PES, PEC, EXP, JEF, VP3, XXX, U01, TAP)", &formats::READ_EXTS),
            ],
            self.last_dir.as_ref(),
        );
    }

    /// A file came back from a picker (desktop: at once; browser: after upload).
    fn handle_picked(&mut self, want: Want, p: PathBuf) {
        match want {
            Want::Open => self.open_any(p),
            Want::AddSvg => {
                if p.extension().map(|e| e.eq_ignore_ascii_case("svg")).unwrap_or(false) {
                    self.open_svg_path(p, true)
                } else {
                    self.open_art_file(p, true)
                }
            }
            Want::Names => self.load_names_path(p),
            Want::Chart => self.add_library_file(&p),
            Want::Font => match stitchforge::vfs::read(&p)
                .map_err(|e| e.to_string())
                .and_then(import::add_font_data)
            {
                Ok(fam) => {
                    self.fonts = None;
                    self.font_opts = None;
                    self.font = fam.clone();
                    self.status = format!("Font added: {fam}");
                    self.dirty = true;
                }
                Err(e) => self.error = Some(format!("Couldn't use that font: {e}")),
            },
        }
    }

    fn open_any(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        self.last_dir = path.parent().map(|p| p.to_path_buf());
        match ext.as_str() {
            "sfg" | "json" => match Project::load(&path) {
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
            e if formats::READ_EXTS.contains(&e) => self.open_machine(path),
            e if ART_EXT.contains(&e) => self.open_art_file(path, false),
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

    /// Turn the open stitch file into editable layers (a new, unsaved design).
    fn convert_machine(&mut self) {
        let Some(m) = self.machine.take() else { return };
        let colors: Vec<[u8; 3]> = m.threads.iter().map(|t| t.rgb).collect();
        let (art, sum) = stitchforge::unstitch::unstitch(&m.file.cmds, &colors);
        if art.shapes.is_empty() {
            self.machine = Some(m);
            self.error = Some("No stitched areas or lines were found to convert".into());
            return;
        }
        self.hard_checkpoint();
        let mut layers = self.layers_from_art(art);
        // keep the exact thread each stop had
        for l in &mut layers {
            let rgb = l.shapes.first().map(|s| s.rgb);
            if let Some(t) = m.threads.iter().find(|t| Some(t.rgb) == rgb) {
                l.custom = Some(t.clone());
            }
        }
        self.layers = layers;
        self.project_path = None;
        self.file_notes.clear();
        self.selected = None;
        self.keep_order = true;
        self.fit = true;
        self.dirty = true;
        self.modified = true;
        self.tab = Tab::Design;
        self.error = None;
        let mut parts = Vec::new();
        for (n, w) in [(sum.fills, "fill"), (sum.satins, "satin"), (sum.runs, "run line")] {
            if n > 0 {
                parts.push(format!("{n} {w}{}", if n == 1 { "" } else { "s" }));
            }
        }
        self.status = format!(
            "Converted {} into {}. Shapes are rebuilt from the stitches: check edges, angles and densities before sewing.",
            m.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            parts.join(", ")
        );
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
            stitchforge::design::build_with(
                "tip",
                &art,
                &chart,
                &self.recipe,
                self.profile,
                BuildOptions {
                    keep_order: keep,
                    auto_stops: self.auto_stops,
                },
            )
            .map(|d| d.stops.len())
            .unwrap_or(0)
        };
        let (mine, auto) = (build(true), build(false));
        if auto + 1 < mine {
            self.status = format!(
                "Tip: Auto-sequence layers (right panel) would cut stops from {mine} to {auto}."
            );
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
        let convert;
        {
            let Some(m) = &self.machine else { return };
            ui.label(RichText::new(format!("Stitch file · {}", m.file.format)).strong());
            ui.label(RichText::new("Stitches are read-only here. Pick the thread for each stop, play the sew-out, print the operator sheet, or export to another format.").weak().small());
            ui.add_space(4.0);
            convert = ui
                .button(RichText::new("Convert to editable shapes").strong())
                .on_hover_text("Rebuild fills, satins and run lines from the stitches, as layers you can reshape, re-angle and re-stitch. The stitch file itself is left as it is.")
                .clicked();
            ui.separator();
        }
        if convert {
            self.convert_machine();
            return;
        }
        let Some(m) = &mut self.machine else { return };
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
        let filters: &[(&str, &[&str])] = &[
            ("Artwork and pictures", ART_EXT),
            ("Vector art (SVG, PDF, AI)", &["svg", "pdf", "ai"]),
            ("Pictures and mockups (auto-traced)", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"]),
        ];
        if add {
            pick(Want::AddSvg, "Add art", filters, self.last_dir.as_ref());
        } else {
            pick(Want::Open, "Open artwork", filters, self.last_dir.as_ref());
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
            ui.label(RichText::new("Always included: operator sheet as a PDF (thread colours print exactly) and as a web page, color-stop CSV, PNG preview, job summary.").weak().small());
            if self.design.as_ref().is_some_and(|d| !d.pieces.is_empty()) {
                ui.add_space(4.0);
                ui.label("Twill / appliqué cut files");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.job.cut.svg, "SVG");
                    ui.checkbox(&mut self.job.cut.dxf, "DXF");
                    ui.checkbox(&mut self.job.cut.plt, "PLT (HPGL)");
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.job.cut.mirror, "Mirror")
                        .on_hover_text("Flip left-right, for cutting with the adhesive side up");
                    ui.label("Kerf");
                    ui.add(egui::DragValue::new(&mut self.job.cut.kerf).range(-1.0..=1.0).speed(0.01).suffix(" mm"))
                        .on_hover_text("Extra allowance added all round for the laser beam or blade");
                });
                ui.label(RichText::new("One set per twill colour, at real size.").weak().small());
            }
            ui.checkbox(&mut self.job.sequence_page, "Sheet: stitch list by object on page 2")
                .on_hover_text("Page 1 is what the operator needs at the machine; the object-by-object stitch list goes on its own page. Untick to leave it out.");
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
        let mut focus: Option<(u64, usize)> = None;
        let mut own_layer: Option<(usize, usize)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (li, l) in self.layers.iter().enumerate() {
                let open = self.expanded.contains(&l.id);
                let sel = self.selected == Some(l.id) || self.multi.contains(&l.id);
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
                        if l.out_of_sync() {
                            ui.label(RichText::new("⚠ check").small().color(Color32::from_rgb(220, 140, 40))).on_hover_text("Stitches were edited by hand, then the shape or settings changed");
                        } else if l.embroidery.edited {
                            ui.label(RichText::new("edited").small().color(Color32::from_rgb(90, 150, 240))).on_hover_text("Stitches edited by hand");
                        }
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
                        let many = objs.len() > 1;
                        for (k, o) in objs.iter().enumerate() {
                            let angle = if has_angle(o.stitch_type) { format!(" · {:.0}°", o.angle_deg) } else { String::new() };
                            let on = self.focus_part == Some((l.id, o.sequence));
                            ui.horizontal(|ui| {
                                let text = RichText::new(format!("#{}  {}{} · {} stitches", o.sequence, o.stitch_type, angle, group(o.stitches))).small();
                                if ui.selectable_label(on, text).on_hover_text("Click to pick this part: it shows on the canvas and its own settings open on the right").clicked() {
                                    focus = Some((l.id, o.sequence));
                                }
                                if l.opts.parts.iter().any(|p| p.index + 1 == o.label.rsplit(' ').next().and_then(|t| t.parse::<usize>().ok()).unwrap_or(0)) {
                                    ui.label(RichText::new("own settings").small().color(Color32::from_rgb(90, 150, 240)));
                                }
                                let _ = (many, k);
                            });
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
        if let Some((id, seq)) = focus {
            // pick the part: its layer is selected (its settings show) and the
            // part is highlighted; click it again to let go
            if self.focus_part == Some((id, seq)) {
                self.focus_part = None;
            } else {
                self.focus_part = Some((id, seq));
                self.selected = Some(id);
                self.multi = self.selected.into_iter().collect();
            }
        }
        if let Some((li, k)) = own_layer {
            // split the layer into its pieces, keep the others as they were
            // (each on its own layer) and select the one asked for
            self.selected = Some(self.layers[li].id);
            self.multi = self.selected.into_iter().collect();
            let before = self.layers.len();
            self.break_apart();
            let made = self.layers.len() + 1 - before;
            if made >= 2 {
                let j = li + k.min(made - 1);
                self.selected = Some(self.layers[j].id);
                self.multi = self.selected.into_iter().collect();
                self.focus_part = None;
                self.status = format!("{} is now its own layer: change its settings on the right", self.layers[j].name);
            }
        }
        if let Some(id) = select {
            if ui.input(|i| i.modifiers.shift) {
                if self.multi.contains(&id) && self.multi.len() > 1 {
                    self.multi.remove(&id);
                    if self.selected == Some(id) {
                        self.selected = self.multi.iter().next().copied();
                    }
                } else {
                    self.multi.insert(id);
                    self.selected = Some(id);
                }
            } else {
                self.focus_part = None;
                self.selected = if self.selected == Some(id) && self.multi.len() <= 1 {
                    None
                } else {
                    Some(id)
                };
                self.multi = self.selected.into_iter().collect();
            }
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
        let design_wt = self.recipe.weight;
        let mut open_guide = false;

        // A part picked in the layers list (one letter, one piece): its own
        // stitch type and angle, while the layer stays whole (text stays text).
        let lid = self.layers[li].id;
        let picked: Option<(usize, String, &'static str)> = self.focus_part.filter(|f| f.0 == lid).and_then(|(_, seq)| {
            let d = self.design.as_ref()?;
            let o = d.objects.iter().find(|o| o.layer == li && o.sequence == seq)?;
            let n = o.label.rsplit(' ').next().and_then(|t| t.parse::<usize>().ok()).unwrap_or(1);
            Some((n.saturating_sub(1), o.label.clone(), o.stitch_type))
        });
        if let Some((pi, label, kind)) = picked {
            section(ui, "This part");
            ui.label(RichText::new(format!("{label} · now {kind}")).weak().small());
            let l = &mut self.layers[li];
            let cur = l
                .opts
                .parts
                .iter()
                .find(|p| p.index == pi)
                .cloned()
                .unwrap_or(stitchforge::import::PartOpts { index: pi, ..Default::default() });
            let mut po = cur.clone();
            ui.horizontal(|ui| {
                ui.label("Type");
                let name = |c: StitchChoice| if c == StitchChoice::Auto { "Same as layer".to_string() } else { c.label().to_string() };
                egui::ComboBox::from_id_salt("part-stitch").selected_text(name(po.stitch)).show_ui(ui, |ui| {
                    for c in StitchChoice::ALL {
                        ui.selectable_value(&mut po.stitch, c, name(c));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Angle");
                let mut own = po.angle_deg.is_some();
                if ui.checkbox(&mut own, "own").on_hover_text("Give this part its own stitch angle").changed() {
                    po.angle_deg = if own { Some(l.opts.angle_deg.unwrap_or(45.0)) } else { None };
                }
                if let Some(a) = po.angle_deg.as_mut() {
                    ui.add(egui::DragValue::new(a).range(0.0..=180.0).speed(1.0).suffix("°"));
                    for v in [0.0, 45.0, 90.0, 135.0] {
                        if ui.small_button(format!("{v:.0}")).clicked() {
                            *a = v;
                        }
                    }
                }
            });
            let custom = po.stitch != StitchChoice::Auto || po.angle_deg.is_some();
            if custom && ui.small_button("Use the layer's settings").clicked() {
                po = stitchforge::import::PartOpts { index: pi, ..Default::default() };
            }
            if po != cur {
                l.opts.parts.retain(|p| p.index != pi);
                if po.stitch != StitchChoice::Auto || po.angle_deg.is_some() {
                    l.opts.parts.push(po);
                    l.opts.parts.sort_by_key(|p| p.index);
                }
                changed = true;
            }
        }

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
                // spacing: the text stays editable; spacing is applied by the
                // font engine every time it is laid out
                egui::Grid::new("text-spacing").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.label("Letter spacing");
                    ui.horizontal(|ui| {
                        text_changed |= ui
                            .add(egui::DragValue::new(&mut spec.letter_spacing).speed(0.02).range(-5.0..=20.0).suffix(" mm").max_decimals(2))
                            .on_hover_text("Space added between every letter (negative tightens)")
                            .changed();
                    });
                    ui.end_row();
                    ui.label("Word spacing");
                    text_changed |= ui
                        .add(egui::DragValue::new(&mut spec.word_spacing).speed(0.05).range(-5.0..=30.0).suffix(" mm").max_decimals(2))
                        .on_hover_text("Extra space at each space between words")
                        .changed();
                    ui.end_row();
                    ui.label("Line spacing");
                    text_changed |= ui
                        .add(egui::DragValue::new(&mut spec.line_spacing).speed(0.01).range(0.3..=4.0).suffix(" ×").max_decimals(2))
                        .on_hover_text("1 = the font's normal line height")
                        .changed();
                    ui.end_row();
                    ui.label("Align");
                    ui.horizontal(|ui| {
                        for (a, lbl) in [(import::TextAlign::Left, "Left"), (import::TextAlign::Center, "Center"), (import::TextAlign::Right, "Right")] {
                            if ui.selectable_label(spec.align == a, lbl).clicked() && spec.align != a {
                                spec.align = a;
                                text_changed = true;
                            }
                        }
                    });
                    ui.end_row();
                });
                // kerning, pair by pair
                let chars: Vec<char> = spec.text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                if chars.len() >= 2 {
                    let kerned = spec.kerning.iter().any(|k| k.abs() > 1e-6);
                    egui::CollapsingHeader::new(RichText::new(if kerned { "Kerning (letter by letter) •" } else { "Kerning (letter by letter)" }).small().strong())
                        .id_salt("kerning")
                        .show(ui, |ui| {
                            ui.label(RichText::new("Space before each letter, mm. Drag a number or type it.").weak().small());
                            spec.kerning.resize(chars.len(), 0.0);
                            // pairs within a line only
                            let mut pos = 0usize;
                            ui.horizontal_wrapped(|ui| {
                                for line in spec.text.lines() {
                                    let lc: Vec<char> = line.chars().collect();
                                    for k in 1..lc.len() {
                                        let i = pos + k;
                                        ui.label(RichText::new(format!("{}{}", lc[k - 1], lc[k])).monospace());
                                        if let Some(v) = spec.kerning.get_mut(i) {
                                            text_changed |= ui
                                                .add(egui::DragValue::new(v).speed(0.02).range(-5.0..=10.0).max_decimals(2))
                                                .changed();
                                        }
                                        ui.add_space(6.0);
                                    }
                                    pos += lc.len();
                                }
                            });
                            if kerned && ui.small_button("Reset kerning").clicked() {
                                spec.kerning.clear();
                                text_changed = true;
                            }
                        });
                }
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

            // The design object: its vector shape and its stitches, both kept
            // and both editable.
            section(ui, "Shape and stitches");
            let anchors: usize = l
                .shapes
                .iter()
                .map(|s| {
                    if s.curves.is_empty() {
                        0
                    } else {
                        s.curves.iter().map(|c| c.nodes.len()).sum()
                    }
                })
                .sum();
            ui.horizontal(|ui| {
                ui.label("Shape");
                ui.label(
                    RichText::new(if l.text.is_some() {
                        "live text".to_string()
                    } else if anchors > 0 {
                        format!("{anchors} anchors")
                    } else {
                        "outline".to_string()
                    })
                    .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Edit shape")
                        .on_hover_text("Move anchors and curve handles; stitches follow")
                        .clicked()
                    {
                        action = Some("edit-shape");
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Stitches");
                ui.label(
                    RichText::new(if l.embroidery.edited {
                        "edited by hand"
                    } else {
                        "made from the shape"
                    })
                    .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Edit stitches")
                        .on_hover_text("Move, add or delete needle points by hand")
                        .clicked()
                    {
                        action = Some("edit-stitches");
                    }
                    if l.embroidery.edited && !l.out_of_sync() && ui
                        .small_button("Regenerate")
                        .on_hover_text("Drop the hand edits and make stitches from the shape again")
                        .clicked()
                    {
                        action = Some("regen");
                    }
                });
            });
            if l.out_of_sync() {
                egui::Frame::NONE
                    .fill(Color32::from_rgb(250, 236, 205))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.label(RichText::new("⚠ The shape or stitch settings changed after these stitches were edited by hand.").color(Color32::from_rgb(120, 70, 0)));
                        ui.horizontal(|ui| {
                            if ui.button("Regenerate stitches").clicked() {
                                action = Some("regen");
                            }
                            if ui.button("Keep my stitches").clicked() {
                                action = Some("keep");
                            }
                        });
                    });
            }

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
                        let several = chart.first().map(|f| chart.iter().any(|t| t.brand != f.brand)).unwrap_or(false);
                        let mut last_brand = "";
                        for (i, t) in chart.iter().enumerate() {
                            if !ql.is_empty()
                                && !t.code.to_lowercase().contains(&ql)
                                && !t.name.to_lowercase().contains(&ql)
                                && !t.brand.to_lowercase().contains(&ql)
                            {
                                continue;
                            }
                            if several && t.brand != last_brand {
                                last_brand = &t.brand;
                                ui.add_space(4.0);
                                ui.label(RichText::new(&t.brand).small().strong().weak());
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

            ui.horizontal(|ui| {
                ui.label("Weight");
                let cur = l.opts.weight;
                let design = format!("Design ({})", design_wt.label());
                egui::ComboBox::from_id_salt("layer-weight")
                    .selected_text(cur.map(|w| w.label().to_string()).unwrap_or(design.clone()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(cur.is_none(), design).clicked() {
                            l.opts.weight = None;
                            changed = true;
                        }
                        for w in thread::ThreadWeight::ALL {
                            if ui
                                .selectable_label(cur == Some(w), format!("{} — {}", w.label(), w.use_for()))
                                .clicked()
                            {
                                l.opts.weight = Some(w);
                                changed = true;
                            }
                        }
                    });
                if ui.small_button("?").on_hover_text("Thread weight guide").clicked() {
                    open_guide = true;
                }
            });

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
            let is_outline_layer = l
                .shapes
                .iter()
                .all(|s| matches!(s.kind, ShapeKind::Line { .. }));
            let shaped_fill = l.opts.stitch == StitchChoice::Fill && l.opts.fill != FillPattern::Tatami;
            if !l.opts.guides.is_empty() && !is_outline_layer {
                ui.horizontal(|ui| {
                    ui.label("Angle");
                    ui.label(RichText::new(format!("follows {} direction line{}", l.opts.guides.len(), if l.opts.guides.len() == 1 { "" } else { "s" })).strong());
                    if ui.small_button("Edit").on_hover_text("Direction lines tool (A)").clicked() {
                        action = Some("guides");
                    }
                    if ui.small_button("Clear").on_hover_text("Back to one straight angle").clicked() {
                        l.opts.guides.clear();
                        changed = true;
                    }
                });
            } else if !is_outline_layer && !shaped_fill {
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
                if l.opts.angle_deg.is_none() && kinds_turning(&self.design, li) {
                    ui.label(RichText::new("Auto: satin turns with each stroke. Type a number for one fixed angle instead.").weak().small());
                } else if l.opts.angle_deg.is_none() && varies {
                    ui.label(RichText::new("Auto: angle varies per piece (each letter gets its own). Type a number to set one angle for all.").weak().small());
                }
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
            // 3D puff: satin over foam, with a stop to place the foam
            ui.horizontal(|ui| {
                ui.label("3D puff");
                let puff_label = if l.opts.puff { "On" } else { "Off" };
                if ui
                    .checkbox(&mut l.opts.puff, puff_label)
                    .on_hover_text("Raised satin sewn over foam: a contour first, a stop to place the foam, then tight satin with end caps. Sews after everything flat. Best for bold letters and shapes at least 3 mm wide.")
                    .changed()
                {
                    if l.opts.puff && l.opts.stitch == StitchChoice::Fill {
                        l.opts.stitch = StitchChoice::Satin;
                    }
                    changed = true;
                }
                if l.opts.puff {
                    ui.label("Foam");
                    let cur = l.opts.foam_mm.unwrap_or(3.0);
                    for f in [2.0, 3.0, 6.0] {
                        if ui
                            .selectable_label((cur - f).abs() < 0.01, format!("{f:.0} mm"))
                            .on_hover_text(if f >= 6.0 { "Tall look; needs bold, wide columns" } else { "Standard for caps" })
                            .clicked()
                        {
                            l.opts.foam_mm = if (f - 3.0).abs() < 0.01 { None } else { Some(f) };
                            changed = true;
                        }
                    }
                }
            });
            // twill / appliqué: a fabric piece instead of a fill
            let is_area = l.shapes.iter().any(|s| matches!(s.kind, ShapeKind::Area { .. }));
            if is_area {
                ui.horizontal(|ui| {
                    ui.label("Twill / appliqué");
                    let mut on = l.opts.applique.is_some();
                    let lbl = if on { "On" } else { "Off" };
                    if ui
                        .checkbox(&mut on, lbl)
                        .on_hover_text("Sew a fabric piece (tackle twill, felt…) instead of filling the shape: placement line, stop to lay the piece, tack-down, then a border over the edge. Cut files for the pieces come with the export.")
                        .changed()
                    {
                        l.opts.applique = if on {
                            let rgb = l.custom.as_ref().map(|t| t.rgb).or(l.shapes.first().map(|s| s.rgb)).unwrap_or([200, 30, 40]);
                            Some(stitchforge::applique::Applique { material_rgb: rgb, ..Default::default() })
                        } else {
                            None
                        };
                        if on {
                            l.opts.puff = false;
                        }
                        changed = true;
                    }
                });
            }
            if let Some(a) = l.opts.applique.as_mut() {
                use stitchforge::applique::{Cover, Method, Tack};
                let before = a.clone();
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Material");
                        ui.add(egui::TextEdit::singleline(&mut a.material).desired_width(90.0));
                        ui.color_edit_button_srgb(&mut a.material_rgb)
                            .on_hover_text("Colour of the twill or fabric (previews and the cut file name)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Pieces");
                        for m in [Method::PreCut, Method::CutInPlace] {
                            ui.selectable_value(&mut a.method, m, m.label()).on_hover_text(match m {
                                Method::PreCut => "Cut beforehand on a laser, plotter or die from the cut file; laid on the placement line",
                                Method::CutInPlace => "Lay a larger piece, the machine tacks it down and stops so you trim it, then sews the border",
                            });
                        }
                    });
                    ui.checkbox(&mut a.placement, "Placement line first (machine stops after it)");
                    ui.horizontal(|ui| {
                        ui.label("Tack-down");
                        egui::ComboBox::from_id_salt("twill-tack")
                            .selected_text(a.tack.label())
                            .show_ui(ui, |ui| {
                                for t in Tack::ALL {
                                    ui.selectable_value(&mut a.tack, t, t.label());
                                }
                            });
                        if matches!(a.tack, Tack::Zigzag | Tack::EStitch) {
                            ui.add(egui::DragValue::new(&mut a.tack_width).range(0.4..=4.0).speed(0.05).suffix(" mm wide"));
                        }
                        if a.tack != Tack::None {
                            ui.add(egui::DragValue::new(&mut a.tack_inset).range(0.0..=4.0).speed(0.05).suffix(" mm in"))
                                .on_hover_text("How far inside the piece's edge the tack-down runs (the border should cover it)");
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Border");
                        let mut c = a.cover;
                        egui::ComboBox::from_id_salt("twill-cover")
                            .selected_text(c.label())
                            .show_ui(ui, |ui| {
                                for k in Cover::ALL {
                                    ui.selectable_value(&mut c, k, k.label()).on_hover_text(k.hint());
                                }
                            });
                        if c != a.cover {
                            a.set_cover(c);
                        }
                    });
                    match a.cover {
                        Cover::Satin | Cover::Zigzag | Cover::EStitch => {
                            ui.horizontal(|ui| {
                                ui.label(if a.cover == Cover::EStitch { "Prong" } else { "Width" });
                                ui.add(egui::DragValue::new(&mut a.cover_width).range(0.5..=10.0).speed(0.05).suffix(" mm"));
                                ui.label(if a.cover == Cover::Satin { "Density" } else { "Spacing" });
                                ui.add(egui::DragValue::new(&mut a.cover_spacing).range(0.25..=6.0).speed(0.02).suffix(" mm"));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Over the edge");
                                ui.add(egui::DragValue::new(&mut a.cover_outside).range(-3.0..=3.0).speed(0.05).suffix(" mm"))
                                    .on_hover_text("How far the border reaches past the cut edge onto the garment (the rest lies on the piece)");
                                for (lbl, v) in [("◀ in", -0.1), ("out ▶", 0.1)] {
                                    if ui.small_button(lbl).clicked() {
                                        a.cover_outside = ((a.cover_outside + v) * 100.0).round() / 100.0;
                                    }
                                }
                            });
                        }
                        Cover::DoubleRun | Cover::Bean => {
                            ui.horizontal(|ui| {
                                ui.label("Inside the edge");
                                let mut v = -a.cover_outside;
                                if ui.add(egui::DragValue::new(&mut v).range(0.0..=4.0).speed(0.05).suffix(" mm")).changed() {
                                    a.cover_outside = -v;
                                }
                                ui.label("Stitch");
                                ui.add(egui::DragValue::new(&mut a.cover_spacing).range(1.0..=4.0).speed(0.05).suffix(" mm"));
                            });
                        }
                        Cover::None => {}
                    }
                    if a.method == Method::PreCut {
                        ui.horizontal(|ui| {
                            ui.label("Cut line");
                            ui.add(egui::DragValue::new(&mut a.cut_offset).range(-3.0..=3.0).speed(0.05).suffix(" mm"))
                                .on_hover_text("Cut the piece this much bigger (+) or smaller (−) than the art's outline; the placement line follows it");
                        });
                    }
                    ui.label(RichText::new("Cut files (SVG, DXF, PLT) for the pieces are written with the export.").weak().small());
                });
                if *a != before {
                    changed = true;
                }
            }
            ui.horizontal(|ui| {
                ui.label("Under layers above");
                let mut keep = l.opts.keep_under;
                ui.selectable_value(&mut keep, false, "Cut out")
                    .on_hover_text("Skip the parts covered by layers above (fewer stitches)");
                ui.selectable_value(&mut keep, true, "Keep fill")
                    .on_hover_text("Keep sewing the full shape underneath");
                if keep != l.opts.keep_under {
                    l.opts.keep_under = keep;
                    changed = true;
                }
            });
            let outline = l
                .shapes
                .iter()
                .any(|s| matches!(s.kind, ShapeKind::Line { .. }));
            let art_width = l.shapes.iter().find_map(|s| {
                if let ShapeKind::Line { width, .. } = s.kind {
                    Some(width * (l.sx * l.sy).abs().sqrt())
                } else {
                    None
                }
            });
            if outline {
                ui.horizontal(|ui| {
                    ui.label("Satin width");
                    let mut auto = l.opts.column_width.is_none();
                    if ui.checkbox(&mut auto, "From art").changed() {
                        l.opts.column_width = if auto {
                            None
                        } else {
                            Some(art_width.unwrap_or(3.0))
                        };
                        changed = true;
                    }
                    let shown = l.opts.column_width.or(art_width).unwrap_or(3.0);
                    if let Some(w) = num_field(ui, "col-width", shown, 1, "mm") {
                        if w > 0.2 {
                            l.opts.column_width = Some(w.min(12.0));
                            changed = true;
                        }
                    }
                });
                // where the satin sits on the art's line: 50/50, or shifted
                let w = l.opts.column_width.or(art_width).unwrap_or(3.0);
                let closed = l.shapes.iter().any(|s| match &s.kind {
                    ShapeKind::Line { paths, .. } => paths.iter().any(|p| p.len() > 2 && p[0].dist(*p.last().unwrap()) < 0.05),
                    _ => false,
                });
                let (inner, outer) = if closed { ("Inside", "Outside") } else { ("Left", "Right") };
                ui.horizontal(|ui| {
                    ui.label("Satin position").on_hover_text(
                        "Where the satin column sits on the line: centred (half on each side), or moved toward one side",
                    );
                    let off = l.opts.satin_offset;
                    if ui.small_button("◀").on_hover_text(format!("Move 0.1 mm {}", inner.to_lowercase())).clicked() {
                        l.opts.satin_offset = ((off - 0.1) * 10.0).round() / 10.0;
                        changed = true;
                    }
                    if let Some(v) = num_field(ui, "satin-off", off, 1, "mm") {
                        l.opts.satin_offset = v;
                        changed = true;
                    }
                    if ui.small_button("▶").on_hover_text(format!("Move 0.1 mm {}", outer.to_lowercase())).clicked() {
                        l.opts.satin_offset = ((off + 0.1) * 10.0).round() / 10.0;
                        changed = true;
                    }
                    let pct = (50.0 + 50.0 * off / (w / 2.0)).clamp(0.0, 100.0);
                    ui.label(RichText::new(format!("{:.0}/{:.0} {}/{}", 100.0 - pct, pct, inner.to_lowercase(), outer.to_lowercase())).weak().small());
                });
                ui.horizontal(|ui| {
                    ui.add_space(96.0);
                    for (label, v, tip) in [
                        (format!("{inner} edge"), -w / 2.0, format!("The whole column {} the line", if closed { "inside" } else { "left of" })),
                        ("Centred 50/50".to_string(), 0.0, "Half the column on each side of the line".to_string()),
                        (format!("{outer} edge"), w / 2.0, format!("The whole column {} the line", if closed { "outside" } else { "right of" })),
                    ] {
                        if ui
                            .selectable_label((l.opts.satin_offset - v).abs() < 0.005, label)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            l.opts.satin_offset = (v * 100.0).round() / 100.0;
                            changed = true;
                        }
                    }
                });
                l.opts.satin_offset = l.opts.satin_offset.clamp(-w, w);
                ui.horizontal(|ui| {
                    ui.label("Corners");
                    egui::ComboBox::from_id_salt("corner")
                        .selected_text(l.opts.corner.label())
                        .show_ui(ui, |ui| {
                            for c in Corner::ALL {
                                if ui
                                    .selectable_value(&mut l.opts.corner, c, c.label())
                                    .clicked()
                                {
                                    changed = true;
                                }
                            }
                        });
                    ui.label("Ends");
                    egui::ComboBox::from_id_salt("cap")
                        .selected_text(l.opts.cap.label())
                        .show_ui(ui, |ui| {
                            for c in Cap::ALL {
                                if ui.selectable_value(&mut l.opts.cap, c, c.label()).clicked() {
                                    changed = true;
                                }
                            }
                        });
                });
            } else if l.opts.stitch != StitchChoice::Fill {
                ui.horizontal(|ui| {
                    ui.label("Satin widen");
                    if let Some(w) = num_field(ui, "widen", l.opts.widen, 2, "mm") {
                        l.opts.widen = w.clamp(-2.0, 3.0);
                        changed = true;
                    }
                    ui.label(RichText::new("+ thicker, − thinner").weak().small());
                });
            }
            // ---- fill pattern and this layer's own stitch settings
            let kinds: Vec<&str> = self
                .design
                .as_ref()
                .map(|d| {
                    d.objects
                        .iter()
                        .filter(|o| o.layer == li)
                        .map(|o| o.stitch_type)
                        .collect()
                })
                .unwrap_or_default();
            let is_fill = !is_outline_layer
                && (l.opts.stitch == StitchChoice::Fill || kinds.iter().any(|k| k.ends_with("fill")));
            let is_satin = l.opts.stitch == StitchChoice::Satin
                || kinds.iter().any(|k| k.to_lowercase().contains("satin"));
            if is_fill {
                ui.horizontal(|ui| {
                    ui.label("Fill pattern");
                    egui::ComboBox::from_id_salt("fill-pattern")
                        .width(210.0)
                        .selected_text(l.opts.fill.label())
                        .show_ui(ui, |ui| {
                            for f in FillPattern::ALL {
                                if ui.selectable_value(&mut l.opts.fill, f, f.label()).clicked() {
                                    if l.opts.stitch == StitchChoice::Auto {
                                        l.opts.stitch = StitchChoice::Fill;
                                    }
                                    changed = true;
                                }
                            }
                        });
                });
            }
            // underlay and the satin / fill specifics
            let underlay_combo = |ui: &mut egui::Ui, salt: &str, label: &str, v: &mut Underlay, list: &[Underlay], no: &mut bool| -> bool {
                let mut ch = false;
                ui.horizontal(|ui| {
                    ui.label(label);
                    let shown = if *no { Underlay::Off } else { *v };
                    egui::ComboBox::from_id_salt(salt)
                        .width(170.0)
                        .selected_text(shown.label())
                        .show_ui(ui, |ui| {
                            for u in list {
                                if ui.selectable_label(shown == *u, u.label()).on_hover_text(u.hint()).clicked() {
                                    *v = *u;
                                    *no = false;
                                    ch = true;
                                }
                            }
                        })
                        .response
                        .on_hover_text(shown.hint());
                });
                ch
            };
            if is_satin || l.opts.puff {
                let o = &mut l.opts;
                changed |= underlay_combo(ui, "satin-ul", "Satin underlay", &mut o.satin_underlay, &Underlay::SATIN, &mut o.no_underlay);
                ui.horizontal(|ui| {
                    let mut short = o.short_stitches.unwrap_or(true);
                    if ui
                        .checkbox(&mut short, "Short stitches on curves")
                        .on_hover_text("Every other stitch stops short on the inside of tight curves so needle holes don't pile up")
                        .changed()
                    {
                        o.short_stitches = if short { None } else { Some(false) };
                        changed = true;
                    }
                });
                let def_split = if o.puff { 12.0 } else { 8.0 };
                changed |= own_setting(ui, "Split satin over", "own-split", &mut o.split_width, def_split, 1, "mm", (3.0, 12.5));
            }
            if is_fill && !l.opts.puff {
                let o = &mut l.opts;
                changed |= underlay_combo(ui, "fill-ul", "Fill underlay", &mut o.fill_underlay, &Underlay::FILL, &mut o.no_underlay);
            }
            // what the underlay actually is, with its numbers
            if let Some(d) = &self.design {
                let objs: Vec<&stitchforge::design::ObjectInfo> = d.objects.iter().filter(|o| o.layer == li).collect();
                let mut kinds: Vec<&str> = objs.iter().map(|o| o.underlay.as_str()).filter(|u| !u.is_empty()).collect();
                kinds.dedup();
                let n: usize = objs.iter().map(|o| o.underlay_stitches).sum();
                let total: usize = objs.iter().map(|o| o.stitches).sum();
                if !kinds.is_empty() && (is_fill || is_satin) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Underlay:").small());
                        let shown = if kinds.len() > 2 { format!("{} (+{} more)", kinds[..2].join("; "), kinds.len() - 2) } else { kinds.join("; ") };
                        ui.label(RichText::new(shown).small().strong());
                        if n > 0 {
                            ui.label(RichText::new(format!("· {} of {} stitches ({:.0}%)", group(n), group(total), 100.0 * n as f64 / total.max(1) as f64)).small().weak());
                        }
                    });
                }
            }
            if is_fill || is_satin {
                let rec = &self.recipe;
                let own = l.opts.has_own_settings();
                let head = if own {
                    "Stitch settings (this layer) •"
                } else {
                    "Stitch settings (this layer)"
                };
                egui::CollapsingHeader::new(RichText::new(head).small().strong())
                    .id_salt("own-settings")
                    .default_open(own)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Blank = the fabric's setting. Type a value to change it for this layer only; ↺ goes back.")
                                .weak()
                                .small(),
                        );
                        let o = &mut l.opts;
                        if is_fill {
                            changed |= own_setting(ui, "Row spacing", "own-dens", &mut o.density, rec.fill_spacing, 2, "mm", (0.15, 3.0));
                            changed |= own_setting(ui, "Stitch length", "own-len", &mut o.stitch_len, rec.fill_len, 1, "mm", (0.8, 12.0));
                        } else {
                            changed |= own_setting(ui, "Satin spacing", "own-dens", &mut o.density, rec.satin_spacing, 2, "mm", (0.15, 3.0));
                        }
                        changed |= own_setting(ui, "Pull comp.", "own-pc", &mut o.pull_comp, rec.pull_comp, 2, "mm", (-0.5, 2.0));
                        ui.label(RichText::new("UNDERLAY").small().strong());
                        changed |= own_setting(ui, if is_fill { "Underlay rows" } else { "Zigzag spacing" }, "own-us", &mut o.underlay_spacing, rec.underlay_spacing, 1, "mm", (0.5, 8.0));
                        changed |= own_setting(ui, "Underlay inset", "own-ui", &mut o.underlay_inset, rec.underlay_inset, 2, "mm", (0.0, 3.0));
                        changed |= own_setting(ui, "Underlay stitch", "own-ul", &mut o.underlay_len, rec.underlay_len, 1, "mm", (1.0, 7.0));
                        if is_fill && matches!(o.fill, FillPattern::Tatami | FillPattern::Radial) {
                            ui.horizontal(|ui| {
                                ui.label("Needle pattern");
                                let cur = o.stagger.unwrap_or(3);
                                for (n, lbl) in [(2u8, "½"), (3, "⅓"), (4, "¼"), (5, "⅕")] {
                                    if ui
                                        .selectable_label(cur == n, lbl)
                                        .on_hover_text(format!("Penetrations shift by 1/{n} of a stitch each row"))
                                        .clicked()
                                    {
                                        o.stagger = if n == 3 { None } else { Some(n) };
                                        changed = true;
                                    }
                                }
                            });
                        }
                        if o.has_own_settings() && ui.small_button("All back to fabric settings").clicked() {
                            o.satin_underlay = Underlay::Auto;
                            o.fill_underlay = Underlay::Auto;
                            o.split_width = None;
                            o.short_stitches = None;
                            o.density = None;
                            o.stitch_len = None;
                            o.pull_comp = None;
                            o.underlay_spacing = None;
                            o.underlay_inset = None;
                            o.underlay_len = None;
                            o.stagger = None;
                            changed = true;
                        }
                    });
            }
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
        if open_guide {
            self.show_weights = true;
        }
        if let Some(a) = action {
            self.layer_action(a, li);
        }
    }

    // ---------- name drops ----------

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = None;
        self.moving = None;
        if tab == Tab::Drops {
            let d = &mut self.drops;
            let name_ok = d
                .name_layer
                .map(|id| self.layers.iter().any(|l| l.id == id && l.text.is_some()))
                .unwrap_or(false);
            if !name_ok {
                d.name_layer = self
                    .layers
                    .iter()
                    .rev()
                    .find(|l| l.text.is_some())
                    .map(|l| l.id);
            }
            d.logo_layers
                .retain(|id| self.layers.iter().any(|l| l.id == *id));
            if d.logo_layers.is_empty() {
                d.logo_layers = self
                    .layers
                    .iter()
                    .filter(|l| Some(l.id) != d.name_layer && l.visible)
                    .map(|l| l.id)
                    .collect();
            }
        }
        self.fit = true;
        self.dirty = true;
        self.quiet_regen = true;
    }

    fn pick_names_file(&mut self) {
        pick(
            Want::Names,
            "Names: Excel or CSV",
            &[("Spreadsheet", &["xlsx", "xlsm", "xls", "ods", "csv", "tsv", "txt"])],
            self.last_dir.as_ref(),
        );
    }

    fn load_names_path(&mut self, path: PathBuf) {
        match drops_mod::read_table(&path) {
            Ok(t) => {
                // Guess the setup from the header row.
                let first = t.raw.first().cloned().unwrap_or_default();
                let looks_header =
                    !first.is_empty() && first.iter().all(|c| c.trim().parse::<f64>().is_err());
                let low: Vec<String> = first.iter().map(|c| c.to_lowercase()).collect();
                let mut cols: Vec<usize> = (0..low.len())
                    .filter(|i| low[*i].contains("name"))
                    .collect();
                if cols.is_empty() || !looks_header {
                    cols = vec![0];
                }
                let qty = if looks_header {
                    (0..low.len()).find(|i| low[*i].contains("qty") || low[*i].contains("quantity"))
                } else {
                    None
                };
                let d = &mut self.drops;
                d.first_row_headers = looks_header;
                d.columns = cols;
                d.qty_column = qty;
                d.edits.clear();
                self.status = format!(
                    "Loaded {} rows from {}",
                    t.data_rows(looks_header).len(),
                    t.source
                );
                d.table = Some(t);
                self.drop_view = 0;
                self.drop_sel = 0;
                self.last_dir = path.parent().map(|p| p.to_path_buf());
                self.dirty = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn export_drops_to(&mut self, dir: &std::path::Path, fmts: &[Format]) {
        let proj = self.to_project();
        let generated = stamp();
        let res = {
            let opt = self.fonts_db();
            drops_mod::export_drops(&proj, opt, dir, fmts, &generated)
        };
        match res {
            Ok(ex) => {
                self.status = format!(
                    "Wrote {} drop file(s) and the run sheet to {}",
                    ex.files.len() / fmts.len().max(1),
                    dir.display()
                );
                deliver(dir, &format!("{}-name-drops", proj.name), &ex.sheet, &ex.sheet);
                self.last_dir = Some(dir.to_path_buf());
                self.show_export = false;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Left panel in the Name drops tab: the drop shown as its layers.
    fn drops_panel(&mut self, ui: &mut egui::Ui) {
        let n = self.drops.drop_count();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Name drops").strong());
            if n > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(self.drop_view + 1 < n, egui::Button::new("▶"))
                        .clicked()
                    {
                        self.drop_view += 1;
                        self.drop_sel = 0;
                        self.dirty = true;
                        self.quiet_regen = true;
                    }
                    ui.label(format!("Drop {} of {}", self.drop_view + 1, n));
                    if ui
                        .add_enabled(self.drop_view > 0, egui::Button::new("◀"))
                        .clicked()
                    {
                        self.drop_view -= 1;
                        self.drop_sel = 0;
                        self.dirty = true;
                        self.quiet_regen = true;
                    }
                });
            }
        });
        ui.label(RichText::new("Each drop is one machine file: the logo on all heads, then a stop before every name. Click a name to see it.").weak().small());
        ui.separator();
        if let Some(p) = self.drops.problem(&self.layers) {
            ui.label(
                RichText::new(format!("To start: {p}")).color(Color32::from_rgb(196, 130, 20)),
            );
            return;
        }
        let (Some(d), Some((n_logo, slots))) = (&self.design, &self.drop_info) else {
            return;
        };
        let plan = drops_mod::stop_plan(d, *n_logo, slots);
        let logo_names: Vec<String> = self
            .layers
            .iter()
            .filter(|l| self.drops.logo_layers.contains(&l.id))
            .map(|l| l.name.clone())
            .collect();
        let mut pick: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if *n_logo > 0 {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Logo").strong());
                        ui.label(RichText::new("· all heads").weak());
                    });
                    ui.indent("logo", |ui| {
                        for nm in &logo_names {
                            ui.label(RichText::new(nm).small());
                        }
                    });
                    ui.add_space(4.0);
                } else {
                    ui.label(
                        RichText::new("No logo in this drop (logo is set to drop 1 only)")
                            .weak()
                            .small(),
                    );
                }
                for (stop, thread, what, heads) in plan.iter().filter(|r| r.2.starts_with("Name")) {
                    let k = slots
                        .iter()
                        .position(|s| what.starts_with(&format!("Name {}:", s.index + 1)))
                        .unwrap_or(0);
                    let sel = k == self.drop_sel;
                    ui.horizontal(|ui| {
                        let chip = egui::Button::new(
                            RichText::new("STOP").size(10.5).color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(200, 60, 50))
                        .corner_radius(9.0)
                        .min_size(Vec2::new(44.0, 18.0));
                        ui.add(chip).on_hover_text(format!(
                            "Stop {stop}: turn on {}",
                            heads.to_lowercase()
                        ));
                        let label = format!(
                            "{}  ·  {}",
                            what.split_once(": ").map(|x| x.1).unwrap_or(what),
                            heads
                        );
                        if ui
                            .selectable_label(sel, label)
                            .on_hover_text(thread.as_str())
                            .clicked()
                        {
                            pick = Some(k);
                        }
                    });
                }
            });
        if let Some(k) = pick {
            self.drop_sel = k;
        }
    }

    /// Right panel in the Name drops tab: data, columns, layers, machine.
    fn drops_setup(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        section(ui, "1 · Names");
        ui.horizontal(|ui| {
            if ui.button("Load Excel / CSV…").clicked() {
                self.pick_names_file();
            }
            if let Some(t) = &self.drops.table {
                ui.label(
                    RichText::new(format!(
                        "{} · {} rows",
                        t.source,
                        t.data_rows(self.drops.first_row_headers).len()
                    ))
                    .weak(),
                );
            }
        });
        if self.drops.table.is_some() {
            let d = &mut self.drops;
            changed |= ui
                .checkbox(&mut d.first_row_headers, "First row is column titles")
                .changed();
            let headers = d.table.as_ref().unwrap().headers(d.first_row_headers);
            ui.label("Name columns (combined in this order)");
            ui.horizontal_wrapped(|ui| {
                for (i, h) in headers.iter().enumerate() {
                    let mut on = d.columns.contains(&i);
                    if ui.checkbox(&mut on, h).changed() {
                        if on {
                            d.columns.push(i);
                        } else {
                            d.columns.retain(|c| *c != i);
                        }
                        d.edits.clear();
                        changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Join");
                changed |= ui
                    .selectable_value(&mut d.join, Join::Space, "Same line")
                    .changed();
                changed |= ui
                    .selectable_value(&mut d.join, Join::NewLine, "Separate lines")
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Letters");
                changed |= ui
                    .selectable_value(&mut d.case, Case::AsTyped, "As typed")
                    .changed();
                changed |= ui
                    .selectable_value(&mut d.case, Case::Upper, "UPPER")
                    .changed();
                changed |= ui
                    .selectable_value(&mut d.case, Case::Title, "Title Case")
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Quantity column");
                let shown = d
                    .qty_column
                    .and_then(|c| headers.get(c).cloned())
                    .unwrap_or("None (1 each)".into());
                egui::ComboBox::from_id_salt("qty")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(d.qty_column.is_none(), "None (1 each)")
                            .clicked()
                        {
                            d.qty_column = None;
                            changed = true;
                        }
                        for (i, h) in headers.iter().enumerate() {
                            if ui.selectable_label(d.qty_column == Some(i), h).clicked() {
                                d.qty_column = Some(i);
                                changed = true;
                            }
                        }
                    });
            });
            changed |= ui.checkbox(&mut d.skip_blank, "Skip blank rows").changed();
            ui.horizontal(|ui| {
                ui.label("Order");
                changed |= ui.selectable_value(&mut d.order, drops_mod::Order::AsListed, "As in sheet").changed();
                changed |= ui.selectable_value(&mut d.order, drops_mod::Order::AToZ, "A–Z").changed();
                changed |= ui.selectable_value(&mut d.order, drops_mod::Order::LastName, "By last name").changed();
            });
            ui.horizontal(|ui| {
                ui.label("Group by").on_hover_text("Keep names of one garment, size or team together");
                let shown = d
                    .group_column
                    .and_then(|c| headers.get(c).cloned())
                    .unwrap_or("No grouping".into());
                egui::ComboBox::from_id_salt("group-col")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(d.group_column.is_none(), "No grouping").clicked() {
                            d.group_column = None;
                            changed = true;
                        }
                        for (i, h) in headers.iter().enumerate() {
                            if ui.selectable_label(d.group_column == Some(i), h).clicked() {
                                d.group_column = Some(i);
                                changed = true;
                            }
                        }
                    });
            });
            if d.group_column.is_some() {
                changed |= ui
                    .checkbox(&mut d.group_new_drop, "Each group starts a new drop")
                    .on_hover_text("A drop never mixes groups, so one hooping run is one garment or size")
                    .changed();
            }
        }

        section(ui, "2 · Layers");
        {
            let d = &mut self.drops;
            let text_layers: Vec<(u64, String)> = self
                .layers
                .iter()
                .filter(|l| l.text.is_some())
                .map(|l| (l.id, l.name.clone()))
                .collect();
            ui.horizontal(|ui| {
                ui.label("Name layer");
                let shown = d
                    .name_layer
                    .and_then(|id| text_layers.iter().find(|t| t.0 == id))
                    .map(|t| t.1.clone())
                    .unwrap_or("Pick a text layer".into());
                egui::ComboBox::from_id_salt("name-layer")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, nm) in &text_layers {
                            if ui.selectable_label(d.name_layer == Some(*id), nm).clicked() {
                                d.name_layer = Some(*id);
                                d.logo_layers.retain(|l| l != id);
                                changed = true;
                            }
                        }
                    });
            });
            if text_layers.is_empty() {
                ui.label(RichText::new("Add a text layer in the Design tab (Add text…) and place it where names go. Its font, size, color and stitching are used for every name.").weak().small());
            }
            ui.horizontal(|ui| {
                ui.label("Logo layers (all heads)");
                if ui.small_button("All").clicked() {
                    d.logo_layers = self
                        .layers
                        .iter()
                        .filter(|l| Some(l.id) != d.name_layer)
                        .map(|l| l.id)
                        .collect();
                    changed = true;
                }
                if ui.small_button("None").clicked() {
                    d.logo_layers.clear();
                    changed = true;
                }
            });
            ui.indent("logo-layers", |ui| {
                for l in self.layers.iter().filter(|l| Some(l.id) != d.name_layer) {
                    let mut on = d.logo_layers.contains(&l.id);
                    if ui.checkbox(&mut on, &l.name).changed() {
                        if on {
                            d.logo_layers.push(l.id);
                        } else {
                            d.logo_layers.retain(|x| *x != l.id);
                        }
                        changed = true;
                    }
                }
            });
        }

        section(ui, "3 · Machine");
        {
            let d = &mut self.drops;
            ui.horizontal(|ui| {
                ui.label("Heads");
                if let Some(h) = num_field(ui, "heads", d.heads as f64, 0, "") {
                    d.heads = (h.round() as usize).clamp(1, 60);
                    d.per_drop = d.per_drop.clamp(1, d.heads * 2);
                    changed = true;
                }
                ui.label("Names per drop");
                if let Some(n) = num_field(ui, "per-drop", d.per_drop as f64, 0, "") {
                    d.per_drop = (n.round() as usize).clamp(1, d.heads * 2);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button(format!("= heads ({})", d.heads)).clicked() {
                    d.per_drop = d.heads;
                    changed = true;
                }
                if ui
                    .small_button(format!("= 2 × heads ({})", d.heads * 2))
                    .clicked()
                {
                    d.per_drop = d.heads * 2;
                    changed = true;
                }
            });
            if d.per_drop > d.heads {
                ui.label(
                    RichText::new(format!(
                        "Names {}–{} of each drop are a second pass on heads 1–{}.",
                        d.heads + 1,
                        d.per_drop,
                        d.per_drop - d.heads
                    ))
                    .weak()
                    .small(),
                );
            }
            changed |= ui
                .checkbox(&mut d.logo_every_drop, "Sew the logo in every drop")
                .changed();
            ui.horizontal(|ui| {
                let mut on = d.max_width_mm.is_some();
                if ui.checkbox(&mut on, "Names no wider than").changed() {
                    d.max_width_mm = if on { Some(76.2) } else { None };
                    changed = true;
                }
                if let Some(w) = d.max_width_mm {
                    if let Some(v) = num_field(ui, "max-w", w / 25.4, 2, "in") {
                        d.max_width_mm = Some(v.max(0.3) * 25.4);
                        changed = true;
                    }
                }
            });
            if d.max_width_mm.is_some() {
                ui.horizontal(|ui| {
                    ui.label("Long names");
                    changed |= ui
                        .selectable_value(&mut d.fit, drops_mod::Fit::Condense, "Narrow, then shrink")
                        .on_hover_text("Letters get narrower first so the name keeps its height; only very long names also get smaller")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut d.fit, drops_mod::Fit::Shrink, "Shrink")
                        .on_hover_text("The whole name gets smaller, keeping its proportions")
                        .changed();
                });
                if d.fit == drops_mod::Fit::Condense {
                    ui.horizontal(|ui| {
                        ui.label("Narrowest letters");
                        if let Some(v) = num_field(ui, "min-cond", d.min_condense * 100.0, 0, "%") {
                            d.min_condense = (v / 100.0).clamp(0.5, 1.0);
                            changed = true;
                        }
                        ui.label(RichText::new("of normal width (80% reads well)").weak().small());
                    });
                }
            }
        }

        let names = self.drops.names();
        if !names.is_empty() {
            section(
                ui,
                &format!(
                    "4 · {} names in {} drop file(s)",
                    names.len(),
                    self.drops.drop_count()
                ),
            );
            ui.label(
                RichText::new("Fix a name by typing over it. Click a row to show its drop.")
                    .weak()
                    .small(),
            );
            // how names fit, and which are too small for the thread
            let weight = self
                .drops
                .name_layer
                .and_then(|id| self.layers.iter().find(|l| l.id == id))
                .and_then(|l| l.opts.weight)
                .unwrap_or(self.recipe.weight);
            let min_h = weight.min_text_height();
            let fit: std::collections::HashMap<usize, drops_mod::FitInfo> = self.drop_fit.iter().copied().collect();
            let narrowed = names.iter().filter(|n| fit.get(&n.index).map(|f| f.sx < 0.999 && f.sy >= 0.999).unwrap_or(false)).count();
            let shrunk = names.iter().filter(|n| fit.get(&n.index).map(|f| f.sy < 0.999).unwrap_or(false)).count();
            let small = names.iter().filter(|n| fit.get(&n.index).map(|f| f.height_mm < min_h - 0.05).unwrap_or(false)).count();
            let dups = names.iter().filter(|n| n.duplicate).count();
            let mut bits = Vec::new();
            if narrowed > 0 { bits.push(format!("{narrowed} narrowed")); }
            if shrunk > 0 { bits.push(format!("{shrunk} shrunk")); }
            if dups > 0 { bits.push(format!("{dups} repeated on another row")); }
            if !bits.is_empty() {
                ui.label(RichText::new(bits.join(" · ")).small().color(Color32::from_rgb(196, 130, 20)));
            }
            if small > 0 {
                ui.label(RichText::new(format!("{small} name{} under {:.1} mm tall: too small to read in {} thread. Allow a wider name, use a thinner thread or split onto two lines.", if small == 1 { " is" } else { "s are" }, min_h, weight.label())).small().color(Color32::from_rgb(200, 60, 40)));
            }
            let mut edit: Option<(usize, Option<String>)> = None;
            let mut show: Option<(usize, usize)> = None;
            let groups = if self.drops.group_column.is_some() {
                names.iter().map(|n| n.group.as_str()).collect::<std::collections::BTreeSet<_>>().len()
            } else {
                0
            };
            let list_h = (((names.len() + groups) as f32) * 25.0).min(300.0);
            egui::ScrollArea::vertical()
                .id_salt("names")
                .max_height(list_h)
                .min_scrolled_height(list_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut last_group: Option<&str> = None;
                    for n in &names {
                        if self.drops.group_column.is_some() && last_group != Some(n.group.as_str()) {
                            ui.label(RichText::new(if n.group.is_empty() { "(blank)".to_string() } else { n.group.clone() }).small().strong());
                            last_group = Some(n.group.as_str());
                        }
                        let here = n.drop == self.drop_view
                            && Some(n.index)
                                == self
                                    .drop_info
                                    .as_ref()
                                    .and_then(|(_, sl)| sl.get(self.drop_sel))
                                    .map(|s| s.index);
                        ui.horizontal(|ui| {
                            let tag = format!(
                                "{:>2}  D{} H{}{}",
                                n.index + 1,
                                n.drop + 1,
                                n.head,
                                if n.pass > 1 { " (2nd)" } else { "" }
                            );
                            if ui
                                .add_sized(
                                    [86.0, 18.0],
                                    egui::Button::selectable(here, RichText::new(tag).small()),
                                )
                                .on_hover_text(format!(
                                    "Sheet row {} · click to show this name",
                                    n.row
                                ))
                                .clicked()
                            {
                                show = Some((n.drop, n.pos));
                            }
                            if let Some(v) = text_field(
                                ui,
                                &format!("nm-{}", n.key),
                                &n.text.replace('\n', " / "),
                                170.0,
                            ) {
                                edit = Some((n.key, Some(v.replace(" / ", "\n"))));
                            }
                            if n.edited
                                && ui
                                    .small_button("↺")
                                    .on_hover_text("Undo your fix")
                                    .clicked()
                            {
                                edit = Some((n.key, None));
                            }
                            if let Some(f) = fit.get(&n.index) {
                                let warn = Color32::from_rgb(196, 130, 20);
                                if f.height_mm < min_h - 0.05 {
                                    ui.label(RichText::new(format!("{:.1} mm", f.height_mm)).small().color(Color32::from_rgb(200, 60, 40)))
                                        .on_hover_text(format!("Letters {:.1} mm tall: under the {:.1} mm minimum for {}", f.height_mm, min_h, weight.label()));
                                } else if f.sy < 0.999 {
                                    ui.label(RichText::new(format!("{:.0}%", f.sy * 100.0)).small().color(warn))
                                        .on_hover_text(format!("Shrunk to {:.0}% to fit ({:.1} mm tall)", f.sy * 100.0, f.height_mm));
                                } else if f.sx < 0.999 {
                                    ui.label(RichText::new(format!("{:.0}% w", f.sx * 100.0)).small().color(warn))
                                        .on_hover_text(format!("Letters narrowed to {:.0}% to fit; full height kept", f.sx * 100.0));
                                }
                            }
                            if n.duplicate {
                                ui.label(RichText::new("2×").small().color(Color32::from_rgb(196, 130, 20)))
                                    .on_hover_text("This name is also on another row of the sheet: check it isn't a double entry");
                            }
                        });
                    }
                });
            if let Some((i, v)) = edit {
                match v {
                    Some(t) => {
                        self.drops.edits.insert(i, t);
                    }
                    None => {
                        self.drops.edits.remove(&i);
                    }
                }
                changed = true;
            }
            if let Some((drop, pos)) = show {
                self.drop_view = drop;
                self.drop_sel = pos;
                self.dirty = true;
                self.quiet_regen = true;
            }
            ui.add_space(6.0);
            if ui
                .add_enabled(
                    self.design.is_some(),
                    egui::Button::new(RichText::new("Export all drops…").strong()),
                )
                .on_hover_text("Machine files for every drop + printable run sheet")
                .clicked()
            {
                self.show_export = true;
            }
        }
        if changed {
            self.dirty = true;
            self.modified = true;
            self.quiet_regen = true;
        }
    }

    fn layer_action(&mut self, a: &str, li: usize) {
        match a {
            "edit-stitches" => return self.start_stitch_edit(li),
            "edit-shape" => return self.start_shape_edit(li),
            "guides" => {
                self.selected = Some(self.layers[li].id);
                return self.set_tool(Tool::Guides);
            }
            _ => {}
        }
        self.hard_checkpoint();
        match a {
            "regen" => {
                self.layers[li].embroidery.edited = false;
                self.status = "Stitches regenerated from the shape".into();
            }
            "keep" => {
                let l = &mut self.layers[li];
                l.embroidery.source = l.source_fingerprint();
                self.status = "Keeping your hand-edited stitches".into();
            }
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

    fn props_selection(&mut self, ui: &mut egui::Ui, now: f64) {
                self.edit_panel(ui);
                if self.tool.draws() && self.machine.is_none() {
                    self.draw_options(ui);
                    ui.separator();
                }
                let n_sel = self.sel_layers().len();
                if n_sel >= 2 && self.machine.is_none() {
                    section(ui, &format!("{n_sel} layers selected"));
                    self.path_tools(ui, false);
                    ui.add_space(4.0);
                    ui.separator();
                } else if n_sel == 1 && self.machine.is_none() {
                    section(ui, "Shape tools");
                    self.path_tools(ui, false);
                    ui.add_space(4.0);
                }
                self.layer_props(ui, now);
    }

    /// The design itself: size, garment, fabric, thread, order, stitch settings.
    fn props_document(&mut self, ui: &mut egui::Ui) {
            if self.machine.is_none() && self.tab == Tab::Design {
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
                    for (target, l) in [(1.0, "1\""), (2.0, "2\""), (3.5, "3.5\" chest")] {
                        if ui.small_button(l).clicked() && b.width() > 0.0 {
                            self.hard_checkpoint();
                            scale_layers(&mut self.layers, b.center(), target * 25.4 / b.width());
                            self.dirty = true;
                        }
                    }
                    // largest size that fits the cap front (adult 4 × 2.25 in,
                    // youth 4 × 2 in), keeping proportions
                    let (cw, ch) = self.recipe.cap_size.limit();
                    let cap_lbl = match self.recipe.cap_size {
                        settings::CapSize::Adult => "Fit cap",
                        settings::CapSize::Youth => "Fit youth cap",
                    };
                    if ui
                        .small_button(cap_lbl)
                        .on_hover_text(format!("As big as fits a {} front", self.recipe.cap_size.label()))
                        .clicked()
                        && b.width() > 0.0
                        && b.height() > 0.0
                    {
                        self.hard_checkpoint();
                        let k = (cw / b.width()).min(ch / b.height());
                        scale_layers(&mut self.layers, b.center(), k);
                        self.dirty = true;
                    }
                });
            }

            }
            section(ui, "Sewing on");
            ui.horizontal(|ui| {
                let before = self.profile;
                ui.selectable_value(&mut self.profile, Profile::Flat, "Flat goods");
                ui.selectable_value(&mut self.profile, Profile::Cap, "Cap (center-out)");
                if self.profile == Profile::Cap {
                    let before = self.recipe.cap_size;
                    egui::ComboBox::from_id_salt("cap-size")
                        .selected_text(self.recipe.cap_size.label())
                        .show_ui(ui, |ui| {
                            for c in [settings::CapSize::Adult, settings::CapSize::Youth] {
                                ui.selectable_value(&mut self.recipe.cap_size, c, c.label());
                            }
                        });
                    if self.recipe.cap_size != before {
                        self.dirty = true;
                    }
                }
                if self.profile != before {
                    self.fabric = self.profile.default_recipe().into();
                    self.recipe = {
                        let (w, cs) = (self.recipe.weight, self.recipe.cap_size);
                        let mut r = settings::recipe(&self.fabric).unwrap();
                        r.weight = w;
                        r.cap_size = cs;
                        r
                    };
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
                    self.recipe = {
                        let (w, cs) = (self.recipe.weight, self.recipe.cap_size);
                        let mut r = settings::recipe(&self.fabric).unwrap();
                        r.weight = w;
                        r.cap_size = cs;
                        r
                    };
                    self.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Thread");
                let before = self.recipe.weight;
                egui::ComboBox::from_id_salt("design-weight")
                    .selected_text(self.recipe.weight.label())
                    .show_ui(ui, |ui| {
                        for w in thread::ThreadWeight::ALL {
                            ui.selectable_value(&mut self.recipe.weight, w, format!("{} — {}", w.label(), w.use_for()));
                        }
                    })
                    .response
                    .on_hover_text("Default thread weight for the design; any layer can use its own. Density, needle size and the size checks follow it.");
                if self.recipe.weight != before {
                    self.dirty = true;
                }
                ui.label(RichText::new(format!("needle {}", self.recipe.weight.needle())).weak().small());
                if ui.small_button("Weight guide").clicked() {
                    self.show_weights = true;
                }
            });

            if self.machine.is_none() && self.tab == Tab::Design {
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
                    self.recipe = {
                        let (w, cs) = (self.recipe.weight, self.recipe.cap_size);
                        let mut r = settings::recipe(&self.fabric).unwrap();
                        r.weight = w;
                        r.cap_size = cs;
                        r
                    };
                    self.dirty = true;
                }
            }
            }
    }

    /// Thread library and the machine's colour stops.
    fn threads_panel(&mut self, ui: &mut egui::Ui) {
        if self.machine.is_none() && self.tab == Tab::Design && self.pro {
                self.libraries_ui(ui);
        }
        if let Some(d) = &self.design {
                section(ui, "Color stops");
                for (i, st) in d.stops.iter().enumerate() {
                    ui.horizontal(|ui| {
                        swatch(ui, st.thread.rgb);
                        ui.label(format!("{} · needle {}", i + 1, st.needle));
                        ui.label(RichText::new(format!("{} {}", st.thread.code, st.thread.name)).strong());
                        if st.weight != thread::ThreadWeight::W40 {
                            ui.label(RichText::new(st.weight.label()).color(Color32::from_rgb(214, 60, 120)))
                                .on_hover_text(format!("Load {} on this needle (needle size {})", st.weight.label(), st.weight.needle()));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(group(st.stitches)).weak());
                        });
                    });
                }
        }
    }

    /// Stitch counts, run time, checks and the job sheet.
    fn production_panel(&mut self, ui: &mut egui::Ui) {
            if let Some(d) = &self.design {
                section(ui, "Production");
                let s = &d.stats;
                egui::Grid::new("stats").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    kv(ui, "Size", &format!("{:.2} × {:.2} in  ({:.0} × {:.0} mm)", s.width_mm / 25.4, s.height_mm / 25.4, s.width_mm, s.height_mm));
                    kv(ui, "Stitches", &group(s.stitches));
                    kv(ui, "Stops", &d.stops.len().to_string());
                    kv(ui, "Trims", &s.trims.to_string());
                    kv(ui, "Est. run time", &format!("{:.1} min", s.run_minutes));
                    kv(ui, "Thread (each)", &format!("{:.0} m top · {:.0} m bobbin", s.top_m, s.bobbin_m));
                    if let Some(st) = d.stops.iter().max_by(|a, b| a.top_m.partial_cmp(&b.top_m).unwrap()) {
                        if st.top_m > 0.1 {
                            kv(ui, "Per cone", &format!(
                                "~{} pieces ({} {}, {:.0} m cone)",
                                group((st.weight.cone_m() / st.top_m).floor() as usize),
                                st.thread.code,
                                st.weight.label(),
                                st.weight.cone_m()
                            ));
                        }
                    }
                    kv(ui, "Backing", &d.recipe.backing);
                    kv(ui, "Topping", &d.recipe.topping);
                });
                if !d.warnings.is_empty() || !self.file_notes.is_empty() {
                    section(ui, "Check before running");
                    for w in self.file_notes.iter().chain(d.warnings.iter()) {
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
    }

    // ---------- canvas ----------

    fn canvas(&mut self, ui: &mut egui::Ui, now: f64) {
        let rope_id = self.rope_texture(ui.ctx());
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        if self.view == View::Vector {
            self.refresh_vector_texture(ui.ctx());
        } else {
            self.refresh_piece_textures(ui.ctx());
        }
        let g = self.job.garment_rgb.unwrap_or(CANVAS);
        let fabric = Color32::from_rgb(g[0], g[1], g[2]);
        let dark_garment = (g[0] as u32 * 299 + g[1] as u32 * 587 + g[2] as u32 * 114) / 1000 < 110;

        // An empty design is a blank canvas to draw on.
        let blank = Design::empty(self.profile, &self.recipe);
        let empty_canvas = self.design.is_none();
        let d = self.design.as_ref().unwrap_or(&blank);

        let b = d.bbox;
        let fit_box = if empty_canvas && self.show_hoop { self.hoop_rect(d).0 } else { b };
        if self.fit {
            let b = fit_box;
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
        self.cursor_mm = resp.hover_pos().map(to_mm);

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
        let editing = self.tab == Tab::Design && self.machine.is_none();
        let drawing = editing && self.tool.draws();
        if drawing {
            self.draw_input(&resp, ui, &to_mm, &to_screen, zoom);
        }
        let cut_tool = matches!(self.tool, Tool::Knife | Tool::Eraser);
        let object_edit = editing && !drawing && !cut_tool && self.edit_layer().is_some();
        if object_edit {
            self.edit_input(&resp, ui, &to_mm, &to_screen, zoom);
        }
        let measuring = self.tool == Tool::Measure;
        if measuring {
            if let Some(pos) = resp.interact_pointer_pos() {
                let mut p = to_mm(pos);
                if resp.drag_started() {
                    self.measure = Some((p, p));
                } else if resp.dragged() {
                    if let Some((a, _)) = self.measure {
                        if ui.input(|i| i.modifiers.shift) {
                            // snap to 15° steps
                            let d = p.sub(a);
                            let step = std::f64::consts::PI / 12.0;
                            let ang = (d.y.atan2(d.x) / step).round() * step;
                            let len = d.len();
                            p = Pt::new(a.x + len * ang.cos(), a.y + len * ang.sin());
                        }
                        self.measure = Some((a, p));
                    }
                }
            }
        }
        let cutting = editing && matches!(self.tool, Tool::Knife | Tool::Eraser);
        if cutting {
            self.cut_input(&resp, ui, &to_mm);
        }
        let guiding = editing && self.tool == Tool::Guides;
        if guiding {
            self.guide_input(&resp, ui, &to_mm, &to_screen, zoom);
        }
        // Angle handles: the two ends of the selected layer's stitch-angle
        // guide. Press on an end and drag to turn the stitch angle.
        let angle_id = egui::Id::new("angle-handles");
        if !(object_edit || drawing || measuring || cutting || guiding) {
            if let Some((t1, t2, li)) = ui.ctx().data(|d| d.get_temp::<(Pos2, Pos2, usize)>(angle_id)) {
                let near = |p: Pos2| p.distance(t1) < 16.0 || p.distance(t2) < 16.0;
                if resp.hover_pos().map(near).unwrap_or(false) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                // the drag only starts after the pointer has moved, so test where the press began
                let origin = ui.input(|i| i.pointer.press_origin());
                if resp.drag_started_by(egui::PointerButton::Primary)
                    && origin.map(near).unwrap_or(false)
                {
                    self.angle_drag = Some(li);
                }
            }
        }
        if let Some(li) = self.angle_drag {
            if let (Some(pos), Some((t1, t2, _))) = (
                resp.interact_pointer_pos(),
                ui.ctx().data(|d| d.get_temp::<(Pos2, Pos2, usize)>(angle_id)),
            ) {
                if resp.dragged_by(egui::PointerButton::Primary) || resp.drag_started_by(egui::PointerButton::Primary) {
                    let c = Pos2::new((t1.x + t2.x) / 2.0, (t1.y + t2.y) / 2.0);
                    let v = pos - c;
                    // screen y runs down, so flip it for a counter-clockwise angle
                    let deg = (-(v.y as f64)).atan2(v.x as f64).to_degrees().rem_euclid(180.0);
                    let deg = (deg * 10.0).round() / 10.0;
                    if self.layers[li].opts.angle_deg.map(|a| (a - deg).abs() > 0.05).unwrap_or(true) {
                        self.layers[li].opts.angle_deg = Some(deg);
                        self.dirty = true;
                    }
                }
            }
            if resp.drag_stopped() {
                self.angle_drag = None;
            }
        }
        let object_edit = object_edit || drawing || measuring || cutting || guiding || self.angle_drag.is_some();
        let no_layers: Vec<Layer> = Vec::new();
        let hit_src: &[Layer] = if editing { &self.layers } else { &no_layers };
        if !object_edit && resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let hit = hit_layer(pos, hit_src, zoom);
                if hit.is_some() && (hit == self.selected || ui.input(|i| !i.modifiers.shift)) {
                    if !hit.map(|h| self.multi.contains(&h)).unwrap_or(false) {
                        self.multi = hit.into_iter().collect();
                    }
                    self.selected = hit;
                    self.moving = Some(Pt::new(0.0, 0.0));
                } else {
                    self.moving = None;
                }
            }
        }
        if !object_edit && resp.dragged() {
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
        if !object_edit && resp.drag_stopped() {
            if let Some(m) = self.moving.take() {
                if m.len() > 0.01 {
                    self.hard_checkpoint();
                    let mut moved: std::collections::BTreeSet<usize> = Default::default();
                    for li in self.sel_layers() {
                        for j in self.linked(li) {
                            if moved.insert(j) {
                                self.layers[j].offset = self.layers[j].offset.add(m);
                            }
                        }
                    }
                    self.dirty = true;
                }
            }
        }
        if !object_edit && resp.clicked() && editing {
            if let Some(pos) = resp.interact_pointer_pos() {
                let hit = hit_layer(pos, &self.layers, zoom);
                let shift = ui.input(|i| i.modifiers.shift);
                match hit {
                    Some(id) if shift => {
                        if self.multi.contains(&id) && self.multi.len() > 1 {
                            self.multi.remove(&id);
                            if self.selected == Some(id) {
                                self.selected = self.multi.iter().next().copied();
                            }
                        } else {
                            self.multi.insert(id);
                            self.selected = Some(id);
                        }
                    }
                    _ => {
                        self.selected = hit;
                        self.multi = hit.into_iter().collect();
                    }
                }
                if let (Some(li), Tool::Nodes | Tool::Stitches) = (self.sel_idx(), self.tool) {
                    if self.tool == Tool::Nodes {
                        self.start_shape_edit(li);
                    } else {
                        self.start_stitch_edit(li);
                    }
                }
            }
        }
        if !object_edit && resp.double_clicked() && self.selected.is_none() {
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
                if !object_edit && !resp.dragged() && editing && hit_layer(cur, &self.layers, zoom).is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }
        if self.moving.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let _ = now;

        let d = self.design.as_ref().unwrap_or(&blank);
        let painter = painter.with_clip_rect(rect);
        let hoop_r = self.paint_artboard(&painter, rect, d, &to_screen, fabric);
        let gp = painter.with_clip_rect(hoop_r.intersect(rect));

        // grid every 10 mm, and a slightly darker hoop-ish area around the design
        let grid = if dark_garment {
            Color32::from_rgba_unmultiplied(255, 255, 255, 14)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 12)
        };
        // the garment's weave (twill) when zoomed in on the 3D view
        if self.view == View::ThreeD && zoom >= 3.0 && self.job.garment_rgb.is_some() {
            let pitch = 0.6 * zoom; // twill ribs about 0.6 mm apart
            let ink = if dark_garment {
                Color32::from_rgba_unmultiplied(255, 255, 255, 9)
            } else {
                Color32::from_rgba_unmultiplied(0, 0, 0, 13)
            };
            let h = rect.height();
            let mut x = rect.left() - h;
            while x < rect.right() {
                gp.line_segment(
                    [Pos2::new(x, rect.bottom()), Pos2::new(x + h, rect.top())],
                    Stroke::new((pitch * 0.35).max(1.0), ink),
                );
                x += pitch;
            }
        }
        let tl = to_mm(rect.min);
        let br = to_mm(rect.max);
        if zoom > 1.2 {
            let mut x = (tl.x / 10.0).floor() * 10.0;
            while x <= br.x {
                let sx = to_screen(Pt::new(x, 0.0)).x;
                gp.line_segment(
                    [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
                    Stroke::new(1.0_f32, grid),
                );
                x += 10.0;
            }
            let mut y = (tl.y / 10.0).floor() * 10.0;
            while y <= br.y {
                let sy = to_screen(Pt::new(0.0, y)).y;
                gp.line_segment(
                    [Pos2::new(rect.left(), sy), Pos2::new(rect.right(), sy)],
                    Stroke::new(1.0_f32, grid),
                );
                y += 10.0;
            }
        }

        // ---- stitches
        let end = self.progress.min(d.cmds.len());
        let sel_layer = if editing { self.sel_idx() } else { None };
        let moving = self.moving.filter(|m| m.len() > 0.0);
        // Name drops: all names share one spot, so show only the chosen one.
        let only_name: Option<(usize, usize)> = if self.tab == Tab::Drops {
            self.drop_info
                .as_ref()
                .map(|(n_logo, _)| (*n_logo, *n_logo + self.drop_sel))
        } else {
            None
        };
        let moving_set: Vec<usize> = if editing {
            self.sel_layers().into_iter().flat_map(|li| self.linked(li)).collect()
        } else {
            Vec::new()
        };
        let view = self.view;
        let show_jumps = self.show_jumps;
        let rope = if view == View::ThreeD { rope_id } else { None };
        let draw_obj = |o: &stitchforge::design::ObjectInfo, shift: Pt, painter: &egui::Painter| {
            let (a, z) = o.cmd_range;
            if a >= end {
                return;
            }
            let to = |p: Pt| to_screen(p.add(shift));
            let rgb = d.stops[o.stop].thread.rgb;
            // strands drawn as wide as the thread weight lays them
            let wk = (d.stops[o.stop].weight.strand_mm() / 0.33) as f32;
            painter.extend(stitch_shapes(
                d,
                a,
                z.min(end),
                rgb,
                &to,
                zoom * wk,
                view,
                show_jumps,
                rect.expand(20.0),
                if o.puff { 2.2 } else { 1.0 },
                rope,
            ));
        };
        let hide_layer = match self.edit {
            Edit::Stitches(_) if editing => self.edit_layer(),
            _ => None,
        };
        let vector_view = self.view == View::Vector && self.machine.is_none();
        if vector_view {
            if let Some(vt) = &self.vec_tex {
                let r = Rect::from_two_pos(to_screen(vt.origin), to_screen(vt.origin.add(vt.size)));
                painter.image(vt.tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
        // appliqué pieces go on the garment at their tack-down
        let mut next_piece = 0usize;
        let draw_pieces_until = |upto: usize, next: &mut usize, painter: &egui::Painter| {
            while *next < self.piece_tex.len() && self.piece_tex[*next].0 <= upto {
                let (at, vt) = &self.piece_tex[*next];
                if *at < end {
                    let r = Rect::from_two_pos(to_screen(vt.origin), to_screen(vt.origin.add(vt.size)));
                    painter.image(vt.tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                *next += 1;
            }
        };
        for o in d.objects.iter().filter(|_| !vector_view) {
            draw_pieces_until(o.cmd_range.0, &mut next_piece, &painter);
            if moving.is_some() && moving_set.contains(&o.layer) {
                continue;
            }
            if Some(o.layer) == hide_layer {
                continue;
            }
            if let Some((n_logo, keep)) = only_name {
                if o.layer >= n_logo && o.layer != keep {
                    continue;
                }
            }
            draw_obj(o, Pt::new(0.0, 0.0), &painter);
        }
        if !vector_view {
            draw_pieces_until(usize::MAX, &mut next_piece, &painter);
        }
        // the part picked in the layers list: its stitches outlined in the
        // highlight colour, and a box round it
        if let Some((lid, seq)) = self.focus_part {
            let li = self.layers.iter().position(|l| l.id == lid);
            if let Some(o) = d.objects.iter().find(|o| Some(o.layer) == li && o.sequence == seq) {
                let (a, z) = o.cmd_range;
                let hi = Color32::from_rgb(255, 196, 0);
                let mut pts: Vec<Pos2> = Vec::new();
                let mut bb = Rect::NOTHING;
                for c in &d.cmds[a..z.min(d.cmds.len())] {
                    match c {
                        Cmd::Stitch(p) => {
                            let q = to_screen(*p);
                            bb.extend_with(q);
                            pts.push(q);
                        }
                        _ => {
                            if pts.len() >= 2 {
                                painter.add(EShape::line(std::mem::take(&mut pts), Stroke::new(1.0_f32, hi)));
                            }
                            pts.clear();
                        }
                    }
                }
                if pts.len() >= 2 {
                    painter.add(EShape::line(pts, Stroke::new(1.0_f32, hi)));
                }
                if bb.is_positive() {
                    painter.rect_stroke(bb.expand(4.0), 3.0, Stroke::new(2.0_f32, hi), egui::StrokeKind::Outside);
                }
            }
        }
        if let Some(m) = moving {
            // while moving, the moved layers show as stitches in every view
            for o in d.objects.iter().filter(|o| moving_set.contains(&o.layer)) {
                draw_obj(o, m, &painter);
            }
        }

        // underlay view: wash out the top stitches, draw the underlay on top
        if self.show_underlay && !vector_view {
            painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(250, 248, 244, 175));
            let orange = Color32::from_rgb(232, 96, 16);
            let w = (0.12 * zoom).clamp(1.0, 2.5);
            for &(a, b) in &d.underlay_cmds {
                if a >= end {
                    continue;
                }
                let b = b.min(end);
                let mut pts: Vec<Pos2> = Vec::new();
                if a > 0 {
                    if let Cmd::Stitch(p) = d.cmds[a - 1] {
                        pts.push(to_screen(p));
                    }
                }
                for c in &d.cmds[a..b] {
                    if let Cmd::Stitch(p) = c {
                        pts.push(to_screen(*p));
                    }
                }
                if pts.len() >= 2 {
                    painter.add(EShape::line(pts.clone(), Stroke::new(w, orange)));
                    if zoom >= 4.0 {
                        for p in &pts {
                            painter.circle_filled(*p, w + 0.6, orange);
                        }
                    }
                }
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

        // direction lines of the selected layer (always while that tool is on)
        if let Some(li) = self.sel_idx() {
            let l = &self.layers[li];
            if !l.opts.guides.is_empty() && (self.tool == Tool::Guides || self.edit == Edit::None) {
                let col = Color32::from_rgb(0, 150, 140);
                for (k, (a, b)) in l.opts.guides.iter().enumerate() {
                    let (sa, sb) = (to_screen(l.tf(*a)), to_screen(l.tf(*b)));
                    let picked = self.tool == Tool::Guides && self.guide_sel == Some(k);
                    let w: f32 = if picked { 3.2 } else { 2.0 };
                    painter.line_segment([sa, sb], Stroke::new(w + 2.5, Color32::from_white_alpha(190)));
                    painter.line_segment([sa, sb], Stroke::new(w, if picked { Color32::from_rgb(230, 120, 0) } else { col }));
                    let u = (sb - sa).normalized();
                    let nrm = Vec2::new(-u.y, u.x);
                    painter.add(EShape::convex_polygon(vec![sb, sb - u * 11.0 + nrm * 5.5, sb - u * 11.0 - nrm * 5.5], if picked { Color32::from_rgb(230, 120, 0) } else { col }, Stroke::new(1.0_f32, Color32::WHITE)));
                    painter.circle_filled(sa, 3.5, col);
                }
            }
        }
        if let Some((a, b)) = self.guide_drag {
            let col = Color32::from_rgb(0, 150, 140);
            let (sa, sb) = (to_screen(a), to_screen(b));
            painter.line_segment([sa, sb], Stroke::new(2.0_f32, col));
            let deg = ((-(b.y - a.y)).atan2(b.x - a.x).to_degrees()).rem_euclid(180.0);
            painter.text(sb + Vec2::new(8.0, -8.0), egui::Align2::LEFT_BOTTOM, format!("{deg:.0}°"), egui::FontId::proportional(13.0), col);
        }
        if let Some((a, b)) = self.cut {
            let red = Color32::from_rgb(220, 50, 40);
            let (sa, sb) = (to_screen(a), to_screen(b));
            painter.line_segment([sa, sb], Stroke::new(4.0_f32, Color32::from_white_alpha(170)));
            for seg in EShape::dashed_line(&[sa, sb], Stroke::new(2.0_f32, red), 6.0, 4.0) {
                painter.add(seg);
            }
            painter.circle_filled(sa, 3.5, red);
            painter.circle_filled(sb, 3.5, red);
        }
        if self.lasso.len() >= 2 {
            let red = Color32::from_rgb(220, 50, 40);
            let pts: Vec<Pos2> = self.lasso.iter().map(|p| to_screen(*p)).collect();
            let mut closed = pts.clone();
            closed.push(pts[0]);
            painter.add(EShape::line(closed.clone(), Stroke::new(4.0_f32, Color32::from_white_alpha(150))));
            for seg in EShape::dashed_line(&closed, Stroke::new(1.8_f32, red), 5.0, 3.0) {
                painter.add(seg);
            }
        }
        if let Some((a, b)) = self.measure {
            measure_overlay(&painter, to_screen(a), to_screen(b), a.dist(b), ui.visuals().dark_mode);
        }
        // ---- the object being edited: its needle points or its anchors
        if editing {
            self.edit_overlay(&painter, &to_screen, zoom, ui);
            self.draw_overlay(&painter, &to_screen, &to_mm, ui);
            // the rest of a multi-selection
            if self.edit == Edit::None {
                let shift = moving.unwrap_or(Pt::new(0.0, 0.0));
                for li in self.sel_layers() {
                    if Some(self.layers[li].id) == self.selected {
                        continue;
                    }
                    let lb = self.layers[li].bbox();
                    if lb.is_valid() {
                        let r = Rect::from_min_max(to_screen(lb.min.add(shift)), to_screen(lb.max.add(shift))).expand(5.0);
                        for seg in EShape::dashed_line(
                            &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()],
                            Stroke::new(1.0_f32, Color32::from_rgb(90, 150, 240)),
                            4.0,
                            4.0,
                        ) {
                            painter.add(seg);
                        }
                    }
                }
            }
        }
        if empty_canvas && self.layers.is_empty() {
            let c = rect.center();
            painter.text(
                c - Vec2::new(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                "Draw with the tools on the left",
                egui::FontId::proportional(22.0),
                Color32::from_rgb(90, 86, 80),
            );
            painter.text(
                c + Vec2::new(0.0, 16.0),
                egui::Align2::CENTER_CENTER,
                "or drop art, a PDF or a picture here, Open…, Add text…",
                egui::FontId::proportional(14.0),
                Color32::from_rgb(130, 126, 118),
            );
        }

        // ---- selection box and stitch-direction arrow
        if let Some(li) = sel_layer.filter(|_| self.edit == Edit::None) {
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
                if let Some(o) = pieces.first().filter(|_| same && self.layers[li].opts.guides.is_empty()) {
                    {
                        // Stitch-direction guide: a dashed, arrow-tipped
                        // overlay in the selection colour with a labelled
                        // pill, so it never reads as a stitch.
                        let a = o.angle_deg.to_radians() as f32;
                        let c = r.center();
                        let len = (r.width().min(r.height()) * 0.35).clamp(20.0, 70.0);
                        let u = Vec2::new(a.cos(), -a.sin());
                        let dir = u * len;
                        let halo = Stroke::new(4.0_f32, Color32::from_white_alpha(170));
                        let ink = Stroke::new(1.6_f32, accent);
                        for st in [halo, ink] {
                            for seg in EShape::dashed_line(&[c - dir, c + dir], st, 5.0, 4.0) {
                                painter.add(seg);
                            }
                        }
                        let n = Vec2::new(-u.y, u.x);
                        // round handles at the ends: drag one to turn the stitch angle
                        painter.ctx().data_mut(|d| d.insert_temp(egui::Id::new("angle-handles"), (c + dir, c - dir, li)));
                        for tip in [c + dir, c - dir] {
                            painter.circle_filled(tip, 6.0, accent);
                            painter.circle_stroke(tip, 6.0, Stroke::new(1.5_f32, Color32::WHITE));
                            let back = if tip == c + dir { -u } else { u };
                            let pts = vec![tip + back * 9.0, tip + back * 16.0 + n * 5.0, tip + back * 16.0 - n * 5.0];
                            painter.add(EShape::convex_polygon(pts, accent, Stroke::new(1.0_f32, Color32::WHITE)));
                        }
                        let label = format!("Stitch angle {:.0}° · drag an end", o.angle_deg);
                        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(12.0), Color32::WHITE);
                        let pill = Rect::from_center_size(c, galley.size() + Vec2::new(14.0, 6.0));
                        painter.rect_filled(pill, 9.0, accent);
                        painter.galley(pill.center() - galley.size() / 2.0, galley, Color32::WHITE);
                    }
                }
            }
        }

        // size callout
        if !empty_canvas {
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
        let origin = if empty_canvas { Pt::new(0.0, 0.0) } else { b.center() };
        self.paint_rulers(&painter, rect, &to_screen, &to_mm, origin);
        let sel_screen = self.sel_idx().map(|li| {
            let lb = self.layers[li].bbox();
            Rect::from_min_max(to_screen(lb.min), to_screen(lb.max))
        });
        if let Some(sr) = sel_screen.filter(|r| r.is_positive() && rect.intersects(*r)) {
            if self.sel_layers().len() == 1 {
                self.task_bar(ui.ctx(), rect, sr, now);
            }
        }
    }

    // ---------- drawing ----------

    fn set_tool(&mut self, t: Tool) {
        if t == self.tool && !matches!(t, Tool::Nodes | Tool::Stitches) {
            return;
        }
        self.finish_pen(false);
        self.shape_drag = None;
        // the eraser works on needle points while stitches are being edited
        let keep = t == Tool::Eraser && matches!(self.edit, Edit::Stitches(_));
        if self.edit != Edit::None && !keep {
            self.end_edit();
        }
        self.tool = t;
        match t {
            Tool::Nodes | Tool::Stitches => match self.sel_idx() {
                Some(li) if self.machine.is_none() => {
                    if t == Tool::Nodes {
                        self.start_shape_edit(li)
                    } else {
                        self.start_stitch_edit(li)
                    }
                }
                _ => self.status = "Click a shape to edit its points".into(),
            },
            Tool::Pen => self.status = "Pen: click for corners, drag for curves; click the first point to close, double-click or Enter to finish, Esc to cancel".into(),
            Tool::Text => self.status = "Text: click where the words go".into(),
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Line => {
                self.status = "Drag on the canvas to draw; Shift constrains. Style is in the panel on the right".into()
            }
            Tool::Measure => {
                self.status = "Measure: drag across a column, letter stroke or gap".into()
            }
            Tool::Knife => {
                self.status = "Knife: drag a line across a shape to cut it in two (the selected layers, or whatever the line crosses)".into()
            }
            Tool::Guides => {
                self.status = "Direction lines: select a fill, then drag across it the way the stitches should run; add lines where they should turn".into()
            }
            Tool::Eraser if keep => {
                self.status = "Eraser: draw around needle points to remove them; Esc when done".into()
            }
            Tool::Eraser => {
                self.status = "Eraser: draw around what to remove from the selected layers (or from anything under it)".into()
            }
            Tool::Select => {}
        }
        if t != Tool::Measure {
            self.measure = None;
        }
        self.cut = None;
        self.lasso.clear();
    }

    /// Snap a point: to an existing anchor within reach, else the 1 mm grid
    /// when snapping is on.
    fn snap_pt(&self, p: Pt, to_screen: &dyn Fn(Pt) -> Pos2) -> Pt {
        let sp = to_screen(p);
        let mut best: Option<(f32, Pt)> = None;
        for l in self.layers.iter().filter(|l| l.visible) {
            for sh in &l.shapes {
                for c in &sh.curves {
                    for n in &c.nodes {
                        let q = l.tf(n.p);
                        let d = to_screen(q).distance(sp);
                        if d < 7.0 && best.map(|b| d < b.0).unwrap_or(true) {
                            best = Some((d, q));
                        }
                    }
                }
            }
        }
        for n in &self.pen {
            let d = to_screen(n.p).distance(sp);
            if d < 7.0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, n.p));
            }
        }
        match best {
            Some((_, q)) => q,
            None if self.snap => Pt::new(p.x.round(), p.y.round()),
            None => p,
        }
    }

    fn pen_add(&mut self, p: Pt, to_screen: &dyn Fn(Pt) -> Pos2) {
        if self.pen.len() >= 2 && to_screen(self.pen[0].p).distance(to_screen(p)) < 8.0 {
            self.finish_pen(true);
            return;
        }
        if let Some(last) = self.pen.last() {
            if last.p.dist(p) < 0.05 {
                return;
            }
        }
        self.pen.push(Node::corner(p));
    }

    fn finish_pen(&mut self, closed: bool) {
        let mut nodes = std::mem::take(&mut self.pen);
        nodes.dedup_by(|a, b| a.p.dist(b.p) < 0.2);
        self.pen_dragging = false;
        let enough = if closed { nodes.len() >= 3 } else { nodes.len() >= 2 };
        if enough {
            self.add_drawn(VPath { nodes, closed }, "Path");
        }
    }

    /// The shape a drag makes with the current tool.
    fn shape_from_drag(&self, a: Pt, b: Pt, shift: bool) -> Option<(VPath, &'static str)> {
        let st = &self.style;
        let (mut dx, mut dy) = (b.x - a.x, b.y - a.y);
        Some(match self.tool {
            Tool::Rect => {
                if shift {
                    let m = dx.abs().max(dy.abs());
                    dx = m * dx.signum();
                    dy = m * dy.signum();
                }
                (vector::rect(a.x, a.y, dx, dy, st.corner), "Rectangle")
            }
            Tool::Ellipse => {
                if shift {
                    let m = dx.abs().max(dy.abs());
                    dx = m * dx.signum();
                    dy = m * dy.signum();
                }
                (
                    vector::ellipse(a.x + dx / 2.0, a.y + dy / 2.0, dx / 2.0, dy / 2.0),
                    "Ellipse",
                )
            }
            Tool::Polygon => {
                let r = a.dist(b);
                let mut rot = dy.atan2(dx).to_degrees() + 90.0;
                if shift {
                    rot = (rot / 15.0).round() * 15.0;
                }
                let inner = st.star.then_some(st.inner);
                (
                    vector::polygon(a.x, a.y, r, st.sides, inner, rot),
                    if st.star { "Star" } else { "Polygon" },
                )
            }
            Tool::Line => {
                let mut b = b;
                if shift {
                    let len = a.dist(b);
                    let ang = (dy.atan2(dx).to_degrees() / 15.0).round() * 15.0;
                    let r = ang.to_radians();
                    b = Pt::new(a.x + len * r.cos(), a.y + len * r.sin());
                }
                (vector::line(a, b), "Line")
            }
            _ => return None,
        })
    }

    /// Turn a drawn path into a layer (fill) and/or an outline layer.
    fn add_drawn(&mut self, v: VPath, kind: &str) {
        self.hard_checkpoint();
        let st = self.style.clone();
        let count = self.layers.iter().filter(|l| l.name.starts_with(kind)).count() + 1;
        let name = format!("{kind} {count}");
        let fill = v.closed && st.fill_on;
        let stroke = !v.closed || st.stroke_on || !fill;
        let mut made: Vec<Layer> = Vec::new();
        if fill {
            let mut sh = Shape {
                kind: ShapeKind::Area {
                    rings: Vec::new(),
                    rule: FillRule::EvenOdd,
                },
                rgb: st.fill,
                label: name.clone(),
                group: 0,
                opts: ShapeOpts::default(),
                curves: vec![v.clone()],
            };
            sh.reflatten();
            let id = self.next_id;
            self.next_id += 1;
            made.push(Layer::new(id, name.clone(), vec![sh], false));
        }
        if stroke {
            let mut sh = Shape {
                kind: ShapeKind::Line {
                    paths: Vec::new(),
                    width: st.stroke_w,
                },
                rgb: st.stroke,
                label: format!("{name} outline"),
                group: 0,
                opts: ShapeOpts::default(),
                curves: vec![v],
            };
            sh.reflatten();
            let id = self.next_id;
            self.next_id += 1;
            let label = if fill { format!("{name} outline") } else { name.clone() };
            made.push(Layer::new(id, label, vec![sh], true));
        }
        if made.len() == 2 {
            let link = self.next_id + 7_000_000;
            let o = made[0].origin;
            for l in &mut made {
                l.link = Some(link);
                l.origin = o;
            }
        }
        if self.layers.is_empty() {
            self.design_name = "drawing".into();
        }
        let first = made.first().map(|l| l.id);
        self.layers.extend(made);
        self.selected = first;
        self.multi = first.into_iter().collect();
        self.dirty = true;
        self.status = format!("Added {name}");
    }

    /// Mouse input while a drawing tool is active.
    fn draw_input(
        &mut self,
        resp: &egui::Response,
        ui: &egui::Ui,
        to_mm: &dyn Fn(Pos2) -> Pt,
        to_screen: &dyn Fn(Pt) -> Pos2,
        zoom: f32,
    ) {
        let shift = ui.input(|i| i.modifiers.shift);
        let origin = ui.input(|i| i.pointer.press_origin());
        let latest = ui.input(|i| i.pointer.latest_pos());
        if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
            let dlt = resp.drag_delta();
            self.view_center = self
                .view_center
                .sub(Pt::new((dlt.x / zoom) as f64, (dlt.y / zoom) as f64));
            return;
        }
        match self.tool {
            Tool::Pen => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(o) = origin {
                        let p = self.snap_pt(to_mm(o), to_screen);
                        let before = self.pen.len();
                        self.pen_add(p, to_screen);
                        self.pen_dragging = self.pen.len() > before;
                    }
                }
                if self.pen_dragging && resp.dragged_by(egui::PointerButton::Primary) {
                    if let (Some(l), Some(n)) = (latest, self.pen.last_mut()) {
                        let q = to_mm(l);
                        n.cout = q;
                        n.cin = n.p.sub(q.sub(n.p));
                    }
                }
                if resp.drag_stopped() {
                    self.pen_dragging = false;
                }
                if resp.double_clicked() {
                    self.finish_pen(false);
                } else if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let p = self.snap_pt(to_mm(pos), to_screen);
                        self.pen_add(p, to_screen);
                    }
                }
            }
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Line => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(o) = origin {
                        let p = self.snap_pt(to_mm(o), to_screen);
                        self.shape_drag = Some((p, p));
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary) {
                    if let (Some((a, _)), Some(l)) = (self.shape_drag, latest) {
                        let b = self.snap_pt(to_mm(l), to_screen);
                        self.shape_drag = Some((a, b));
                    }
                }
                if resp.drag_stopped() {
                    if let Some((a, b)) = self.shape_drag.take() {
                        if a.dist(b) > 0.5 {
                            if let Some((v, kind)) = self.shape_from_drag(a, b, shift) {
                                self.add_drawn(v, kind);
                            }
                        }
                    }
                } else if resp.clicked() {
                    // a click alone makes a 20 mm shape there
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let c = self.snap_pt(to_mm(pos), to_screen);
                        let (a, b) = match self.tool {
                            Tool::Polygon => (c, Pt::new(c.x, c.y - 10.0)),
                            Tool::Line => (c, Pt::new(c.x + 20.0, c.y)),
                            _ => (Pt::new(c.x - 10.0, c.y - 10.0), Pt::new(c.x + 10.0, c.y + 10.0)),
                        };
                        if let Some((v, kind)) = self.shape_from_drag(a, b, false) {
                            self.add_drawn(v, kind);
                        }
                    }
                }
            }
            Tool::Text => {
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        self.text_at = Some(to_mm(pos));
                        self.show_text = true;
                    }
                }
            }
            _ => {}
        }
    }

    /// Preview of the path or shape being drawn.
    fn draw_overlay(&self, painter: &egui::Painter, to_screen: &dyn Fn(Pt) -> Pos2, to_mm: &dyn Fn(Pos2) -> Pt, ui: &egui::Ui) {
        let accent = Color32::from_rgb(30, 110, 230);
        let preview = |v: &VPath| {
            let pts: Vec<Pos2> = v.flatten().iter().map(|p| to_screen(*p)).collect();
            if v.closed && self.style.fill_on && pts.len() >= 3 {
                let c = self.style.fill;
                painter.add(EShape::convex_polygon(Vec::new(), Color32::TRANSPARENT, Stroke::NONE));
                let fill = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], 70);
                // fan from the first point is fine for a preview of simple shapes
                for k in 1..pts.len().saturating_sub(1) {
                    painter.add(EShape::convex_polygon(vec![pts[0], pts[k], pts[k + 1]], fill, Stroke::NONE));
                }
            }
            painter.add(EShape::line(pts, Stroke::new(1.5_f32, accent)));
        };
        if let Some((a, b)) = self.shape_drag {
            let shift = ui.input(|i| i.modifiers.shift);
            if let Some((v, _)) = self.shape_from_drag(a, b, shift) {
                preview(&v);
            }
        }
        if !self.pen.is_empty() {
            let mut nodes = self.pen.clone();
            if !self.pen_dragging {
                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                    nodes.push(Node::corner(to_mm(pos)));
                }
            }
            let v = VPath { nodes, closed: false };
            let pts: Vec<Pos2> = v.flatten().iter().map(|p| to_screen(*p)).collect();
            painter.add(EShape::line(pts, Stroke::new(1.5_f32, accent)));
            for (i, n) in self.pen.iter().enumerate() {
                let p = to_screen(n.p);
                if n.cout.dist(n.p) > 1e-6 {
                    for h in [n.cin, n.cout] {
                        let hp = to_screen(h);
                        painter.line_segment([p, hp], Stroke::new(1.0_f32, accent));
                        painter.circle_filled(hp, 3.5, Color32::WHITE);
                        painter.circle_stroke(hp, 3.5, Stroke::new(1.2_f32, accent));
                    }
                }
                let r = Rect::from_center_size(p, Vec2::splat(if i == 0 { 9.0 } else { 7.0 }));
                painter.rect_filled(r, 1.0, if i == 0 { accent } else { Color32::WHITE });
                painter.rect_stroke(r, 1.0, Stroke::new(1.2_f32, accent), egui::StrokeKind::Inside);
            }
        }
    }

    // ---------- path operations ----------

    /// Selected layers (multi-select plus the main one), bottom of the stack first.
    fn sel_layers(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.layers.len())
            .filter(|&i| {
                let id = self.layers[i].id;
                self.multi.contains(&id) || self.selected == Some(id)
            })
            .collect();
        v.sort();
        v
    }

    /// A layer's area in design mm (outlines become the area their stroke covers).
    fn layer_area(&self, li: usize) -> Vec<(Vec<stitchforge::geom::Ring>, FillRule)> {
        let l = &self.layers[li];
        let mut out = Vec::new();
        for s in l.transformed() {
            match s.kind {
                ShapeKind::Area { rings, rule } => out.push((rings, rule)),
                ShapeKind::Line { paths, width } => {
                    let w = l.opts.column_width.unwrap_or(width) * (l.sx * l.sy).abs().sqrt();
                    out.push((vector::stroke_area(&paths, w, true), FillRule::EvenOdd))
                }
            }
        }
        out
    }

    fn layer_color(&self, li: usize) -> [u8; 3] {
        self.layer_thread(li)
            .map(|t| t.rgb)
            .unwrap_or_else(|| self.layers[li].shapes.first().map(|s| s.rgb).unwrap_or([60, 60, 60]))
    }

    /// A new filled layer from rings (design mm), styled like layer `like`.
    fn area_layer(&mut self, rings: Vec<stitchforge::geom::Ring>, name: String, like: usize) -> Layer {
        let mut sh = Shape {
            kind: ShapeKind::Area {
                rings: Vec::new(),
                rule: FillRule::EvenOdd,
            },
            rgb: self.layer_color(like),
            label: name.clone(),
            group: 0,
            opts: ShapeOpts::default(),
            curves: vector::rings_to_curves(&rings),
        };
        sh.reflatten();
        let id = self.next_id;
        self.next_id += 1;
        let mut l = Layer::new(id, name, vec![sh], false);
        l.custom = self.layers[like].custom.clone();
        l.opts.thread = self.layers[like].opts.thread;
        l
    }

    fn path_bool(&mut self, op: BoolOp) {
        let idx = self.sel_layers();
        if idx.len() < 2 {
            self.error = Some(format!("{}: select two or more layers (Shift-click)", op.label()));
            return;
        }
        let inputs: Vec<(Vec<stitchforge::geom::Ring>, FillRule)> = idx
            .iter()
            .map(|&li| {
                let parts = self.layer_area(li);
                // one input per layer: its own parts merged
                let merged = vector::boolean(&parts, BoolOp::Union);
                (merged, FillRule::EvenOdd)
            })
            .collect();
        let rings = vector::boolean(&inputs, op);
        if rings.is_empty() {
            self.error = Some(format!("{} left nothing to keep", op.label()));
            return;
        }
        self.hard_checkpoint();
        let name = format!("{} ({})", self.layers[idx[0]].name, op.label().to_lowercase());
        let l = self.area_layer(rings, name, idx[0]);
        let id = l.id;
        for &li in idx.iter().rev() {
            self.layers.remove(li);
        }
        self.layers.insert(idx[0].min(self.layers.len()), l);
        self.selected = Some(id);
        self.multi = [id].into_iter().collect();
        self.dirty = true;
        self.status = format!("{} done", op.label());
    }

    fn combine(&mut self) {
        let idx = self.sel_layers();
        if idx.len() < 2 {
            self.error = Some("Combine: select two or more layers (Shift-click)".into());
            return;
        }
        self.hard_checkpoint();
        let mut shapes = Vec::new();
        for &li in &idx {
            shapes.extend(self.layers[li].baked());
        }
        let any_area = shapes.iter().any(|s| matches!(s.kind, ShapeKind::Area { .. }));
        let first = idx[0];
        let id = self.next_id;
        self.next_id += 1;
        let mut l = Layer::new(id, format!("{} (combined)", self.layers[first].name), shapes, !any_area);
        l.custom = self.layers[first].custom.clone();
        l.opts = self.layers[first].opts.clone();
        for &li in idx.iter().rev() {
            self.layers.remove(li);
        }
        self.layers.insert(first.min(self.layers.len()), l);
        self.selected = Some(id);
        self.multi = [id].into_iter().collect();
        self.dirty = true;
        self.status = "Combined into one layer".into();
    }

    /// Knife and Eraser: collect the drag, then cut when it ends.
    fn cut_input(&mut self, resp: &egui::Response, ui: &egui::Ui, to_mm: &dyn Fn(Pos2) -> Pt) {
        let Some(pos) = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos())) else { return };
        let mut p = to_mm(pos);
        // a drag is only recognised after the pointer has moved a little:
        // start from where the button went down
        let origin = ui.input(|i| i.pointer.press_origin()).map(|o| to_mm(o)).unwrap_or(p);
        match self.tool {
            Tool::Knife => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    self.cut = Some((origin, p));
                } else if resp.dragged_by(egui::PointerButton::Primary) {
                    if let Some((a, _)) = self.cut {
                        if ui.input(|i| i.modifiers.shift) {
                            let d = p.sub(a);
                            let step = std::f64::consts::PI / 12.0;
                            let ang = (d.y.atan2(d.x) / step).round() * step;
                            p = Pt::new(a.x + d.len() * ang.cos(), a.y + d.len() * ang.sin());
                        }
                        self.cut = Some((a, p));
                    }
                }
                if resp.drag_stopped() {
                    if let Some((a, b)) = self.cut.take() {
                        self.apply_knife(a, b);
                    }
                }
            }
            Tool::Eraser => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    self.lasso = vec![origin, p];
                } else if resp.dragged_by(egui::PointerButton::Primary) {
                    if self.lasso.last().map(|l| l.dist(p) > 0.2).unwrap_or(true) {
                        self.lasso.push(p);
                    }
                }
                if resp.drag_stopped() {
                    let lasso = std::mem::take(&mut self.lasso);
                    if lasso.len() >= 3 {
                        self.apply_eraser(&stitchforge::geom::simplify(&lasso, 0.05));
                    }
                }
            }
            _ => {}
        }
    }

    /// Direction lines: drag to add one to the selected fill; click one to
    /// pick it (Delete removes it).
    fn guide_input(&mut self, resp: &egui::Response, ui: &egui::Ui, to_mm: &dyn Fn(Pos2) -> Pt, to_screen: &dyn Fn(Pt) -> Pos2, zoom: f32) {
        let Some(pos) = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos())) else { return };
        let mut p = to_mm(pos);
        let origin = ui.input(|i| i.pointer.press_origin()).map(|o| to_mm(o)).unwrap_or(p);
        if resp.clicked() {
            // pick a line of the selected layer, or select the layer clicked
            let mut hit = None;
            if let Some(li) = self.sel_idx() {
                let l = &self.layers[li];
                for (k, (a, b)) in l.opts.guides.iter().enumerate() {
                    let (sa, sb) = (to_screen(l.tf(*a)), to_screen(l.tf(*b)));
                    let ab = sb - sa;
                    let t = ((pos - sa).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                    if (sa + ab * t).distance(pos) < 8.0 {
                        hit = Some(k);
                    }
                }
            }
            if hit.is_some() {
                self.guide_sel = hit;
                self.status = "Direction line picked: Delete removes it".into();
            } else {
                self.guide_sel = None;
                let tol = (4.0 / zoom) as f64;
                if let Some(l) = self.layers.iter().rev().find(|l| l.visible && l.hit(p, tol)) {
                    self.selected = Some(l.id);
                    self.multi = [l.id].into_iter().collect();
                }
            }
            return;
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.guide_drag = Some((origin, p));
        } else if resp.dragged_by(egui::PointerButton::Primary) {
            if let Some((a, _)) = self.guide_drag {
                if ui.input(|i| i.modifiers.shift) {
                    let d = p.sub(a);
                    let step = std::f64::consts::PI / 12.0;
                    let ang = (d.y.atan2(d.x) / step).round() * step;
                    p = Pt::new(a.x + d.len() * ang.cos(), a.y + d.len() * ang.sin());
                }
                self.guide_drag = Some((a, p));
            }
        }
        if resp.drag_stopped() {
            let Some((a, b)) = self.guide_drag.take() else { return };
            if a.dist(b) < 1.0 {
                return;
            }
            // the selected layer, else the one the line starts on
            let li = self.sel_idx().or_else(|| {
                let tol = (4.0 / zoom) as f64;
                self.layers.iter().rposition(|l| l.visible && l.hit(a, tol))
            });
            let Some(li) = li else {
                self.status = "Direction lines: select a filled shape first".into();
                return;
            };
            self.hard_checkpoint();
            let l = &mut self.layers[li];
            let (la, lb) = (l.inv(a), l.inv(b));
            l.opts.guides.push((la, lb));
            if l.opts.stitch != StitchChoice::Fill {
                l.opts.stitch = StitchChoice::Fill;
            }
            l.opts.fill = FillPattern::Tatami;
            self.selected = Some(l.id);
            self.guide_sel = Some(l.opts.guides.len() - 1);
            let n = l.opts.guides.len();
            self.dirty = true;
            self.status = if n == 1 {
                "Direction line added. Add more where the stitches should turn; the rows bend smoothly between them".into()
            } else {
                format!("{n} direction lines: the rows turn to follow them")
            };
        }
    }

    /// Layers a cut applies to: the selection, or every visible layer the
    /// test says is under the tool.
    fn cut_targets(&self, under: &dyn Fn(&Self, usize) -> bool) -> Vec<usize> {
        let sel: Vec<usize> = self.sel_layers().into_iter().filter(|&li| self.layers[li].visible).collect();
        if !sel.is_empty() {
            return sel.into_iter().filter(|&li| under(self, li)).collect();
        }
        (0..self.layers.len()).filter(|&li| self.layers[li].visible && under(self, li)).collect()
    }

    /// A layer rebuilt from new geometry, keeping everything else about it.
    fn reshaped(&mut self, li: usize, areas: Option<Vec<stitchforge::geom::Ring>>, lines: Option<Vec<Vec<Pt>>>, name: String, keep_id: bool) -> Layer {
        let old = self.layers[li].clone();
        let id = if keep_id { old.id } else { let id = self.next_id; self.next_id += 1; id };
        let rgb = self.layer_color(li);
        let mut shapes = Vec::new();
        if let Some(rings) = areas {
            let mut sh = Shape {
                kind: ShapeKind::Area { rings: Vec::new(), rule: FillRule::EvenOdd },
                rgb,
                label: name.clone(),
                group: 0,
                opts: ShapeOpts::default(),
                curves: vector::rings_to_curves(&rings),
            };
            sh.reflatten();
            shapes.push(sh);
        }
        if let Some(paths) = lines {
            let width = old.transformed().iter().find_map(|s| match &s.kind {
                ShapeKind::Line { width, .. } => Some(*width * (old.sx * old.sy).abs().sqrt()),
                _ => None,
            }).unwrap_or(1.0);
            let mut sh = Shape {
                kind: ShapeKind::Line { paths: Vec::new(), width },
                rgb,
                label: name.clone(),
                group: 0,
                opts: ShapeOpts::default(),
                curves: paths
                    .iter()
                    .map(|p| stitchforge::geom::VPath { nodes: p.iter().map(|q| stitchforge::geom::Node::corner(*q)).collect(), closed: false })
                    .collect(),
            };
            sh.reflatten();
            shapes.push(sh);
        }
        let mut l = Layer::new(id, name, shapes, old.is_outline);
        l.opts = old.opts.clone();
        if old.opts.column_width.is_some() {
            // the stroke width is now baked into the new geometry
            l.opts.column_width = old.opts.column_width.map(|w| w * (old.sx * old.sy).abs().sqrt());
        }
        l.custom = old.custom.clone();
        l.visible = old.visible;
        l
    }

    /// Area and stroke geometry of a layer in design mm.
    fn layer_geometry(&self, li: usize) -> (Vec<stitchforge::geom::Ring>, Vec<Vec<Pt>>) {
        let l = &self.layers[li];
        let (mut rings, mut paths) = (Vec::new(), Vec::new());
        let mut parts = Vec::new();
        for s in l.transformed() {
            match s.kind {
                ShapeKind::Area { rings: r, rule } => parts.push((r, rule)),
                ShapeKind::Line { paths: p, .. } => paths.extend(p),
            }
        }
        if !parts.is_empty() {
            rings = vector::boolean(&parts, BoolOp::Union);
        }
        (rings, paths)
    }

    fn apply_knife(&mut self, a: Pt, b: Pt) {
        if a.dist(b) < 0.5 {
            return;
        }
        if self.edit != Edit::None {
            self.end_edit();
        }
        let crosses = |s: &Self, li: usize| -> bool {
            let (rings, paths) = s.layer_geometry(li);
            stitchforge::cut::knife_rings(&rings, FillRule::EvenOdd, a, b).is_some()
                || stitchforge::cut::knife_paths(&paths, a, b).is_some()
        };
        let targets = self.cut_targets(&crosses);
        if targets.is_empty() {
            self.status = "Knife: the line has to cross a shape all the way to cut it".into();
            return;
        }
        self.hard_checkpoint();
        let mut made = 0;
        let mut first_ids = Vec::new();
        for &li in targets.iter().rev() {
            let (rings, paths) = self.layer_geometry(li);
            let ra = stitchforge::cut::knife_rings(&rings, FillRule::EvenOdd, a, b);
            let pa = stitchforge::cut::knife_paths(&paths, a, b);
            let (al, ar) = match ra {
                Some((l, r)) => (Some(l), Some(r)),
                None => (Some(rings.clone()).filter(|r| !r.is_empty()), None),
            };
            let (pl, pr) = match pa {
                Some((l, r)) => (Some(l), Some(r)),
                None => (Some(paths.clone()).filter(|p| !p.is_empty()), None),
            };
            let name = self.layers[li].name.clone();
            let one = self.reshaped(li, al, pl, format!("{name} 1"), true);
            let two = self.reshaped(li, ar, pr, format!("{name} 2"), false);
            first_ids.push(one.id);
            self.layers[li] = one;
            self.layers.insert(li + 1, two);
            made += 1;
        }
        self.multi = first_ids.iter().copied().collect();
        self.selected = first_ids.first().copied();
        self.dirty = true;
        self.status = if made == 1 {
            "Cut in two: the pieces are separate layers now".into()
        } else {
            format!("Cut {made} layers in two")
        };
    }

    fn apply_eraser(&mut self, lasso: &[Pt]) {
        // While editing stitches, the eraser removes needle points instead.
        if let (Edit::Stitches(_), Some(li)) = (self.edit, self.edit_layer()) {
            let l = &self.layers[li];
            let gone: Vec<usize> = (0..l.embroidery.needles.len())
                .filter(|&i| stitchforge::cut::point_in_poly(lasso, l.tf(l.embroidery.needles[i].p)))
                .collect();
            if gone.is_empty() {
                return;
            }
            self.hard_checkpoint();
            let n = gone.len();
            let l = &mut self.layers[li];
            l.embroidery.needles = stitchforge::project::remove_needles(&l.embroidery.needles, |i| gone.binary_search(&i).is_ok());
            self.st_sel.clear();
            self.touch_stitches(li);
            self.status = format!("Erased {n} needle point{}; the thread jumps over the gap. For a clean edge, erase the shape instead (outside stitch editing) and the stitches are rebuilt around the hole", if n == 1 { "" } else { "s" });
            return;
        }
        let under = |s: &Self, li: usize| -> bool {
            let (rings, paths) = s.layer_geometry(li);
            (!rings.is_empty() && stitchforge::cut::lasso_touches(&rings, FillRule::EvenOdd, lasso))
                || paths.iter().flatten().any(|p| stitchforge::cut::point_in_poly(lasso, *p))
                || stitchforge::cut::erase_paths(&paths, lasso).len() != paths.len()
        };
        let targets = self.cut_targets(&under);
        if targets.is_empty() {
            self.status = "Eraser: draw around part of a shape to remove it".into();
            return;
        }
        self.hard_checkpoint();
        let mut removed = 0;
        for &li in targets.iter().rev() {
            let (rings, paths) = self.layer_geometry(li);
            let left = if rings.is_empty() { None } else { Some(stitchforge::cut::erase_rings(&rings, FillRule::EvenOdd, lasso)).filter(|r| !r.is_empty()) };
            let lines = if paths.is_empty() { None } else { Some(stitchforge::cut::erase_paths(&paths, lasso)).filter(|p| !p.is_empty()) };
            if left.is_none() && lines.is_none() {
                self.layers.remove(li);
                removed += 1;
                continue;
            }
            let name = self.layers[li].name.clone();
            let nl = self.reshaped(li, left, lines, name, true);
            self.layers[li] = nl;
        }
        if self.selected.map(|id| !self.layers.iter().any(|l| l.id == id)).unwrap_or(false) {
            self.selected = None;
        }
        self.multi.retain(|id| self.layers.iter().any(|l| l.id == *id));
        self.dirty = true;
        self.status = match (targets.len(), removed) {
            (n, 0) => format!("Erased from {n} layer{}", if n == 1 { "" } else { "s" }),
            (_, r) => format!("Erased; {r} layer{} removed completely", if r == 1 { " was" } else { "s were" }),
        };
    }

    /// Clean-up commands for the layer whose stitches are being edited.
    fn stitch_cleanup(&mut self, li: usize, what: u8) {
        let (rings, _) = self.layer_geometry(li);
        let l = &self.layers[li];
        // work in design mm, store back in the layer's own frame
        let design: Vec<Needle> = l.embroidery.needles.iter().map(|n| Needle { p: l.tf(n.p), ..*n }).collect();
        let (new, msg) = match what {
            0 => {
                if rings.is_empty() {
                    self.status = "Stray clean-up needs a filled shape".into();
                    return;
                }
                let (v, gone, bridged) = stitchforge::cut::clean_strays(&design, &rings, FillRule::EvenOdd, 0.6);
                (v, format!("Removed {gone} stray point{} and turned {bridged} stitch{} across open fabric into jumps", if gone == 1 { "" } else { "s" }, if bridged == 1 { "" } else { "es" }))
            }
            1 => {
                let t = stitchforge::cut::tiny(&design, Limits::SHORT_STITCH);
                let n = t.len();
                (stitchforge::project::remove_needles(&design, |i| t.binary_search(&i).is_ok()), format!("Removed {n} stitch{} shorter than {:.1} mm", if n == 1 { "" } else { "es" }, Limits::SHORT_STITCH))
            }
            _ => {
                let (v, n) = stitchforge::cut::split_long(&design, 7.0);
                (v, format!("Split long stitches: {n} point{} added (no stitch over 7 mm)", if n == 1 { "" } else { "s" }))
            }
        };
        if new.len() == design.len() && new.iter().zip(&design).all(|(a, b)| a.jump == b.jump) {
            self.status = "Nothing to clean up".into();
            return;
        }
        self.hard_checkpoint();
        let l = &mut self.layers[li];
        l.embroidery.needles = new.into_iter().map(|n| Needle { p: l.inv(n.p), ..n }).collect();
        self.st_sel.clear();
        self.touch_stitches(li);
        self.status = msg;
    }

    fn break_apart(&mut self) {
        let Some(li) = self.sel_idx() else { return };
        let l = self.layers[li].clone();
        let mut made: Vec<Layer> = Vec::new();
        let mut k = 0;
        for s in l.baked() {
            match &s.kind {
                ShapeKind::Area { rings, rule } => {
                    for island in stitchforge::geom::split_islands(rings, *rule) {
                        k += 1;
                        let mut nl = self.area_layer(island, format!("{} {k}", l.name), li);
                        nl.opts = l.opts.clone();
                        made.push(nl);
                    }
                }
                ShapeKind::Line { .. } => {
                    for c in &s.curves {
                        k += 1;
                        let mut sh = s.clone();
                        sh.curves = vec![c.clone()];
                        sh.reflatten();
                        let id = self.next_id;
                        self.next_id += 1;
                        let mut nl = Layer::new(id, format!("{} {k}", l.name), vec![sh], true);
                        nl.custom = l.custom.clone();
                        nl.opts = l.opts.clone();
                        made.push(nl);
                    }
                }
            }
        }
        if made.len() < 2 {
            self.status = "Nothing to break apart: it's already one piece".into();
            return;
        }
        self.hard_checkpoint();
        let n = made.len();
        self.layers.remove(li);
        self.multi = made.iter().map(|l| l.id).collect();
        self.selected = made.first().map(|l| l.id);
        for (j, nl) in made.into_iter().enumerate() {
            self.layers.insert(li + j, nl);
        }
        self.dirty = true;
        self.status = format!("Broke apart into {n} layers");
    }

    fn outline_stroke(&mut self) {
        let idx: Vec<usize> = self
            .sel_layers()
            .into_iter()
            .filter(|&li| self.layers[li].is_outline)
            .collect();
        if idx.is_empty() {
            self.error = Some("Outline stroke: select an outline layer".into());
            return;
        }
        self.hard_checkpoint();
        for &li in idx.iter().rev() {
            let area: Vec<(Vec<stitchforge::geom::Ring>, FillRule)> = self.layer_area(li);
            let rings = vector::boolean(&area, BoolOp::Union);
            let name = format!("{} (filled)", self.layers[li].name);
            let mut nl = self.area_layer(rings, name, li);
            nl.link = None;
            self.layers[li] = nl;
        }
        self.selected = self.layers.get(idx[0]).map(|l| l.id);
        self.dirty = true;
        self.status = "Outline turned into a filled shape".into();
    }

    /// Border around the selection (or the whole design): grown by a distance,
    /// as a satin outline on top or a backing fill underneath.
    fn add_border(&mut self) {
        let idx = {
            let s = self.sel_layers();
            if s.is_empty() {
                (0..self.layers.len()).filter(|&i| self.layers[i].visible).collect()
            } else {
                s
            }
        };
        if idx.is_empty() {
            return;
        }
        let parts: Vec<(Vec<stitchforge::geom::Ring>, FillRule)> =
            idx.iter().flat_map(|&li| self.layer_area(li)).collect();
        // a border follows the outside only: holes are filled first
        let merged = stitchforge::geom::outer_rings(&vector::boolean(&parts, BoolOp::Union));
        let grown = vector::offset(&merged, FillRule::EvenOdd, self.border_dist, self.border_round);
        let grown = stitchforge::geom::outer_rings(&grown);
        if grown.is_empty() {
            self.error = Some("Border: nothing to go around".into());
            return;
        }
        self.hard_checkpoint();
        let id = self.next_id;
        self.next_id += 1;
        let curves = vector::rings_to_curves(&grown);
        if self.border_outline {
            // only the outside edge(s): a satin outline sewn last
            let mut sh = Shape {
                kind: ShapeKind::Line {
                    paths: Vec::new(),
                    width: self.border_width,
                },
                rgb: self.style.stroke,
                label: "Border".into(),
                group: 0,
                opts: ShapeOpts::default(),
                curves,
            };
            sh.reflatten();
            let l = Layer::new(id, "Border".into(), vec![sh], true);
            self.layers.push(l);
        } else {
            let mut sh = Shape {
                kind: ShapeKind::Area {
                    rings: Vec::new(),
                    rule: FillRule::EvenOdd,
                },
                rgb: self.style.fill,
                label: "Backing".into(),
                group: 0,
                opts: ShapeOpts {
                    keep_under: true,
                    ..ShapeOpts::default()
                },
                curves,
            };
            sh.reflatten();
            let mut l = Layer::new(id, "Backing".into(), vec![sh], false);
            l.opts.keep_under = true;
            self.layers.insert(0, l);
        }
        self.selected = Some(id);
        self.multi = [id].into_iter().collect();
        self.show_border = false;
        self.dirty = true;
        self.status = "Border added".into();
    }

    fn flip(&mut self, horizontal: bool) {
        let idx = self.sel_layers();
        if idx.is_empty() {
            return;
        }
        self.hard_checkpoint();
        // flip the whole selection as one: mirror each layer about the
        // selection's centre
        let mut b = BBox::empty();
        for &li in &idx {
            let lb = self.layers[li].bbox();
            b.add(lb.min);
            b.add(lb.max);
        }
        let c = b.center();
        for &li in &idx {
            let l = &mut self.layers[li];
            let lc = l.center();
            if horizontal {
                l.sx = -l.sx;
                l.rotation = -l.rotation;
                l.offset.x += 2.0 * (c.x - lc.x);
            } else {
                l.sy = -l.sy;
                l.rotation = -l.rotation;
                l.offset.y += 2.0 * (c.y - lc.y);
            }
        }
        self.dirty = true;
    }

    /// Align the selection: 0 left, 1 centre, 2 right, 3 top, 4 middle, 5 bottom,
    /// 6 spread evenly across, 7 spread evenly down.
    fn align(&mut self, how: u8) {
        let idx = self.sel_layers();
        if idx.len() < 2 {
            return;
        }
        self.hard_checkpoint();
        let mut b = BBox::empty();
        let boxes: Vec<BBox> = idx.iter().map(|&li| self.layers[li].bbox()).collect();
        for lb in &boxes {
            b.add(lb.min);
            b.add(lb.max);
        }
        let mut order: Vec<usize> = (0..idx.len()).collect();
        if how == 6 {
            order.sort_by(|&i, &j| boxes[i].center().x.partial_cmp(&boxes[j].center().x).unwrap());
        } else if how == 7 {
            order.sort_by(|&i, &j| boxes[i].center().y.partial_cmp(&boxes[j].center().y).unwrap());
        }
        let n = idx.len();
        for (rank, &k) in order.iter().enumerate() {
            let lb = boxes[k];
            let t = if n > 1 { rank as f64 / (n - 1) as f64 } else { 0.0 };
            let d = match how {
                0 => Pt::new(b.min.x - lb.min.x, 0.0),
                1 => Pt::new(b.center().x - lb.center().x, 0.0),
                2 => Pt::new(b.max.x - lb.max.x, 0.0),
                3 => Pt::new(0.0, b.min.y - lb.min.y),
                4 => Pt::new(0.0, b.center().y - lb.center().y),
                5 => Pt::new(0.0, b.max.y - lb.max.y),
                6 => {
                    let lo = boxes[order[0]].center().x;
                    let hi = boxes[order[n - 1]].center().x;
                    Pt::new(lo + (hi - lo) * t - lb.center().x, 0.0)
                }
                _ => {
                    let lo = boxes[order[0]].center().y;
                    let hi = boxes[order[n - 1]].center().y;
                    Pt::new(0.0, lo + (hi - lo) * t - lb.center().y)
                }
            };
            let li = idx[k];
            self.layers[li].offset = self.layers[li].offset.add(d);
        }
        self.dirty = true;
    }

    fn export_art_svg(&mut self) {
        let svg = stitchforge::project::art_svg(&self.layers, &self.chart);
        let name = format!("{}-art.svg", self.design_name);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Save the artwork as SVG")
                .add_filter("SVG", &["svg"])
                .set_file_name(&name);
            if let Some(d) = &self.last_dir {
                dlg = dlg.set_directory(d);
            }
            if let Some(p) = dlg.save_file() {
                match std::fs::write(&p, svg) {
                    Ok(()) => self.status = format!("Saved the artwork to {}", p.display()),
                    Err(e) => self.error = Some(format!("Couldn't save: {e}")),
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            web::download(&name, svg.into_bytes());
        }
    }

    /// Path operations, alignment and border, for the menu and the panel.
    fn path_tools(&mut self, ui: &mut egui::Ui, in_menu: bool) {
        let n = self.sel_layers().len();
        let mut go: Option<u8> = None;
        let b = |ui: &mut egui::Ui, on: bool, label: &str, tip: &str| -> bool {
            ui.add_enabled(on, egui::Button::new(label)).on_hover_text(tip).clicked()
        };
        let wrap = |ui: &mut egui::Ui, f: &mut dyn FnMut(&mut egui::Ui)| {
            if in_menu {
                f(ui)
            } else {
                ui.horizontal_wrapped(|ui| f(ui));
            }
        };
        wrap(ui, &mut |ui| {
            if b(ui, n >= 2, "Union", "Merge the selected shapes into one") { go = Some(0); }
            if b(ui, n >= 2, "Subtract", "Cut the upper shapes out of the lowest one") { go = Some(1); }
            if b(ui, n >= 2, "Intersect", "Keep only where they all overlap") { go = Some(2); }
            if b(ui, n >= 2, "Exclude", "Keep where exactly one covers") { go = Some(3); }
        });
        if in_menu { ui.separator(); }
        wrap(ui, &mut |ui| {
            if b(ui, n >= 2, "Combine", "Put the selected shapes in one layer") { go = Some(4); }
            if b(ui, n == 1, "Break apart", "Split a layer into its separate pieces") { go = Some(5); }
            if b(ui, n >= 1, "Outline to fill", "Turn an outline into the filled shape it covers") { go = Some(6); }
            if b(ui, true, "Border…", "Add a border around the selection (or the whole design)") { go = Some(7); }
        });
        if in_menu { ui.separator(); }
        wrap(ui, &mut |ui| {
            if b(ui, n >= 1, "Flip ↔", "Mirror left to right") { go = Some(8); }
            if b(ui, n >= 1, "Flip ↕", "Mirror top to bottom") { go = Some(9); }
        });
        if !in_menu && n >= 2 {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Align").weak());
                for (k, lbl, tip) in [
                    (10u8, "Left", "Align left edges"),
                    (11, "Center", "Align centres across"),
                    (12, "Right", "Align right edges"),
                    (13, "Top", "Align top edges"),
                    (14, "Middle", "Align centres down"),
                    (15, "Bottom", "Align bottom edges"),
                ] {
                    if b(ui, true, lbl, tip) { go = Some(k); }
                }
                if b(ui, n >= 3, "Spread ↔", "Even spacing across") { go = Some(16); }
                if b(ui, n >= 3, "Spread ↕", "Even spacing down") { go = Some(17); }
            });
        }
        if let Some(k) = go {
            if in_menu {
                ui.close();
            }
            match k {
                0 => self.path_bool(BoolOp::Union),
                1 => self.path_bool(BoolOp::Subtract),
                2 => self.path_bool(BoolOp::Intersect),
                3 => self.path_bool(BoolOp::Exclude),
                4 => self.combine(),
                5 => self.break_apart(),
                6 => self.outline_stroke(),
                7 => self.show_border = true,
                8 => self.flip(true),
                9 => self.flip(false),
                10..=17 => self.align(k - 10),
                _ => {}
            }
        }
    }

    /// Style of new shapes (shown while a drawing tool is active).
    fn draw_options(&mut self, ui: &mut egui::Ui) {
        section(ui, "New shapes");
        let st = &mut self.style;
        ui.horizontal(|ui| {
            ui.checkbox(&mut st.fill_on, "Fill");
            ui.color_edit_button_srgb(&mut st.fill);
            ui.add_space(8.0);
            ui.checkbox(&mut st.stroke_on, "Outline");
            ui.color_edit_button_srgb(&mut st.stroke);
        });
        ui.horizontal(|ui| {
            ui.label("Outline width");
            ui.add(egui::DragValue::new(&mut st.stroke_w).speed(0.05).range(0.2..=10.0).suffix(" mm").max_decimals(2));
            ui.label(RichText::new("≥ 1.2 mm sews as satin").weak().small());
        });
        match self.tool {
            Tool::Rect => {
                ui.horizontal(|ui| {
                    ui.label("Corner radius");
                    ui.add(egui::DragValue::new(&mut st.corner).speed(0.1).range(0.0..=50.0).suffix(" mm").max_decimals(1));
                });
            }
            Tool::Polygon => {
                ui.horizontal(|ui| {
                    ui.label("Points");
                    ui.add(egui::DragValue::new(&mut st.sides).range(3..=24));
                    ui.checkbox(&mut st.star, "Star");
                    if st.star {
                        ui.label("Inner");
                        ui.add(egui::DragValue::new(&mut st.inner).speed(0.01).range(0.1..=0.9).max_decimals(2));
                    }
                });
            }
            _ => {}
        }
        ui.checkbox(&mut self.snap, "Snap to the 1 mm grid")
            .on_hover_text("Points always snap to existing anchors nearby");
        ui.label(RichText::new("Open paths and outline-only shapes become outlines; closed shapes with Fill become filled layers.").weak().small());
        ui.add_space(4.0);
    }

    /// Border settings window.
    /// Side-by-side guide to thread weights: what each is for, its limits, and
    /// a true-scale swatch of a satin column sewn in it.
    fn weights_window(&mut self, ctx: &egui::Context) {
        if !self.show_weights {
            return;
        }
        let mut open = true;
        let thin = self.thinnest_strokes();
        egui::Window::new("Thread weights")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(RichText::new("Higher number = finer thread. Stitch settings are tuned for 40 wt and scale for the others.").weak());
                ui.add_space(4.0);
                egui::Grid::new("wt-grid").num_columns(7).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                    for h in ["", "Swatch (8×)", "Needle", "Spacing", "Narrowest satin", "Smallest letters", "Best for"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for w in thread::ThreadWeight::ALL.iter().rev() {
                        let r = if *w == self.recipe.weight {
                            RichText::new(format!("{} (design)", w.label())).strong()
                        } else {
                            RichText::new(w.label())
                        };
                        ui.label(r);
                        weight_swatch(ui, *w, self.recipe.satin_spacing * w.spacing_factor());
                        ui.label(w.needle());
                        ui.label(format!("{:.2} mm", self.recipe.satin_spacing * w.spacing_factor()));
                        ui.label(format!("{:.1} mm", w.min_column()));
                        ui.label(format!("{:.1} mm ({:.2} in)", w.min_text_height(), w.min_text_height() / 25.4));
                        ui.label(w.use_for());
                        ui.end_row();
                    }
                });
                if !thin.is_empty() {
                    section(ui, "This design");
                    for (name, wt, mm) in thin.iter().take(8) {
                        let ok: Vec<&str> = thread::ThreadWeight::ALL
                            .iter()
                            .rev()
                            .filter(|w| *mm >= w.min_column() - 0.05)
                            .map(|w| w.label())
                            .collect();
                        ui.label(format!(
                            "{name}: thinnest stroke {:.1} mm on {} — {}",
                            mm,
                            wt.label(),
                            if ok.is_empty() { "too thin for any weight; enlarge it".to_string() } else { format!("works with {}", ok.join(", ")) }
                        ));
                    }
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Tip: the Measure tool (D) shows which weights suit any column, stroke or letter you drag across.").weak().small());
            });
        if !open {
            self.show_weights = false;
        }
    }

    /// Thinnest satin stroke per layer from the last build: (layer name, its weight, mm).
    fn thinnest_strokes(&self) -> Vec<(String, thread::ThreadWeight, f64)> {
        let Some(d) = &self.design else { return Vec::new() };
        let mut out = Vec::new();
        for (li, l) in self.layers.iter().enumerate() {
            if !d.objects.iter().any(|o| o.layer == li && o.stitch_type.ends_with("atin")) {
                continue;
            }
            let mut best = f64::MAX;
            let k = (l.sx * l.sy).abs().sqrt();
            for sh in &l.shapes {
                if let import::ShapeKind::Area { rings, rule } = &sh.kind {
                    let a = stitchforge::geom::rings_area(rings, *rule) * k * k;
                    let per = stitchforge::geom::rings_perimeter(rings) * k;
                    if per > 0.0 && a > 0.5 {
                        best = best.min(2.0 * a / per);
                    }
                }
            }
            if best < f64::MAX {
                out.push((l.name.clone(), l.opts.weight.unwrap_or(self.recipe.weight), best));
            }
        }
        out.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
        out
    }

    fn border_window(&mut self, ctx: &egui::Context) {
        if !self.show_border {
            return;
        }
        let mut open = true;
        let mut go = false;
        egui::Window::new("Add a border")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let n = self.sel_layers().len();
                ui.label(if n == 0 {
                    "Around the whole design".to_string()
                } else {
                    format!("Around the {n} selected layer(s)")
                });
                ui.horizontal(|ui| {
                    ui.label("Distance out");
                    ui.add(egui::DragValue::new(&mut self.border_dist).speed(0.1).range(-10.0..=30.0).suffix(" mm").max_decimals(1));
                });
                ui.checkbox(&mut self.border_round, "Rounded corners");
                ui.radio_value(&mut self.border_outline, true, "Satin outline (sewn last, on top)");
                ui.radio_value(&mut self.border_outline, false, "Backing shape (fill, sewn first, underneath)");
                if self.border_outline {
                    ui.horizontal(|ui| {
                        ui.label("Satin width");
                        ui.add(egui::DragValue::new(&mut self.border_width).speed(0.05).range(0.6..=8.0).suffix(" mm").max_decimals(1));
                    });
                }
                if ui.button(RichText::new("Add border").strong()).clicked() {
                    go = true;
                }
            });
        if go {
            self.add_border();
        }
        if !open {
            self.show_border = false;
        }
    }

    /// Mouse input while editing a design object's stitches or shape.
    fn edit_input(
        &mut self,
        resp: &egui::Response,
        ui: &egui::Ui,
        to_mm: &dyn Fn(Pos2) -> Pt,
        to_screen: &dyn Fn(Pt) -> Pos2,
        zoom: f32,
    ) {
        let Some(li) = self.edit_layer() else { return };
        let shift = ui.input(|i| i.modifiers.shift);
        let latest = ui.input(|i| i.pointer.latest_pos());
        let pan = |s: &mut Self| {
            let dlt = resp.drag_delta();
            s.view_center = s
                .view_center
                .sub(Pt::new((dlt.x / zoom) as f64, (dlt.y / zoom) as f64));
        };
        if resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle)
        {
            pan(self);
            return;
        }
        let primary_start = resp.drag_started_by(egui::PointerButton::Primary);
        let primary_drag = resp.dragged_by(egui::PointerButton::Primary);
        match self.edit {
            Edit::Stitches(_) => {
                let near = |s: &Self, pos: Pos2| -> Option<usize> {
                    let l = &s.layers[li];
                    let mut best: Option<(usize, f32)> = None;
                    for (i, n) in l.embroidery.needles.iter().enumerate() {
                        let d = to_screen(l.tf(n.p)).distance(pos);
                        if d < 8.0 && best.map(|b| d < b.1).unwrap_or(true) {
                            best = Some((i, d));
                        }
                    }
                    best.map(|b| b.0)
                };
                if primary_start {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if let Some(i) = near(self, pos) {
                            if !self.st_sel.contains(&i) {
                                if !shift {
                                    self.st_sel.clear();
                                }
                                self.st_sel.insert(i);
                            }
                            self.hard_checkpoint();
                            let l = &self.layers[li];
                            let orig = self
                                .st_sel
                                .iter()
                                .filter_map(|&k| l.embroidery.needles.get(k).map(|n| (k, l.tf(n.p))))
                                .collect();
                            self.edit_drag = Some(EditDrag::Stitches(to_mm(pos), orig));
                        } else if ui.input(|i| i.modifiers.alt) {
                            self.edit_drag = Some(EditDrag::Lasso(vec![pos]));
                        } else {
                            self.edit_drag = Some(EditDrag::Box(pos));
                        }
                    }
                }
                if let (true, Some(pos), Some(EditDrag::Lasso(pts))) = (primary_drag, latest, &mut self.edit_drag) {
                    if pts.last().map(|l| l.distance(pos) > 2.0).unwrap_or(true) {
                        pts.push(pos);
                    }
                }
                if primary_drag {
                    if let (Some(pos), Some(EditDrag::Stitches(start, orig))) = (latest, &self.edit_drag) {
                        let d = to_mm(pos).sub(*start);
                        let l = &mut self.layers[li];
                        for (k, p0) in orig {
                            let q = l.inv(p0.add(d));
                            l.embroidery.needles[*k].p = q;
                        }
                    }
                }
                if resp.drag_stopped() {
                    match self.edit_drag.take() {
                        Some(EditDrag::Stitches(..)) => self.touch_stitches(li),
                        Some(EditDrag::Lasso(pts)) if pts.len() >= 3 => {
                            if !shift {
                                self.st_sel.clear();
                            }
                            let poly: Vec<Pt> = pts.iter().map(|p| Pt::new(p.x as f64, p.y as f64)).collect();
                            let l = &self.layers[li];
                            for (i, n) in l.embroidery.needles.iter().enumerate() {
                                let q = to_screen(l.tf(n.p));
                                if stitchforge::cut::point_in_poly(&poly, Pt::new(q.x as f64, q.y as f64)) {
                                    self.st_sel.insert(i);
                                }
                            }
                        }
                        Some(EditDrag::Box(a)) => {
                            if let Some(b) = latest {
                                let r = Rect::from_two_pos(a, b);
                                if !shift {
                                    self.st_sel.clear();
                                }
                                let l = &self.layers[li];
                                for (i, n) in l.embroidery.needles.iter().enumerate() {
                                    if r.contains(to_screen(l.tf(n.p))) {
                                        self.st_sel.insert(i);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if resp.double_clicked() {
                    // add a needle point on the nearest stitch
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let l = &self.layers[li];
                        let pts: Vec<Pos2> = l.embroidery.needles.iter().map(|n| to_screen(l.tf(n.p))).collect();
                        let mut best: Option<(usize, f32, f32)> = None;
                        for k in 0..pts.len().saturating_sub(1) {
                            if l.embroidery.needles[k + 1].jump {
                                continue;
                            }
                            let (a, b) = (pts[k], pts[k + 1]);
                            let ab = b - a;
                            let t = ((pos - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                            let d = (a + ab * t).distance(pos);
                            if d < 8.0 && best.map(|x| d < x.1).unwrap_or(true) {
                                best = Some((k, d, t));
                            }
                        }
                        if let Some((k, _, t)) = best {
                            self.hard_checkpoint();
                            let l = &mut self.layers[li];
                            let a = l.embroidery.needles[k].p;
                            let b = l.embroidery.needles[k + 1].p;
                            l.embroidery.needles.insert(k + 1, Needle { p: a.lerp(b, t as f64), jump: false, under: l.embroidery.needles[k + 1].under });
                            self.st_sel = [k + 1].into_iter().collect();
                            self.touch_stitches(li);
                        }
                    }
                } else if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        match near(self, pos) {
                            Some(i) if shift => {
                                if !self.st_sel.remove(&i) {
                                    self.st_sel.insert(i);
                                }
                            }
                            Some(i) => self.st_sel = [i].into_iter().collect(),
                            None if !shift => self.st_sel.clear(),
                            None => {}
                        }
                    }
                }
            }
            Edit::Shape(_) => {
                let hit = |s: &Self, pos: Pos2| -> Option<NodeRef> {
                    let l = &s.layers[li];
                    // handles of the selected anchor first, then any anchor
                    if let Some(r) = s.node_sel {
                        if let Some(n) = l.shapes.get(r.shape).and_then(|sh| sh.curves.get(r.path)).and_then(|c| c.nodes.get(r.node)) {
                            for (part, h) in [(1u8, n.cin), (2u8, n.cout)] {
                                if h.dist(n.p) > 1e-6 && to_screen(l.tf(h)).distance(pos) < 8.0 {
                                    return Some(NodeRef { part, ..r });
                                }
                            }
                        }
                    }
                    for (si, sh) in l.shapes.iter().enumerate() {
                        for (ci, c) in sh.curves.iter().enumerate() {
                            for (ni, n) in c.nodes.iter().enumerate() {
                                if to_screen(l.tf(n.p)).distance(pos) < 8.0 {
                                    return Some(NodeRef { shape: si, path: ci, node: ni, part: 0 });
                                }
                            }
                        }
                    }
                    None
                };
                if primary_start {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if let Some(r) = hit(self, pos) {
                            self.hard_checkpoint();
                            self.node_sel = Some(NodeRef { part: 0, ..r });
                            let n = self.layers[li].shapes[r.shape].curves[r.path].nodes[r.node];
                            self.edit_drag = Some(EditDrag::Node(r, to_mm(pos), n));
                        } else {
                            self.edit_drag = None;
                        }
                    }
                }
                if primary_drag {
                    match (&self.edit_drag, latest) {
                        (Some(EditDrag::Node(r, start, orig)), Some(pos)) => {
                            let (r, start, orig) = (*r, *start, *orig);
                            let l = &mut self.layers[li];
                            let d = l.inv(to_mm(pos)).sub(l.inv(start));
                            let smooth = orig.is_smooth();
                            let n = &mut l.shapes[r.shape].curves[r.path].nodes[r.node];
                            match r.part {
                                0 => {
                                    n.p = orig.p.add(d);
                                    n.cin = orig.cin.add(d);
                                    n.cout = orig.cout.add(d);
                                }
                                1 => {
                                    n.cin = orig.cin.add(d);
                                    if smooth {
                                        let len = orig.cout.dist(orig.p);
                                        n.cout = n.p.sub(n.cin.sub(n.p).norm().mul(len));
                                    }
                                }
                                _ => {
                                    n.cout = orig.cout.add(d);
                                    if smooth {
                                        let len = orig.cin.dist(orig.p);
                                        n.cin = n.p.sub(n.cout.sub(n.p).norm().mul(len));
                                    }
                                }
                            }
                            l.shapes[r.shape].reflatten();
                        }
                        (None, _) => pan(self),
                        _ => {}
                    }
                }
                if resp.drag_stopped() {
                    if let Some(EditDrag::Node(..)) = self.edit_drag.take() {
                        self.dirty = true;
                    }
                }
                if resp.double_clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let l = &self.layers[li];
                        let q = l.inv(to_mm(pos));
                        let scale = l.sx.abs().min(l.sy.abs()).max(1e-6);
                        let tol = (8.0 / zoom) as f64 / scale;
                        let mut best: Option<(usize, usize, usize, f64, f64)> = None;
                        for (si, sh) in l.shapes.iter().enumerate() {
                            for (ci, c) in sh.curves.iter().enumerate() {
                                if let Some((seg, u, d)) = c.nearest(q) {
                                    if d < tol && best.map(|b| d < b.4).unwrap_or(true) {
                                        best = Some((si, ci, seg, u, d));
                                    }
                                }
                            }
                        }
                        if let Some((si, ci, seg, u, _)) = best {
                            self.hard_checkpoint();
                            let sh = &mut self.layers[li].shapes[si];
                            let n = sh.curves[ci].nodes.len();
                            sh.curves[ci].split(seg, u.clamp(0.02, 0.98));
                            let j = (seg + 1) % n;
                            let ni = if j == 0 { n } else { j };
                            sh.reflatten();
                            self.node_sel = Some(NodeRef { shape: si, path: ci, node: ni, part: 0 });
                            self.dirty = true;
                        }
                    }
                } else if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        self.node_sel = hit(self, pos).map(|r| NodeRef { part: 0, ..r });
                    }
                }
            }
            Edit::None => {}
        }
    }

    /// Draw the object being edited: needle points, or anchors and handles.
    fn edit_overlay(&self, painter: &egui::Painter, to_screen: &dyn Fn(Pt) -> Pos2, zoom: f32, ui: &egui::Ui) {
        let Some(li) = self.edit_layer() else { return };
        let l = &self.layers[li];
        let accent = Color32::from_rgb(30, 110, 230);
        let orange = Color32::from_rgb(245, 140, 20);
        match self.edit {
            Edit::Stitches(_) => {
                let rgb = self
                    .design
                    .as_ref()
                    .and_then(|d| d.objects.iter().find(|o| o.layer == li).map(|o| d.stops[o.stop].thread.rgb))
                    .or_else(|| self.layer_thread(li).map(|t| t.rgb))
                    .unwrap_or([60, 60, 60]);
                let col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                let pts: Vec<(Pos2, bool)> = l
                    .embroidery
                    .needles
                    .iter()
                    .map(|n| (to_screen(l.tf(n.p)), n.jump))
                    .collect();
                let w = (0.35 * zoom).clamp(1.0, 6.0);
                let jump = Stroke::new(1.0_f32, Color32::from_black_alpha(90));
                for k in 1..pts.len() {
                    let (a, b) = (pts[k - 1].0, pts[k].0);
                    if pts[k].1 {
                        for seg in EShape::dashed_line(&[a, b], jump, 4.0, 3.0) {
                            painter.add(seg);
                        }
                    } else {
                        painter.line_segment([a, b], Stroke::new(w, col));
                    }
                }
                let r = if zoom >= 6.0 { 2.6 } else { 1.8 };
                for (i, (p, _)) in pts.iter().enumerate() {
                    if self.st_sel.contains(&i) {
                        painter.circle_filled(*p, r + 1.6, orange);
                        painter.circle_stroke(*p, r + 1.6, Stroke::new(1.0_f32, Color32::BLACK));
                    } else if zoom >= 3.0 {
                        painter.circle_filled(*p, r, Color32::WHITE);
                        painter.circle_stroke(*p, r, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
                    }
                }
                if let Some(EditDrag::Lasso(pts)) = &self.edit_drag {
                    if pts.len() >= 2 {
                        let mut closed = pts.clone();
                        closed.push(pts[0]);
                        for seg in EShape::dashed_line(&closed, Stroke::new(1.2_f32, accent), 4.0, 3.0) {
                            painter.add(seg);
                        }
                    }
                }
                if let Some(EditDrag::Box(a)) = &self.edit_drag {
                    if let Some(b) = ui.input(|i| i.pointer.latest_pos()) {
                        let rr = Rect::from_two_pos(*a, b);
                        painter.rect_filled(rr, 0.0, accent.gamma_multiply(0.12));
                        painter.rect_stroke(rr, 0.0, Stroke::new(1.0_f32, accent), egui::StrokeKind::Inside);
                    }
                }
            }
            Edit::Shape(_) => {
                for (si, sh) in l.shapes.iter().enumerate() {
                    for (ci, c) in sh.curves.iter().enumerate() {
                        let poly: Vec<Pos2> = c.flatten().iter().map(|p| to_screen(l.tf(*p))).collect();
                        painter.add(EShape::line(poly.clone(), Stroke::new(3.0_f32, Color32::from_white_alpha(200))));
                        painter.add(EShape::line(poly, Stroke::new(1.5_f32, accent)));
                        for (ni, n) in c.nodes.iter().enumerate() {
                            let sel = self.node_sel.map(|r| r.shape == si && r.path == ci && r.node == ni).unwrap_or(false);
                            let p = to_screen(l.tf(n.p));
                            if sel {
                                for h in [n.cin, n.cout] {
                                    if h.dist(n.p) > 1e-6 {
                                        let hp = to_screen(l.tf(h));
                                        painter.line_segment([p, hp], Stroke::new(1.0_f32, accent));
                                        painter.circle_filled(hp, 4.0, Color32::WHITE);
                                        painter.circle_stroke(hp, 4.0, Stroke::new(1.5_f32, accent));
                                    }
                                }
                            }
                            let rect = Rect::from_center_size(p, Vec2::splat(if sel { 9.0 } else { 7.0 }));
                            painter.rect_filled(rect, 1.0, if sel { accent } else { Color32::WHITE });
                            painter.rect_stroke(rect, 1.0, Stroke::new(1.2_f32, accent), egui::StrokeKind::Inside);
                        }
                    }
                }
            }
            Edit::None => {}
        }
    }

    /// The tool strip down the left edge.
    // ---------- shell: menu bar, document bar, dock, hint and status bars ----------

    fn tokens(&self) -> theme::Tokens {
        theme::tokens(self.theme)
    }

    fn set_theme(&mut self, ctx: &egui::Context, k: ThemeKind) {
        self.theme = k;
        theme::apply(ctx, k);
        self.save_prefs();
    }

    fn save_prefs(&self) {
        theme::save_prefs(&theme::Prefs {
            theme: self.theme,
            workspace: self.workspace.key().into(),
            keys: if self.ai_keys { "illustrator" } else { "classic" }.into(),
            libs: self.libs.iter().filter(|l| l.on && !l.key.starts_with("design:")).map(|l| l.key.clone()).collect(),
        });
    }

    /// Switch workspace: which panels lead, and Simple or full controls.
    fn set_workspace(&mut self, w: Workspace) {
        self.workspace = w;
        self.dock_open = true;
        let to_design = |s: &mut Self| {
            if s.tab != Tab::Design {
                s.set_tab(Tab::Design);
            }
        };
        match w {
            Workspace::Design => {
                self.pro = true;
                to_design(self);
                self.dock = Dock::Properties;
            }
            Workspace::Lettering => {
                self.pro = true;
                to_design(self);
                self.dock = Dock::Properties;
                if self.machine.is_none() {
                    self.set_tool(Tool::Text);
                }
                self.status = "Lettering: click where the words go, or Type > Add text".into();
            }
            Workspace::NameDrops => {
                self.pro = true;
                if self.machine.is_none() && !self.layers.is_empty() {
                    if self.tab != Tab::Drops {
                        self.set_tab(Tab::Drops);
                    }
                } else {
                    self.status = "Name drops need a design first: open or draw the logo, then add a name layer".into();
                }
                self.dock = Dock::Properties;
            }
            Workspace::Production => {
                self.pro = true;
                to_design(self);
                self.dock = Dock::Production;
            }
            Workspace::Simple => {
                self.pro = false;
                to_design(self);
            }
        }
        self.save_prefs();
    }

    /// Dock tabs offered now (Simple hides the thread libraries).
    fn dock_tabs(&self) -> Vec<Dock> {
        vec![Dock::Properties, Dock::Layers, Dock::Threads, Dock::Production]
    }

    fn dock_label(&self, d: Dock) -> &'static str {
        match d {
            Dock::Properties if self.tab == Tab::Drops && self.machine.is_none() => "Names",
            Dock::Properties => "Properties",
            Dock::Layers if self.machine.is_some() => "Stitch file",
            Dock::Layers if self.tab == Tab::Drops => "Drops",
            Dock::Layers => "Layers",
            Dock::Threads => "Threads",
            Dock::Production => "Production",
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = self.tokens();
        let art = self.machine.is_none();
        let has = art && !self.layers.is_empty();
        let n_sel = self.sel_layers().len();
        let sel = self.sel_idx().filter(|_| art && self.tab == Tab::Design);
        let item = |ui: &mut egui::Ui, on: bool, label: &str, keys: &str| -> bool {
            let mut b = egui::Button::new(label);
            if !keys.is_empty() {
                b = b.shortcut_text(keys);
            }
            ui.add_enabled(on, b).clicked()
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // brand mark
            let (r, _) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::hover());
            ui.painter().rect_filled(r, 5.0, t.accent);
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "SF", egui::FontId::proportional(11.0), Color32::WHITE);
            ui.add_space(8.0);
            ui.menu_button("File", |ui| {
                ui.set_min_width(240.0);
                if item(ui, true, "New", &kb("N")) {
                    ui.close();
                    self.guard(Pending::New, &ctx);
                }
                if item(ui, true, "Open…", &kb("O")) {
                    ui.close();
                    self.guard(Pending::Open, &ctx);
                }
                ui.separator();
                if item(ui, has, "Save", &kb("S")) {
                    ui.close();
                    self.save();
                }
                if item(ui, has, "Save As…", &kb("⇧S")) {
                    ui.close();
                    self.save_as();
                }
                ui.separator();
                if item(ui, has, "Add art…", "") {
                    ui.close();
                    self.pick_svg(true);
                }
                if item(ui, art, "Trace a picture…", "") {
                    ui.close();
                    pick(
                        if self.layers.is_empty() { Want::Open } else { Want::AddSvg },
                        "Trace a picture",
                        &[("Pictures, mockups, PDF", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "pdf", "ai"])],
                        self.last_dir.as_ref(),
                    );
                }
                ui.separator();
                if item(ui, self.design.is_some(), "Export machine files…", &kb("E")) {
                    ui.close();
                    self.show_export = true;
                }
                if item(ui, has, "Export artwork as SVG…", "") {
                    ui.close();
                    self.export_art_svg();
                }
                ui.separator();
                if item(ui, true, "Sample logo", "") {
                    ui.close();
                    self.guard(Pending::Sample, &ctx);
                }
                if item(ui, true, "Calibration sampler", "") {
                    ui.close();
                    self.guard(Pending::Sampler, &ctx);
                }
            });
            ui.menu_button("Edit", |ui| {
                ui.set_min_width(220.0);
                if item(ui, !self.undo.is_empty(), "Undo", &kb("Z")) {
                    ui.close();
                    self.do_undo();
                }
                if item(ui, !self.redo.is_empty(), "Redo", &kb("⇧Z")) {
                    ui.close();
                    self.do_redo();
                }
                ui.separator();
                if item(ui, sel.is_some(), "Duplicate", &kb("D")) {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("dup", li);
                    }
                }
                if item(ui, sel.is_some(), "Delete", "Del") {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("del", li);
                    }
                }
            });
            ui.menu_button("Object", |ui| {
                ui.set_min_width(220.0);
                ui.add_enabled_ui(has, |ui| self.path_tools(ui, true));
                ui.separator();
                if item(ui, sel.map(|li| li > 0).unwrap_or(false), "Sew earlier", "") {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("up", li);
                    }
                }
                if item(ui, sel.map(|li| li + 1 < self.layers.len()).unwrap_or(false), "Sew later", "") {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("down", li);
                    }
                }
                if item(ui, has, "Auto-sequence layers", "") {
                    ui.close();
                    self.auto_sequence();
                }
            });
            ui.menu_button("Type", |ui| {
                ui.set_min_width(220.0);
                if item(ui, art, "Add text…", "") {
                    ui.close();
                    self.show_text = true;
                }
                if item(ui, art, "Text tool", Tool::Text.key_label(self.ai_keys)) {
                    ui.close();
                    self.set_tool(Tool::Text);
                }
                ui.separator();
                if item(ui, has, "Name drops…", "") {
                    ui.close();
                    self.set_workspace(Workspace::NameDrops);
                }
            });
            ui.menu_button("Stitch", |ui| {
                ui.set_min_width(240.0);
                if item(ui, sel.is_some(), "Edit shape points", Tool::Nodes.key_label(self.ai_keys)) {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("edit-shape", li);
                    }
                }
                if item(ui, sel.is_some(), "Edit stitch points", Tool::Stitches.key_label(self.ai_keys)) {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("edit-stitches", li);
                    }
                }
                if item(ui, sel.is_some(), "Direction lines", Tool::Guides.key_label(self.ai_keys)) {
                    ui.close();
                    if let Some(li) = sel {
                        self.layer_action("guides", li);
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.show_underlay, "Show underlay");
                ui.checkbox(&mut self.show_jumps, "Show jumps");
                ui.separator();
                if item(ui, true, "Thread weight guide…", "") {
                    ui.close();
                    self.show_weights = true;
                }
                if self.machine.is_some() && item(ui, true, "Turn stitches into objects", "") {
                    ui.close();
                    self.convert_machine();
                }
            });
            ui.menu_button("Select", |ui| {
                ui.set_min_width(200.0);
                if item(ui, has && self.tab == Tab::Design, "All", &kb("A")) {
                    ui.close();
                    self.select_all();
                }
                if item(ui, n_sel > 0, "None", "Esc") {
                    ui.close();
                    self.selected = None;
                    self.focus_part = None;
                }
            });
            ui.menu_button("View", |ui| {
                ui.set_min_width(220.0);
                for (k, v) in View::ALL.iter().enumerate() {
                    if ui.add(egui::Button::selectable(self.view == *v, v.label()).shortcut_text(format!("{}", k + 1))).clicked() {
                        ui.close();
                        self.view = *v;
                    }
                }
                ui.separator();
                if item(ui, true, "Fit in window", "F") {
                    ui.close();
                    self.fit = true;
                }
                if item(ui, true, "Zoom in", &kb("=")) {
                    ui.close();
                    self.zoom = (self.zoom * 1.25).min(200.0);
                }
                if item(ui, true, "Zoom out", &kb("-")) {
                    ui.close();
                    self.zoom = (self.zoom / 1.25).max(0.3);
                }
                if item(ui, true, "Actual size (100%)", &kb("1")) {
                    ui.close();
                    self.zoom = MM_PX;
                }
                ui.separator();
                ui.checkbox(&mut self.show_rulers, "Rulers");
                ui.checkbox(&mut self.show_hoop, "Hoop");
                ui.checkbox(&mut self.show_underlay, "Underlay");
                ui.checkbox(&mut self.show_jumps, "Jumps");
                ui.separator();
                ui.menu_button("Garment colour", |ui| self.garment_picker(ui));
            });
            ui.menu_button("Window", |ui| {
                ui.set_min_width(220.0);
                ui.menu_button("Workspace", |ui| {
                    for w in Workspace::ALL {
                        if ui.add(egui::Button::selectable(self.workspace == w, w.label())).on_hover_text(w.hint()).clicked() {
                            ui.close();
                            self.set_workspace(w);
                        }
                    }
                });
                ui.menu_button("Theme", |ui| {
                    for k in ThemeKind::ALL {
                        if ui.add(egui::Button::selectable(self.theme == k, k.label())).clicked() {
                            ui.close();
                            self.set_theme(&ctx, k);
                        }
                    }
                });
                ui.menu_button("Keyboard shortcuts", |ui| {
                    if ui.add(egui::Button::selectable(!self.ai_keys, "StitchForge")).clicked() {
                        ui.close();
                        self.ai_keys = false;
                        self.save_prefs();
                    }
                    if ui.add(egui::Button::selectable(self.ai_keys, "Vector-editor style")).on_hover_text("Tool keys like most vector drawing apps: A edits points, M rectangle, L ellipse").clicked() {
                        ui.close();
                        self.ai_keys = true;
                        self.save_prefs();
                    }
                });
                ui.separator();
                for d in self.dock_tabs() {
                    if ui.add(egui::Button::selectable(self.dock_open && self.dock == d, self.dock_label(d))).clicked() {
                        ui.close();
                        self.dock = d;
                        self.dock_open = true;
                    }
                }
                ui.separator();
                let lbl = if self.dock_open { "Hide panels" } else { "Show panels" };
                if item(ui, true, lbl, "") {
                    ui.close();
                    self.dock_open = !self.dock_open;
                }
            });
            ui.menu_button("Help", |ui| {
                ui.set_min_width(200.0);
                if item(ui, true, "Keyboard shortcuts", "?") {
                    ui.close();
                    self.show_keys = true;
                }
                if item(ui, true, "About StitchForge", "") {
                    ui.close();
                    self.show_about = true;
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let export = egui::Button::new(RichText::new("Export").color(Color32::WHITE).strong()).fill(t.accent);
                if ui
                    .add_enabled(self.design.is_some(), export)
                    .on_hover_text(format!("Machine files and operator sheet ({})", kb("E")))
                    .clicked()
                {
                    self.show_export = true;
                }
                let before = self.workspace;
                let mut w = self.workspace;
                egui::ComboBox::from_id_salt("workspace")
                    .width(130.0)
                    .selected_text(w.label())
                    .show_ui(ui, |ui| {
                        for x in Workspace::ALL {
                            ui.selectable_value(&mut w, x, x.label()).on_hover_text(x.hint());
                        }
                    })
                    .response
                    .on_hover_text("Workspace: which panels lead. Simple keeps only the essentials");
                if w != before {
                    self.set_workspace(w);
                }
                // theme: one click cycles Dark, Neutral grey, Light
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
                let c = r.center();
                ui.painter().circle_stroke(c, 7.0, Stroke::new(1.5_f32, t.text_weak));
                ui.painter().add(EShape::convex_polygon(
                    (0..=12).map(|i| {
                        let a = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                        c + Vec2::new(a.cos(), -a.sin()) * 7.0
                    }).collect(),
                    t.text_weak,
                    Stroke::NONE,
                ));
                if resp.on_hover_text(format!("Theme: {} (click to change)", self.theme.label())).clicked() {
                    let i = ThemeKind::ALL.iter().position(|k| *k == self.theme).unwrap_or(0);
                    self.set_theme(&ctx, ThemeKind::ALL[(i + 1) % ThemeKind::ALL.len()]);
                }
            });
        });
    }

    fn select_all(&mut self) {
        self.multi = self.layers.iter().filter(|l| l.visible).map(|l| l.id).collect();
        self.selected = self.multi.iter().next().copied();
    }

    fn garment_picker(&mut self, ui: &mut egui::Ui) {
        let cur = self.job.garment_rgb.unwrap_or(CANVAS);
        let before = self.job.garment_rgb;
        for (n, rgb) in GARMENTS {
            ui.horizontal(|ui| {
                swatch(ui, *rgb);
                if ui.selectable_label(cur == *rgb, *n).clicked() {
                    self.job.garment_rgb = if *rgb == CANVAS { None } else { Some(*rgb) };
                }
            });
        }
        let mut c = cur;
        ui.horizontal(|ui| {
            ui.label("Any colour");
            if ui.color_edit_button_srgb(&mut c).changed() {
                self.job.garment_rgb = Some(c);
            }
        });
        if self.job.garment_rgb != before {
            self.modified = true;
        }
    }

    /// The document tab, and the view switch.
    fn doc_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.tokens();
        ui.horizontal(|ui| {
            let pct = self.zoom / MM_PX * 100.0;
            let name = format!(
                "{}{}  @ {:.0}%  ({})",
                self.display_name(),
                if self.modified { " •" } else { "" },
                pct,
                self.view.label()
            );
            egui::Frame::new()
                .fill(t.panel)
                .corner_radius(CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                .inner_margin(egui::Margin::symmetric(14, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(name).strong().color(t.text))
                        .on_hover_text(if self.modified { "Unsaved changes" } else { "All changes saved" });
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                ui.toggle_value(&mut self.show_underlay, "Underlay")
                    .on_hover_text("Show the underlay in orange with the top stitches faded");
                ui.toggle_value(&mut self.show_jumps, "Jumps");
                ui.separator();
                // garment colour the design is shown on (also on the sheet)
                let g = self.job.garment_rgb.unwrap_or(CANVAS);
                let gname = GARMENTS.iter().find(|x| x.1 == g).map(|x| x.0).unwrap_or("Custom");
                ui.menu_button(format!("Garment: {gname}"), |ui| self.garment_picker(ui))
                    .response
                    .on_hover_text("The garment colour the design is shown on, also printed on the operator sheet");
                let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 7.0, Color32::from_rgb(g[0], g[1], g[2]));
                ui.painter().circle_stroke(r.center(), 7.0, Stroke::new(1.0_f32, t.border));
                ui.separator();
                egui::Frame::new()
                    .fill(t.field)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            // right-to-left row: last first
                            for (k, v) in View::ALL.iter().enumerate().rev() {
                                if self.machine.is_some() && *v == View::Vector {
                                    continue;
                                }
                                let on = self.view == *v;
                                let txt = RichText::new(v.label()).color(if on { Color32::WHITE } else { t.text_weak });
                                let b = egui::Button::new(txt).fill(if on { t.accent } else { Color32::TRANSPARENT });
                                if ui.add(b).on_hover_text(format!("Press {}", k + 1)).clicked() {
                                    self.view = *v;
                                }
                            }
                        });
                    });
            });
        });
    }

    /// One-line help for what the pointer does now, plus the sew-out player.
    fn hint_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.tokens();
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            ui.painter().circle_stroke(r.center(), 7.5, Stroke::new(1.2_f32, t.text_weak));
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(11.0), t.text_weak);
            ui.add_space(4.0);
            let parts = self.hint_parts();
            for (i, (b, rest)) in parts.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("|").color(t.border));
                }
                ui.label(RichText::new(*b).strong().color(t.text));
                ui.label(RichText::new(*rest).color(t.text_weak));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.player(ui));
        });
    }

    fn hint_parts(&self) -> Vec<(&'static str, &'static str)> {
        if self.machine.is_some() {
            return vec![("Drag", "to pan"), ("Scroll", "to zoom"), ("Stitch > Turn stitches into objects", "to edit")];
        }
        if self.tab == Tab::Drops {
            return vec![("Drops", "panel: pick a drop to see it"), ("Names", "panel: the sheet, layers and machine")];
        }
        match self.edit {
            Edit::Shape(_) => return vec![("Drag", "an anchor or handle"), ("Double-click", "the outline to add a point"), ("Esc", "to finish")],
            Edit::Stitches(_) => return vec![("Click", "a needle point"), ("Drag", "to move it"), ("Delete", "removes it"), ("Esc", "to finish")],
            Edit::None => {}
        }
        match self.tool {
            Tool::Select if self.selected.is_some() => vec![
                ("Drag", "to move"),
                ("Drag an angle end", "to turn the stitches"),
                ("Shift+Click", "to add to the selection"),
                ("Arrows", "nudge"),
            ],
            Tool::Select => vec![("Click", "an object to select it"), ("Shift+Click", "to select several"), ("Drag", "empty space to pan"), ("Double-click", "to fit")],
            Tool::Nodes => vec![("Click", "an object"), ("Drag", "its anchors and handles")],
            Tool::Stitches => vec![("Click", "an object"), ("Drag", "its needle points")],
            Tool::Pen => vec![("Click", "for a corner"), ("Drag", "for a curve"), ("Click the first point", "to close"), ("Enter", "to finish")],
            Tool::Rect => vec![("Drag", "to draw"), ("Shift", "for a square")],
            Tool::Ellipse => vec![("Drag", "to draw"), ("Shift", "for a circle")],
            Tool::Polygon => vec![("Drag", "from the centre"), ("Shift", "snaps the angle")],
            Tool::Line => vec![("Drag", "to draw"), ("Shift", "snaps to 15°")],
            Tool::Text => vec![("Click", "where the words go")],
            Tool::Measure => vec![("Drag", "across a column or gap"), ("Shift", "snaps to 15°")],
            Tool::Knife => vec![("Drag", "a line across a shape to cut it"), ("Shift", "snaps to 15°")],
            Tool::Eraser => vec![("Draw", "around the part to remove")],
            Tool::Guides => vec![("Drag", "across a fill to set the stitch direction"), ("Delete", "removes the picked line")],
        }
    }

    /// Sew-out player: play, rewind, scrub, speed.
    fn player(&mut self, ui: &mut egui::Ui) {
        let total = self.design.as_ref().map(|d| d.cmds.len()).unwrap_or(0);
        let on = total > 0;
        ui.add_enabled(
            on,
            egui::Slider::new(&mut self.speed, 50.0..=8000.0).logarithmic(true).show_value(false),
        )
        .on_hover_text("Sew-out speed");
        if let Some(d) = &self.design {
            let n = d.cmds[..self.progress.min(d.cmds.len())]
                .iter()
                .filter(|c| matches!(c, Cmd::Stitch(_)))
                .count();
            ui.label(RichText::new(format!("{} / {}", group(n), group(d.stats.stitches))).monospace());
        }
        ui.add_enabled(on, egui::Slider::new(&mut self.progress, 0..=total.max(1)).show_value(false));
        if ui.add_enabled(on, egui::Button::new("⏭")).on_hover_text("Show everything").clicked() {
            self.progress = total;
            self.playing = false;
        }
        if ui.add_enabled(on, egui::Button::new("⏮")).on_hover_text("Back to start").clicked() {
            self.progress = 0;
            self.playing = false;
        }
        let label = if self.playing { "⏸ Pause" } else { "▶ Sew-out" };
        if ui.add_enabled(on, egui::Button::new(label)).on_hover_text("Play the sew-out (Space)").clicked() {
            if !self.playing && self.progress >= total {
                self.progress = 0;
            }
            self.playing = !self.playing;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.tokens();
        ui.horizontal(|ui| {
            let pct = self.zoom / MM_PX * 100.0;
            egui::ComboBox::from_id_salt("zoom-pct")
                .width(64.0)
                .selected_text(format!("{pct:.0}%"))
                .show_ui(ui, |ui| {
                    for z in [25.0, 50.0, 100.0, 200.0, 400.0, 800.0, 1600.0] {
                        if ui.selectable_label(false, format!("{z:.0}%")).clicked() {
                            self.zoom = z / 100.0 * MM_PX;
                        }
                    }
                    if ui.selectable_label(false, "Fit (F)").clicked() {
                        self.fit = true;
                    }
                });
            ui.separator();
            let origin = self.design.as_ref().map(|d| d.bbox.center()).unwrap_or(Pt::new(0.0, 0.0));
            let pos = match self.cursor_mm {
                Some(p) => format!("X {:>6.1}   Y {:>6.1} mm", p.x - origin.x, p.y - origin.y),
                None => "X      –   Y      – mm".into(),
            };
            ui.label(RichText::new(pos).monospace().color(t.text_weak))
                .on_hover_text("Pointer position from the design centre");
            ui.separator();
            ui.label(RichText::new(self.tool.name()).color(t.text_weak));
            if let Some(d) = &self.design {
                ui.separator();
                let s = &d.stats;
                ui.label(
                    RichText::new(format!(
                        "{} stitches · {} stops · {} trims · {:.1} min",
                        group(s.stitches),
                        d.stops.len(),
                        s.trims,
                        s.run_minutes
                    ))
                    .color(t.text_weak),
                );
                if !d.warnings.is_empty() {
                    let w = ui.label(RichText::new(format!("⚠ {} to check", d.warnings.len())).color(t.warn));
                    if w.on_hover_text("Open the Production panel").interact(Sense::click()).clicked() {
                        self.dock = Dock::Production;
                        self.dock_open = true;
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut s = self.status.clone();
                if s.chars().count() > 90 {
                    s = s.chars().take(88).collect::<String>() + "…";
                }
                ui.label(RichText::new(s).color(t.text_weak)).on_hover_text(self.status.clone());
            });
        });
    }

    /// The icon strip beside the dock: one icon per panel.
    fn dock_icons(&mut self, ui: &mut egui::Ui) {
        let t = self.tokens();
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(28.0, 20.0), Sense::click());
            let ink = if resp.hovered() { t.text } else { t.text_weak };
            let s = if self.dock_open { "»" } else { "«" };
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, s, egui::FontId::proportional(14.0), ink);
            if resp.on_hover_text(if self.dock_open { "Hide panels" } else { "Show panels" }).clicked() {
                self.dock_open = !self.dock_open;
            }
            ui.add_space(6.0);
            for d in self.dock_tabs() {
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                let on = self.dock_open && self.dock == d;
                let bg = if on {
                    t.accent_soft
                } else if resp.hovered() {
                    t.hover
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(r, 6.0, bg);
                let ink = if on { if t.dark { Color32::WHITE } else { t.accent } } else { t.text_weak };
                dock_icon(ui.painter(), r.shrink(8.0), d, ink);
                if resp.on_hover_text(self.dock_label(d)).clicked() {
                    if on {
                        self.dock_open = false;
                    } else {
                        self.dock = d;
                        self.dock_open = true;
                    }
                }
                ui.add_space(2.0);
            }
        });
    }

    /// The docked panels: a tab strip and the open panel.
    fn dock(&mut self, ui: &mut egui::Ui, now: f64) {
        let t = self.tokens();
        if !self.dock_tabs().contains(&self.dock) {
            self.dock = Dock::Properties;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for d in self.dock_tabs() {
                let on = self.dock == d;
                let txt = RichText::new(self.dock_label(d)).strong().color(if on { t.text } else { t.text_weak });
                let b = egui::Button::new(txt).fill(if on { t.panel_alt } else { Color32::TRANSPARENT });
                if ui.add(b).clicked() {
                    self.dock = d;
                }
            }
        });
        ui.add_space(2.0);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt(("dock", self.dock as u8))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match self.dock {
                    Dock::Properties => self.props_panel(ui, now),
                    Dock::Layers => {
                        if self.machine.is_some() {
                            self.machine_panel(ui)
                        } else if self.tab == Tab::Drops {
                            self.drops_panel(ui)
                        } else {
                            self.layers_panel(ui)
                        }
                    }
                    Dock::Threads => self.threads_panel(ui),
                    Dock::Production => self.production_panel(ui),
                }
                ui.add_space(12.0);
            });
    }

    /// Properties: what is selected, or the design itself when nothing is.
    fn props_panel(&mut self, ui: &mut egui::Ui, now: f64) {
        let t = self.tokens();
        if self.tab == Tab::Drops && self.machine.is_none() {
            self.drops_setup(ui);
            return;
        }
        let kind = if self.machine.is_some() {
            "Stitch file".to_string()
        } else {
            match (self.sel_layers().len(), self.sel_idx()) {
                (0, _) => "Design".to_string(),
                (1, Some(li)) => {
                    let l = &self.layers[li];
                    format!("{} · {}", if l.text.is_some() { "Text" } else { "Shape" }, l.name)
                }
                (n, _) => format!("{n} objects"),
            }
        };
        ui.label(RichText::new(kind).color(t.text_weak));
        if self.selected.is_some() || self.tool.draws() || self.edit != Edit::None {
            self.props_selection(ui, now);
            ui.add_space(6.0);
            egui::CollapsingHeader::new(RichText::new("Design").strong())
                .id_salt("doc-props")
                .default_open(false)
                .show(ui, |ui| self.props_document(ui));
        } else {
            self.props_document(ui);
        }
    }

    // ---------- canvas furniture: hoop artboard, rulers, task bar ----------

    /// The hoop (or cap frame) the design sews in, in mm, centred on the design.
    fn hoop_rect(&self, d: &Design) -> (BBox, String) {
        let c = if self.design.is_some() { d.bbox.center() } else { Pt::new(0.0, 0.0) };
        let (w, h, name) = if self.profile == Profile::Cap {
            let (w, h) = self.recipe.cap_size.limit();
            (w + 16.0, h + 16.0, format!("Cap frame · {}", self.recipe.cap_size.label()))
        } else {
            let need = (d.bbox.width() + 10.0, d.bbox.height() + 10.0);
            let i = self.hoop.unwrap_or_else(|| {
                HOOPS
                    .iter()
                    .position(|h| (h.1 >= need.0 && h.2 >= need.1) || (h.2 >= need.0 && h.1 >= need.1))
                    .unwrap_or(HOOPS.len() - 1)
            });
            let (n, a, b) = HOOPS[i];
            // turn the hoop to suit the design
            let (w, h) = if (need.0 > need.1) == (a > b) { (a, b) } else { (b, a) };
            let auto = if self.hoop.is_none() { " (auto)" } else { "" };
            (w, h, format!("Hoop {n}{auto}"))
        };
        let half = Pt::new(w / 2.0, h / 2.0);
        (BBox { min: Pt::new(c.x - half.x, c.y - half.y), max: Pt::new(c.x + half.x, c.y + half.y) }, name)
    }

    /// Pasteboard, then the hoop area in the garment colour.
    fn paint_artboard(&self, painter: &egui::Painter, rect: Rect, d: &Design, to_screen: &dyn Fn(Pt) -> Pos2, fabric: Color32) -> Rect {
        let t = self.tokens();
        painter.rect_filled(rect, 0.0, t.pasteboard);
        if !self.show_hoop {
            painter.rect_filled(rect, 0.0, fabric);
            return rect;
        }
        let (hb, name) = self.hoop_rect(d);
        let r = Rect::from_min_max(to_screen(hb.min), to_screen(hb.max));
        let round = (6.0 * self.zoom).min(r.width() / 4.0) as u8;
        painter.add(egui::epaint::RectShape::filled(r.translate(Vec2::new(0.0, 3.0)), round, Color32::from_black_alpha(60)));
        painter.rect_filled(r, round, fabric);
        painter.rect_stroke(r, round, Stroke::new(1.0_f32, Color32::from_black_alpha(50)), egui::StrokeKind::Outside);
        painter.text(
            r.left_top() - Vec2::new(0.0, 6.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{name}  ·  {}", self.profile.label()),
            egui::FontId::proportional(11.5),
            t.text_weak,
        );
        r
    }

    /// Rulers along the top and left of the canvas, in mm from the design centre.
    fn paint_rulers(&self, painter: &egui::Painter, rect: Rect, to_screen: &dyn Fn(Pt) -> Pos2, to_mm: &dyn Fn(Pos2) -> Pt, origin: Pt) {
        if !self.show_rulers {
            return;
        }
        let t = self.tokens();
        let s = RULER;
        let top = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + s));
        let left = Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + s, rect.bottom()));
        painter.rect_filled(top, 0.0, t.chrome);
        painter.rect_filled(left, 0.0, t.chrome);
        painter.line_segment([top.left_bottom(), top.right_bottom()], Stroke::new(1.0_f32, t.border));
        painter.line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0_f32, t.border));
        // label step: about every 60 px, at 1/2/5 × 10^n mm
        let zoom = self.zoom as f64;
        let mut step = 1.0;
        for cand in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0] {
            step = cand;
            if cand * zoom >= 60.0 {
                break;
            }
        }
        let minor = step / if step == 2.0 || step == 20.0 || step == 200.0 { 4.0 } else { 5.0 };
        let font = egui::FontId::proportional(9.5);
        let ink = t.text_weak;
        let tick = Stroke::new(1.0_f32, t.text_weak.gamma_multiply(0.7));
        let a = to_mm(Pos2::new(top.left() + s, 0.0)).x - origin.x;
        let b = to_mm(Pos2::new(top.right(), 0.0)).x - origin.x;
        let mut v = (a / minor).floor() * minor;
        while v <= b {
            let x = to_screen(Pt::new(v + origin.x, 0.0)).x;
            if x > top.left() + s {
                let major = ((v / step).round() * step - v).abs() < 1e-6;
                let len = if major { s * 0.6 } else { s * 0.25 };
                painter.line_segment([Pos2::new(x, top.bottom() - len), Pos2::new(x, top.bottom())], tick);
                if major {
                    painter.text(Pos2::new(x + 3.0, top.top() + 2.0), egui::Align2::LEFT_TOP, format!("{}", v.round() as i64), font.clone(), ink);
                }
            }
            v += minor;
        }
        let a = to_mm(Pos2::new(0.0, left.top() + s)).y - origin.y;
        let b = to_mm(Pos2::new(0.0, left.bottom())).y - origin.y;
        let mut v = (a / minor).floor() * minor;
        while v <= b {
            let y = to_screen(Pt::new(0.0, v + origin.y)).y;
            if y > left.top() + s {
                let major = ((v / step).round() * step - v).abs() < 1e-6;
                let len = if major { s * 0.6 } else { s * 0.25 };
                painter.line_segment([Pos2::new(left.right() - len, y), Pos2::new(left.right(), y)], tick);
                if major {
                    let g = painter.layout_no_wrap(format!("{}", v.round() as i64), font.clone(), ink);
                    let pos = Pos2::new(left.left() + 2.0, y + 3.0);
                    painter.add(egui::epaint::TextShape::new(pos + Vec2::new(0.0, g.size().x), g, ink).with_angle(-std::f32::consts::FRAC_PI_2));
                }
            }
            v += minor;
        }
        // pointer position marks
        if let Some(p) = self.cursor_mm {
            let sp = to_screen(p);
            let mark = Stroke::new(1.0_f32, t.accent);
            if sp.x > top.left() + s {
                painter.line_segment([Pos2::new(sp.x, top.top()), Pos2::new(sp.x, top.bottom())], mark);
            }
            if sp.y > left.top() + s {
                painter.line_segment([Pos2::new(left.left(), sp.y), Pos2::new(left.right(), sp.y)], mark);
            }
        }
        let corner = Rect::from_min_size(rect.left_top(), Vec2::splat(s));
        painter.rect_filled(corner, 0.0, t.chrome);
        painter.text(corner.center(), egui::Align2::CENTER_CENTER, "mm", egui::FontId::proportional(8.5), ink);
    }

    /// Floating bar under the selection: the common next steps for it.
    fn task_bar(&mut self, ctx: &egui::Context, canvas: Rect, sel_screen: Rect, now: f64) {
        let Some(li) = self.sel_idx() else { return };
        if self.edit != Edit::None || self.machine.is_some() || self.tab != Tab::Design || self.moving.is_some() {
            return;
        }
        if self.show_export || self.trace_dlg.is_some() || self.show_text {
            return;
        }
        let t = self.tokens();
        let w_guess = 470.0;
        let mut pos = Pos2::new(sel_screen.center().x - w_guess / 2.0, sel_screen.bottom() + 34.0);
        let bottom_room = canvas.bottom() - 54.0;
        if pos.y > bottom_room {
            pos.y = (sel_screen.top() - 50.0).max(canvas.top() + RULER + 6.0);
        }
        pos.x = pos.x.clamp(canvas.left() + RULER + 6.0, (canvas.right() - w_guess - 6.0).max(canvas.left() + RULER + 6.0));
        let mut action: Option<&'static str> = None;
        let mut set_stitch: Option<StitchChoice> = None;
        let cur = self.layers[li].opts.stitch;
        let rgb = self.layers[li].custom.as_ref().map(|c| c.rgb).or(self.layers[li].shapes.first().map(|s| s.rgb)).unwrap_or([128, 128, 128]);
        egui::Area::new(egui::Id::new("task-bar"))
            .fixed_pos(pos)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.chrome)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(CornerRadius::same(8))
                    .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(70) })
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            swatch(ui, rgb);
                            egui::ComboBox::from_id_salt("tb-stitch")
                                .width(132.0)
                                .selected_text(cur.label())
                                .show_ui(ui, |ui| {
                                    for c in StitchChoice::ALL {
                                        if ui.selectable_label(cur == c, c.label()).clicked() {
                                            set_stitch = Some(c);
                                        }
                                    }
                                })
                                .response
                                .on_hover_text("Stitch type for this layer");
                            ui.separator();
                            let b = |ui: &mut egui::Ui, label: &str, tip: &str| ui.add(egui::Button::new(label).frame(false)).on_hover_text(tip).clicked();
                            if b(ui, "Shape", "Edit shape points") {
                                action = Some("edit-shape");
                            }
                            if b(ui, "Stitches", "Edit stitch points") {
                                action = Some("edit-stitches");
                            }
                            if b(ui, "Direction", "Direction lines: draw which way the stitches run") {
                                action = Some("guides");
                            }
                            ui.separator();
                            if b(ui, "Duplicate", &format!("Duplicate ({})", kb("D"))) {
                                action = Some("dup");
                            }
                            if b(ui, "🗑", "Delete") {
                                action = Some("del");
                            }
                            ui.menu_button("…", |ui| {
                                ui.set_min_width(200.0);
                                self.path_tools(ui, true);
                            })
                            .response
                            .on_hover_text("More shape tools");
                        });
                    });
            });
        if let Some(c) = set_stitch {
            if c != cur {
                self.checkpoint(now);
                self.layers[li].opts.stitch = c;
                self.sync_links(li);
                self.dirty = true;
            }
        }
        if let Some(a) = action {
            self.layer_action(a, li);
        }
    }

    fn keys_window(&mut self, ctx: &egui::Context) {
        if !self.show_keys {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard shortcuts").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new(if self.ai_keys { "Vector-editor style tool keys" } else { "StitchForge tool keys" }).weak());
            egui::Grid::new("keys").num_columns(2).spacing([24.0, 3.0]).show(ui, |ui| {
                for t in Tool::ALL {
                    ui.label(t.name());
                    ui.label(RichText::new(t.key_label(self.ai_keys)).monospace().strong());
                    ui.end_row();
                }
                for (a, b) in [
                    ("Vector / Stitches 3D / Flat", "1  2  3".to_string()),
                    ("Fit in window", "F".into()),
                    ("Play the sew-out", "Space".into()),
                    ("Nudge (Shift = 5 mm)", "Arrows".into()),
                    ("Undo / Redo", format!("{}  /  {}", kb("Z"), kb("⇧Z"))),
                    ("Save / Open / Export", format!("{}  {}  {}", kb("S"), kb("O"), kb("E"))),
                    ("Duplicate", kb("D")),
                ] {
                    ui.label(a);
                    ui.label(RichText::new(b).monospace().strong());
                    ui.end_row();
                }
            });
        });
        if !open {
            self.show_keys = false;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        egui::Window::new("About StitchForge").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("StitchForge 0.20").strong().size(16.0));
            ui.label("Embroidery design, lettering, name drops and machine files.");
            ui.label(RichText::new("Written from scratch. Reads and writes DST, EXP and PES; SVG, PDF and pictures for art.").weak());
        });
        if !open {
            self.show_about = false;
        }
    }

    // ---------- thread libraries (several at once) ----------

    /// Generic palette plus every library in the library folder; the ones
    /// saved in the preferences are ticked.
    fn with_libraries(mut self, on: &[String]) -> Self {
        self.libs.push(LoadedLib {
            key: "generic".into(),
            name: "Generic 40wt".into(),
            note: "Built-in generic 40 wt palette, made for StitchForge.".into(),
            threads: self.chart.clone(),
            on: on.is_empty() || on.iter().any(|k| k == "generic"),
        });
        for p in thread::scan_libraries(&thread::library_dir()) {
            if let Ok(lib) = thread::load_library(&p.to_string_lossy()) {
                let key = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let on = on.iter().any(|k| *k == key);
                self.libs.push(LoadedLib::from_library(key, lib, on));
            }
        }
        // libraries that ship with StitchForge (a copy in the folder wins)
        for (file, data) in thread::BUILT_IN {
            if self.libs.iter().any(|l| l.key == *file) {
                continue;
            }
            if let Ok(lib) = thread::parse_library(file, data, file) {
                let on = on.iter().any(|k| k == file);
                self.libs.push(LoadedLib::from_library(file.to_string(), lib, on));
            }
        }
        if !self.libs.iter().any(|l| l.on) {
            self.libs[0].on = true;
        }
        self.rebuild_chart(false);
        self
    }

    /// The working chart is every ticked library in list order. Layers that
    /// picked a thread keep it (found again by library and code).
    fn rebuild_chart(&mut self, save: bool) {
        if !self.libs.iter().any(|l| l.on) {
            if let Some(g) = self.libs.iter_mut().find(|l| l.key == "generic") {
                g.on = true;
            }
        }
        let keep: Vec<Option<(String, String)>> = self
            .layers
            .iter()
            .map(|l| l.opts.thread.and_then(|i| self.chart.get(i)).map(|t| (t.brand.clone(), t.code.clone())))
            .collect();
        let mut chart = Vec::new();
        for l in self.libs.iter().filter(|l| l.on) {
            for t in &l.threads {
                let mut t = t.clone();
                t.brand = l.name.clone();
                chart.push(t);
            }
        }
        self.chart = chart;
        for (l, k) in self.layers.iter_mut().zip(keep) {
            l.opts.thread = k.and_then(|(b, c)| self.chart.iter().position(|t| t.brand == b && t.code == c));
        }
        let on: Vec<&LoadedLib> = self.libs.iter().filter(|l| l.on).collect();
        self.chart_name = on.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(" + ");
        self.chart_note = if on.len() == 1 {
            on[0].note.clone()
        } else {
            format!("{} threads from {} libraries. Colours are matched from all of them.", self.chart.len(), on.len())
        };
        self.dirty = true;
        if save {
            self.save_prefs();
        }
    }

    /// Add a library file: copied into the library folder (so it is there
    /// next time) and ticked. Already known: just ticked.
    fn add_library_file(&mut self, p: &std::path::Path) {
        let lib = match thread::load_library(&p.to_string_lossy()) {
            Ok(l) => l,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let file = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "library.csv".into());
        #[cfg(not(target_arch = "wasm32"))]
        let key = {
            let dir = thread::library_dir();
            let dest = dir.join(&file);
            let same = p.canonicalize().ok() == dest.canonicalize().ok();
            if same || (std::fs::create_dir_all(&dir).is_ok() && std::fs::copy(p, &dest).is_ok()) {
                self.libraries = thread::scan_libraries(&dir);
                file.clone()
            } else {
                p.to_string_lossy().to_string()
            }
        };
        #[cfg(target_arch = "wasm32")]
        let key = file.clone();
        let name = lib.name.clone();
        let n = lib.threads.len();
        match self.libs.iter_mut().find(|l| l.key == key) {
            Some(l) => {
                *l = LoadedLib::from_library(key, lib, true);
            }
            None => self.libs.push(LoadedLib::from_library(key, lib, true)),
        }
        self.rebuild_chart(true);
        self.status = format!("Added {name} ({n} threads). It stays in your Thread libraries folder.");
    }

    fn libraries_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "Thread libraries");
        ui.label(RichText::new("Tick every library you stock. Threads are matched from all ticked libraries.").small().weak());
        ui.add_space(2.0);
        let mut changed = false;
        for l in self.libs.iter_mut() {
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut l.on, RichText::new(&l.name));
                if r.changed() {
                    changed = true;
                }
                r.on_hover_text(&l.note);
                ui.label(RichText::new(format!("{}", l.threads.len())).small().weak());
                let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 10.0), Sense::hover());
                // a strip of the library's colours
                let k = l.threads.len().max(1);
                let step = (k / 16).max(1);
                let cols: Vec<[u8; 3]> = l.threads.iter().step_by(step).take(16).map(|t| t.rgb).collect();
                let w = rect.width() / cols.len().max(1) as f32;
                for (i, c) in cols.iter().enumerate() {
                    let r = Rect::from_min_size(rect.min + Vec2::new(i as f32 * w, 0.0), Vec2::new(w, rect.height()));
                    ui.painter().rect_filled(r, 0.0, Color32::from_rgb(c[0], c[1], c[2]));
                }
            });
        }
        if changed {
            self.rebuild_chart(true);
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add library…").on_hover_text("A CSV with code,name,r,g,b. It's copied into your Thread libraries folder").clicked() {
                pick(Want::Chart, "Add a thread library", &[("Thread library", &["csv"])], self.last_dir.as_ref());
            }
            #[cfg(not(target_arch = "wasm32"))]
            if ui.button("Open folder").on_hover_text(thread::library_dir().display().to_string()).clicked() {
                let d = thread::library_dir();
                let _ = std::fs::create_dir_all(&d);
                open_path(&d);
            }
            if ui.button("Make my own").on_hover_text("Creates a template CSV in your Thread libraries folder").clicked() {
                match thread::write_template(&thread::library_dir()) {
                    Ok(p) => self.error = Some(format!("Template saved to {}. Fill it in, rename it without the _ at the front, then Add library.", p.display())),
                    Err(e) => self.error = Some(format!("Could not create the template: {e}")),
                }
            }
        });
        ui.label(RichText::new(&self.chart_note).small().weak());
    }

    fn tool_strip(&mut self, ui: &mut egui::Ui) {
        let t = self.tokens();
        let art = self.machine.is_none() && self.tab == Tab::Design;
        ui.add_space(4.0);
        let mut pick: Option<Tool> = None;
        for tool in Tool::ALL {
            if matches!(tool, Tool::Pen | Tool::Measure) {
                ui.add_space(3.0);
                let (r, _) = ui.allocate_exact_size(Vec2::new(36.0, 5.0), Sense::hover());
                ui.painter().line_segment([r.left_center() + Vec2::new(6.0, 0.0), r.right_center() - Vec2::new(6.0, 0.0)], Stroke::new(1.0_f32, t.border));
                ui.add_space(1.0);
            }
            let on = self.tool == tool && art;
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(36.0), if art { Sense::click() } else { Sense::hover() });
            let bg = if on {
                t.accent
            } else if resp.hovered() && art {
                t.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(r, 5.0, bg);
            let ink = if on {
                Color32::WHITE
            } else if art {
                t.text
            } else {
                t.text_weak.gamma_multiply(0.45)
            };
            tool_icon(ui.painter(), r.shrink(9.0), tool, ink);
            if art && resp.on_hover_text(tool.tip(self.ai_keys)).clicked() {
                pick = Some(tool);
            }
        }
        if let Some(tool) = pick {
            self.set_tool(tool);
        }
        // the colour the next shape is drawn in (fill over outline)
        ui.add_space(10.0);
        let (r, resp) = ui.allocate_exact_size(Vec2::new(36.0, 36.0), Sense::click());
        let p = ui.painter();
        let back = Rect::from_min_size(r.min + Vec2::new(13.0, 13.0), Vec2::splat(18.0));
        let front = Rect::from_min_size(r.min + Vec2::new(4.0, 4.0), Vec2::splat(18.0));
        let c = |rgb: [u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        if self.style.stroke_on {
            p.rect_stroke(back.shrink(2.0), 2.0, Stroke::new(4.0_f32, c(self.style.stroke)), egui::StrokeKind::Middle);
        } else {
            p.rect_stroke(back, 2.0, Stroke::new(1.0_f32, t.border), egui::StrokeKind::Inside);
        }
        if self.style.fill_on {
            p.rect_filled(front, 2.0, c(self.style.fill));
        } else {
            p.rect_filled(front, 2.0, t.panel);
            p.line_segment([front.left_bottom(), front.right_top()], Stroke::new(1.5_f32, Color32::from_rgb(220, 60, 50)));
        }
        p.rect_stroke(front, 2.0, Stroke::new(1.0_f32, t.border), egui::StrokeKind::Outside);
        if art && resp.on_hover_text("Fill and outline of the next shape you draw: pick a drawing tool to change them").clicked() {
            self.set_tool(Tool::Rect);
            self.dock = Dock::Properties;
            self.dock_open = true;
        }
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
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Selected: {}", self.font)).weak());
                    if ui
                        .small_button("Load font file…")
                        .on_hover_text("Any .ttf or .otf font file")
                        .clicked()
                    {
                        pick(Want::Font, "Font file", &[("Font", &["ttf", "otf", "ttc"])], self.last_dir.as_ref());
                    }
                });
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
        CTX.with(|c| {
            if c.borrow().is_none() {
                *c.borrow_mut() = Some(ctx.clone());
            }
        });
        for (want, p) in PICKED.with(|v| std::mem::take(&mut *v.borrow_mut())) {
            self.handle_picked(want, p);
        }
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
                .filter_map(|f| {
                    f.path.clone().or_else(|| {
                        // browser: the file's bytes come with the drop
                        let bytes = f.bytes.as_ref()?;
                        let p = PathBuf::from(format!("/in/{}", f.name));
                        stitchforge::vfs::write(&p, &bytes[..]).ok()?;
                        Some(p)
                    })
                })
                .collect()
        });
        for p in dropped {
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if ext == "svg" && !self.layers.is_empty() && self.machine.is_none() {
                self.open_svg_path(p, true);
            } else if ART_EXT.contains(&ext.as_str()) && !self.layers.is_empty() && self.machine.is_none() {
                self.open_art_file(p, true);
            } else {
                self.guard(Pending::OpenPath(p), ctx);
            }
        }
        // shell state follows the workspace and theme
        if ctx.data(|d| d.get_temp::<ThemeKind>(egui::Id::new("sf-theme"))) != Some(self.theme) {
            theme::apply(ctx, self.theme);
        }
        self.pro = self.workspace != Workspace::Simple;
        if self.tab == Tab::Drops && self.workspace != Workspace::NameDrops {
            self.workspace = Workspace::NameDrops;
        } else if self.tab == Tab::Design && self.workspace == Workspace::NameDrops {
            self.workspace = Workspace::Design;
        }
        // shortcuts (ignored while typing in a field)
        let typing = ctx.wants_keyboard_input();
        if !typing && ctx.input(|i| i.key_pressed(egui::Key::Questionmark) || (i.modifiers.shift && i.key_pressed(egui::Key::Slash))) {
            self.show_keys = !self.show_keys;
        }
        let edit_used = !typing && self.edit != Edit::None && self.edit_keys(ctx);
        let mut pen_used = false;
        if !typing && !self.pen.is_empty() {
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.finish_pen(false);
                pen_used = true;
            } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.pen.clear();
                pen_used = true;
            } else if ctx.input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)) {
                self.pen.pop();
                pen_used = true;
            }
        }
        let edit_used = edit_used || pen_used;
        if !typing && !edit_used && self.machine.is_none() && self.tab == Tab::Design {
            let plain = ctx.input(|i| !i.modifiers.command && !i.modifiers.alt);
            if plain {
                for t in Tool::ALL {
                    if ctx.input(|i| i.key_pressed(t.key_for(self.ai_keys)) && !i.modifiers.shift) && !(t == Tool::Text && self.show_text) {
                        self.set_tool(t);
                    }
                }
            }
            for v in View::ALL {
                if plain && ctx.input(|i| i.key_pressed(v.key())) {
                    self.view = v;
                }
            }
            if (self.tool.draws() || matches!(self.tool, Tool::Measure | Tool::Knife | Tool::Eraser | Tool::Guides))
                && ctx.input(|i| i.key_pressed(egui::Key::Escape))
            {
                self.set_tool(Tool::Select);
            }
        }
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
        if !typing && !edit_used {
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
            if cmd && (pressed(egui::Key::Equals) || pressed(egui::Key::Plus)) {
                self.zoom = (self.zoom * 1.25).min(200.0);
            }
            if cmd && pressed(egui::Key::Minus) {
                self.zoom = (self.zoom / 1.25).max(0.3);
            }
            if cmd && pressed(egui::Key::Num1) {
                self.zoom = MM_PX;
            }
            if cmd && pressed(egui::Key::A) && self.tab == Tab::Design && self.machine.is_none() && self.edit == Edit::None {
                self.select_all();
            }
            if pressed(egui::Key::Space) && self.design.is_some() {
                self.playing = !self.playing;
            }
            if let Some(li) = self.sel_idx().filter(|_| self.edit == Edit::None) {
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
                    if let (Tool::Guides, Some(g)) = (self.tool, self.guide_sel) {
                        // remove the picked direction line, not the layer
                        if g < self.layers[li].opts.guides.len() {
                            self.hard_checkpoint();
                            self.layers[li].opts.guides.remove(g);
                            self.guide_sel = None;
                            self.dirty = true;
                            self.status = "Direction line removed".into();
                        }
                    } else {
                        self.layer_action("del", li);
                    }
                }
                if cmd && pressed(egui::Key::D) {
                    self.layer_action("dup", li);
                }
                if pressed(egui::Key::Escape) {
                    self.selected = None;
                }
            }
        }

        // multi-select always holds the main selection
        {
            let ids: std::collections::HashSet<u64> = self.layers.iter().map(|l| l.id).collect();
            self.multi.retain(|id| ids.contains(id));
            match self.selected {
                Some(sid) if !self.multi.contains(&sid) => self.multi = [sid].into_iter().collect(),
                None => self.multi.clear(),
                _ => {}
            }
        }
        // editing ends if the object is no longer the selected one
        if self.edit != Edit::None {
            let still = self
                .edit_layer()
                .map(|i| Some(self.layers[i].id) == self.selected)
                .unwrap_or(false);
            if !still || self.tab != Tab::Design || self.machine.is_some() {
                self.end_edit();
            }
        }
        if self.dirty && self.moving.is_none() && self.edit_drag.is_none() {
            if self.clean_after_regen {
                self.clean_after_regen = false;
                self.modified = false;
            } else if self.machine.is_none() && !self.quiet_regen {
                self.modified = true;
            }
            self.quiet_regen = false;
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

        let t = self.tokens();
        let chrome = egui::Frame::new().fill(t.chrome);
        egui::TopBottomPanel::top("menu")
            .frame(chrome.inner_margin(egui::Margin::symmetric(8, 4)))
            .show_separator_line(false)
            .show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("docbar")
            .frame(chrome.inner_margin(egui::Margin { left: 56, right: 8, top: 4, bottom: 0 }))
            .show_separator_line(false)
            .show(ctx, |ui| self.doc_bar(ui));
        egui::TopBottomPanel::bottom("status")
            .frame(chrome.inner_margin(egui::Margin::symmetric(10, 3)))
            .show_separator_line(false)
            .show(ctx, |ui| self.status_bar(ui));
        egui::TopBottomPanel::bottom("hint")
            .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ctx, |ui| self.hint_bar(ui));
        let title = format!(
            "StitchForge — {}{}",
            self.display_name(),
            if self.modified { " •" } else { "" }
        );
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        egui::SidePanel::left("tools")
            .resizable(false)
            .exact_width(48.0)
            .frame(chrome.inner_margin(egui::Margin::symmetric(6, 4)))
            .show_separator_line(false)
            .show(ctx, |ui| self.tool_strip(ui));
        egui::SidePanel::right("dock-icons")
            .resizable(false)
            .exact_width(44.0)
            .frame(chrome)
            .show_separator_line(false)
            .show(ctx, |ui| self.dock_icons(ui));
        if self.dock_open {
            egui::SidePanel::right("dock")
                .resizable(true)
                .default_width(360.0)
                .width_range(280.0..=620.0)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin::symmetric(10, 0)))
                .show(ctx, |ui| self.dock(ui, now));
        }
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
        self.border_window(ctx);
        self.weights_window(ctx);
        self.trace_window(ctx);
        self.export_window(ctx);
        self.confirm_window(ctx);
        self.keys_window(ctx);
        self.about_window(ctx);
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
    relief: f32,
    rope: Option<egui::TextureId>,
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
        View::ThreeD | View::Vector => {
            // Thread as it lies on fabric: each stitch is a rounded strand
            // that tapers and darkens where it dives into the needle holes,
            // casts a soft shadow, and catches the light by its angle (so
            // satin shows long glossy strands and tatami its woven,
            // brick-like texture from the staggered needle points).
            let light = Vec2::new(-0.45, -0.9).normalized(); // from the upper left
            let shadow_off = Vec2::new(0.22, 0.38) * w * relief;
            let detailed = w >= 2.2;
            let across: &[(f32, f32)] = if detailed {
                // (position across the strand, base shading) — lit edge first
                &[(1.0, -0.1), (0.62, 0.06), (0.3, 0.05), (0.0, 0.0), (-0.35, -0.08), (-0.7, -0.2), (-1.0, -0.34)]
            } else {
                &[(1.0, 0.02), (0.0, 0.0), (-1.0, -0.3)]
            };
            let mut mesh = egui::Mesh::with_texture(rope.unwrap_or_default());
            let px_per_mm = zoom / 1.0;
            // texture coordinates: across the strand (u) and along it (v), so
            // the rope tile repeats every couple of thread widths
            let uv = |sgn: f32, dist: f32| -> egui::Pos2 {
                match rope {
                    Some(_) => egui::Pos2::new(0.5 + 0.5 * sgn, dist / (w * 2.0)),
                    None => egui::epaint::WHITE_UV,
                }
            };
            for r in runs.iter().filter(|r| r.len() >= 2) {
                let mut acc = 0.0_f32;
                let unit = |i: usize, j: usize| -> Option<Vec2> {
                    let v = r[j] - r[i];
                    (v.length() > 0.2).then(|| v.normalized())
                };
                for si in 1..r.len() {
                    let (a, b) = (r[si - 1], r[si]);
                    let v = b - a;
                    let len = v.length();
                    let start = acc; // distance along this run before this stitch
                    acc += len;
                    if len < 0.3
                        || (!clip.contains(a)
                            && !clip.contains(b)
                            && !clip.intersects(Rect::from_two_pos(a, b)))
                    {
                        continue;
                    }
                    let dir = v / len;
                    // satin: neighbours run back the other way (zigzag), none
                    // runs across (a fill row's turn); split satin pieces keep
                    // one neighbour in line and get their own bevel each
                    let prev = if si >= 2 { unit(si - 2, si - 1) } else { None };
                    let next = if si + 1 < r.len() { unit(si, si + 1) } else { None };
                    let dots: Vec<f32> = [prev, next].iter().flatten().map(|d| d.dot(dir)).collect();
                    let satin = len / px_per_mm >= 0.7
                        && dots.iter().any(|d| *d < -0.7)
                        && dots.iter().all(|d| d.abs() > 0.5);
                    let mut n = Vec2::new(-dir.y, dir.x);
                    if n.dot(light) < 0.0 {
                        n = -n; // +n points toward the light
                    }
                    let half = w * 0.64;
                    // sheen: thread is brightest lying across the light
                    let cross = (dir.x * light.y - dir.y * light.x).abs();
                    let sheen = cross * cross;
                    let long = (len / (w * 3.0)).clamp(0.0, 1.0);
                    let lift = (sheen - 0.5) * 0.14;
                    let gloss = (0.03 + 0.15 * sheen) * (0.4 + 0.6 * long);
                    // stations along the stitch: (distance, width factor, darkening)
                    let tl = (len * 0.32).min(w * 0.9);
                    let st: [(f32, f32, f32); 6] = [
                        (0.0, 0.62, -0.3),
                        (tl * 0.45, 0.88, -0.12),
                        (tl, 1.0, 0.0),
                        (len - tl, 1.0, 0.0),
                        (len - tl * 0.45, 0.88, -0.12),
                        (len, 0.62, -0.3),
                    ];
                    // satin columns are rounded: a bevel along each stitch,
                    // lit on the side facing the light, darker toward the
                    // edges (puff: taller)
                    let bevel_st: [(f32, f32, f32); 9];
                    let stations: &[(f32, f32, f32)] = if detailed && satin {
                        let dl = dir.dot(light);
                        let bev = |t: f32| -> f32 {
                            let u = (t / len).clamp(0.0, 1.0);
                            let pi = std::f32::consts::PI;
                            relief * (-0.2 * (pi * u).cos() * dl + 0.1 * ((pi * u).sin() - 0.6))
                        };
                        let tl2 = tl.min(len * 0.2);
                        let ts = [0.0, tl2 * 0.45, tl2, len * 0.3, len * 0.5, len * 0.7, len - tl2, len - tl2 * 0.45, len];
                        let wf = [0.62, 0.88, 1.0, 1.0, 1.0, 1.0, 1.0, 0.88, 0.62];
                        let dk = [-0.3, -0.12, 0.0, 0.0, 0.0, 0.0, 0.0, -0.12, -0.3];
                        bevel_st = std::array::from_fn(|k| (ts[k], wf[k], dk[k] + bev(ts[k])));
                        &bevel_st
                    } else if detailed {
                        &st
                    } else {
                        &[(0.0, 0.6, -0.25), (len * 0.5, 1.0, 0.0), (len, 0.6, -0.25)]
                    };
                    // soft shadow on the fabric under the strand
                    if detailed {
                        let base = mesh.vertices.len() as u32;
                        let sc = Color32::from_black_alpha(28);
                        for &(t, wf, _) in stations {
                            let c = a + dir * t + shadow_off;
                            mesh.colored_vertex(c + n * (half * wf * 1.1), sc);
                            mesh.colored_vertex(c - n * (half * wf * 1.1), sc);
                        }
                        for i in 0..(stations.len() as u32 - 1) {
                            let i0 = base + i * 2;
                            mesh.add_triangle(i0, i0 + 1, i0 + 2);
                            mesh.add_triangle(i0 + 1, i0 + 3, i0 + 2);
                        }
                    }
                    let base = mesh.vertices.len() as u32;
                    let cols = across.len() as u32;
                    for &(t, wf, dk) in stations {
                        for &(sgn, tone) in across {
                            let g = if sgn > 0.2 && sgn < 0.8 { gloss } else if sgn > 0.0 && sgn <= 0.2 { gloss * 0.45 } else { 0.0 };
                            let p = a + dir * t + n * (sgn * half * wf);
                            mesh.vertices.push(egui::epaint::Vertex {
                                pos: p,
                                uv: uv(sgn, start + t),
                                color: tone_col(col, tone + dk + lift + g * (1.0 + dk * 2.0).max(0.0)),
                            });
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
                    let full = std::mem::replace(&mut mesh, egui::Mesh::with_texture(rope.unwrap_or_default()));
                    shapes.push(EShape::mesh(full));
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

/// Small line icons for the tool strip, drawn in the box `r`.
fn tool_icon(p: &egui::Painter, r: Rect, t: Tool, ink: Color32) {
    let s = Stroke::new(1.6_f32, ink);
    let at = |x: f32, y: f32| Pos2::new(r.left() + x * r.width(), r.top() + y * r.height());
    match t {
        Tool::Select => {
            p.add(EShape::convex_polygon(
                vec![at(0.2, 0.0), at(0.85, 0.55), at(0.5, 0.6), at(0.3, 0.95)],
                ink,
                Stroke::NONE,
            ));
        }
        Tool::Nodes => {
            p.add(EShape::QuadraticBezier(egui::epaint::QuadraticBezierShape::from_points_stroke(
                [at(0.05, 0.85), at(0.5, -0.1), at(0.95, 0.85)],
                false,
                Color32::TRANSPARENT,
                s,
            )));
            for q in [at(0.05, 0.85), at(0.95, 0.85)] {
                p.rect_filled(Rect::from_center_size(q, Vec2::splat(6.0)), 1.0, ink);
            }
            p.rect_stroke(Rect::from_center_size(at(0.5, 0.38), Vec2::splat(6.0)), 1.0, s, egui::StrokeKind::Inside);
        }
        Tool::Stitches => {
            let pts = [at(0.05, 0.2), at(0.3, 0.8), at(0.5, 0.2), at(0.7, 0.8), at(0.95, 0.2)];
            p.add(EShape::line(pts.to_vec(), s));
            for q in pts {
                p.circle_filled(q, 2.2, ink);
            }
        }
        Tool::Pen => {
            p.add(EShape::convex_polygon(
                vec![at(0.5, 0.0), at(0.85, 0.45), at(0.62, 0.95), at(0.38, 0.95), at(0.15, 0.45)],
                Color32::TRANSPARENT,
                s,
            ));
            p.line_segment([at(0.5, 0.15), at(0.5, 0.6)], s);
            p.circle_filled(at(0.5, 0.62), 2.0, ink);
        }
        Tool::Rect => {
            p.rect_stroke(Rect::from_min_max(at(0.05, 0.15), at(0.95, 0.85)), 2.0, s, egui::StrokeKind::Inside);
        }
        Tool::Ellipse => {
            p.add(EShape::ellipse_stroke(at(0.5, 0.5), Vec2::new(r.width() * 0.47, r.height() * 0.36), s));
        }
        Tool::Polygon => {
            let mut v = Vec::new();
            for i in 0..10 {
                let a = std::f32::consts::TAU * i as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
                let rr = if i % 2 == 0 { 0.5 } else { 0.22 };
                v.push(at(0.5 + rr * a.cos(), 0.53 + rr * a.sin()));
            }
            v.push(v[0]);
            p.add(EShape::line(v, s));
        }
        Tool::Line => {
            p.line_segment([at(0.05, 0.95), at(0.95, 0.05)], s);
        }
        Tool::Text => {
            p.text(at(0.5, 0.5), egui::Align2::CENTER_CENTER, "T", egui::FontId::proportional(r.height() * 1.15), ink);
        }
        Tool::Knife => {
            // a blade on the diagonal and a dashed cut line through a square
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.35), at(0.65, 0.9)), 1.0, Stroke::new(1.2_f32, ink.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            for seg in EShape::dashed_line(&[at(0.0, 1.0), at(0.75, 0.25)], s, 3.0, 2.0) {
                p.add(seg);
            }
            p.add(EShape::convex_polygon(vec![at(0.7, 0.3), at(1.0, 0.0), at(0.95, 0.15), at(0.8, 0.4)], ink, Stroke::NONE));
        }
        Tool::Guides => {
            // curved rows with an arrow: stitches turning
            for k in 0..3 {
                let o = k as f32 * 0.22;
                p.add(EShape::QuadraticBezier(egui::epaint::QuadraticBezierShape::from_points_stroke(
                    [at(0.05, 0.95 - o), at(0.1 + o, 0.15 + o * 0.3), at(0.95 - o * 0.2, 0.1 + o)],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(1.3_f32, ink.gamma_multiply(if k == 0 { 1.0 } else { 0.6 })),
                )));
            }
            p.add(EShape::convex_polygon(vec![at(1.0, 0.1), at(0.78, 0.0), at(0.8, 0.22)], ink, Stroke::NONE));
        }
        Tool::Eraser => {
            // an eraser block, tilted
            let body = vec![at(0.05, 0.62), at(0.5, 0.17), at(0.95, 0.55), at(0.5, 1.0)];
            p.add(EShape::convex_polygon(body, Color32::TRANSPARENT, s));
            p.add(EShape::convex_polygon(vec![at(0.05, 0.62), at(0.28, 0.39), at(0.73, 0.78), at(0.5, 1.0)], ink, Stroke::NONE));
        }
        Tool::Measure => {
            // a ruler on the diagonal with tick marks
            let (a, b) = (at(0.0, 0.75), at(1.0, 0.25));
            let n = Vec2::new(0.25, 0.5).normalized();
            p.line_segment([a, b], s);
            for k in 0..=5 {
                let t = k as f32 / 5.0;
                let q = a + (b - a) * t;
                let len = if k % 5 == 0 { 7.0 } else { 3.5 };
                p.line_segment([q, q + n * len], s);
            }
        }
    }
}

/// A short satin column at 8× scale: strands as wide as the thread lying at
/// the weight's spacing, so the difference in fineness is visible.
fn weight_swatch(ui: &mut egui::Ui, w: thread::ThreadWeight, spacing_mm: f64) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(110.0, 26.0), Sense::hover());
    let k = 8.0_f64; // 8 points per mm
    let p = ui.painter_at(r);
    p.rect_filled(r, 3.0, Color32::from_rgb(240, 236, 228));
    let strand = (w.strand_mm() * k) as f32;
    let step = (spacing_mm * k) as f32;
    let mut x = r.left() + 3.0;
    while x < r.right() - 3.0 {
        p.line_segment(
            [Pos2::new(x - 3.0, r.bottom() - 3.0), Pos2::new(x + 3.0, r.top() + 3.0)],
            Stroke::new(strand, Color32::from_rgb(28, 60, 140)),
        );
        p.line_segment(
            [Pos2::new(x - 3.0 - strand * 0.25, r.bottom() - 3.0), Pos2::new(x + 3.0 - strand * 0.25, r.top() + 3.0)],
            Stroke::new((strand * 0.25).max(0.5), Color32::from_rgb(90, 130, 210)),
        );
        x += step.max(1.0);
    }
}

/// The measuring line on the canvas: the distance, and which thread weights
/// a satin column that wide suits.
fn measure_overlay(p: &egui::Painter, a: Pos2, b: Pos2, mm: f64, dark: bool) {
    let ink = Color32::from_rgb(214, 60, 120);
    p.line_segment([a, b], Stroke::new(1.6_f32, ink));
    let d = (b - a).normalized();
    let n = Vec2::new(-d.y, d.x) * 6.0;
    for q in [a, b] {
        p.line_segment([q - n, q + n], Stroke::new(1.6_f32, ink));
    }
    if mm < 0.05 {
        return;
    }
    let text_col = if dark { Color32::WHITE } else { Color32::from_rgb(30, 30, 34) };
    let good = Color32::from_rgb(40, 160, 80);
    let bad = Color32::from_rgb(200, 60, 50);
    let font = egui::FontId::proportional(13.0);
    let mut job = egui::text::LayoutJob::default();
    let put = |job: &mut egui::text::LayoutJob, t: &str, c: Color32| {
        job.append(t, 0.0, egui::TextFormat { font_id: font.clone(), color: c, ..Default::default() });
    };
    put(&mut job, &format!("{:.2} mm · {:.3} in\n", mm, mm / 25.4), text_col);
    put(&mut job, "As a satin column:  ", text_col);
    for w in thread::ThreadWeight::ALL.iter().rev() {
        let ok = mm >= w.min_column() && mm <= settings::Limits::SATIN_MAX + 2.0;
        put(&mut job, &format!("{} wt {}   ", w.number(), if ok { "ok" } else { "no" }), if ok { good } else { bad });
    }
    put(&mut job, "\nAs letter height:  ", text_col);
    for w in thread::ThreadWeight::ALL.iter().rev() {
        let ok = mm >= w.min_text_height();
        put(&mut job, &format!("{} wt {}   ", w.number(), if ok { "ok" } else { "no" }), if ok { good } else { bad });
    }
    let galley = p.layout_job(job);
    // keep the readout on the canvas
    let clip = p.clip_rect();
    let mut at = b + Vec2::new(12.0, 10.0);
    if at.x + galley.size().x + 12.0 > clip.right() {
        at.x = b.x - 12.0 - galley.size().x;
    }
    if at.y + galley.size().y + 12.0 > clip.bottom() {
        at.y = b.y - 10.0 - galley.size().y;
    }
    let r = Rect::from_min_size(at, galley.size()).expand(6.0);
    p.rect_filled(
        r,
        5.0,
        if dark { Color32::from_rgba_unmultiplied(30, 30, 36, 235) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 240) },
    );
    p.rect_stroke(r, 5.0, Stroke::new(1.0_f32, ink), egui::StrokeKind::Inside);
    p.galley(at, galley, Color32::WHITE);
}

/// A stitch setting a layer can set for itself; blank means the fabric's.
#[allow(clippy::too_many_arguments)]
fn own_setting(
    ui: &mut egui::Ui,
    label: &str,
    salt: &str,
    v: &mut Option<f64>,
    fabric: f64,
    decimals: usize,
    unit: &str,
    range: (f64, f64),
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([112.0, 18.0], egui::Label::new(label));
        if let Some(n) = num_field(ui, salt, v.unwrap_or(fabric), decimals, unit) {
            if (n - fabric).abs() > 1e-9 || v.is_some() {
                *v = Some(n.clamp(range.0, range.1));
                changed = true;
            }
        }
        if v.is_some() {
            if ui
                .small_button("↺")
                .on_hover_text(format!("Back to the fabric's {fabric:.2} {unit}"))
                .clicked()
            {
                *v = None;
                changed = true;
            }
        } else {
            ui.label(RichText::new("fabric").weak().small());
        }
    });
    changed
}

fn kinds_turning(d: &Option<Design>, li: usize) -> bool {
    d.as_ref()
        .map(|d| d.objects.iter().any(|o| o.layer == li && o.stitch_type == "Turning satin"))
        .unwrap_or(false)
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

#[cfg(not(target_arch = "wasm32"))]
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

// ---------------------------------------------------------------- platform
// Desktop: native dialogs and the real disk. Browser: an upload picker that
// puts the file in the in-memory file store, and downloads for results.

/// What a picked file is for.
/// The default canvas colour (no garment chosen).
const CANVAS: [u8; 3] = [242, 239, 232];

/// Common garment colours for the canvas and sheet previews.
const GARMENTS: &[(&str, [u8; 3])] = &[
    ("Canvas (none)", CANVAS),
    ("White", [250, 250, 248]),
    ("Black", [22, 22, 24]),
    ("Navy", [28, 34, 58]),
    ("Charcoal", [62, 64, 68]),
    ("Heather gray", [160, 162, 164]),
    ("Royal", [36, 66, 150]),
    ("Red", [170, 28, 36]),
    ("Maroon", [96, 26, 36]),
    ("Forest", [30, 70, 46]),
    ("Khaki", [196, 180, 140]),
    ("Orange", [232, 110, 36]),
];

/// File types opened as artwork (vector, PDF, or traced pictures).
const ART_EXT: &[&str] = &["svg", "pdf", "ai", "png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"];

struct TraceDlg {
    name: String,
    add: bool,
    src: trace::Raster,
    opts: trace::TraceOptions,
    width_in: f64,
    result: Option<Result<trace::Trace, String>>,
    /// When a setting last changed (seconds; -1 = nothing pending).
    changed_at: f64,
    src_tex: Option<egui::TextureHandle>,
    out_tex: Option<egui::TextureHandle>,
    /// Rough stitch preview of the current trace (built with the open design's recipe).
    stitch_tex: Option<egui::TextureHandle>,
    /// Show the rough stitch preview in the result box instead of the traced art.
    show_stitch: bool,
    drag: Option<(Pos2, Pos2)>,
    notes: Vec<String>,
}

/// Rough stitch preview of a traced SVG, using the same build as the
/// stitch file and the picture the operator sheet uses. Returns RGBA pixels.
fn stitch_preview(
    svg: &str,
    width_mm: f64,
    chart: &[thread::Thread],
    recipe: &settings::Recipe,
    profile: Profile,
) -> Option<(usize, usize, Vec<u8>)> {
    let mut art = stitchforge::import::load_svg_str(svg, &usvg::Options::default()).ok()?;
    stitchforge::import::fit(&mut art, Some(width_mm), None);
    let d = stitchforge::design::build("trace-preview", &art, chart, recipe, profile).ok()?;
    // about 520 px across, like the traced picture
    let px_per_mm = (520.0 / width_mm.max(1.0)) as f32;
    let png = stitchforge::sheet::preview_png(&d, px_per_mm, "#f2efe8").ok()?;
    let raster = stitchforge::trace::Raster::decode(&png).ok()?;
    let px: Vec<u8> = raster.px.iter().flatten().copied().collect();
    Some((raster.w as usize, raster.h as usize, px))
}

/// Render SVG to RGBA pixels, longest side `max_px`.
fn render_svg(svg: &str, max_px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let sz = tree.size();
    let k = max_px as f32 / sz.width().max(sz.height());
    let (w, h) = ((sz.width() * k).ceil() as u32, (sz.height() * k).ceil() as u32);
    let mut pix = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pix.as_mut());
    // tiny-skia is premultiplied; egui wants straight alpha here
    let mut px = pix.take();
    for c in px.chunks_mut(4) {
        let a = c[3] as u32;
        if a > 0 && a < 255 {
            for v in c.iter_mut().take(3) {
                *v = ((*v as u32 * 255) / a).min(255) as u8;
            }
        }
    }
    Some((w as usize, h as usize, px))
}

/// Light checkerboard (transparent areas of a picture).
fn checker(p: &egui::Painter, r: Rect) {
    p.rect_filled(r, 4.0, Color32::from_gray(236));
    let s = 10.0;
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = r.left() + if row % 2 == 0 { 0.0 } else { s };
        while x < r.right() {
            p.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(s)).intersect(r), 0.0, Color32::from_gray(222));
            x += 2.0 * s;
        }
        y += s;
        row += 1;
    }
}

#[derive(Clone, Copy)]
enum Want {
    Open,
    AddSvg,
    Names,
    Chart,
    Font,
}

thread_local! {
    static PICKED: std::cell::RefCell<Vec<(Want, PathBuf)>> = const { std::cell::RefCell::new(Vec::new()) };
    static CTX: std::cell::RefCell<Option<egui::Context>> = const { std::cell::RefCell::new(None) };
}

fn picked(want: Want, p: PathBuf) {
    PICKED.with(|v| v.borrow_mut().push((want, p)));
    CTX.with(|c| {
        if let Some(c) = c.borrow().as_ref() {
            c.request_repaint();
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn pick(want: Want, title: &str, filters: &[(&str, &[&str])], dir: Option<&PathBuf>) {
    let mut dlg = rfd::FileDialog::new().set_title(title);
    for (n, e) in filters {
        dlg = dlg.add_filter(*n, e);
    }
    if let Some(d) = dir {
        dlg = dlg.set_directory(d);
    }
    if let Some(p) = dlg.pick_file() {
        picked(want, p);
    }
}

#[cfg(target_arch = "wasm32")]
fn pick(want: Want, title: &str, filters: &[(&str, &[&str])], _dir: Option<&PathBuf>) {
    // One combined filter: browsers show a single "accept" list anyway.
    let mut all: Vec<&str> = Vec::new();
    for (_, e) in filters {
        all.extend(e.iter().copied());
    }
    let dlg = rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter("Files", &all);
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(f) = dlg.pick_file().await {
            let name = f.file_name();
            let data = f.read().await;
            let p = PathBuf::from(format!("/in/{name}"));
            let _ = stitchforge::vfs::write(&p, &data);
            picked(want, p);
        }
    });
}

fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        static T0: OnceLock<std::time::Instant> = OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// Where exports go: a folder the user picks, or (browser) a scratch
/// folder that is zipped and downloaded afterwards.
#[cfg(not(target_arch = "wasm32"))]
fn export_dir(title: &str, last: Option<&PathBuf>) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new().set_title(title);
    if let Some(d) = last {
        dlg = dlg.set_directory(d);
    }
    dlg.pick_folder()
}
#[cfg(target_arch = "wasm32")]
fn export_dir(_title: &str, _last: Option<&PathBuf>) -> Option<PathBuf> {
    let d = PathBuf::from("/out/export");
    stitchforge::vfs::remove_dir_all(&d);
    Some(d)
}

/// After an export: open the sheet (desktop) or download everything as a
/// zip and show the sheet in the page (browser).
#[cfg(not(target_arch = "wasm32"))]
fn deliver(_dir: &std::path::Path, _zip_name: &str, sheet: &std::path::Path, pdf: &std::path::Path) {
    open_path(if pdf.exists() { pdf } else { sheet });
}
#[cfg(target_arch = "wasm32")]
fn deliver(dir: &std::path::Path, zip_name: &str, sheet: &std::path::Path, _pdf: &std::path::Path) {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o: zip::write::SimpleFileOptions =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut files = stitchforge::vfs::list(dir);
        files.sort();
        for f in files {
            if let Ok(data) = stitchforge::vfs::read(&f) {
                let name = f.file_name().unwrap().to_string_lossy().to_string();
                if z.start_file(name, o).is_ok() {
                    let _ = z.write_all(&data);
                }
            }
        }
        let _ = z.finish();
    }
    web::download(&format!("{zip_name}.zip"), buf.into_inner());
    if let Ok(html) = stitchforge::vfs::read_to_string(sheet) {
        web::show_html(&html);
    }
}

#[cfg(target_arch = "wasm32")]
pub mod web {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_name = sfDownload)]
        fn sf_download(name: &str, data: js_sys::Uint8Array);
        #[wasm_bindgen(js_name = sfShowHtml)]
        fn sf_show_html(html: &str);
    }

    pub fn download(name: &str, data: Vec<u8>) {
        sf_download(name, js_sys::Uint8Array::from(&data[..]));
    }
    pub fn show_html(html: &str) {
        sf_show_html(html);
    }

    /// Fonts bundled with the web app (no installed fonts in a browser).
    pub fn bundled_fonts() -> Vec<&'static [u8]> {
        crate::BUNDLED_FONTS.to_vec()
    }

    #[wasm_bindgen]
    pub struct WebHandle {
        runner: eframe::WebRunner,
    }

    #[wasm_bindgen]
    impl WebHandle {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            eframe::WebLogger::init(log::LevelFilter::Warn).ok();
            Self { runner: eframe::WebRunner::new() }
        }
        #[wasm_bindgen]
        pub async fn start(&self, canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
            for f in bundled_fonts() {
                let _ = stitchforge::import::add_font_data(f.to_vec());
            }
            self.runner
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(|cc| Ok(Box::new(super::App::new(cc)))),
                )
                .await
        }
    }
}
