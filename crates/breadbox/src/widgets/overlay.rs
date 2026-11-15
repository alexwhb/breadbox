//! Toasts, modal dialogs and collapsible side panels.

use super::controls::{Btn, galley, section_header};
use super::icons::Icon;
use crate::theme::{font, pal, pal_ctx};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------
// Toasts

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub created: Instant,
    pub ttl: Duration,
}

/// Bottom-right toast stack. Push from anywhere; `show` once per frame.
#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        let ttl = match kind {
            ToastKind::Error => Duration::from_secs(8),
            ToastKind::Warning => Duration::from_secs(6),
            _ => Duration::from_secs(4),
        };
        self.items.push(Toast {
            kind,
            text: text.into(),
            created: Instant::now(),
            ttl,
        });
        if self.items.len() > 5 {
            self.items.remove(0);
        }
    }
    pub fn info(&mut self, t: impl Into<String>) {
        self.push(ToastKind::Info, t)
    }
    pub fn success(&mut self, t: impl Into<String>) {
        self.push(ToastKind::Success, t)
    }
    pub fn warn(&mut self, t: impl Into<String>) {
        self.push(ToastKind::Warning, t)
    }
    pub fn error(&mut self, t: impl Into<String>) {
        self.push(ToastKind::Error, t)
    }
    pub fn items(&self) -> &[Toast] {
        &self.items
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.items.retain(|t| now.duration_since(t.created) < t.ttl);
        if self.items.is_empty() {
            return;
        }
        let p = pal_ctx(ctx);
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 16.0;
        let mut dismiss = None;
        for (i, t) in self.items.iter().enumerate().rev() {
            let (accent, icon) = match t.kind {
                ToastKind::Info => (p.info, Icon::Info),
                ToastKind::Success => (p.ok, Icon::Check),
                ToastKind::Warning => (p.accent, Icon::Warning),
                ToastKind::Error => (p.danger_soft, Icon::Warning),
            };
            let id = Id::unique("toast").with(i).with(t.created);
            let area = egui::Area::new(id)
                .order(egui::Order::Tooltip)
                .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -(screen.bottom() - y)))
                .interactable(true);
            let r = area.show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.bg_panel)
                    .stroke(Stroke::new(1.0, p.border_strong))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal(|ui| {
                            icon.show(ui, 16.0, accent);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&t.text)
                                        .font(font::ui(13.0))
                                        .color(p.text),
                                )
                                .wrap(),
                            );
                            if Btn::plain("Dismiss")
                                .icon_only(Icon::Close)
                                .height(22.0)
                                .show(ui)
                                .clicked()
                            {
                                dismiss = Some(i);
                            }
                        });
                    })
            });
            y -= r.response.rect.height() + 8.0;
        }
        if let Some(i) = dismiss {
            self.items.remove(i);
        }
        let next = self
            .items
            .iter()
            .map(|t| t.ttl.saturating_sub(now.duration_since(t.created)))
            .min()
            .unwrap_or(Duration::from_secs(1));
        ctx.request_repaint_after(next);
    }
}

// ------------------------------------------------------------------------------------------
// Modal dialogs

/// Result of a modal frame.
pub struct ModalResult<R> {
    pub inner: R,
    /// Escape / backdrop click / close button.
    pub close: bool,
}

/// Shows a modal dialog with a title row and a close button.
pub fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    width: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> ModalResult<R> {
    let p = pal_ctx(ctx);
    let mut close_clicked = false;
    let resp = egui::Modal::new(Id::unique(id))
        .backdrop_color(Color32::from_black_alpha(if p.dark { 160 } else { 80 }))
        .frame(
            egui::Frame::new()
                .fill(p.bg_panel)
                .stroke(Stroke::new(1.0, p.border_strong))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::same(18))
                .shadow(ctx.global_style().visuals.window_shadow),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .font(font::heading(17.0))
                        .color(p.text_strong),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Btn::plain("Close")
                        .icon_only(Icon::Close)
                        .height(26.0)
                        .show(ui)
                        .clicked()
                    {
                        close_clicked = true;
                    }
                });
            });
            ui.add_space(10.0);
            content(ui)
        });
    ModalResult {
        close: close_clicked || resp.should_close(),
        inner: resp.inner,
    }
}

// ------------------------------------------------------------------------------------------
// Collapsible side panels

/// Persistent open/closed state of a side panel (`<|` / `|>` handles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PanelMode {
    Open,
    Collapsed,
}

/// A side panel that collapses to a 30 px `.tab-out` handle with a vertical label.
/// Returns `None` when collapsed.
pub fn collapsible_side<R>(
    ui: &mut Ui,
    id: &str,
    side: Side,
    title: &str,
    mode: &mut PanelMode,
    width: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let p = pal(ui);
    match *mode {
        PanelMode::Open => {
            let panel = match side {
                Side::Left => egui::Panel::left(id.to_string()),
                Side::Right => egui::Panel::right(id.to_string()),
            };
            let r = panel
                .exact_size(width)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(p.bg_panel))
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    let edge = match side {
                        Side::Left => [rect.right_top(), rect.right_bottom()],
                        Side::Right => [rect.left_top(), rect.left_bottom()],
                    };
                    ui.painter().line_segment(edge, Stroke::new(1.0, p.border));
                    content(ui)
                });
            Some(r.inner)
        }
        PanelMode::Collapsed => {
            let panel = match side {
                Side::Left => egui::Panel::left(format!("{id}-collapsed")),
                Side::Right => egui::Panel::right(format!("{id}-collapsed")),
            };
            panel
                .exact_size(30.0)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    let h = 26.0 + 8.0 * title.len() as f32;
                    let tab =
                        Rect::from_center_size(rect.center(), vec2(30.0, h.min(rect.height())));
                    let resp = ui.interact(tab, Id::unique(id).with("tab-out"), Sense::click());
                    let label = format!("Show {title}");
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                    });
                    let fill = if resp.hovered() {
                        p.bg_hover
                    } else {
                        p.bg_panel
                    };
                    let cr = match side {
                        Side::Right => CornerRadius {
                            nw: 8,
                            sw: 8,
                            ne: 0,
                            se: 0,
                        },
                        Side::Left => CornerRadius {
                            nw: 0,
                            sw: 0,
                            ne: 8,
                            se: 8,
                        },
                    };
                    ui.painter().rect(
                        tab,
                        cr,
                        fill,
                        Stroke::new(1.0, p.border_strong),
                        StrokeKind::Inside,
                    );
                    let icon = match side {
                        Side::Right => Icon::ChevronLeft,
                        Side::Left => Icon::ChevronRight,
                    };
                    icon.paint(
                        ui.painter(),
                        Rect::from_center_size(
                            pos2(tab.center().x, tab.top() + 18.0),
                            vec2(14.0, 14.0),
                        ),
                        p.text_dim,
                    );
                    // vertical label
                    let mut y = tab.top() + 34.0;
                    for ch in title.chars() {
                        let g = galley(ui, ch.to_string(), font::medium(12.0), p.text_dim);
                        ui.painter().galley(
                            pos2(tab.center().x - g.size().x / 2.0, y),
                            g,
                            p.text_dim,
                        );
                        y += 13.0;
                    }
                    if resp.clicked() {
                        *mode = PanelMode::Open;
                    }
                });
            None
        }
    }
}

/// Which side a collapsible panel is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Panel title row with a collapse handle (`|>` for right panels).
pub fn panel_title_with_collapse(
    ui: &mut Ui,
    title: &str,
    caption: &str,
    side: Side,
    mode: &mut PanelMode,
) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        section_header(ui, title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let icon = match side {
                Side::Right => Icon::CollapseRight,
                Side::Left => Icon::ExpandLeft,
            };
            if Btn::plain(&format!("Collapse {title}"))
                .icon_only(icon)
                .height(24.0)
                .show(ui)
                .clicked()
            {
                *mode = PanelMode::Collapsed;
            }
            if !caption.is_empty() {
                ui.label(
                    egui::RichText::new(caption)
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
            }
        });
    });
}
