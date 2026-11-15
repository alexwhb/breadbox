//! Pro → Schematic (`PRO-1`, `WB-7`): the full schematic editor over the shared circuit —
//! toolbar (ANSI / IEC symbols, fit, SVG / PNG export), the sheet, and a net list with live
//! values while the bench runs.

use crate::app::AppCtx;
use crate::canvas::bench::net_display;
use crate::canvas::schematic::SchematicCanvas;
use crate::router::{Command, Screen};
use crate::screens::workbench::share::{Export, ShareState};
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon, Seg};
use eframe::egui::{self, Rect, Stroke, Ui, vec2};

#[derive(Default)]
pub struct ProSchematicScreen {
    pub canvas: SchematicCanvas,
    share: ShareState,
    seen_project: Option<u64>,
}

impl Screen for ProSchematicScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some("Schematic".into())
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let r = Btn::ghost("Export").icon(Icon::Export).show(ui);
            let mut pick = None;
            egui::Popup::menu(&r).show(|ui| {
                ui.set_min_width(200.0);
                for e in [
                    Export::SchematicSvg,
                    Export::SchematicPng,
                    Export::Cir,
                    Export::Bom,
                ] {
                    if widgets::Btn::plain(e.label())
                        .full_width()
                        .show(ui)
                        .clicked()
                    {
                        pick = Some(e);
                    }
                }
            });
            if let Some(e) = pick {
                self.share.export(app, e);
            }
        });
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        if self.seen_project != Some(app.project_generation) {
            self.seen_project = Some(app.project_generation);
            self.canvas = SchematicCanvas::default();
        }
        let _ = app.sim();
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        // net list
        egui::Panel::right("pro-sch-nets")
            .exact_size(260.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg_panel).inner_margin(16))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        r.left_top() - vec2(16.0, 16.0),
                        r.left_bottom() + vec2(-16.0, 16.0),
                    ],
                    Stroke::new(1.0, p.border),
                );
                widgets::section_header_row(
                    ui,
                    "Nets",
                    if app.bench.running { "live" } else { "" },
                );
                let Some(doc) = app.circuit.as_mut() else {
                    return;
                };
                let conn = doc.connectivity();
                egui::ScrollArea::vertical()
                    .id_salt("sch-nets")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for n in &conn.nets {
                            // nets that connect something (lone unconnected pins are noise)
                            if n.pins.len() + n.holes.len().min(1) < 2 && n.labels.is_empty() {
                                continue;
                            }
                            let name = net_display(&n.name);
                            let v = tel
                                .as_ref()
                                .and_then(|t| t.nets.iter().find(|(x, _)| x == &name))
                                .map(|(_, v)| bb_core::si::format(*v, "V"))
                                .unwrap_or_default();
                            let hot = self.canvas.hover_net.as_deref() == Some(n.name.as_str());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&name)
                                        .font(font::mono_medium(12.0))
                                        .color(if hot { p.info } else { p.text }),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(v)
                                                .font(font::mono(12.0))
                                                .color(p.accent_soft),
                                        );
                                    },
                                );
                            });
                            let pins: Vec<String> = n
                                .pins
                                .iter()
                                .take(6)
                                .map(|t| {
                                    let r = doc
                                        .project()
                                        .part(&t.part)
                                        .map(|x| x.reference.clone())
                                        .unwrap_or_default();
                                    format!("{r}.{}", t.pin)
                                })
                                .collect();
                            ui.label(
                                egui::RichText::new(pins.join("  "))
                                    .font(font::mono(10.5))
                                    .color(p.text_faint),
                            );
                            ui.add_space(4.0);
                        }
                    });
            });
        // toolbar + sheet
        let full = ui.available_rect_before_wrap();
        let bar = Rect::from_min_size(full.min, vec2(full.width(), 40.0));
        let sheet = Rect::from_min_max(bar.left_bottom(), full.max);
        ui.painter().rect_filled(bar, 0.0, p.bg_header);
        ui.painter().line_segment(
            [bar.left_bottom(), bar.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        let mut bui = widgets::child_ui(ui, bar.shrink2(vec2(14.0, 0.0)));
        bui.horizontal_centered(|ui| {
            let name = app
                .project
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!("Sheet 1 · {name}.sch — same netlist as the bench"))
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(
                    egui::RichText::new(format!("{:.0}%", self.canvas.zoom_pct()))
                        .font(font::mono(12.0))
                        .color(p.text_muted),
                );
                if Btn::mini("Fit").show(ui).clicked() {
                    self.canvas.fit();
                }
                if let Some(doc) = app.circuit.as_mut() {
                    let mut iec = doc.project().schematic.iec;
                    if widgets::mini_segmented(
                        ui,
                        "Symbols",
                        &mut iec,
                        &[Seg::new(false, "ANSI"), Seg::new(true, "IEC")],
                    ) {
                        doc.editor.project.schematic.iec = iec;
                        doc.editor.generation += 1;
                        doc.touched();
                    }
                }
                ui.label(
                    egui::RichText::new("drag from a pin to draw a wire · R rotates")
                        .font(font::ui(11.0))
                        .color(p.text_faint),
                );
            });
        });
        if let Some(doc) = app.circuit.as_mut() {
            let gen_ = app.project_generation;
            self.canvas
                .show(ui, sheet, doc, tel.as_deref(), gen_, app.theme);
        }
    }

    fn command(&mut self, cmd: &Command, app: &mut AppCtx) -> bool {
        match cmd {
            Command::Undo | Command::Redo if !app.text_has_focus() => {
                if let Some(doc) = app.circuit.as_mut() {
                    if *cmd == Command::Undo {
                        doc.undo();
                    } else {
                        doc.redo();
                    }
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}
