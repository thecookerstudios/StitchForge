//! Interface themes: Dark, Neutral (60% grey) and Light, each one set of
//! colour tokens. Every panel colour comes from here, so switching the theme
//! restyles the whole app. Thread and garment colours are not themed: they
//! are the real colours of the work.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeKind {
    Dark,
    Neutral,
    Light,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 3] = [ThemeKind::Dark, ThemeKind::Neutral, ThemeKind::Light];
    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "Dark",
            ThemeKind::Neutral => "Neutral grey",
            ThemeKind::Light => "Light",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ThemeKind::Dark => "dark",
            ThemeKind::Neutral => "neutral",
            ThemeKind::Light => "light",
        }
    }
    pub fn from_key(s: &str) -> Option<ThemeKind> {
        ThemeKind::ALL.into_iter().find(|t| t.key() == s.trim())
    }
}

/// The colours of one theme.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    /// Menu bar, status bar, tool column.
    pub chrome: Color32,
    /// Docked panels.
    pub panel: Color32,
    /// Raised rows and the panel tab strip.
    pub panel_alt: Color32,
    /// Text fields and sunken areas.
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    /// The work area around the hoop (the pasteboard).
    pub pasteboard: Color32,
    /// Brand accent: the active tool, the active tab underline.
    pub accent: Color32,
    /// Accent behind selected items (quieter than `accent`).
    pub accent_soft: Color32,
    /// Selection outlines and handles on the canvas.
    pub select: Color32,
    pub warn: Color32,
    pub hover: Color32,
}

pub fn tokens(k: ThemeKind) -> Tokens {
    let c = Color32::from_rgb;
    match k {
        ThemeKind::Dark => Tokens {
            dark: true,
            chrome: c(30, 31, 34),
            panel: c(38, 39, 43),
            panel_alt: c(46, 47, 52),
            field: c(25, 26, 29),
            border: c(60, 61, 67),
            text: c(232, 232, 235),
            text_weak: c(158, 160, 166),
            pasteboard: c(24, 25, 27),
            accent: c(200, 92, 52),
            accent_soft: c(98, 48, 34),
            select: c(77, 144, 254),
            warn: c(232, 168, 56),
            hover: c(56, 57, 63),
        },
        // Mid-grey interface; the work area is 60% grey.
        ThemeKind::Neutral => Tokens {
            dark: true,
            chrome: c(72, 72, 72),
            panel: c(83, 83, 83),
            panel_alt: c(92, 92, 92),
            field: c(62, 62, 62),
            border: c(108, 108, 108),
            text: c(242, 242, 242),
            text_weak: c(204, 204, 204),
            pasteboard: c(102, 102, 102),
            accent: c(214, 104, 62),
            accent_soft: c(122, 70, 52),
            select: c(110, 168, 255),
            warn: c(255, 196, 92),
            hover: c(100, 100, 100),
        },
        ThemeKind::Light => Tokens {
            dark: false,
            chrome: c(233, 233, 236),
            panel: c(245, 245, 247),
            panel_alt: c(236, 236, 240),
            field: c(255, 255, 255),
            border: c(206, 207, 213),
            text: c(29, 30, 33),
            text_weak: c(92, 94, 102),
            pasteboard: c(214, 215, 219),
            accent: c(184, 72, 31),
            accent_soft: c(246, 220, 208),
            select: c(28, 110, 236),
            warn: c(170, 104, 0),
            hover: c(224, 225, 230),
        },
    }
}

/// Apply a theme to egui's visuals.
pub fn apply(ctx: &egui::Context, k: ThemeKind) {
    let t = tokens(k);
    let mut v = if t.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.panel_alt;
    v.code_bg_color = t.field;
    v.window_stroke = Stroke::new(1.0_f32, t.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.override_text_color = None;
    v.hyperlink_color = t.select;
    v.warn_fg_color = t.warn;
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0_f32, if t.dark { Color32::WHITE } else { t.text });
    let r = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = t.panel_alt;
    w.inactive.weak_bg_fill = t.panel_alt;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = t.hover;
    w.hovered.weak_bg_fill = t.hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, t.text);
    w.hovered.corner_radius = r;
    w.active.bg_fill = t.accent_soft;
    w.active.weak_bg_fill = t.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0_f32, t.accent);
    w.active.fg_stroke = Stroke::new(1.5_f32, t.text);
    w.active.corner_radius = r;
    w.open.bg_fill = t.panel_alt;
    w.open.weak_bg_fill = t.panel_alt;
    w.open.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.open.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.open.corner_radius = r;
    ctx.set_visuals(v);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("sf-theme"), k));
}

#[allow(dead_code)]
/// The theme in use (Dark until one is applied).
pub fn current(ctx: &egui::Context) -> Tokens {
    tokens(ctx.data(|d| d.get_temp::<ThemeKind>(egui::Id::new("sf-theme"))).unwrap_or(ThemeKind::Dark))
}

/// Interface preferences kept between runs (desktop only).
pub struct Prefs {
    pub theme: ThemeKind,
    pub workspace: String,
    pub keys: String,
    /// Thread libraries switched on (file names, or "generic").
    pub libs: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn prefs_path() -> std::path::PathBuf {
    stitchforge::thread::library_dir()
        .parent()
        .map(|p| p.join("ui-prefs.txt"))
        .unwrap_or_else(|| std::path::PathBuf::from("ui-prefs.txt"))
}

pub fn load_prefs() -> Prefs {
    let mut p = Prefs { theme: ThemeKind::Dark, workspace: "design".into(), keys: "classic".into(), libs: Vec::new() };
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(s) = std::fs::read_to_string(prefs_path()) {
        for line in s.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "theme" => p.theme = ThemeKind::from_key(v).unwrap_or(p.theme),
                    "workspace" => p.workspace = v.trim().to_string(),
                    "keys" => p.keys = v.trim().to_string(),
                    "libs" => p.libs = v.split('|').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
                    _ => {}
                }
            }
        }
    }
    p
}

pub fn save_prefs(p: &Prefs) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = prefs_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("theme={}\nworkspace={}\nkeys={}\nlibs={}\n", p.theme.key(), p.workspace, p.keys, p.libs.join("|")));
    }
    #[cfg(target_arch = "wasm32")]
    let _ = p;
}
