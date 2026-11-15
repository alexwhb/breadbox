//! Buttons, segmented controls, tiles, chips, section headers, sliders with named stops,
//! checkboxes, tab strips and right-rail buttons — the design's `.ghost`, `.seg`, `.mini`,
//! `.tile`, `.fchip`, `.tag`, `.rb`, `.tab` classes.
//!
//! Every interactive widget reports an AccessKit label (`WidgetInfo`) so `egui_kittest` can
//! find it by name.

use super::icons::Icon;
use crate::theme::{self, Palette, font, pal};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Galley, Pos2, Rect, Response, Role,
    Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, pos2, text::LayoutJob, vec2,
};
use std::sync::Arc;

/// Lays out one line of text.
pub fn galley(ui: &Ui, text: impl Into<String>, font: FontId, color: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.into(), font, color)
}

/// Lays out text with extra letter spacing (uppercase section headers: `.08em`).
pub fn galley_spaced(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// Paints a galley centred vertically at `pos` (left aligned).
pub fn paint_galley_left(ui: &Ui, g: &Arc<Galley>, left: f32, center_y: f32, color: Color32) {
    ui.painter()
        .galley(pos2(left, center_y - g.size().y / 2.0), g.clone(), color);
}

// ------------------------------------------------------------------------------------------
// Buttons

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    /// `.ghost` — bordered, raised.
    Ghost,
    /// `.ghost` with the amber primary colours (Run, Upload to sim, Run analyses, Check).
    Amber,
    /// `.mini` — borderless 26 px.
    Mini,
    /// Borderless text button (links, toolbar actions).
    Plain,
    /// Destructive ghost.
    Danger,
}

/// A design button. `Btn::ghost("Verify").show(ui)`.
#[must_use]
pub struct Btn<'a> {
    label: &'a str,
    kind: BtnKind,
    icon: Option<Icon>,
    icon_only: bool,
    selected: bool,
    enabled: bool,
    height: Option<f32>,
    font_size: Option<f32>,
    min_width: f32,
    tooltip: Option<&'a str>,
    full_width: bool,
}

impl<'a> Btn<'a> {
    pub fn new(kind: BtnKind, label: &'a str) -> Self {
        Btn {
            label,
            kind,
            icon: None,
            icon_only: false,
            selected: false,
            enabled: true,
            height: None,
            font_size: None,
            min_width: 0.0,
            tooltip: None,
            full_width: false,
        }
    }
    pub fn ghost(label: &'a str) -> Self {
        Self::new(BtnKind::Ghost, label)
    }
    pub fn amber(label: &'a str) -> Self {
        Self::new(BtnKind::Amber, label)
    }
    pub fn mini(label: &'a str) -> Self {
        Self::new(BtnKind::Mini, label)
    }
    pub fn plain(label: &'a str) -> Self {
        Self::new(BtnKind::Plain, label)
    }
    pub fn danger(label: &'a str) -> Self {
        Self::new(BtnKind::Danger, label)
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    /// Only the icon is drawn; `label` is still the accessible name + tooltip.
    pub fn icon_only(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self.icon_only = true;
        self
    }
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = Some(h);
        self
    }
    pub fn small(mut self) -> Self {
        self.height = Some(28.0);
        self.font_size = Some(12.0);
        self
    }
    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = Some(s);
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let p = pal(ui);
        let (h, fs, pad) = match self.kind {
            BtnKind::Mini => (
                self.height.unwrap_or(theme::size::MINI_BUTTON),
                self.font_size.unwrap_or(12.0),
                10.0,
            ),
            BtnKind::Plain => (
                self.height.unwrap_or(28.0),
                self.font_size.unwrap_or(13.0),
                8.0,
            ),
            _ => (
                self.height.unwrap_or(theme::size::BUTTON),
                self.font_size.unwrap_or(13.0),
                12.0,
            ),
        };
        let g = (!self.icon_only).then(|| galley(ui, self.label, font::medium(fs), p.text));
        let icon_size = if self.kind == BtnKind::Mini {
            12.0
        } else {
            14.0
        };
        let mut w = pad * 2.0;
        if let Some(g) = &g {
            w += g.size().x;
        }
        if self.icon.is_some() {
            w += icon_size;
            if g.is_some() {
                w += 8.0;
            }
        }
        if self.icon_only {
            w = h;
        }
        w = w.max(self.min_width);
        if self.full_width {
            w = w.max(ui.available_width());
        }
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, mut resp) = ui.allocate_exact_size(vec2(w, h), sense);
        let label = self.label.to_string();
        let enabled = self.enabled;
        let selected = self.selected;
        resp.widget_info(|| {
            let mut i = WidgetInfo::labeled(Role::Button, enabled, &label);
            i.selected = Some(selected);
            i
        });
        if ui.is_rect_visible(rect) {
            let hovered = resp.hovered() && enabled;
            let pressed = resp.is_pointer_button_down_on() && enabled;
            let (fill, stroke, fg) = button_colors(p, self.kind, self.selected, hovered, pressed);
            let r = if self.kind == BtnKind::Mini { 5 } else { 6 };
            ui.painter().rect(
                rect,
                CornerRadius::same(r),
                fill,
                stroke,
                StrokeKind::Inside,
            );
            let fg = if enabled { fg } else { fg.gamma_multiply(0.45) };
            let mut x = if self.icon_only {
                rect.center().x - icon_size / 2.0
            } else {
                rect.center().x - (w - pad * 2.0) / 2.0
            };
            if self.full_width || self.min_width > 0.0 {
                // keep content centred
                let content = w - pad * 2.0;
                let natural = g.as_ref().map(|g| g.size().x).unwrap_or(0.0)
                    + self.icon.map(|_| icon_size).unwrap_or(0.0)
                    + if self.icon.is_some() && g.is_some() {
                        8.0
                    } else {
                        0.0
                    };
                x = rect.center().x - natural.min(content) / 2.0;
            }
            if let Some(icon) = self.icon {
                let ir = Rect::from_min_size(
                    pos2(x, rect.center().y - icon_size / 2.0),
                    vec2(icon_size, icon_size),
                );
                icon.paint_with(ui.painter(), ir, fg, p.accent);
                x += icon_size + 8.0;
            }
            if let Some(g) = g {
                paint_galley_left(ui, &g, x, rect.center().y, fg);
            }
        }
        if enabled {
            resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        }
        if let Some(t) = self.tooltip {
            resp = resp.on_hover_text(t);
        } else if self.icon_only {
            resp = resp.on_hover_text(self.label);
        }
        resp
    }
}

fn button_colors(
    p: &Palette,
    kind: BtnKind,
    selected: bool,
    hovered: bool,
    pressed: bool,
) -> (Color32, Stroke, Color32) {
    match kind {
        BtnKind::Ghost => {
            if selected {
                (
                    p.accent_bg2,
                    Stroke::new(1.0, p.accent_border),
                    p.accent_soft,
                )
            } else {
                let fill = if pressed {
                    p.bg_selected
                } else if hovered {
                    theme::mix(p.bg_raised, p.bg_selected, 0.35)
                } else {
                    p.bg_raised
                };
                (fill, Stroke::new(1.0, p.border_strong), p.text_dim)
            }
        }
        BtnKind::Amber => {
            let fill = if pressed {
                theme::mix(p.accent_bg, p.accent_border, 0.5)
            } else if hovered {
                theme::mix(p.accent_bg, p.accent_border, 0.25)
            } else {
                p.accent_bg
            };
            (fill, Stroke::new(1.0, p.accent_border), p.accent_soft)
        }
        BtnKind::Mini => {
            if selected {
                (p.bg_selected, Stroke::NONE, p.text_strong)
            } else if hovered {
                (Color32::TRANSPARENT, Stroke::NONE, p.text)
            } else {
                (Color32::TRANSPARENT, Stroke::NONE, p.text_muted)
            }
        }
        BtnKind::Plain => {
            let fill = if hovered {
                p.bg_hover
            } else {
                Color32::TRANSPARENT
            };
            (
                fill,
                Stroke::NONE,
                if selected { p.accent_soft } else { p.text_dim },
            )
        }
        BtnKind::Danger => {
            let fill = if hovered {
                theme::mix(p.bg_raised, p.danger, 0.15)
            } else {
                p.bg_raised
            };
            (
                fill,
                Stroke::new(1.0, p.danger.gamma_multiply(0.6)),
                p.danger_soft,
            )
        }
    }
}

// ------------------------------------------------------------------------------------------
// Segmented controls

/// One option of a segmented control.
pub struct Seg<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub icon: Option<Icon>,
}

impl<'a, T> Seg<'a, T> {
    pub fn new(value: T, label: &'a str) -> Self {
        Seg {
            value,
            label,
            icon: None,
        }
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// `.seg` group (header view toggles): 32 px buttons in a sunken well. Returns true when the
/// value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    group_label: &str,
    value: &mut T,
    options: &[Seg<'_, T>],
) -> bool {
    seg_impl(ui, group_label, value, options, false)
}

/// `.mini` group (e.g. Simple / Advanced): 26 px buttons in a tighter well.
pub fn mini_segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    group_label: &str,
    value: &mut T,
    options: &[Seg<'_, T>],
) -> bool {
    seg_impl(ui, group_label, value, options, true)
}

fn seg_impl<T: PartialEq + Copy>(
    ui: &mut Ui,
    group_label: &str,
    value: &mut T,
    options: &[Seg<'_, T>],
    mini: bool,
) -> bool {
    let p = pal(ui);
    let (pad, h, fs, item_pad, well_r, item_r) = if mini {
        (2.0, 26.0, 12.0, 10.0, 7, 5)
    } else {
        (3.0, 32.0, 13.0, 14.0, 8, 6)
    };
    let icon_size = 14.0;
    let galleys: Vec<_> = options
        .iter()
        .map(|o| galley(ui, o.label, font::medium(fs), p.text))
        .collect();
    let widths: Vec<f32> = options
        .iter()
        .zip(&galleys)
        .map(|(o, g)| g.size().x + item_pad * 2.0 + o.icon.map(|_| icon_size + 8.0).unwrap_or(0.0))
        .collect();
    let total = widths.iter().sum::<f32>() + 2.0 * (options.len().saturating_sub(1)) as f32;
    let well_size = vec2(total + pad * 2.0, h + pad * 2.0);
    let (well, resp) = ui.allocate_exact_size(well_size, Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Group, true, group_label));
    let (well_fill, well_stroke) = if mini {
        (p.bg_sunken, p.border)
    } else {
        (p.bg_canvas, p.border)
    };
    ui.painter().rect(
        well,
        CornerRadius::same(well_r),
        well_fill,
        Stroke::new(1.0, well_stroke),
        StrokeKind::Inside,
    );
    let mut changed = false;
    let mut x = well.left() + pad;
    for (i, o) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, well.top() + pad), vec2(widths[i], h));
        x += widths[i] + 2.0;
        let id = ui.scope_id().with(group_label).with(i);
        let item = ui.interact(r, id, Sense::click());
        let on = *value == o.value;
        let label = o.label.to_string();
        item.widget_info(|| {
            let mut info = WidgetInfo::selected(Role::Button, true, on, &label);
            info.selected = Some(on);
            info
        });
        if item.clicked() && !on {
            *value = o.value;
            changed = true;
        }
        let fg = if on {
            p.text_strong
        } else if item.hovered() {
            p.text
        } else {
            p.text_muted
        };
        if on {
            ui.painter()
                .rect_filled(r, CornerRadius::same(item_r), p.bg_selected);
        }
        let mut cx = r.left() + item_pad;
        if let Some(icon) = o.icon {
            icon.paint(
                ui.painter(),
                Rect::from_min_size(
                    pos2(cx, r.center().y - icon_size / 2.0),
                    vec2(icon_size, icon_size),
                ),
                fg,
            );
            cx += icon_size + 8.0;
        }
        paint_galley_left(ui, &galleys[i], cx, r.center().y, fg);
        item.on_hover_cursor(CursorIcon::PointingHand);
    }
    changed
}

// ------------------------------------------------------------------------------------------
// Part tiles

/// `.tile` — a part in the parts palette (icon above a label). Returns the response (drag
/// source for the bench).
pub fn part_tile(ui: &mut Ui, icon: Icon, label: &str, size: Vec2, selected: bool) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let l = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &l));
    if ui.is_rect_visible(rect) {
        let hot = resp.hovered() || selected;
        let (fill, stroke) = if hot {
            (p.accent_hover, p.accent_border)
        } else {
            (p.bg_tile, p.border)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(8),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        let g = ui.painter().layout(
            label.to_string(),
            font::medium(11.0),
            p.text_dim,
            rect.width() - 8.0,
        );
        let total = 30.0 + 6.0 + g.size().y;
        let top = rect.center().y - total / 2.0;
        let ir = Rect::from_center_size(pos2(rect.center().x, top + 15.0), vec2(30.0, 30.0));
        icon.paint_with(ui.painter(), ir, p.text_dim, p.accent);
        ui.painter().galley(
            pos2(rect.center().x - g.size().x / 2.0, top + 36.0),
            g,
            p.text_dim,
        );
    }
    resp.on_hover_cursor(CursorIcon::Grab)
}

// ------------------------------------------------------------------------------------------
// Chips and tags

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Ok,
    Warn,
    Danger,
    Accent,
}

fn tone_colors(p: &Palette, tone: Tone) -> (Color32, Color32) {
    match tone {
        Tone::Neutral => (p.text_muted, p.border_strong),
        Tone::Info => (p.info, p.info_border),
        Tone::Ok => (p.ok, p.ok_border),
        Tone::Warn => (p.accent, p.accent_border),
        Tone::Danger => (p.danger_soft, p.danger.gamma_multiply(0.5)),
        Tone::Accent => (p.accent_soft, p.accent_border),
    }
}

/// Small bordered tag (`Playground`, `Pro`, `connected`). 11 px Inter, radius 4.
pub fn tag(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    tag_with_font(ui, text, tone, font::ui(11.0))
}

/// `.tag` from the plugins screen: 10 px mono (`INSTRUMENT · installed`), cyan by default.
pub fn kind_tag(ui: &mut Ui, text: &str) -> Response {
    tag_with_font(ui, text, Tone::Info, font::mono_medium(10.0))
}

fn tag_with_font(ui: &mut Ui, text: &str, tone: Tone, font: FontId) -> Response {
    let p = pal(ui);
    let (fg, border) = tone_colors(p, tone);
    let g = galley(ui, text, font, fg);
    let size = vec2(g.size().x + 12.0, g.size().y + 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let t = text.to_string();
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, &t));
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(rect.left() + 6.0, rect.center().y - g.size().y / 2.0),
        g,
        fg,
    );
    resp
}

/// `.fchip` — pill filter chip (26 px, radius 999). Click toggles.
pub fn filter_chip(ui: &mut Ui, text: &str, on: bool) -> Response {
    let p = pal(ui);
    let fg = if on { p.accent_soft } else { p.text_muted };
    let g = galley(ui, text, font::medium(11.0), fg);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 26.0), Sense::click());
    let t = text.to_string();
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, on, &t));
    let (fill, stroke) = if on {
        (p.accent_bg2, p.accent_border)
    } else if resp.hovered() {
        (p.bg_hover, p.border_strong)
    } else {
        (Color32::TRANSPARENT, p.border_strong)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(13),
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0),
        g,
        fg,
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// A coloured status dot (7 px) + label, as in `● running`.
pub fn status_dot(ui: &mut Ui, color: Color32, label: &str, label_color: Color32) -> Response {
    let g = galley(ui, label, font::mono(12.0), label_color);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(7.0 + 6.0 + g.size().x, g.size().y.max(14.0)),
        Sense::hover(),
    );
    ui.painter()
        .circle_filled(pos2(rect.left() + 3.5, rect.center().y), 3.5, color);
    ui.painter().galley(
        pos2(rect.left() + 13.0, rect.center().y - g.size().y / 2.0),
        g,
        label_color,
    );
    let l = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, &l));
    resp
}

/// Legend swatch: a 14×3 bar + label (`— V(D9)`).
pub fn legend_swatch(ui: &mut Ui, color: Color32, label: &str) -> Response {
    let p = pal(ui);
    let g = galley(ui, label, font::ui(12.0), p.text_muted);
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0 + 6.0 + g.size().x, 16.0), Sense::hover());
    ui.painter().rect_filled(
        Rect::from_min_size(pos2(rect.left(), rect.center().y - 1.5), vec2(14.0, 3.0)),
        0.0,
        color,
    );
    ui.painter().galley(
        pos2(rect.left() + 20.0, rect.center().y - g.size().y / 2.0),
        g,
        p.text_muted,
    );
    resp
}

// ------------------------------------------------------------------------------------------
// Headers and labels

/// Uppercase section header: 11 px, semibold, tracking .08em, muted (`PARTS`, `RADIOS`).
pub fn section_header(ui: &mut Ui, text: &str) -> Response {
    let p = pal(ui);
    let up = text.to_uppercase();
    let g = galley_spaced(ui, &up, font::semibold(11.0), p.text_muted, 0.88);
    let (rect, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.text_muted);
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, &up));
    resp
}

/// Header row: uppercase title left, muted caption right (`RADIOS ... virtual RF space`).
pub fn section_header_row(ui: &mut Ui, title: &str, caption: &str) {
    ui.horizontal(|ui| {
        section_header(ui, title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let p = pal(ui);
            ui.label(
                egui::RichText::new(caption)
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
        });
    });
}

/// Plain text label in a given font/colour, with an accessible name.
pub fn text(ui: &mut Ui, text: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.label(egui::RichText::new(text.into()).font(font).color(color))
}

/// Two-column key/value grid (`Supply  3.3–5.5 V`): muted Inter keys, mono values.
pub fn kv_grid(ui: &mut Ui, id: &str, rows: &[(String, String)], key_width: f32) {
    let p = pal(ui);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing(vec2(12.0, 6.0))
        .min_col_width(key_width)
        .show(ui, |ui| {
            for (k, v) in rows {
                ui.label(
                    egui::RichText::new(k)
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(v).font(font::mono(12.0)).color(p.text))
                        .wrap(),
                );
                ui.end_row();
            }
        });
}

// ------------------------------------------------------------------------------------------
// Checkbox

/// Design checkbox: 16 px amber box with a check mark, 13 px label, 10 px gap.
pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let p = pal(ui);
    let g = ui.painter().layout(
        label.to_string(),
        font::ui(13.0),
        p.text,
        (ui.available_width() - 26.0).max(40.0),
    );
    let size = vec2(16.0 + 10.0 + g.size().x, g.size().y.max(18.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    let on = *value;
    let l = label.to_string();
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, on, &l));
    let b = Rect::from_min_size(pos2(rect.left(), rect.center().y - 8.0), vec2(16.0, 16.0));
    if on {
        ui.painter().rect_filled(b, CornerRadius::same(3), p.accent);
        let pts = vec![
            pos2(b.left() + 3.6, b.center().y + 0.3),
            pos2(b.left() + 6.6, b.bottom() - 4.0),
            pos2(b.right() - 3.4, b.top() + 4.2),
        ];
        ui.painter().add(egui::Shape::line(
            pts,
            Stroke::new(
                2.0,
                if p.dark {
                    Color32::from_rgb(0x1A, 0x12, 0x05)
                } else {
                    Color32::WHITE
                },
            ),
        ));
    } else {
        let stroke = if resp.hovered() {
            p.text_muted
        } else {
            p.text_faint
        };
        ui.painter().rect(
            b,
            CornerRadius::same(3),
            if p.dark {
                Color32::from_rgb(0x0E, 0x0E, 0x0E)
            } else {
                Color32::WHITE
            },
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
    }
    ui.painter().galley(
        pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0),
        g,
        p.text,
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

// ------------------------------------------------------------------------------------------
// Slider with named stops (World panel)

/// A named stop under a slider (`Bone dry`, `Field capacity`, `Saturated`).
#[derive(Debug, Clone, Copy)]
pub struct Stop<'a> {
    pub value: f64,
    pub label: &'a str,
}

/// World-panel slider: label + mono amber value readout, an amber track, named stops below.
pub struct StopSlider<'a> {
    pub label: &'a str,
    pub range: std::ops::RangeInclusive<f64>,
    pub step: f64,
    pub stops: &'a [Stop<'a>],
    /// Formats the readout (`42 % VWC`).
    pub format: &'a dyn Fn(f64) -> String,
    /// Snap to a stop when released within this fraction of the range.
    pub snap: f64,
}

impl<'a> StopSlider<'a> {
    pub fn new(
        label: &'a str,
        range: std::ops::RangeInclusive<f64>,
        format: &'a dyn Fn(f64) -> String,
    ) -> Self {
        StopSlider {
            label,
            range,
            step: 0.0,
            stops: &[],
            format,
            snap: 0.0,
        }
    }
    pub fn stops(mut self, stops: &'a [Stop<'a>]) -> Self {
        self.stops = stops;
        self
    }
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }
    pub fn snap(mut self, fraction: f64) -> Self {
        self.snap = fraction;
        self
    }

    /// Value after snapping/stepping (pure, unit-tested).
    pub fn quantize(&self, v: f64) -> f64 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let mut v = v.clamp(lo, hi);
        if self.snap > 0.0 {
            let tol = (hi - lo) * self.snap;
            if let Some(s) = self
                .stops
                .iter()
                .filter(|s| (s.value - v).abs() <= tol)
                .min_by(|a, b| (a.value - v).abs().total_cmp(&(b.value - v).abs()))
            {
                return s.value;
            }
        }
        if self.step > 0.0 {
            v = lo + ((v - lo) / self.step).round() * self.step;
        }
        v.clamp(lo, hi)
    }

    /// Nearest stop label for a value (for the accessible description).
    pub fn nearest_stop(&self, v: f64) -> Option<&'a str> {
        self.stops
            .iter()
            .min_by(|a, b| (a.value - v).abs().total_cmp(&(b.value - v).abs()))
            .map(|s| s.label)
    }

    pub fn show(self, ui: &mut Ui, value: &mut f64) -> Response {
        let p = pal(ui);
        let w = ui.available_width();
        ui.allocate_ui(vec2(w, 0.0), |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            // label row
            let readout = (self.format)(*value);
            let lg = galley(ui, self.label, font::ui(13.0), p.text);
            let vg = galley(ui, &readout, font::mono(13.0), p.accent_soft);
            let (row, _) =
                ui.allocate_exact_size(vec2(w, lg.size().y.max(vg.size().y)), Sense::hover());
            ui.painter().galley(
                pos2(row.left(), row.center().y - lg.size().y / 2.0),
                lg,
                p.text,
            );
            ui.painter().galley(
                pos2(
                    row.right() - vg.size().x,
                    row.center().y - vg.size().y / 2.0,
                ),
                vg,
                p.accent_soft,
            );
            // track
            let (track_rect, mut resp) =
                ui.allocate_exact_size(vec2(w, 16.0), Sense::click_and_drag());
            let (lo, hi) = (*self.range.start(), *self.range.end());
            let knob_r = 7.0;
            let x0 = track_rect.left() + knob_r;
            let x1 = track_rect.right() - knob_r;
            let to_x = |v: f64| x0 + ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 * (x1 - x0);
            if let Some(pos) = resp.interact_pointer_pos()
                && (resp.dragged() || resp.clicked())
            {
                let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64;
                let raw = lo + t * (hi - lo);
                let nv = if resp.drag_stopped() || resp.clicked() {
                    self.quantize(raw)
                } else if self.step > 0.0 {
                    lo + ((raw - lo) / self.step).round() * self.step
                } else {
                    raw
                };
                if (nv - *value).abs() > f64::EPSILON {
                    *value = nv;
                    resp.mark_changed();
                }
            }
            if resp.drag_stopped() {
                let q = self.quantize(*value);
                if (q - *value).abs() > f64::EPSILON {
                    *value = q;
                    resp.mark_changed();
                }
            }
            // keyboard
            if resp.has_focus() {
                let step = if self.step > 0.0 {
                    self.step
                } else {
                    (hi - lo) / 100.0
                };
                let (l, r) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowLeft),
                        i.key_pressed(egui::Key::ArrowRight),
                    )
                });
                if l || r {
                    *value = (*value + if r { step } else { -step }).clamp(lo, hi);
                    resp.mark_changed();
                }
            }
            let cy = track_rect.center().y;
            let track = Rect::from_min_max(pos2(x0, cy - 2.0), pos2(x1, cy + 2.0));
            ui.painter().rect_filled(
                track,
                CornerRadius::same(2),
                if p.dark {
                    Color32::from_rgb(0x2E, 0x2E, 0x2E)
                } else {
                    p.border_strong
                },
            );
            let kx = to_x(*value);
            ui.painter().rect_filled(
                Rect::from_min_max(track.min, pos2(kx, track.max.y)),
                CornerRadius::same(2),
                p.accent,
            );
            for s in self.stops {
                let sx = to_x(s.value);
                ui.painter().circle_filled(
                    pos2(sx, cy),
                    1.5,
                    if s.value <= *value {
                        p.accent_bg
                    } else {
                        p.text_ghost
                    },
                );
            }
            let hot = resp.hovered() || resp.dragged();
            ui.painter().circle(
                pos2(kx, cy),
                if hot { knob_r + 1.0 } else { knob_r },
                p.accent,
                Stroke::new(
                    2.0,
                    if p.dark {
                        Color32::from_rgb(0x2A, 0x1F, 0x0E)
                    } else {
                        Color32::WHITE
                    },
                ),
            );
            let label = self.label.to_string();
            let v = *value;
            let desc = format!("{label} {readout}");
            resp.widget_info(|| {
                let mut i = WidgetInfo::slider(true, v, &label);
                i.label = Some(desc.clone());
                i
            });
            // stops row
            if !self.stops.is_empty() {
                let (srow, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
                let n = self.stops.len();
                for (i, s) in self.stops.iter().enumerate() {
                    let g = galley(ui, s.label, font::ui(11.0), p.text_muted);
                    let x = if i == 0 {
                        srow.left()
                    } else if i == n - 1 {
                        srow.right() - g.size().x
                    } else {
                        (to_x(s.value) - g.size().x / 2.0)
                            .clamp(srow.left(), srow.right() - g.size().x)
                    };
                    ui.painter().galley(pos2(x, srow.top()), g, p.text_muted);
                }
            }
            resp
        })
        .inner
    }
}

// ------------------------------------------------------------------------------------------
// Tabs

/// `.tab` strip: 36 px tabs, 12 px medium labels, 2 px amber underline on the active one,
/// 18 px gap. Returns true when the selection changed.
pub fn tab_strip(ui: &mut Ui, selected: &mut usize, tabs: &[&str]) -> bool {
    let p = pal(ui);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        for (i, t) in tabs.iter().enumerate() {
            let on = *selected == i;
            if tab(ui, t, on).clicked() && !on {
                *selected = i;
                changed = true;
            }
        }
    });
    let _ = p;
    changed
}

/// One `.tab` (36 px, amber underline when on).
pub fn tab(ui: &mut Ui, label: &str, on: bool) -> Response {
    tab_sized(ui, label, on, theme::size::TAB, 12.0)
}

/// A tab with explicit height / font size (editor tabs are 38 px with 12 px labels).
pub fn tab_sized(ui: &mut Ui, label: &str, on: bool, h: f32, fs: f32) -> Response {
    let p = pal(ui);
    let fg = |hover: bool| {
        if on {
            p.text_strong
        } else if hover {
            p.text
        } else {
            p.text_muted
        }
    };
    let g = galley(ui, label, font::medium(fs), p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 8.0, h), Sense::click());
    let l = label.to_string();
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, on, &l));
    let c = fg(resp.hovered());
    ui.painter().galley(
        pos2(rect.left() + 4.0, rect.center().y - g.size().y / 2.0),
        g,
        c,
    );
    if on {
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max),
            0.0,
            p.accent,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

// ------------------------------------------------------------------------------------------
// Right rail

/// `.rb` — 40×40 icon button of the right rail; `on` = amber (`.rb-on`).
pub fn rail_button(ui: &mut Ui, icon: Icon, label: &str, on: bool) -> Response {
    rail_button_sized(ui, icon, label, on, theme::size::RAIL_BUTTON, 20.0)
}

pub fn rail_button_sized(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    on: bool,
    size: f32,
    icon_size: f32,
) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let l = label.to_string();
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, on, &l));
    let (fill, stroke, fg) = if on {
        (
            p.accent_bg2,
            Stroke::new(1.0, p.accent_border),
            p.accent_soft,
        )
    } else if resp.hovered() {
        (p.bg_hover, Stroke::NONE, p.text)
    } else {
        (
            Color32::TRANSPARENT,
            Stroke::NONE,
            if p.dark {
                Color32::from_rgb(0xC7, 0xC7, 0xC7)
            } else {
                p.text_dim
            },
        )
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(8),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    icon.paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(icon_size, icon_size)),
        fg,
    );
    resp.on_hover_text(label)
        .on_hover_cursor(CursorIcon::PointingHand)
}

// ------------------------------------------------------------------------------------------
// Progress

/// Thin amber progress bar with an optional label above. `fraction = None` = indeterminate.
pub fn progress(ui: &mut Ui, fraction: Option<f32>, label: &str) -> Response {
    let p = pal(ui);
    let w = ui.available_width();
    if !label.is_empty() {
        ui.label(
            egui::RichText::new(label)
                .font(font::ui(12.0))
                .color(p.text_muted),
        );
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 4.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(2), p.bg_selected);
    match fraction {
        Some(f) => {
            let f = f.clamp(0.0, 1.0);
            ui.painter().rect_filled(
                Rect::from_min_max(
                    rect.min,
                    pos2(rect.left() + rect.width() * f, rect.bottom()),
                ),
                CornerRadius::same(2),
                p.accent,
            );
        }
        None => {
            let t = ui.input(|i| i.time) as f32;
            let seg = rect.width() * 0.3;
            let x = rect.left() + ((t * 0.8).fract() * (rect.width() + seg)) - seg;
            let r = Rect::from_min_max(
                pos2(x.max(rect.left()), rect.top()),
                pos2((x + seg).min(rect.right()), rect.bottom()),
            );
            ui.painter().rect_filled(r, CornerRadius::same(2), p.accent);
            ui.ctx().request_repaint();
        }
    }
    let l = label.to_string();
    let v = fraction.unwrap_or(0.0) as f64;
    resp.widget_info(|| {
        let mut i = WidgetInfo::labeled(Role::ProgressIndicator, true, &l);
        i.value = Some(v);
        i
    });
    resp
}

/// A small spinner (amber arc).
pub fn spinner(ui: &mut Ui, size: f32) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let c = rect.center();
    let r = size / 2.0 - 1.5;
    let start = t * 6.0;
    let pts: Vec<Pos2> = (0..=20)
        .map(|i| {
            let a = start + i as f32 / 20.0 * 4.2;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(pts, Stroke::new(1.6, p.accent)));
    ui.ctx().request_repaint();
    resp
}

// ------------------------------------------------------------------------------------------
// Frames

/// Card frame (`border #242424, radius 10, rgba(12,12,12,.8)`, padding 14/16).
pub fn card_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg_header)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(16, 14))
}

/// Inner card (`.an` / radio card: radius 8, padding 12–14).
pub fn inner_card_frame(p: &Palette, on: bool) -> egui::Frame {
    let (fill, stroke) = if on {
        (p.accent_bg2, p.accent_border)
    } else {
        (p.bg_card, p.border)
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
}

/// Side panel frame (`rgba(17,17,17,.84)` + border).
pub fn panel_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg_panel)
        .inner_margin(egui::Margin::same(16))
}

/// Paints a 1 px separator line.
pub fn hline(ui: &mut Ui, color: Color32) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, color);
}

/// A dashed rounded rectangle (empty-state cards: `1px dashed #2E2E2E`).
pub fn dashed_rect(painter: &egui::Painter, rect: Rect, radius: f32, color: Color32) {
    let r = radius;
    let mut pts = Vec::new();
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    for (c, start) in corners {
        for k in 0..=6 {
            let a = (start + k as f32 * 15.0).to_radians();
            pts.push(c + vec2(a.cos(), a.sin()) * r);
        }
    }
    pts.push(pts[0]);
    let shapes = egui::Shape::dashed_line(&pts, Stroke::new(1.0, color), 4.0, 3.0);
    painter.extend(shapes);
}

/// Text centred in a rect.
pub fn paint_centered(ui: &Ui, rect: Rect, text: &str, font: FontId, color: Color32) {
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, text, font, color);
}

/// A top-down child `Ui` filling `rect` (independent of the parent's layout direction).
pub fn child_ui(ui: &mut Ui, rect: Rect) -> Ui {
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    )
}

/// Wrapped text limited to `max_rows` rows (ellipsis), as a label.
pub fn wrapped_text(
    ui: &mut Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_rows: usize,
) -> Response {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap.max_width = ui.available_width();
    job.wrap.max_rows = max_rows;
    job.wrap.break_anywhere = false;
    let g = ui.painter().layout_job(job);
    let (r, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, color);
    let t = text.to_string();
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, &t));
    resp
}

/// Design `<select>`: 32 px, raised fill, chevron; clicking opens a menu built by `items`.
/// `label` is the accessible name (e.g. `"Target"`).
pub fn select<R>(
    ui: &mut Ui,
    label: &str,
    selected_text: &str,
    width: f32,
    items: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let l = format!("{label}: {selected_text}");
    resp.widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, &l));
    let fill = if resp.hovered() {
        theme::mix(p.bg_raised, p.bg_selected, 0.35)
    } else {
        p.bg_raised
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        fill,
        Stroke::new(1.0, p.border_strong),
        StrokeKind::Inside,
    );
    let g = ui.painter().layout(
        selected_text.to_string(),
        font::ui(13.0),
        p.text,
        rect.width() - 34.0,
    );
    ui.painter().with_clip_rect(rect.shrink(1.0)).galley(
        pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0),
        g,
        p.text,
    );
    Icon::ChevronDown.paint(
        ui.painter(),
        Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(12.0, 12.0)),
        p.text_muted,
    );
    let inner = egui::Popup::menu(&resp)
        .width(width.max(180.0))
        .show(items)
        .map(|r| r.inner);
    (resp.on_hover_cursor(CursorIcon::PointingHand), inner)
}

/// A menu row for [`select`] / popups. Returns true when clicked.
pub fn menu_item(ui: &mut Ui, text: &str, selected: bool, enabled: bool) -> bool {
    let p = pal(ui);
    let w = ui.available_width().max(160.0);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(w, 28.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let t = text.to_string();
    resp.widget_info(|| WidgetInfo::selected(Role::MenuItem, enabled, selected, &t));
    if resp.hovered() && enabled {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), p.bg_hover);
    }
    let color = if !enabled {
        p.text_faint
    } else if selected {
        p.accent_soft
    } else {
        p.text
    };
    if selected {
        Icon::Check.paint(
            ui.painter(),
            Rect::from_center_size(pos2(rect.left() + 12.0, rect.center().y), vec2(12.0, 12.0)),
            p.accent,
        );
    }
    ui.painter().text(
        pos2(rect.left() + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font::ui(13.0),
        color,
    );
    let clicked = resp.clicked() && enabled;
    if clicked {
        ui.close();
    }
    clicked
}

/// Compact dropdown for dense bars (serial monitor): mono label + chevron, 24 px tall, no
/// box until hovered. Returns the item picked by `items`.
pub fn mini_select<R>(
    ui: &mut Ui,
    label: &str,
    selected_text: &str,
    items: impl FnOnce(&mut Ui) -> Option<R>,
) -> Option<R> {
    let p = pal(ui);
    let g = galley(ui, selected_text, font::mono(12.0), p.text_dim);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 24.0), Sense::click());
    let l = format!("{label}: {selected_text}");
    resp.widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, &l));
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), p.bg_hover);
    }
    ui.painter().galley(
        pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0),
        g,
        p.text_dim,
    );
    Icon::ChevronDown.paint(
        ui.painter(),
        Rect::from_center_size(pos2(rect.right() - 11.0, rect.center().y), vec2(10.0, 10.0)),
        p.text_muted,
    );
    egui::Popup::menu(&resp)
        .width(160.0)
        .show(items)
        .and_then(|r| r.inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_slider_snaps_to_named_stops_and_steps() {
        let fmt = |v: f64| format!("{v:.0} % VWC");
        let stops = [
            Stop {
                value: 0.0,
                label: "Bone dry",
            },
            Stop {
                value: 30.0,
                label: "Field capacity",
            },
            Stop {
                value: 60.0,
                label: "Saturated",
            },
        ];
        let s = StopSlider::new("Soil moisture", 0.0..=60.0, &fmt)
            .stops(&stops)
            .step(1.0)
            .snap(0.03);
        assert_eq!(s.quantize(29.0), 30.0); // within 3 % of the range → snaps
        assert_eq!(s.quantize(25.4), 25.0); // otherwise steps
        assert_eq!(s.quantize(-5.0), 0.0);
        assert_eq!(s.quantize(61.0), 60.0);
        assert_eq!(s.nearest_stop(42.0), Some("Field capacity"));
        assert_eq!(s.nearest_stop(55.0), Some("Saturated"));
    }
}
