//! The Inspector (`WB-10`): the selected part's datasheet fields, live readings, editable
//! properties (resistor value with E-series stepping and colour bands, LED colour, …) and
//! health; the selected wire's colour and ends.

use crate::app::AppCtx;
use crate::canvas::art::band_color;
use crate::canvas::bench::{BenchCanvas, wire_color};
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon};
use bb_circuit::part::{PropDef, PropKind};
use bb_circuit::project::Project;
use bb_circuit::{PartDef, PartInstance, PropValue};
use bb_sim::Command;
use eframe::egui::{self, Sense, Ui, vec2};
use std::collections::HashMap;

#[derive(Default)]
pub struct Inspector {
    /// Text being typed into numeric fields (part id + key → text).
    edits: HashMap<(String, String), String>,
}

/// Expands an inspector template: `$(resistance|Ω)` → `10 kΩ`, `$(color)` → `amber`.
pub fn expand(def: &PartDef, part: &PartInstance, tpl: &str) -> String {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(i) = rest.find("$(") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find(')') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..j];
        let (key, unit) = match inner.split_once('|') {
            Some((k, u)) => (k, Some(u)),
            None => (inner, None),
        };
        let v = Project::prop(def, part, key);
        let s = match (v, unit) {
            (Some(v), Some(u)) => match v.as_f64() {
                Some(x) => bb_core::si::format(x, u),
                None => v.as_text(),
            },
            (Some(PropValue::Num(x)), None) => compact(x),
            (Some(v), None) => v.as_text(),
            (None, _) => "—".into(),
        };
        out.push_str(&s);
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// `WORLD-6` on the Inspector: an AVR board's ADC and a sensor's noise follow the World
/// panel's "Sensor noise & ADC quantization" switch (realistic / ideal).
pub fn measurement_row(def: &PartDef, realistic: bool) -> Option<(String, String)> {
    if def.model.mcu.as_ref().is_some_and(|m| m.core == "avr") {
        let v = if realistic {
            "realistic · offset / gain error, INL, ±1 LSB noise"
        } else {
            "ideal · exact codes"
        };
        Some(("ADC".into(), v.into()))
    } else if def.category == bb_circuit::Category::Sensors {
        let v = if realistic {
            "realistic (World)"
        } else {
            "ideal, none (World)"
        };
        Some(("Noise".into(), v.into()))
    } else {
        None
    }
}

fn compact(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 && x.abs() < 1e9 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x}")
    }
}

/// Readable reading labels (`vwc` → `Moisture`).
fn reading_label(k: &str) -> String {
    match k {
        "vwc" => "Moisture".into(),
        "v" | "voltage" => "Voltage".into(),
        "i" | "current" => "Current".into(),
        "lux" => "Light".into(),
        "temp" | "temp_c" => "Temperature".into(),
        "r" => "Resistance".into(),
        "vcc" => "Supply".into(),
        other => {
            let mut c = other.replace('_', " ");
            if let Some(f) = c.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            c
        }
    }
}

fn reading_text(k: &str, v: f64) -> String {
    match k {
        "vwc" => format!("{v:.1} % VWC"),
        "lux" => format!("{v:.0} lx"),
        "temp" | "temp_c" => format!("{v:.1} °C"),
        "v" | "voltage" | "vcc" | "vout" => bb_core::si::format(v, "V"),
        "i" | "current" => bb_core::si::format(v, "A"),
        "r" => bb_core::si::format(v, "Ω"),
        "p" | "power" => bb_core::si::format(v, "W"),
        _ => {
            if v.abs() >= 100.0 || v == v.round() {
                format!("{v:.0}")
            } else {
                format!("{v:.3}")
            }
        }
    }
}

impl Inspector {
    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx, canvas: &mut BenchCanvas) {
        let p = pal(ui);
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        let Some(doc) = app.circuit.as_mut() else {
            return;
        };
        // wire
        if let Some(w) = canvas.selection.single_wire().map(str::to_string) {
            let Some(wire) = doc.project().wire(&w).cloned() else {
                return;
            };
            widgets::section_header(ui, &format!("Inspector · wire {}", wire.id));
            ui.label(
                egui::RichText::new("Jumper wire")
                    .font(font::heading(14.0))
                    .color(p.text),
            );
            let a = super::panels::endpoint_label(doc, &wire.a);
            let b = super::panels::endpoint_label(doc, &wire.b);
            widgets::kv_grid(
                ui,
                "wire-kv",
                &[
                    ("From".into(), a),
                    ("To".into(), b),
                    ("Bends".into(), wire.bends.len().to_string()),
                ],
                110.0,
            );
            ui.label(
                egui::RichText::new("Colour")
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for c in bb_circuit::project::WIRE_COLORS {
                    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            true,
                            wire.color == c,
                            format!("Wire colour {c}"),
                        )
                    });
                    ui.painter().circle_filled(r.center(), 9.0, wire_color(c));
                    if wire.color == c {
                        ui.painter().circle_stroke(
                            r.center(),
                            11.0,
                            egui::Stroke::new(1.5, p.accent),
                        );
                    }
                    if resp.clicked() {
                        let mut nw = wire.clone();
                        nw.color = c.to_string();
                        let _ = doc.editor.update_wire(nw);
                        doc.touched();
                    }
                }
            });
            ui.horizontal(|ui| {
                if Btn::ghost("Delete wire")
                    .icon(Icon::Trash)
                    .small()
                    .show(ui)
                    .clicked()
                {
                    doc.editor.delete(&[], std::slice::from_ref(&w));
                    doc.touched();
                    canvas.selection.clear();
                }
                if !wire.bends.is_empty() && Btn::ghost("Straighten").small().show(ui).clicked() {
                    let mut nw = wire.clone();
                    nw.bends.clear();
                    let _ = doc.editor.update_wire(nw);
                    doc.touched();
                }
            });
            ui.label(
                egui::RichText::new("Double-click the wire to add a bend; drag bends to route it.")
                    .font(font::ui(11.0))
                    .color(p.text_faint),
            );
            return;
        }
        let Some(id) = canvas.selection.single_part().map(str::to_string) else {
            widgets::section_header(ui, "Inspector");
            let n = canvas.selection.parts.len();
            let msg = if n > 1 {
                format!("{n} parts selected · R rotates, ⌫ deletes, ⌘D duplicates")
            } else {
                "Select a part on the bench to see its datasheet values, live readings and settings.".into()
            };
            ui.label(
                egui::RichText::new(msg)
                    .font(font::ui(12.0))
                    .color(p.text_faint),
            );
            self.repair(ui, app, &tel);
            return;
        };
        let Some(part) = doc.project().part(&id).cloned() else {
            return;
        };
        let Some(def) = doc.lib.get(&part.part).cloned() else {
            return;
        };
        let pt = tel
            .as_ref()
            .and_then(|t| t.parts.iter().find(|x| x.part == id).cloned());
        widgets::section_header(ui, &format!("Inspector · {}", part.reference));
        ui.label(
            egui::RichText::new(&def.name)
                .font(font::heading(14.0))
                .color(p.text),
        );
        // datasheet fields
        let mut rows: Vec<(String, String)> = def
            .inspector
            .iter()
            .map(|f| (f.label.clone(), expand(&def, &part, &f.value)))
            .collect();
        // live readings
        if let Some(pt) = &pt {
            for (k, v) in &pt.readings {
                rows.push((reading_label(k), reading_text(k, *v)));
            }
            if let bb_core::Look::Led {
                current,
                brightness,
                ..
            } = &pt.visual.look
            {
                rows.push(("Current".into(), bb_core::si::format(*current, "A")));
                rows.push(("Brightness".into(), format!("{:.0} %", brightness * 100.0)));
            }
            if let bb_core::Look::Readout { text } = &pt.visual.look {
                rows.push(("Reading".into(), text.clone()));
            }
        }
        // WORLD-6: how this part is measured (the World panel's noise switch)
        rows.extend(measurement_row(&def, doc.project().world.noise));
        widgets::kv_grid(ui, "insp-kv", &rows, 120.0);
        // health
        let (label, color, reason) = match pt.as_ref().map(|p| &p.visual.health) {
            Some(bb_core::Health::Damaged { reason }) => ("damaged", p.danger_soft, reason.clone()),
            Some(bb_core::Health::Stressed { reason }) => ("stressed", p.accent, reason.clone()),
            _ => ("OK", p.ok, String::new()),
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Health")
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            ui.add_space(70.0);
            widgets::status_dot(ui, color, label, color);
        });
        if !reason.is_empty() {
            ui.label(
                egui::RichText::new(reason)
                    .font(font::ui(11.5))
                    .color(color),
            );
        }
        // editable properties
        let editable: Vec<&PropDef> = def
            .props
            .iter()
            .filter(|pd| pd.key != "pot" || def.bench.art == "pot_soil")
            .collect();
        if !editable.is_empty() {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Settings")
                    .font(font::semibold(11.0))
                    .color(p.text_muted),
            );
        }
        let mut change: Option<(String, PropValue, bool)> = None;
        for pd in editable {
            let cur = Project::prop(&def, &part, &pd.key).unwrap_or_else(|| pd.default_value());
            ui.horizontal(|ui| {
                let (lr, _) = ui.allocate_exact_size(vec2(110.0, 28.0), Sense::hover());
                ui.painter().text(
                    lr.left_center(),
                    egui::Align2::LEFT_CENTER,
                    &pd.label,
                    font::ui(12.0),
                    p.text_muted,
                );
                match &pd.kind {
                    PropKind::Number { series, .. } => {
                        let key = (id.clone(), pd.key.clone());
                        let shown = cur
                            .as_f64()
                            .map(|v| bb_core::si::format(v, &pd.unit))
                            .unwrap_or_default();
                        let buf = self
                            .edits
                            .entry(key.clone())
                            .or_insert_with(|| shown.clone());
                        let te = ui.add(
                            egui::TextEdit::singleline(buf)
                                .desired_width(92.0)
                                .font(font::mono(12.5))
                                .margin(vec2(6.0, 5.0)),
                        );
                        let lab = pd.label.clone();
                        te.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, &lab)
                        });
                        // typing replaces the value: select it all on focus
                        if te.gained_focus()
                            && let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), te.id)
                        {
                            let n = buf.chars().count();
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                            st.store(ui.ctx(), te.id);
                        }
                        let focused = te.has_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let commit = te.lost_focus() && (enter || !focused);
                        if !te.has_focus() && !commit {
                            *buf = shown.clone();
                        }
                        if commit {
                            let txt = buf
                                .trim()
                                .trim_end_matches(pd.unit.as_str())
                                .trim()
                                .replace(' ', "");
                            match bb_core::si::parse(&txt) {
                                Ok(v) => {
                                    change = Some((pd.key.clone(), PropValue::Num(v), pd.live))
                                }
                                Err(_) => *buf = shown.clone(),
                            }
                        }
                        if let (Some(s), Some(v)) = (series, cur.as_f64()) {
                            let vals = bb_circuit::eseries::values(*s, v / 20.0, v * 20.0);
                            let prev = vals.iter().rev().find(|x| **x < v * 0.999).copied();
                            let next = vals.iter().find(|x| **x > v * 1.001).copied();
                            if Btn::mini(&format!("Smaller {}", pd.label))
                                .icon_only(Icon::ChevronLeft)
                                .show(ui)
                                .clicked()
                                && let Some(x) = prev
                            {
                                change = Some((pd.key.clone(), PropValue::Num(x), pd.live));
                                self.edits.remove(&key);
                            }
                            if Btn::mini(&format!("Larger {}", pd.label))
                                .icon_only(Icon::ChevronRight)
                                .show(ui)
                                .clicked()
                                && let Some(x) = next
                            {
                                change = Some((pd.key.clone(), PropValue::Num(x), pd.live));
                                self.edits.remove(&key);
                            }
                        }
                    }
                    PropKind::Choice { options, .. } => {
                        let cur_t = cur.as_text();
                        let shown = options
                            .iter()
                            .find(|o| o.value == cur_t)
                            .map(|o| {
                                if o.label.is_empty() {
                                    o.value.clone()
                                } else {
                                    o.label.clone()
                                }
                            })
                            .unwrap_or(cur_t.clone());
                        let (_, picked) = widgets::select(ui, &pd.label, &shown, 140.0, |ui| {
                            let mut pick = None;
                            for o in options {
                                let l = if o.label.is_empty() {
                                    o.value.clone()
                                } else {
                                    o.label.clone()
                                };
                                if widgets::menu_item(ui, &l, o.value == cur_t, true) {
                                    pick = Some(o.value.clone());
                                }
                            }
                            pick
                        });
                        if let Some(Some(v)) = picked {
                            change = Some((pd.key.clone(), PropValue::Text(v), false));
                        }
                    }
                    PropKind::Bool { .. } => {
                        let mut b = matches!(cur, PropValue::Bool(true))
                            || cur.as_f64().is_some_and(|x| x != 0.0);
                        if widgets::checkbox(ui, &mut b, "").changed() {
                            change = Some((pd.key.clone(), PropValue::Bool(b), pd.live));
                        }
                    }
                    PropKind::Text { .. } => {
                        let key = (id.clone(), pd.key.clone());
                        let buf = self.edits.entry(key).or_insert_with(|| cur.as_text());
                        let te = ui.add(
                            egui::TextEdit::singleline(buf)
                                .desired_width(140.0)
                                .font(font::mono(12.0)),
                        );
                        if te.lost_focus() {
                            change = Some((pd.key.clone(), PropValue::Text(buf.clone()), false));
                        }
                    }
                }
            });
        }
        // resistor colour bands, as on the bench
        if def.bench.art == "resistor_axial" {
            let ohms = Project::prop_f64(&def, &part, "resistance").unwrap_or(1000.0);
            let tol = Project::prop(&def, &part, "tolerance")
                .map(|v| v.as_text())
                .unwrap_or_default()
                .parse()
                .unwrap_or(5.0);
            let bands = crate::canvas::art::resistor_bands(ohms, tol);
            let (r, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
            let names: Vec<&str> = bands.iter().map(|b| band_name(*b)).collect();
            let a11y = format!("Colour bands {}", names.join(" "));
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &a11y));
            let body = egui::Rect::from_min_size(r.min + vec2(0.0, 3.0), vec2(120.0, 16.0));
            ui.painter()
                .rect_filled(body, 8.0, crate::canvas::pen::rgb(0xE2C896));
            for (i, b) in bands.iter().enumerate() {
                let x = if i == bands.len() - 1 {
                    body.right() - 22.0
                } else {
                    body.left() + 20.0 + i as f32 * 15.0
                };
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x, body.top()), vec2(7.0, 16.0)),
                    0.0,
                    band_color(*b),
                );
            }
            ui.painter().text(
                egui::pos2(body.right() + 10.0, body.center().y),
                egui::Align2::LEFT_CENTER,
                names.join(" · "),
                font::ui(11.0),
                p.text_muted,
            );
        }
        if let Some((key, value, live)) = change {
            let lib = doc.lib.clone();
            match doc.editor.set_prop(&lib, &id, &key, value.clone()) {
                Ok(()) => {
                    doc.touched();
                    self.edits.retain(|(pid, k), _| !(pid == &id && k == &key));
                    if live && let Some(v) = value.as_f64() {
                        if let Some(sim) = app.sim() {
                            sim.send(Command::SetPartParam {
                                part: id.clone(),
                                key: key.clone(),
                                value: v,
                            });
                        }
                        app.mark_bench_synced();
                    }
                }
                Err(e) => app.toasts.error(e.to_string()),
            }
        }
        // reference + actions
        let Some(doc) = app.circuit.as_mut() else {
            return;
        };
        ui.horizontal(|ui| {
            if Btn::ghost("Rotate").small().show(ui).clicked() {
                let _ = doc.editor.rotate(&id);
                doc.touched();
            }
            if Btn::ghost("Duplicate").small().show(ui).clicked() {
                let lib = doc.lib.clone();
                let new = doc.editor.duplicate(&lib, std::slice::from_ref(&id));
                doc.touched();
                canvas.selection.clear();
                canvas.selection.parts.extend(new);
            }
            if Btn::ghost("Delete")
                .icon(Icon::Trash)
                .small()
                .show(ui)
                .clicked()
            {
                doc.editor.delete(std::slice::from_ref(&id), &[]);
                doc.touched();
                canvas.selection.clear();
            }
        });
        self.repair(ui, app, &tel);
    }

    fn repair(
        &self,
        ui: &mut Ui,
        app: &mut AppCtx,
        tel: &Option<std::sync::Arc<bb_sim::Telemetry>>,
    ) {
        let damaged = tel
            .as_ref()
            .is_some_and(|t| t.parts.iter().any(|p| p.visual.health.is_damaged()));
        if damaged
            && Btn::amber("Repair all")
                .icon(Icon::Wrench)
                .small()
                .show(ui)
                .clicked()
            && let Some(sim) = app.sim()
        {
            sim.send(Command::RepairAll);
        }
    }
}

fn band_name(d: i32) -> &'static str {
    match d {
        0 => "black",
        1 => "brown",
        2 => "red",
        3 => "orange",
        4 => "yellow",
        5 => "green",
        6 => "blue",
        7 => "violet",
        8 => "grey",
        9 => "white",
        -1 => "gold",
        _ => "silver",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurement_rows_follow_the_world_switch() {
        let lib = bb_circuit::Library::builtin();
        let uno = lib.get("uno").unwrap();
        let soil = lib.get("capsoil").unwrap();
        let res = lib.get("resistor").unwrap();
        assert_eq!(
            measurement_row(uno, true).unwrap(),
            (
                "ADC".to_string(),
                "realistic · offset / gain error, INL, ±1 LSB noise".to_string()
            )
        );
        assert_eq!(
            measurement_row(uno, false).unwrap().1,
            "ideal · exact codes"
        );
        assert_eq!(measurement_row(soil, true).unwrap().0, "Noise");
        assert_eq!(
            measurement_row(soil, false).unwrap().1,
            "ideal, none (World)"
        );
        assert!(measurement_row(res, true).is_none());
    }

    #[test]
    fn inspector_templates_expand_with_units() {
        let lib = bb_circuit::Library::builtin();
        let def = lib.get("resistor").unwrap();
        let part = PartInstance::new("r1", "R1", "resistor").prop("resistance", 10_000.0);
        let s = expand(def, &part, "$(resistance|Ω) ±$(tolerance) %");
        assert!(
            s.starts_with("10") && s.contains("kΩ") && s.ends_with("±5 %"),
            "{s}"
        );
        let led = lib.get("led").unwrap();
        let p = PartInstance::new("d1", "D1", "led").prop("color", "amber");
        assert_eq!(expand(led, &p, "D · LED_$(color)"), "D · LED_amber");
    }
}
