//! Placeholder screens for slots later agents fill (Workbench, Pro → Schematic / PCB,
//! Plugins, Lessons). Each explains what plugs in and offers working routes meanwhile.

use crate::app::AppCtx;
use crate::router::{ProTab, Route, Screen, ScreenSlot};
use crate::theme::{font, pal};
use crate::widgets::{Btn, Icon, kind_tag};
use eframe::egui::{self, CornerRadius, Rect, Stroke, Ui, pos2, vec2};

pub struct Placeholder {
    pub slot: ScreenSlot,
    pub title: &'static str,
    pub icon: Icon,
    pub blurb: &'static str,
    /// Agent / phase that replaces it.
    pub owner: &'static str,
}

impl Placeholder {
    pub fn new(slot: ScreenSlot, title: &'static str, icon: Icon, blurb: &'static str, owner: &'static str) -> Self {
        Placeholder { slot, title, icon, blurb, owner }
    }
}

impl Screen for Placeholder {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some(self.title.to_string())
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let area = ui.max_rect();
        crate::theme::paint_dots(ui.painter(), area, p);
        let card = Rect::from_center_size(area.center(), vec2(520.0, 300.0));
        ui.painter().rect(card, CornerRadius::same(12), p.bg_panel, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        let mut child = crate::widgets::child_ui(ui, card.shrink(28.0));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
            ui.painter().rect(r, CornerRadius::same(8), p.accent_bg2, Stroke::new(1.0, p.accent_border), egui::StrokeKind::Inside);
            self.icon.paint_with(ui.painter(), r.shrink(9.0), p.accent_soft, p.accent);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(self.title).font(font::heading(22.0)).color(p.text_strong));
                kind_tag(ui, &format!("SCREEN SLOT · {:?}", self.slot));
            });
        });
        ui.add_space(4.0);
        ui.label(egui::RichText::new(self.blurb).font(font::ui(13.0)).color(p.text_dim));
        ui.label(egui::RichText::new(format!("Built by the {} agent — registered in screens::default_registry().", self.owner)).font(font::ui(12.0)).color(p.text_muted));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if app.project.is_some() {
                if self.slot != ScreenSlot::Firmware && Btn::amber("Open Firmware").icon(Icon::Chip).show(ui).clicked() {
                    app.navigate(Route::Firmware);
                }
                if Btn::ghost("Open Pro analysis").icon(Icon::Analysis).show(ui).clicked() {
                    app.navigate(Route::Pro(ProTab::Analysis));
                }
            }
            if Btn::ghost("Home").icon(Icon::Home).show(ui).clicked() {
                app.navigate(Route::Home);
            }
        });
        let _ = pos2;
    }
}
