//! The parts palette (`WB-2`): Simple (the 25 starter parts as tiles) / Advanced (the whole
//! library by category with part numbers), search, filter chips. Tiles and rows are drag
//! sources for the bench; a click arms click-to-place.

use crate::app::AppCtx;
use crate::canvas::bench::{BenchCanvas, PartDrop};
use crate::theme::{font, pal};
use crate::widgets::{self, Icon, PanelMode, Seg};
use bb_circuit::{Category, PartDef};
use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, Ui, pos2, vec2};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Filter {
    #[default]
    All,
    Spice,
    Firmware,
    Interactive,
}

pub struct Palette {
    search: String,
    filter: Filter,
    open: BTreeSet<Category>,
    pub focus_search: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            search: String::new(),
            filter: Filter::All,
            open: [Category::Semiconductors].into_iter().collect(),
            focus_search: false,
        }
    }
}

/// Palette icon for a part definition's `icon` id.
pub fn icon_for(def: &PartDef) -> Icon {
    match def.icon.as_str() {
        "uno" => Icon::Uno,
        "esp32" => Icon::Esp32,
        "pico" => Icon::Pico,
        "bb" => Icon::Breadboard,
        "wire" => Icon::Wire,
        "res" => Icon::Resistor,
        "cap" => Icon::Capacitor,
        "ind" => Icon::Inductor,
        "btn" => Icon::Button,
        "pot" => Icon::Pot,
        "batt" => Icon::Battery,
        "usb" => Icon::Usb,
        "led" => Icon::Led,
        "rgb" => Icon::RgbLed,
        "buzz" => Icon::Buzzer,
        "ldr" => Icon::Ldr,
        "soil" => Icon::Soil,
        "dht" => Icon::Dht,
        "sonar" => Icon::Sonar,
        "pir" => Icon::Pir,
        "servo" => Icon::Servo,
        "motor" => Icon::Motor,
        "relay" => Icon::Relay,
        "pump" => Icon::Pump,
        "oled" => Icon::Oled,
        "lcd" => Icon::Lcd,
        "dip" => Icon::Dip,
        "diode" => Icon::Diode,
        "bjt" => Icon::Bjt,
        "mos" => Icon::Mosfet,
        "opamp" => Icon::OpAmp,
        "reg" => Icon::Regulator,
        "src" => Icon::Source,
        "radio" => Icon::Radio,
        "strip" => Icon::Strip,
        "step" => Icon::Stepper,
        "imu" => Icon::Imu,
        "scope" => Icon::Oscilloscope,
        "dmm" => Icon::Multimeter,
        _ => Icon::Dip,
    }
}

fn category_icon(c: Category) -> Icon {
    match c {
        Category::Boards => Icon::Uno,
        Category::BuildingBlocks => Icon::Resistor,
        Category::LightSound => Icon::Led,
        Category::Sensors => Icon::Ldr,
        Category::Power => Icon::Battery,
        Category::Radios => Icon::Radio,
        Category::Displays => Icon::Lcd,
        Category::MotorsActuators => Icon::Servo,
        Category::Semiconductors => Icon::Bjt,
        Category::Ics => Icon::Dip,
        Category::Instruments => Icon::Oscilloscope,
        Category::Spice => Icon::Source,
        Category::Plugins => Icon::Imu,
    }
}

impl Palette {
    fn keep(&self, d: &PartDef) -> bool {
        if d.bench.hidden && d.id != "jumper_wire" {
            return false;
        }
        match self.filter {
            Filter::All => true,
            Filter::Spice => !d.model.spice.is_empty() || d.has_tag("spice"),
            Filter::Firmware => d.is_mcu(),
            Filter::Interactive => d.model.behaviour.is_some(),
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        app: &mut AppCtx,
        simple: &mut bool,
        canvas: &mut BenchCanvas,
        mode: &mut PanelMode,
    ) {
        let p = pal(ui);
        let Some(lib) = app.circuit.as_ref().map(|d| d.lib.clone()) else {
            return;
        };
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 14,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                // title row: PARTS · Simple / Advanced · collapse
                ui.horizontal(|ui| {
                    widgets::section_header(ui, "Parts");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::Btn::plain("Collapse Parts")
                            .icon_only(Icon::ExpandLeft)
                            .height(24.0)
                            .show(ui)
                            .clicked()
                        {
                            *mode = PanelMode::Collapsed;
                        }
                        let mut s = *simple;
                        if widgets::mini_segmented(
                            ui,
                            "Library mode",
                            &mut s,
                            &[Seg::new(true, "Simple"), Seg::new(false, "Advanced")],
                        ) {
                            *simple = s;
                        }
                    });
                });
                // search
                let hint = if *simple {
                    "Search starter parts"
                } else {
                    "Name, part number or .model"
                };
                let te = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY)
                        .font(font::ui(13.0))
                        .margin(vec2(10.0, 7.0)),
                );
                te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Search parts"));
                if std::mem::take(&mut self.focus_search) {
                    te.request_focus();
                }
                egui::ScrollArea::vertical()
                    .id_salt("palette-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;
                        if !self.search.trim().is_empty() {
                            let hits: Vec<Arc<PartDef>> = lib
                                .search(&self.search, *simple)
                                .into_iter()
                                .filter(|d| self.keep(d))
                                .collect();
                            if hits.is_empty() {
                                ui.label(egui::RichText::new("No matching parts").font(font::ui(12.0)).color(p.text_muted));
                            }
                            if *simple {
                                self.tiles(ui, &hits, canvas, true);
                            } else {
                                for d in &hits {
                                    self.row(ui, d, canvas);
                                }
                            }
                            return;
                        }
                        if *simple {
                            ui.label(
                                egui::RichText::new(
                                    "Starter kit: the 25 parts most first projects use. Drag one onto the bench.",
                                )
                                .font(font::ui(12.0))
                                .color(p.text_muted),
                            );
                            for (cat, parts) in lib.categories(true) {
                                ui.label(egui::RichText::new(cat.label()).font(font::semibold(11.0)).color(p.text_muted));
                                self.tiles(ui, &parts, canvas, true);
                            }
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                for (f, l) in [
                                    (Filter::All, "All"),
                                    (Filter::Spice, "Has SPICE model"),
                                    (Filter::Firmware, "Runs firmware"),
                                    (Filter::Interactive, "Interactive"),
                                ] {
                                    if widgets::filter_chip(ui, l, self.filter == f).clicked() {
                                        self.filter = f;
                                    }
                                }
                            });
                            ui.spacing_mut().item_spacing.y = 1.0;
                            for (cat, parts) in lib.categories(false) {
                                let parts: Vec<_> = parts.into_iter().filter(|d| self.keep(d)).collect();
                                if parts.is_empty() {
                                    continue;
                                }
                                self.category(ui, cat, &parts, canvas);
                            }
                            ui.add_space(10.0);
                            code_card(ui, app);
                        }
                    });
            });
    }

    fn tiles(&self, ui: &mut Ui, parts: &[Arc<PartDef>], canvas: &mut BenchCanvas, simple: bool) {
        let w = ui.available_width();
        let gap = 6.0;
        let tw = ((w - 2.0 * gap) / 3.0).floor();
        for chunk in parts.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for d in chunk {
                    let label = d.display_name(simple);
                    let resp = widgets::part_tile(
                        ui,
                        icon_for(d),
                        label,
                        vec2(tw, 76.0),
                        canvas.placing.as_deref() == Some(&d.id),
                    );
                    self.source(&resp, d, canvas);
                }
            });
        }
    }

    fn source(&self, resp: &egui::Response, d: &PartDef, canvas: &mut BenchCanvas) {
        resp.dnd_set_drag_payload(PartDrop(d.id.clone()));
        if resp.clicked() {
            if d.id == "jumper_wire" || canvas.placing.as_deref() == Some(&d.id) {
                canvas.placing = None;
            } else {
                canvas.placing = Some(d.id.clone());
            }
        }
        let desc = d.description.clone();
        let _ = resp.clone().on_hover_text(desc);
        if resp.dragged() {
            // a floating chip follows the pointer until it reaches the bench
            if let Some(pos) = resp.ctx.pointer_hover_pos() {
                let painter = resp.ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::unique("palette-drag"),
                ));
                let p = &crate::theme::DARK;
                let g = painter.layout_no_wrap(d.name.clone(), font::medium(12.0), p.text);
                let r =
                    egui::Rect::from_min_size(pos + vec2(14.0, 10.0), g.size() + vec2(20.0, 12.0));
                painter.rect(
                    r,
                    CornerRadius::same(6),
                    p.accent_bg,
                    Stroke::new(1.0, p.accent_border),
                    egui::StrokeKind::Inside,
                );
                painter.galley(r.min + vec2(10.0, 6.0), g, p.text);
            }
        }
    }

    fn category(
        &mut self,
        ui: &mut Ui,
        cat: Category,
        parts: &[Arc<PartDef>],
        canvas: &mut BenchCanvas,
    ) {
        let p = pal(ui);
        let open = self.open.contains(&cat);
        let w = ui.available_width();
        let (r, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
        let label = format!("{} ({})", cat.label(), parts.len());
        resp.widget_info(|| {
            let mut i = egui::WidgetInfo::labeled(egui::Role::DisclosureTriangle, true, &label);
            i.selected = Some(open);
            i
        });
        if resp.hovered() {
            ui.painter()
                .rect_filled(r, CornerRadius::same(6), p.bg_raised);
        }
        let chev = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        chev.paint(
            ui.painter(),
            egui::Rect::from_center_size(pos2(r.left() + 12.0, r.center().y), vec2(10.0, 10.0)),
            p.text_faint,
        );
        category_icon(cat).paint_with(
            ui.painter(),
            egui::Rect::from_center_size(pos2(r.left() + 32.0, r.center().y), vec2(18.0, 18.0)),
            p.text_dim,
            p.accent,
        );
        ui.painter().text(
            pos2(r.left() + 48.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            cat.label(),
            font::medium(13.0),
            p.text_dim,
        );
        ui.painter().text(
            pos2(r.right() - 6.0, r.center().y),
            egui::Align2::RIGHT_CENTER,
            parts.len().to_string(),
            font::mono(11.0),
            p.text_faint,
        );
        if resp.clicked() {
            if open {
                self.open.remove(&cat);
            } else {
                self.open.insert(cat);
            }
        }
        if open {
            ui.indent(("cat", cat as u8), |ui| {
                for d in parts {
                    self.row(ui, d, canvas);
                }
            });
            ui.add_space(4.0);
        }
    }

    fn row(&self, ui: &mut Ui, d: &Arc<PartDef>, canvas: &mut BenchCanvas) {
        let p = pal(ui);
        let w = ui.available_width();
        let (r, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click_and_drag());
        let name = if d.part_number.is_empty() {
            d.name.clone()
        } else {
            d.part_number.clone()
        };
        let a11y = d.name.clone();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &a11y));
        let on = canvas.placing.as_deref() == Some(&d.id);
        if resp.hovered() || on {
            ui.painter().rect_filled(
                r,
                CornerRadius::same(6),
                if on { p.accent_hover } else { p.bg_hover },
            );
        }
        icon_for(d).paint_with(
            ui.painter(),
            egui::Rect::from_center_size(pos2(r.left() + 17.0, r.center().y), vec2(18.0, 18.0)),
            p.text_dim,
            p.accent,
        );
        ui.painter().text(
            pos2(r.left() + 36.0, r.top() + 13.0),
            egui::Align2::LEFT_CENTER,
            &name,
            font::mono_medium(12.0),
            p.text,
        );
        let desc = if d.part_number.is_empty() {
            d.description.clone()
        } else {
            d.name.clone()
        };
        let mut job = egui::text::LayoutJob::simple_singleline(desc, font::ui(11.0), p.text_muted);
        job.wrap = egui::text::TextWrapping {
            max_width: w - 44.0,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let g = ui.painter().layout_job(job);
        ui.painter().with_clip_rect(r).galley(
            pos2(r.left() + 36.0, r.top() + 21.0),
            g,
            Color32::WHITE,
        );
        self.source(&resp, d, canvas);
    }
}

fn code_card(ui: &mut Ui, app: &mut AppCtx) {
    let p = pal(ui);
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(10, 10, 10, 230))
        .corner_radius(8)
        .stroke(Stroke::new(1.0, p.border_strong))
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(egui::RichText::new("Not here? Define it in a part file").font(font::heading(13.0)).color(p.text));
            ui.label(
                egui::RichText::new("{ \"id\": \"hx711\", \"pins\": [\"E+\", \"E-\", \"A+\", \"A-\",\n  \"DT:output\", \"SCK:input\"],\n  \"model\": { \"behaviour\": \"…\" } }")
                    .font(font::mono(11.0))
                    .color(p.text_dim),
            );
            ui.horizontal_wrapped(|ui| {
                if widgets::Btn::ghost("Plugin SDK docs").small().show(ui).clicked() {
                    app.help = Some(crate::widgets::help::HelpTopic::PluginSdk);
                }
            });
        });
}
