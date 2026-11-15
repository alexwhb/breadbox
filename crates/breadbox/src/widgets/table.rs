//! Data tables in the design's style: a `#171717` header row, rows separated by `#1C1C1C`
//! lines, mono cells (`.op` table, packet log, probes).

use crate::theme::{font, pal};
use eframe::egui::{self, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// Column definition: title + relative width (fr units) or fixed px.
#[derive(Debug, Clone, Copy)]
pub enum Col<'a> {
    Fr(&'a str, f32),
    Px(&'a str, f32),
}

impl Col<'_> {
    fn title(&self) -> &str {
        match self {
            Col::Fr(t, _) | Col::Px(t, _) => t,
        }
    }
}

/// One cell: text + optional colour override.
#[derive(Debug, Clone)]
pub struct Cell {
    pub text: String,
    pub color: Option<Color32>,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Cell {
            text: text.into(),
            color: None,
        }
    }
    pub fn color(mut self, c: Color32) -> Self {
        self.color = Some(c);
        self
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::new(s)
    }
}
impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::new(s)
    }
}

/// A row: cells + an optional whole-row colour (amber `!` drop rows).
#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub color: Option<Color32>,
}

impl Row {
    pub fn new(cells: Vec<Cell>) -> Self {
        Row { cells, color: None }
    }
    pub fn color(mut self, c: Color32) -> Self {
        self.color = Some(c);
        self
    }
}

/// Table style knobs.
#[derive(Debug, Clone)]
pub struct TableStyle {
    pub font: FontId,
    pub header_font: FontId,
    pub pad_x: f32,
    pub pad_y: f32,
    pub gap: f32,
    pub wrap: bool,
    pub header: bool,
}

impl Default for TableStyle {
    fn default() -> Self {
        TableStyle {
            font: font::mono(12.5),
            header_font: font::mono(12.5),
            pad_x: 12.0,
            pad_y: 6.0,
            gap: 8.0,
            wrap: false,
            header: true,
        }
    }
}

/// Draws a table filling the available width. Returns the clicked row index, if any.
pub fn data_table(
    ui: &mut Ui,
    id: &str,
    cols: &[Col<'_>],
    rows: &[Row],
    style: &TableStyle,
) -> Option<usize> {
    let p = pal(ui);
    let w = ui.available_width();
    let fixed: f32 = cols
        .iter()
        .map(|c| if let Col::Px(_, px) = c { *px } else { 0.0 })
        .sum();
    let fr_total: f32 = cols
        .iter()
        .map(|c| if let Col::Fr(_, f) = c { *f } else { 0.0 })
        .sum();
    let inner = w - style.pad_x * 2.0 - style.gap * (cols.len().saturating_sub(1)) as f32;
    let widths: Vec<f32> = cols
        .iter()
        .map(|c| match c {
            Col::Px(_, px) => *px,
            Col::Fr(_, f) => ((inner - fixed).max(0.0) * f / fr_total.max(1e-6)).max(10.0),
        })
        .collect();
    let mut clicked = None;
    let layout = |ui: &Ui, text: &str, font: FontId, color: Color32, width: f32| {
        if style.wrap {
            ui.painter().layout(text.to_string(), font, color, width)
        } else {
            ui.painter().layout_no_wrap(text.to_string(), font, color)
        }
    };
    if style.header {
        let gs: Vec<_> = cols
            .iter()
            .zip(&widths)
            .map(|(c, w)| layout(ui, c.title(), style.header_font.clone(), p.text_muted, *w))
            .collect();
        let h = gs.iter().map(|g| g.size().y).fold(0.0, f32::max) + style.pad_y * 2.0;
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        ui.painter().rect_filled(r, 0.0, p.bg_raised);
        let mut x = r.left() + style.pad_x;
        for (g, cw) in gs.into_iter().zip(&widths) {
            ui.painter()
                .with_clip_rect(Rect::from_min_size(pos2(x, r.top()), vec2(*cw, h)))
                .galley(pos2(x, r.top() + style.pad_y), g, p.text_muted);
            x += cw + style.gap;
        }
    }
    for (ri, row) in rows.iter().enumerate() {
        let gs: Vec<_> = row
            .cells
            .iter()
            .zip(&widths)
            .map(|(c, cw)| {
                let color = c.color.or(row.color).unwrap_or(p.text);
                (layout(ui, &c.text, style.font.clone(), color, *cw), color)
            })
            .collect();
        let h = gs.iter().map(|g| g.0.size().y).fold(0.0, f32::max) + style.pad_y * 2.0;
        let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let label = row
            .cells
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &label));
        if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, p.bg_hover.gamma_multiply(0.6));
        }
        ui.painter().line_segment(
            [r.left_top(), r.right_top()],
            Stroke::new(1.0, p.border_faint),
        );
        let mut x = r.left() + style.pad_x;
        for ((g, color), cw) in gs.into_iter().zip(&widths) {
            ui.painter()
                .with_clip_rect(Rect::from_min_size(pos2(x, r.top()), vec2(*cw, h)))
                .galley(pos2(x, r.top() + style.pad_y), g, color);
            x += cw + style.gap;
        }
        if resp.clicked() {
            clicked = Some(ri);
        }
    }
    let _ = id;
    clicked
}
