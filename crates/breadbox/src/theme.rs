//! Visual tokens from `docs/DESIGN.md` (dark default + light set), bundled fonts and the egui
//! style built from them.
//!
//! Screens never hard-code colours: they read [`pal`] (the palette for the current theme) and
//! use the font helpers in [`font`].

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    Theme, epaint::text::VariationCoords,
};
use std::sync::Arc;

/// Colour tokens. Field names follow DESIGN.md (`bg.app` → `bg_app`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// `bg.app` — window background, `.paper` grid base.
    pub bg_app: Color32,
    /// `bg.canvas` — bench / editor canvas.
    pub bg_canvas: Color32,
    /// `bg.sunken` — segmented wells, inputs.
    pub bg_sunken: Color32,
    /// `bg.panel` — side panels.
    pub bg_panel: Color32,
    /// Header / card fill (`rgba(12,12,12,.8)` over the paper).
    pub bg_header: Color32,
    /// Analysis cards (`#0E0E0E`).
    pub bg_card: Color32,
    /// `bg.raised` — ghost buttons, inputs, table headers.
    pub bg_raised: Color32,
    /// `bg.hover`.
    pub bg_hover: Color32,
    /// Selected file row (`#1F1F1F`).
    pub bg_row_selected: Color32,
    /// `bg.selected` — segmented "on".
    pub bg_selected: Color32,
    /// Tiles (`rgba(18,18,18,.92)`).
    pub bg_tile: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    /// Row separators inside tables (`#1C1C1C`).
    pub border_faint: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub text_dim: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    /// Breadcrumb slashes and line numbers (`#4A4A4A`).
    pub text_ghost: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_bg: Color32,
    pub accent_bg2: Color32,
    pub accent_border: Color32,
    /// Tile hover background (`#17130D`).
    pub accent_hover: Color32,
    pub info: Color32,
    pub info_border: Color32,
    pub ok: Color32,
    pub ok_border: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    pub trace_yellow: Color32,
    pub trace_blue: Color32,
    /// Plot grid lines (`#1F1F1F`).
    pub grid: Color32,
    /// `.paper` major / minor grid line colours.
    pub paper_major: Color32,
    pub paper_minor: Color32,
    // code
    pub code_kw: Color32,
    pub code_ty: Color32,
    pub code_str: Color32,
    pub code_num: Color32,
    pub code_cm: Color32,
    pub code_fn: Color32,
    pub code_pp: Color32,
    pub code_text: Color32,
    /// Current (executing) line background.
    pub code_current: Color32,
    pub code_cursor_line: Color32,
    pub selection: Color32,
}

pub const DARK: Palette = Palette {
    dark: true,
    bg_app: Color32::from_rgb(0x0A, 0x0A, 0x0A),
    bg_canvas: Color32::from_rgb(0x06, 0x06, 0x06),
    bg_sunken: Color32::from_rgb(0x07, 0x07, 0x07),
    bg_panel: Color32::from_rgb(0x11, 0x11, 0x11),
    bg_header: Color32::from_rgb(0x0C, 0x0C, 0x0C),
    bg_card: Color32::from_rgb(0x0E, 0x0E, 0x0E),
    bg_raised: Color32::from_rgb(0x17, 0x17, 0x17),
    bg_hover: Color32::from_rgb(0x1C, 0x1C, 0x1C),
    bg_row_selected: Color32::from_rgb(0x1F, 0x1F, 0x1F),
    bg_selected: Color32::from_rgb(0x2A, 0x2A, 0x2A),
    bg_tile: Color32::from_rgb(0x12, 0x12, 0x12),
    border: Color32::from_rgb(0x24, 0x24, 0x24),
    border_strong: Color32::from_rgb(0x2E, 0x2E, 0x2E),
    border_faint: Color32::from_rgb(0x1C, 0x1C, 0x1C),
    text: Color32::from_rgb(0xEA, 0xEA, 0xEA),
    text_strong: Color32::from_rgb(0xF5, 0xF5, 0xF5),
    text_dim: Color32::from_rgb(0xD9, 0xD9, 0xD9),
    text_muted: Color32::from_rgb(0xA3, 0xA3, 0xA3),
    text_faint: Color32::from_rgb(0x85, 0x85, 0x85),
    text_ghost: Color32::from_rgb(0x4A, 0x4A, 0x4A),
    accent: Color32::from_rgb(0xF2, 0xA3, 0x3A),
    accent_soft: Color32::from_rgb(0xF7, 0xC7, 0x7A),
    accent_bg: Color32::from_rgb(0x2A, 0x1F, 0x0E),
    accent_bg2: Color32::from_rgb(0x1D, 0x18, 0x10),
    accent_border: Color32::from_rgb(0x6B, 0x4A, 0x16),
    accent_hover: Color32::from_rgb(0x17, 0x13, 0x0D),
    info: Color32::from_rgb(0x4C, 0xC3, 0xF0),
    info_border: Color32::from_rgb(0x1F, 0x4A, 0x5C),
    ok: Color32::from_rgb(0x3F, 0xBF, 0x6A),
    ok_border: Color32::from_rgb(0x23, 0x50, 0x2F),
    danger: Color32::from_rgb(0xC8, 0x35, 0x2B),
    danger_soft: Color32::from_rgb(0xE0, 0x48, 0x3D),
    trace_yellow: Color32::from_rgb(0xF2, 0xD0, 0x3B),
    trace_blue: Color32::from_rgb(0x6F, 0x9B, 0xEA),
    grid: Color32::from_rgb(0x1F, 0x1F, 0x1F),
    paper_major: Color32::from_rgba_premultiplied(10, 10, 10, 10),
    paper_minor: Color32::from_rgba_premultiplied(4, 4, 4, 4),
    code_kw: Color32::from_rgb(0xC7, 0x92, 0xEA),
    code_ty: Color32::from_rgb(0x4C, 0xC3, 0xF0),
    code_str: Color32::from_rgb(0xB5, 0xD8, 0x8A),
    code_num: Color32::from_rgb(0xF7, 0xC7, 0x7A),
    code_cm: Color32::from_rgb(0x85, 0x85, 0x85),
    code_fn: Color32::from_rgb(0x82, 0xAA, 0xFF),
    code_pp: Color32::from_rgb(0xF0, 0x8A, 0x24),
    code_text: Color32::from_rgb(0xD9, 0xD9, 0xD9),
    code_current: Color32::from_rgb(0x2A, 0x1F, 0x0E),
    code_cursor_line: Color32::from_rgb(0x10, 0x10, 0x10),
    selection: Color32::from_rgb(0x3A, 0x2C, 0x12),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    bg_app: Color32::from_rgb(0xF6, 0xF5, 0xF0),
    bg_canvas: Color32::from_rgb(0xFB, 0xFA, 0xF6),
    bg_sunken: Color32::from_rgb(0xEC, 0xEA, 0xE3),
    bg_panel: Color32::from_rgb(0xFF, 0xFE, 0xFB),
    bg_header: Color32::from_rgb(0xFA, 0xF9, 0xF5),
    bg_card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    bg_raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    bg_hover: Color32::from_rgb(0xF0, 0xEE, 0xE7),
    bg_row_selected: Color32::from_rgb(0xEC, 0xE9, 0xE0),
    bg_selected: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    bg_tile: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    border: Color32::from_rgb(0xE2, 0xDF, 0xD6),
    border_strong: Color32::from_rgb(0xD3, 0xCF, 0xC4),
    border_faint: Color32::from_rgb(0xEC, 0xEA, 0xE3),
    text: Color32::from_rgb(0x1C, 0x1C, 0x1C),
    text_strong: Color32::from_rgb(0x0A, 0x0A, 0x0A),
    text_dim: Color32::from_rgb(0x2E, 0x2E, 0x2E),
    text_muted: Color32::from_rgb(0x5C, 0x5C, 0x5C),
    text_faint: Color32::from_rgb(0x7A, 0x7A, 0x7A),
    text_ghost: Color32::from_rgb(0xA8, 0xA4, 0x9A),
    accent: Color32::from_rgb(0xD9, 0x84, 0x14),
    accent_soft: Color32::from_rgb(0x9A, 0x5B, 0x08),
    accent_bg: Color32::from_rgb(0xFB, 0xEC, 0xD2),
    accent_bg2: Color32::from_rgb(0xFD, 0xF4, 0xE4),
    accent_border: Color32::from_rgb(0xE3, 0xB6, 0x6A),
    accent_hover: Color32::from_rgb(0xFD, 0xF6, 0xEA),
    info: Color32::from_rgb(0x1F, 0x7F, 0xB8),
    info_border: Color32::from_rgb(0xA9, 0xD3, 0xEA),
    ok: Color32::from_rgb(0x2E, 0x8B, 0x4E),
    ok_border: Color32::from_rgb(0xA8, 0xD8, 0xB6),
    danger: Color32::from_rgb(0xC8, 0x35, 0x2B),
    danger_soft: Color32::from_rgb(0xD6, 0x45, 0x3A),
    trace_yellow: Color32::from_rgb(0xC9, 0xA2, 0x0E),
    trace_blue: Color32::from_rgb(0x35, 0x67, 0xC6),
    grid: Color32::from_rgb(0xE6, 0xE3, 0xDA),
    paper_major: Color32::from_rgba_premultiplied(0, 0, 0, 12),
    paper_minor: Color32::from_rgba_premultiplied(0, 0, 0, 5),
    code_kw: Color32::from_rgb(0x8A, 0x3F, 0xB8),
    code_ty: Color32::from_rgb(0x1F, 0x7F, 0xB8),
    code_str: Color32::from_rgb(0x4E, 0x7A, 0x1E),
    code_num: Color32::from_rgb(0xA8, 0x62, 0x0A),
    code_cm: Color32::from_rgb(0x85, 0x85, 0x85),
    code_fn: Color32::from_rgb(0x2F, 0x5F, 0xC8),
    code_pp: Color32::from_rgb(0xC0, 0x5A, 0x0A),
    code_text: Color32::from_rgb(0x1C, 0x1C, 0x1C),
    code_current: Color32::from_rgb(0xFB, 0xEC, 0xD2),
    code_cursor_line: Color32::from_rgb(0xF4, 0xF2, 0xEC),
    selection: Color32::from_rgb(0xF6, 0xDB, 0xA8),
};

/// Palette for the theme active in `ui`.
pub fn pal(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

/// Palette for the theme active in `ctx`.
pub fn pal_ctx(ctx: &egui::Context) -> &'static Palette {
    if ctx.theme() == Theme::Dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// Layout sizes from DESIGN.md.
pub mod size {
    pub const HEADER: f32 = 56.0;
    pub const BUTTON: f32 = 32.0;
    pub const MINI_BUTTON: f32 = 26.0;
    pub const TAB: f32 = 36.0;
    pub const RAIL_BUTTON: f32 = 40.0;
    pub const PARTS_PANEL: f32 = 292.0;
    pub const SIDE_RAIL: f32 = 300.0;
    pub const BOTTOM_DOCK: f32 = 220.0;
    pub const SUBHEADER: f32 = 38.0;
}

/// Font helpers. Sizes are CSS pixels (= egui points).
pub mod font {
    use super::*;

    pub const INTER_MEDIUM: &str = "inter-medium";
    pub const INTER_SEMIBOLD: &str = "inter-semibold";
    pub const MONO_MEDIUM: &str = "mono-medium";
    pub const GROTESK_MEDIUM: &str = "grotesk-medium";
    pub const GROTESK_SEMIBOLD: &str = "grotesk-semibold";

    /// Inter 400.
    pub fn ui(size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }
    /// Inter 500 (buttons, tabs, tiles).
    pub fn medium(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(INTER_MEDIUM.into()))
    }
    /// Inter 600 (section headers, card titles).
    pub fn semibold(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(INTER_SEMIBOLD.into()))
    }
    /// JetBrains Mono 400.
    pub fn mono(size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }
    /// JetBrains Mono 500.
    pub fn mono_medium(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(MONO_MEDIUM.into()))
    }
    /// Space Grotesk 600 — the wordmark.
    pub fn brand(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(GROTESK_SEMIBOLD.into()))
    }
    /// Space Grotesk 500 — panel titles (`.tran — night LED drive`, radio cards).
    pub fn heading(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(GROTESK_MEDIUM.into()))
    }
}

pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub static INTER_MEDIUM_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
pub static INTER_SEMIBOLD_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub static MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub static MONO_MEDIUM_TTF: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
pub static GROTESK_VARIABLE: &[u8] =
    include_bytes!("../../../assets/fonts/SpaceGrotesk-Variable.ttf");

/// Font definitions: bundled fonts first, egui's defaults as glyph fallbacks (arrows, symbols).
pub fn font_definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let fallback_prop = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let fallback_mono = defs
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    let mut add = |name: &str, data: FontData| {
        defs.font_data.insert(name.to_string(), Arc::new(data));
    };
    add("Inter-Regular", FontData::from_static(INTER_REGULAR));
    add("Inter-Medium", FontData::from_static(INTER_MEDIUM_TTF));
    add("Inter-SemiBold", FontData::from_static(INTER_SEMIBOLD_TTF));
    add("JetBrainsMono-Regular", FontData::from_static(MONO_REGULAR));
    add("JetBrainsMono-Medium", FontData::from_static(MONO_MEDIUM_TTF));
    let grotesk = |wght: f32| {
        let mut d = FontData::from_static(GROTESK_VARIABLE);
        d.tweak.coords = VariationCoords::new([(b"wght", wght)]);
        d
    };
    add("SpaceGrotesk-Medium", grotesk(500.0));
    add("SpaceGrotesk-SemiBold", grotesk(600.0));

    let chain = |first: &[&str], fallback: &[String]| -> Vec<String> {
        first
            .iter()
            .map(|s| s.to_string())
            .chain(fallback.iter().cloned())
            .collect()
    };
    defs.families.insert(
        FontFamily::Proportional,
        chain(&["Inter-Regular"], &fallback_prop),
    );
    defs.families.insert(
        FontFamily::Monospace,
        chain(&["JetBrainsMono-Regular", "Inter-Regular"], &fallback_mono),
    );
    defs.families.insert(
        FontFamily::Name(font::INTER_MEDIUM.into()),
        chain(&["Inter-Medium", "Inter-Regular"], &fallback_prop),
    );
    defs.families.insert(
        FontFamily::Name(font::INTER_SEMIBOLD.into()),
        chain(&["Inter-SemiBold", "Inter-Regular"], &fallback_prop),
    );
    defs.families.insert(
        FontFamily::Name(font::MONO_MEDIUM.into()),
        chain(
            &["JetBrainsMono-Medium", "JetBrainsMono-Regular", "Inter-Regular"],
            &fallback_mono,
        ),
    );
    defs.families.insert(
        FontFamily::Name(font::GROTESK_MEDIUM.into()),
        chain(&["SpaceGrotesk-Medium", "Inter-Medium"], &fallback_prop),
    );
    defs.families.insert(
        FontFamily::Name(font::GROTESK_SEMIBOLD.into()),
        chain(&["SpaceGrotesk-SemiBold", "Inter-SemiBold"], &fallback_prop),
    );
    defs
}

/// Builds the egui [`egui::Style`] for one palette.
pub fn style_for(p: &Palette, base: &egui::Style) -> egui::Style {
    use egui::TextStyle;
    let mut s = base.clone();
    s.text_styles = [
        (TextStyle::Small, font::ui(11.0)),
        (TextStyle::Body, font::ui(13.0)),
        (TextStyle::Button, font::medium(13.0)),
        (TextStyle::Heading, font::heading(18.0)),
        (TextStyle::Monospace, font::mono(12.5)),
    ]
    .into();
    s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    s.spacing.button_padding = egui::vec2(12.0, 6.0);
    s.spacing.interact_size = egui::vec2(32.0, 26.0);
    s.spacing.window_margin = Margin::same(14);
    s.spacing.menu_margin = Margin::same(6);
    s.spacing.scroll.bar_width = 6.0;
    s.spacing.scroll.floating = true;
    s.interaction.selectable_labels = false;

    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.dark_mode = p.dark;
    v.panel_fill = p.bg_app;
    v.window_fill = p.bg_panel;
    v.window_stroke = Stroke::new(1.0, p.border_strong);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.bg_sunken;
    v.text_edit_bg_color = Some(p.bg_sunken);
    v.faint_bg_color = p.bg_panel;
    v.code_bg_color = p.bg_raised;
    v.hyperlink_color = p.info;
    v.warn_fg_color = p.accent;
    v.error_fg_color = p.danger_soft;
    v.override_text_color = None;
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0, p.accent_soft);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 140 } else { 40 }),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 120 } else { 30 }),
    };
    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg_panel;
    w.noninteractive.weak_bg_fill = p.bg_panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.bg_raised;
    w.inactive.weak_bg_fill = p.bg_raised;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text_dim);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = p.bg_hover;
    w.hovered.weak_bg_fill = p.bg_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = p.bg_selected;
    w.active.weak_bg_fill = p.bg_selected;
    w.active.bg_stroke = Stroke::new(1.0, p.accent_border);
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open = w.active;
    s.visuals = v;
    s
}

/// Installs fonts and both theme styles; dark is the default.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
    let base = ctx.style_of(Theme::Dark);
    ctx.set_style_of(Theme::Dark, style_for(&DARK, &base));
    let base = ctx.style_of(Theme::Light);
    ctx.set_style_of(Theme::Light, style_for(&LIGHT, &base));
    ctx.options_mut(|o| {
        o.theme_preference = egui::ThemePreference::Dark;
    });
}

/// User-facing theme choice (`View → Theme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

impl ThemeChoice {
    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            ThemeChoice::Dark => egui::ThemePreference::Dark,
            ThemeChoice::Light => egui::ThemePreference::Light,
        });
    }
}

/// Mixes two colours (t = 0 → a, 1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// Paints the `.paper` grid (60 px major / 12 px minor) over `rect`.
pub fn paint_paper(painter: &egui::Painter, rect: egui::Rect, p: &Palette) {
    paint_grid(painter, rect, p.bg_app, 60.0, 12.0, p.paper_major, p.paper_minor);
}

/// Paints the `.dots` canvas grid (80 / 16 px).
pub fn paint_dots(painter: &egui::Painter, rect: egui::Rect, p: &Palette) {
    let (maj, min) = if p.dark {
        (
            Color32::from_rgba_premultiplied(19, 19, 19, 19),
            Color32::from_rgba_premultiplied(7, 7, 7, 7),
        )
    } else {
        (p.paper_major, p.paper_minor)
    };
    paint_grid(painter, rect, p.bg_canvas, 80.0, 16.0, maj, min);
}

fn paint_grid(
    painter: &egui::Painter,
    rect: egui::Rect,
    bg: Color32,
    major: f32,
    minor: f32,
    cmaj: Color32,
    cmin: Color32,
) {
    painter.rect_filled(rect, 0.0, bg);
    let mut shapes = Vec::new();
    let x0 = (rect.left() / minor).floor() * minor;
    let mut x = x0;
    while x <= rect.right() {
        let is_major = (x / major).round() * major == x;
        let c = if is_major { cmaj } else { cmin };
        shapes.push(egui::Shape::line_segment(
            [egui::pos2(x + 0.5, rect.top()), egui::pos2(x + 0.5, rect.bottom())],
            Stroke::new(1.0, c),
        ));
        x += minor;
    }
    let y0 = (rect.top() / minor).floor() * minor;
    let mut y = y0;
    while y <= rect.bottom() {
        let is_major = (y / major).round() * major == y;
        let c = if is_major { cmaj } else { cmin };
        shapes.push(egui::Shape::line_segment(
            [egui::pos2(rect.left(), y + 0.5), egui::pos2(rect.right(), y + 0.5)],
            Stroke::new(1.0, c),
        ));
        y += minor;
    }
    painter.with_clip_rect(rect).extend(shapes);
}
