//! StitchForge desktop app (v0.2 review build).
//! Canvas in the middle, sewing sequence on the left, settings and
//! production info on the right, playback along the bottom.

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use std::path::PathBuf;
use stitchforge::design::{Cmd, Design};
use stitchforge::import::{self, Artwork};
use stitchforge::settings::{self, Profile, Recipe};
use stitchforge::sheet::JobInfo;
use stitchforge::{export, thread};

const SAMPLE_SVG: &str = include_str!("../../examples/left-chest-logo.svg");

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("StitchForge")
            .with_inner_size([1380.0, 860.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("StitchForge", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[derive(Clone, PartialEq)]
enum Source {
    Svg { path: Option<PathBuf>, name: String, data: String },
    Text { text: String, font: String, color: [u8; 3] },
}

struct App {
    source: Option<Source>,
    art: Option<Artwork>,
    design: Option<Design>,
    error: Option<String>,
    dirty: bool,

    width_in: f64,
    profile: Profile,
    fabric: String,
    recipe: Recipe,
    pro: bool,
    chart: Vec<thread::Thread>,
    chart_name: String,

    // text tool
    show_text: bool,
    text: String,
    font: String,
    text_color: [u8; 3],
    fonts: Option<Vec<String>>,
    font_filter: String,

    job: JobInfo,

    // view
    zoom: f32,
    pan: Vec2,
    fit: bool,
    show_jumps: bool,
    selected: Option<usize>,

    // playback (index into design.cmds)
    progress: usize,
    playing: bool,
    speed: f32,
    play_acc: f32,

    status: String,
    last_dir: Option<PathBuf>,
    pending_progress: Option<f32>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        cc.egui_ctx.set_style(style);
        let fabric = Profile::Flat.default_recipe().to_string();
        App {
            source: None,
            art: None,
            design: None,
            error: None,
            dirty: false,
            width_in: 3.5,
            profile: Profile::Flat,
            recipe: settings::recipe(&fabric).unwrap(),
            fabric,
            pro: false,
            chart: thread::generic_chart(),
            chart_name: "Generic 40wt".into(),
            show_text: false,
            text: "Sunny Hill".into(),
            font: "Arial".into(),
            text_color: [24, 36, 80],
            fonts: None,
            font_filter: String::new(),
            job: JobInfo::default(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            show_jumps: false,
            selected: None,
            progress: 0,
            playing: false,
            speed: 400.0,
            play_acc: 0.0,
            status: "Open an SVG, drop one on the window, or stitch some text.".into(),
            last_dir: None,
            pending_progress: None,
        }
        .with_launch_args()
    }

    /// `stitchforge-app logo.svg [--cap] [--pro] [--width-in 3.5]`
    fn with_launch_args(mut self) -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--cap" => self.profile = Profile::Cap,
                "--pro" => self.pro = true,
                "--sample" => self.load(Source::Svg { path: None, name: "sample-logo".into(), data: SAMPLE_SVG.into() }),
                "--width-in" => {
                    if let Some(v) = args.get(i + 1).and_then(|v| v.parse().ok()) {
                        self.width_in = v;
                        i += 1;
                    }
                }
                "--select" => {
                    self.selected = args.get(i + 1).and_then(|v| v.parse().ok());
                    i += 1;
                }
                "--progress" => {
                    self.pending_progress = args.get(i + 1).and_then(|v| v.parse().ok());
                    i += 1;
                }
                "--text" => {
                    if let Some(t) = args.get(i + 1) {
                        self.show_text = true;
                        self.text = t.clone();
                        i += 1;
                    }
                }
                a if a.ends_with(".svg") => self.open_svg_path(PathBuf::from(a)),
                _ => {}
            }
            i += 1;
        }
        if self.profile == Profile::Cap {
            self.fabric = "cap".into();
            self.recipe = settings::recipe("cap").unwrap();
        }
        self
    }

    // ---------- loading and regeneration ----------

    fn load(&mut self, src: Source) {
        let art = match &src {
            Source::Svg { data, .. } => import::font_options(None).and_then(|(o, _)| import::load_svg_str(data, &o)),
            Source::Text { text, font, color } => import::load_text(text, Some(font), None, *color),
        };
        match art {
            Ok(a) => {
                self.art = Some(a);
                self.source = Some(src);
                self.error = None;
                self.selected = None;
                self.fit = true;
                self.dirty = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn open_svg_path(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(data) => {
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("design".into());
                self.last_dir = path.parent().map(|p| p.to_path_buf());
                self.load(Source::Svg { path: Some(path), name, data });
            }
            Err(e) => self.error = Some(format!("Couldn't read {}: {e}", path.display())),
        }
    }

    fn name(&self) -> String {
        match &self.source {
            Some(Source::Svg { name, .. }) => name.clone(),
            Some(Source::Text { text, .. }) => {
                let s: String = text.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
                let s = s.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-");
                if s.is_empty() { "text".into() } else { s.chars().take(40).collect() }
            }
            None => "design".into(),
        }
    }

    fn regenerate(&mut self) {
        self.dirty = false;
        let Some(art) = &self.art else { return };
        let mut a = art.clone();
        import::fit(&mut a, Some(self.width_in * 25.4), None);
        let name = format!("{}-{}", self.name(), if self.profile == Profile::Cap { "cap" } else { "flat" });
        let t = std::time::Instant::now();
        match stitchforge::design::build(&name, &a, &self.chart, &self.recipe, self.profile) {
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

    // ---------- UI ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("StitchForge").strong().size(17.0));
            ui.separator();
            if ui.button("Open SVG…").on_hover_text("Vector art from Illustrator, Inkscape, CorelDRAW, Affinity (Ctrl/Cmd+O)").clicked() {
                self.pick_svg();
            }
            if ui.button("Text…").on_hover_text("Stitch lettering in any installed font").clicked() {
                self.show_text = true;
            }
            if ui.button("Sample logo").clicked() {
                self.load(Source::Svg { path: None, name: "sample-logo".into(), data: SAMPLE_SVG.into() });
            }
            ui.separator();
            if let Some(src) = &self.source {
                let label = match src {
                    Source::Svg { path: Some(p), .. } => p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                    Source::Svg { name, .. } => name.clone(),
                    Source::Text { text, font, .. } => format!("\"{text}\" in {font}"),
                };
                ui.label(RichText::new(label).weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = self.design.is_some();
                if ui.add_enabled(can, egui::Button::new(RichText::new("Export DST + sheet…").strong())).clicked() {
                    self.export();
                }
                ui.separator();
                ui.selectable_value(&mut self.pro, true, "Pro");
                ui.selectable_value(&mut self.pro, false, "Simple");
            });
        });
    }

    fn pick_svg(&mut self) {
        let mut dlg = rfd::FileDialog::new().add_filter("SVG artwork", &["svg"]);
        if let Some(dir) = &self.last_dir {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.open_svg_path(p);
        }
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            section(ui, "Size");
            ui.horizontal(|ui| {
                ui.label("Width");
                if ui.add(egui::DragValue::new(&mut self.width_in).speed(0.02).range(0.3..=14.0).suffix(" in").max_decimals(2)).changed() {
                    self.dirty = true;
                }
                if let Some(d) = &self.design {
                    ui.label(RichText::new(format!("× {:.2} in high", d.stats.height_mm / 25.4)).weak());
                }
            });
            ui.horizontal(|ui| {
                for (w, l) in [(1.0, "1\""), (2.0, "2\""), (3.5, "3.5\" chest"), (4.5, "4.5\" cap")] {
                    if ui.small_button(l).clicked() {
                        self.width_in = w;
                        self.dirty = true;
                    }
                }
            });

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
            ui.label(RichText::new(match self.profile {
                Profile::Flat => "Keeps layers in order and groups colors to cut thread changes.",
                Profile::Cap => "Starts bottom-center and works up and outward; fills sew bottom to top.",
            }).weak().small());
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
                                    self.dirty = true;
                                }
                                Err(e) => self.error = Some(e),
                            }
                        }
                    }
                });
                ui.checkbox(&mut self.show_jumps, "Show jumps and travel on canvas");
            }

            if let Some(d) = &self.design {
                section(ui, "Production");
                let s = &d.stats;
                egui::Grid::new("stats").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                    kv(ui, "Size", &format!("{:.2} × {:.2} in", s.width_mm / 25.4, s.height_mm / 25.4));
                    kv(ui, "", &format!("{:.1} × {:.1} mm", s.width_mm, s.height_mm));
                    kv(ui, "Stitches", &group(s.stitches));
                    kv(ui, "Color stops", &d.stops.len().to_string());
                    kv(ui, "Trims", &s.trims.to_string());
                    kv(ui, "Est. run time", &format!("{:.1} min", s.run_minutes));
                    kv(ui, "Backing", d.recipe.backing);
                    kv(ui, "Topping", d.recipe.topping);
                });

                section(ui, "Color stops");
                for (i, st) in d.stops.iter().enumerate() {
                    ui.horizontal(|ui| {
                        swatch(ui, st.thread.rgb);
                        ui.label(format!("{}  ·  needle {}", i + 1, st.needle));
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
            egui::Grid::new("job").num_columns(2).spacing([10.0, 5.0]).show(ui, |ui| {
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
                    ui.add(egui::TextEdit::singleline(v).desired_width(170.0));
                    ui.end_row();
                }
                ui.label("Notes");
                ui.add(egui::TextEdit::multiline(&mut self.job.notes).desired_width(170.0).desired_rows(2));
                ui.end_row();
            });
            ui.add_space(12.0);
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Sewing sequence").strong());
        ui.label(RichText::new("Click an object to highlight it.").weak().small());
        ui.separator();
        let Some(d) = &self.design else {
            ui.label(RichText::new("Nothing loaded yet.").weak());
            return;
        };
        let mut clicked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut last_stop = usize::MAX;
            for (i, o) in d.objects.iter().enumerate() {
                let stop = d.cmds[..o.cmd_range.0.max(1)]
                    .iter()
                    .rev()
                    .find_map(|c| if let Cmd::Color(s) = c { Some(*s) } else { None });
                if let Some(s) = stop {
                    if s != last_stop {
                        last_stop = s;
                        let st = &d.stops[s];
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            swatch(ui, st.thread.rgb);
                            ui.label(RichText::new(format!("Stop {} · needle {} · {}", s + 1, st.needle, st.thread.name)).strong().small());
                        });
                    }
                }
                let sel = self.selected == Some(i);
                let text = format!("{:>2}. {}   {} · {}", o.sequence, o.label, o.stitch_type, group(o.stitches));
                if ui.selectable_label(sel, text).clicked() {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.selected = if self.selected == Some(i) { None } else { Some(i) };
        }
    }

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
                ui.label(format!("stitch {} of {}", group(n), group(d.stats.stitches)));
            }
            ui.label("Speed");
            ui.add(egui::Slider::new(&mut self.speed, 50.0..=5000.0).logarithmic(true).show_value(false));
            ui.separator();
            if ui.button("Fit").on_hover_text("Fit design to view (F)").clicked() {
                self.fit = true;
            }
            ui.label(RichText::new(&self.status).weak());
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let dark = ui.visuals().dark_mode;
        painter.rect_filled(rect, 0.0, if dark { Color32::from_rgb(44, 43, 40) } else { Color32::from_rgb(236, 233, 226) });

        let Some(d) = &self.design else {
            let c = rect.center();
            let ink = ui.visuals().text_color();
            painter.text(c - Vec2::new(0.0, 30.0), egui::Align2::CENTER_CENTER, "Drop an SVG here", egui::FontId::proportional(26.0), ink);
            painter.text(c + Vec2::new(0.0, 8.0), egui::Align2::CENTER_CENTER, "or use Open SVG…, Text…, or Sample logo above", egui::FontId::proportional(15.0), ui.visuals().weak_text_color());
            if let Some(e) = &self.error {
                painter.text(c + Vec2::new(0.0, 44.0), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(14.0), Color32::from_rgb(210, 70, 60));
            }
            return;
        };

        let b = d.bbox;
        let (dw, dh) = (b.width() as f32, b.height() as f32);
        if self.fit {
            self.zoom = ((rect.width() - 80.0) / dw).min((rect.height() - 80.0) / dh).max(0.5);
            self.pan = Vec2::ZERO;
            self.fit = false;
        }
        // pan and zoom
        if resp.dragged() {
            self.pan += resp.drag_delta();
        }
        if resp.hovered() {
            let (scroll, pinch, cursor) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let factor = pinch * (scroll / 400.0).exp();
            if (factor - 1.0).abs() > 1e-4 {
                if let Some(cur) = cursor {
                    let center = rect.center() + self.pan;
                    let before = (cur - center) / self.zoom;
                    self.zoom = (self.zoom * factor).clamp(0.3, 200.0);
                    self.pan = cur - rect.center() - before * self.zoom;
                }
            }
        }
        let zoom = self.zoom;
        let origin = rect.center() + self.pan;
        let c = b.center();
        let to = |p: stitchforge::geom::Pt| -> Pos2 { origin + Vec2::new((p.x - c.x) as f32, (p.y - c.y) as f32) * zoom };

        let painter = painter.with_clip_rect(rect);
        // fabric swatch behind the design, with a 10 mm grid
        let pad = 6.0 * zoom;
        let fab = Rect::from_min_max(to(b.min) - Vec2::splat(pad), to(b.max) + Vec2::splat(pad));
        painter.rect_filled(fab, 6.0, Color32::from_rgb(246, 243, 236));
        if zoom > 2.0 {
            let grid = Color32::from_rgba_unmultiplied(0, 0, 0, 14);
            let mut x = (b.min.x / 10.0).floor() * 10.0;
            while x <= b.max.x + 6.0 {
                let p = to(stitchforge::geom::Pt::new(x, 0.0)).x;
                painter.line_segment([Pos2::new(p, fab.top()), Pos2::new(p, fab.bottom())], Stroke::new(1.0_f32, grid));
                x += 10.0;
            }
            let mut y = (b.min.y / 10.0).floor() * 10.0;
            while y <= b.max.y + 6.0 {
                let p = to(stitchforge::geom::Pt::new(0.0, y)).y;
                painter.line_segment([Pos2::new(fab.left(), p), Pos2::new(fab.right(), p)], Stroke::new(1.0_f32, grid));
                y += 10.0;
            }
        }

        // stitches, in sewing order, up to the playback position
        let end = self.progress.min(d.cmds.len());
        let (shapes, last) = stitch_shapes(d, 0, end, &to, zoom, self.show_jumps);
        painter.extend(shapes);
        if let Some((a, z)) = self.selected.and_then(|i| d.objects.get(i)).map(|o| o.cmd_range) {
            // veil everything, then redraw the selected object on top
            painter.rect_filled(fab, 6.0, Color32::from_rgba_unmultiplied(246, 243, 236, 200));
            let (sel_shapes, _) = stitch_shapes(d, a, z.min(end), &to, zoom, self.show_jumps);
            painter.extend(sel_shapes);
        }
        if end < d.cmds.len() {
            if let Some(l) = last {
                painter.circle_filled(l, 4.0, Color32::from_rgb(230, 50, 40));
                painter.circle_stroke(l, 7.0, Stroke::new(1.5_f32, Color32::from_rgb(230, 50, 40)));
            }
        }
        // size callout
        let ink = Color32::from_rgb(90, 88, 82);
        painter.text(
            Pos2::new(fab.center().x, fab.bottom() + 8.0),
            egui::Align2::CENTER_TOP,
            format!("{:.2}\" × {:.2}\"  ·  {}", b.width() / 25.4, b.height() / 25.4, d.profile.label()),
            egui::FontId::proportional(13.0),
            ink,
        );
        if resp.double_clicked() {
            self.fit = true;
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
        egui::Window::new("Stitch text").open(&mut open).resizable(false).collapsible(false).default_width(360.0).show(ctx, |ui| {
            ui.label("Words");
            let r = ui.add(egui::TextEdit::singleline(&mut self.text).desired_width(330.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            ui.horizontal(|ui| {
                ui.label("Thread color");
                ui.color_edit_button_srgb(&mut self.text_color);
            });
            ui.label("Font (any installed font)");
            ui.add(egui::TextEdit::singleline(&mut self.font_filter).hint_text("Search fonts…").desired_width(330.0));
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let q = self.font_filter.to_lowercase();
                for f in self.fonts.as_ref().unwrap().iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q)).take(300) {
                    if ui.selectable_label(*f == self.font, f).clicked() {
                        self.font = f.clone();
                    }
                }
            });
            ui.label(RichText::new(format!("Selected: {}", self.font)).weak());
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(Color32::from_rgb(210, 70, 60)));
            }
            ui.add_space(4.0);
            if ui.button(RichText::new("Stitch it").strong()).clicked() {
                go = true;
            }
        });
        if go && !self.text.trim().is_empty() {
            self.load(Source::Text { text: self.text.clone(), font: self.font.clone(), color: self.text_color });
            if self.error.is_none() {
                self.width_in = self.width_in.max(2.0);
                self.show_text = false;
            }
        }
        if !open {
            self.show_text = false;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // drag and drop
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open_svg_path(p);
        }
        // shortcuts
        let (open, fit, space) = ctx.input(|i| (
            i.modifiers.command && i.key_pressed(egui::Key::O),
            i.key_pressed(egui::Key::F),
            i.key_pressed(egui::Key::Space),
        ));
        let typing = ctx.wants_keyboard_input();
        if open {
            self.pick_svg();
        }
        if fit && !typing {
            self.fit = true;
        }
        if space && !typing && self.design.is_some() {
            self.playing = !self.playing;
        }

        if self.dirty {
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

        egui::TopBottomPanel::top("top").frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(10, 8))).show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("bottom").frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(10, 6))).show(ctx, |ui| self.bottom_bar(ui));
        egui::SidePanel::left("seq").resizable(true).default_width(290.0).show(ctx, |ui| self.left_panel(ui));
        egui::SidePanel::right("props").resizable(true).default_width(330.0).show(ctx, |ui| self.right_panel(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| self.canvas(ui));
        self.text_window(ctx);
        if let Some(e) = self.error.clone() {
            if self.design.is_some() {
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
    }
}

/// Thread-look line shapes for commands `from..to` (in sewing order).
/// Returns the shapes and the last needle position.
fn stitch_shapes(
    d: &Design,
    from: usize,
    to_idx: usize,
    to: &dyn Fn(stitchforge::geom::Pt) -> Pos2,
    zoom: f32,
    show_jumps: bool,
) -> (Vec<Shape>, Option<Pos2>) {
    let thread_w = (0.36 * zoom).max(1.0);
    let mut stop = d.cmds[..from].iter().rev().find_map(|c| if let Cmd::Color(s) = c { Some(*s) } else { None }).unwrap_or(0);
    let mut shapes: Vec<Shape> = Vec::new();
    let mut line: Vec<Pos2> = Vec::new();
    let mut last: Option<Pos2> = None;
    let mut col = Color32::BLACK;
    let flush = |line: &mut Vec<Pos2>, col: Color32, shapes: &mut Vec<Shape>| {
        if line.len() >= 2 {
            let shade = Color32::from_rgb((col.r() as f32 * 0.55) as u8, (col.g() as f32 * 0.55) as u8, (col.b() as f32 * 0.55) as u8);
            shapes.push(Shape::line(line.clone(), Stroke::new(thread_w * 1.45, shade)));
            shapes.push(Shape::line(std::mem::take(line), Stroke::new(thread_w, col)));
        }
        line.clear();
    };
    let jump_col = Color32::from_rgba_unmultiplied(220, 40, 40, 160);
    for cmd in &d.cmds[from..to_idx] {
        match *cmd {
            Cmd::Color(s) => {
                flush(&mut line, col, &mut shapes);
                stop = s;
            }
            Cmd::Trim => {
                flush(&mut line, col, &mut shapes);
                last = None;
            }
            Cmd::Jump(p) => {
                flush(&mut line, col, &mut shapes);
                if show_jumps {
                    if let Some(l) = last {
                        shapes.extend(Shape::dashed_line(&[l, to(p)], Stroke::new(1.0_f32, jump_col), 4.0, 3.0));
                    }
                }
                last = Some(to(p));
            }
            Cmd::Stitch(p) => {
                let rgb = d.stops[stop].thread.rgb;
                let c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                // short polylines so later stitches paint over earlier ones
                if c != col || line.len() > 24 {
                    let keep = line.last().copied();
                    flush(&mut line, col, &mut shapes);
                    if let Some(k) = keep {
                        line.push(k);
                    }
                    col = c;
                }
                if line.is_empty() {
                    if let Some(l) = last {
                        line.push(l);
                    }
                }
                let q = to(p);
                line.push(q);
                last = Some(q);
            }
            Cmd::End => {}
        }
    }
    flush(&mut line, col, &mut shapes);
    (shapes, last)
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title.to_uppercase()).small().strong().weak());
    ui.add_space(2.0);
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
