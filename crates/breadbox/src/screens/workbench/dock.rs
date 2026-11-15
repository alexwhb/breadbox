//! Bottom dock (`WB-11`): Serial monitor (timestamped, colour-coded lines, per-MCU tabs, port
//! label, baud + line ending, send box, Clear, autoscroll, radio annotations) and Probes &
//! power (live probe cards, supply rails, total power, per-part dissipation, USB budget).

use super::WorkbenchState;
use crate::app::AppCtx;
use crate::bench::{LineKind, SerialLine, clock_label};
use crate::canvas::bench::probe_text;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn};
use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DockTab {
    #[default]
    Serial,
    Probes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    None,
    Nl,
    Cr,
    CrNl,
}

impl Ending {
    fn label(self) -> &'static str {
        match self {
            Ending::None => "No line ending",
            Ending::Nl => "NL",
            Ending::Cr => "CR",
            Ending::CrNl => "CR+NL",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Ending::None => b"",
            Ending::Nl => b"\n",
            Ending::Cr => b"\r",
            Ending::CrNl => b"\r\n",
        }
    }
}

pub struct Dock {
    target: Option<String>,
    send: String,
    ending: Ending,
    baud: u32,
}

impl Default for Dock {
    fn default() -> Self {
        Dock {
            target: None,
            send: String::new(),
            ending: Ending::Nl,
            baud: 115_200,
        }
    }
}

const BAUDS: [u32; 8] = [300, 1200, 2400, 4800, 9600, 19_200, 57_600, 115_200];

pub fn line_color(p: &crate::theme::Palette, k: LineKind) -> Color32 {
    match k {
        LineKind::Normal => p.text_dim,
        LineKind::System => p.text_muted,
        LineKind::Net => p.info,
        LineKind::Warn => p.accent,
        LineKind::Error => p.danger_soft,
    }
}

impl Dock {
    pub fn reset(&mut self) {
        *self = Dock::default();
    }

    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx, state: &mut WorkbenchState) {
        let p = pal(ui);
        let targets: Vec<String> = app
            .project
            .as_ref()
            .map(|pr| pr.bench_targets().into_iter().map(|t| t.id).collect())
            .unwrap_or_default();
        if self.target.as_ref().is_none_or(|t| !targets.contains(t)) {
            self.target = targets.first().cloned();
        }
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        // header row
        let header =
            egui::Rect::from_min_size(ui.max_rect().min, vec2(ui.max_rect().width(), 37.0));
        ui.painter().line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, p.border_faint),
        );
        let mut hui = widgets::child_ui(ui, header.shrink2(vec2(16.0, 0.0)));
        hui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            for (tab, label) in [
                (DockTab::Serial, "Serial monitor"),
                (DockTab::Probes, "Probes & power"),
            ] {
                if widgets::tab(ui, label, state.dock_tab == tab).clicked() {
                    state.dock_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if state.dock_tab == DockTab::Serial {
                    if Btn::ghost("Clear")
                        .height(26.0)
                        .font_size(12.0)
                        .show(ui)
                        .clicked()
                        && let Some(t) = &self.target
                    {
                        app.bench.serial.retain(|l| &l.target != t);
                    }
                    let port = self
                        .target
                        .as_ref()
                        .and_then(|t| {
                            tel.as_ref()
                                .and_then(|tl| tl.mcus.iter().find(|m| &m.reference == t))
                        })
                        .map(|m| port_label(m, app))
                        .unwrap_or_else(|| "no MCU on this bench".into());
                    ui.label(
                        egui::RichText::new(port)
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                    );
                    if targets.len() > 1 {
                        for t in targets.iter().rev() {
                            if widgets::filter_chip(ui, t, self.target.as_ref() == Some(t))
                                .clicked()
                            {
                                self.target = Some(t.clone());
                            }
                        }
                    }
                } else if let Some(t) = &tel {
                    let pw = &t.power;
                    ui.label(
                        egui::RichText::new(format!(
                            "total {} · USB {} of {}",
                            bb_core::si::format(pw.total_power, "W"),
                            bb_core::si::format(pw.usb_current, "A"),
                            bb_core::si::format(pw.usb_budget, "A")
                        ))
                        .font(font::mono(12.0))
                        .color(p.text_muted),
                    );
                }
            });
        });
        let body =
            egui::Rect::from_min_max(header.left_bottom() + vec2(0.0, 1.0), ui.max_rect().max);
        let mut bui = widgets::child_ui(ui, body);
        match state.dock_tab {
            DockTab::Serial => self.serial(&mut bui, app, state),
            DockTab::Probes => probes_power(&mut bui, app),
        }
    }

    fn serial(&mut self, ui: &mut Ui, app: &mut AppCtx, state: &mut WorkbenchState) {
        let p = pal(ui);
        let rect = ui.max_rect();
        let send_h = 36.0;
        let lines_rect =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - send_h));
        let target = self.target.clone().unwrap_or_default();
        let lines: Vec<&SerialLine> = app
            .bench
            .serial
            .iter()
            .filter(|l| l.target == target && (state.annotations || l.kind != LineKind::Net))
            .collect();
        let mut lui = widgets::child_ui(ui, lines_rect.shrink2(vec2(16.0, 8.0)));
        let row_h = 20.5;
        let mono = font::mono(12.5);
        egui::ScrollArea::vertical()
            .id_salt("wb-serial")
            .stick_to_bottom(state.autoscroll)
            .auto_shrink([false, false])
            .show_rows(&mut lui, row_h, lines.len().max(1), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if lines.is_empty() {
                    let msg = if app.bench.running {
                        "waiting for serial output…"
                    } else {
                        "Press Run — the firmware's Serial output appears here."
                    };
                    ui.label(
                        egui::RichText::new(msg)
                            .font(mono.clone())
                            .color(p.text_faint),
                    );
                    return;
                }
                let paused_tail = !app.bench.running && range.end >= lines.len();
                for l in &lines[range] {
                    let (r, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                    let t = clock_label(l.time);
                    let text = l.text.clone();
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
                    ui.painter().text(
                        egui::pos2(r.left(), r.center().y),
                        egui::Align2::LEFT_CENTER,
                        t,
                        mono.clone(),
                        p.text_faint,
                    );
                    ui.painter().with_clip_rect(r).text(
                        egui::pos2(r.left() + 84.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        &l.text,
                        mono.clone(),
                        line_color(p, l.kind),
                    );
                }
                if paused_tail {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("— simulation paused —")
                            .font(mono.clone())
                            .color(p.text_muted),
                    );
                }
            });
        // send row
        let send =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - send_h), rect.max);
        ui.painter().line_segment(
            [send.left_top(), send.right_top()],
            Stroke::new(1.0, p.border_faint),
        );
        let mut sui = widgets::child_ui(ui, send.shrink2(vec2(16.0, 4.0)));
        sui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let te = ui.add(
                egui::TextEdit::singleline(&mut self.send)
                    .hint_text(format!("Send to {target}…"))
                    .desired_width(300.0)
                    .font(font::mono(12.0))
                    .margin(vec2(8.0, 3.0)),
            );
            te.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Serial send")
            });
            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = Btn::ghost("Send")
                .height(24.0)
                .font_size(12.0)
                .show(ui)
                .clicked();
            if (enter || clicked) && !target.is_empty() {
                let mut bytes = self.send.clone().into_bytes();
                bytes.extend_from_slice(self.ending.bytes());
                app.bench_link().send_serial(&target, &bytes);
                self.send.clear();
                if enter {
                    te.request_focus();
                }
            }
            let picked = widgets::mini_select(ui, "Line ending", self.ending.label(), |ui| {
                let mut pick = None;
                for e in [Ending::None, Ending::Nl, Ending::Cr, Ending::CrNl] {
                    if widgets::menu_item(ui, e.label(), e == self.ending, true) {
                        pick = Some(e);
                    }
                }
                pick
            });
            if let Some(e) = picked {
                self.ending = e;
            }
            let picked = widgets::mini_select(ui, "Baud", &format!("{} baud", self.baud), |ui| {
                let mut pick = None;
                for b in BAUDS {
                    if widgets::menu_item(ui, &format!("{b} baud"), b == self.baud, true) {
                        pick = Some(b);
                    }
                }
                pick
            });
            if let Some(b) = picked {
                self.baud = b;
            }
            ui.add_space(6.0);
            widgets::checkbox(ui, &mut state.autoscroll, "Autoscroll");
            widgets::checkbox(ui, &mut state.annotations, "Radio annotations");
        });
    }
}

/// Rounds a measured UART rate to the standard one it approximates (115200 for the AVR's
/// 117647 at 16 MHz); `None` before the firmware configured the port.
pub fn standard_baud(b: f64) -> Option<u32> {
    const STD: [u32; 12] = [
        300, 600, 1200, 2400, 4800, 9600, 14_400, 19_200, 38_400, 57_600, 115_200, 230_400,
    ];
    if b.is_nan() || b <= 0.0 || b > 600_000.0 {
        return None;
    }
    STD.iter()
        .copied()
        .min_by(|a, c| ((*a as f64 - b).abs()).total_cmp(&((*c as f64 - b).abs())))
}

/// `U1 · SoftwareSerial + USB · 115200 baud · NL`.
fn port_label(m: &bb_sim::McuTelemetry, app: &AppCtx) -> String {
    let soft = app.project.as_ref().is_some_and(|p| !p.radios.is_empty());
    let label = if soft && m.serial_label.contains(" · USB · ") {
        m.serial_label
            .replacen(" · USB · ", " · SoftwareSerial + USB · ", 1)
    } else {
        m.serial_label.clone()
    };
    // `… · 117647 baud · NL` → the standard rate
    let parts: Vec<&str> = label.split(" · ").collect();
    parts
        .iter()
        .map(
            |p| match p.strip_suffix(" baud").and_then(|n| n.parse::<f64>().ok()) {
                Some(b) => match standard_baud(b) {
                    Some(s) => format!("{s} baud"),
                    None => "115200 baud".to_string(),
                },
                None => p.to_string(),
            },
        )
        .collect::<Vec<_>>()
        .join(" · ")
}

fn probes_power(ui: &mut Ui, app: &mut AppCtx) {
    let p = pal(ui);
    let tel = app.bench.sim().and_then(|s| s.telemetry());
    let rect = ui.max_rect().shrink2(vec2(16.0, 12.0));
    let mut cui = widgets::child_ui(ui, rect);
    egui::ScrollArea::vertical()
        .id_salt("wb-probes")
        .auto_shrink([false, false])
        .show(&mut cui, |ui| {
            let Some(t) = tel else {
                ui.label(
                    egui::RichText::new("Run the bench to see live probes and power.")
                        .font(font::ui(12.0))
                        .color(p.text_faint),
                );
                return;
            };
            let mut cards: Vec<(String, String, String)> = Vec::new();
            for r in &t.probes {
                let net = r.net.clone().unwrap_or_default();
                let sub = match r.quantity {
                    bb_circuit::project::Quantity::Voltage => {
                        if net.starts_with('A') && net.len() <= 3 {
                            format!(
                                "ADC {:.0} / 1023 · {:.2}–{:.2} V",
                                (r.value / 5.0 * 1023.0).clamp(0.0, 1023.0),
                                r.min,
                                r.max
                            )
                        } else {
                            format!("net {net} · {:.2}–{:.2} V", r.min, r.max)
                        }
                    }
                    bb_circuit::project::Quantity::Duty => {
                        format!("PWM on {net} · avg {:.2} V", r.avg)
                    }
                    bb_circuit::project::Quantity::Logic => format!("digital {net}"),
                    _ => format!("net {net}"),
                };
                cards.push((r.label.clone(), probe_text(r), sub));
            }
            for rail in &t.power.rails {
                cards.push((
                    format!("{} rail", rail.name),
                    bb_core::si::format(rail.voltage, "V"),
                    format!(
                        "{} source · {}",
                        rail.source,
                        bb_core::si::format(rail.current, "A")
                    ),
                ));
            }
            let total_i: f64 = t.power.rails.iter().map(|r| r.current).sum();
            cards.push((
                "Total board draw".into(),
                bb_core::si::format(total_i, "A"),
                format!(
                    "{} delivered",
                    bb_core::si::format(t.power.total_power, "W")
                ),
            ));
            let w = ui.available_width();
            let cols = ((w + 12.0) / 192.0).floor().max(1.0) as usize;
            let cw = (w - 12.0 * (cols as f32 - 1.0)) / cols as f32;
            for chunk in cards.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for (label, value, sub) in chunk {
                        let (r, resp) = ui.allocate_exact_size(vec2(cw, 76.0), Sense::hover());
                        let a11y = format!("{label} {value}");
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Label, true, &a11y)
                        });
                        ui.painter().rect(
                            r,
                            CornerRadius::same(8),
                            Color32::from_rgba_unmultiplied(12, 12, 12, 210),
                            Stroke::new(1.0, p.border),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            r.min + vec2(12.0, 14.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            font::ui(12.0),
                            p.text_muted,
                        );
                        ui.painter().text(
                            r.min + vec2(12.0, 38.0),
                            egui::Align2::LEFT_CENTER,
                            value,
                            font::mono_medium(20.0),
                            p.text,
                        );
                        let g =
                            ui.painter()
                                .layout_no_wrap(sub.clone(), font::ui(11.0), p.text_muted);
                        ui.painter().with_clip_rect(r.shrink(2.0)).galley(
                            r.min + vec2(12.0, 55.0),
                            g,
                            p.text_muted,
                        );
                    }
                });
                ui.add_space(12.0);
            }
            // USB budget + dissipation
            ui.horizontal(|ui| {
                let frac =
                    (t.power.usb_current / t.power.usb_budget.max(1e-9)).clamp(0.0, 1.5) as f32;
                ui.label(
                    egui::RichText::new("USB budget")
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
                let (r, resp) = ui.allocate_exact_size(vec2(220.0, 8.0), Sense::hover());
                let a11y = format!("USB budget {:.0} %", frac * 100.0);
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, &a11y)
                });
                ui.painter().rect_filled(r, 4.0, p.bg_raised);
                let col = if frac > 1.0 {
                    p.danger
                } else if frac > 0.8 {
                    p.accent
                } else {
                    p.ok
                };
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(r.min, vec2(r.width() * frac.min(1.0), r.height())),
                    4.0,
                    col,
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} of 500 mA",
                        bb_core::si::format(t.power.usb_current, "A")
                    ))
                    .font(font::mono(12.0))
                    .color(p.text_dim),
                );
            });
            if !t.power.dissipation.is_empty() {
                ui.add_space(6.0);
                let rows: Vec<widgets::Row> = t
                    .power
                    .dissipation
                    .iter()
                    .map(|(r, w)| {
                        widgets::Row::new(vec![
                            widgets::Cell::new(r),
                            widgets::Cell::new(bb_core::si::format(*w, "W")),
                        ])
                    })
                    .collect();
                widgets::data_table(
                    ui,
                    "dissipation",
                    &[
                        widgets::Col::Fr("part", 1.0),
                        widgets::Col::Fr("dissipation", 1.0),
                    ],
                    &rows,
                    &widgets::TableStyle::default(),
                );
            }
        });
}
