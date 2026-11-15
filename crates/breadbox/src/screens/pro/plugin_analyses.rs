//! Analysis plugins in Pro → Analysis (`PLUG-7`): one card per active `analysis` plugin with
//! its options, a Run button that hands the current netlist (Breadbox-only parts swapped for
//! their stand-ins) to the plugin's sandbox, and the plugin's summary / table / text output.

use super::{demo, model};
use crate::app::AppCtx;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn};
use eframe::egui::{self, Id, Ui};
use std::collections::BTreeMap;

/// A plugin's last result: summary, table rows, text.
type Shown = Result<(String, Vec<Vec<String>>, String), String>;

fn key() -> Id {
    Id::unique("plugin-analyses")
}

/// The netlist as a plugin gets it: stand-ins for parts only the co-simulation knows.
pub fn runnable(text: &str) -> String {
    let (t, _) = model::prepare(text);
    let lower = t.to_ascii_lowercase();
    let mut subckts = String::new();
    for (name, body) in demo::STUB_PARTS {
        if lower.contains(&name.to_ascii_lowercase())
            && !lower.contains(&format!(".subckt {}", name.to_ascii_lowercase()))
        {
            subckts.push_str(body);
            subckts.push('\n');
        }
    }
    if subckts.is_empty() {
        return t;
    }
    match lower.rfind("\n.end") {
        Some(i) => format!("{}\n{subckts}{}", &t[..i], &t[i..]),
        None => format!("{t}\n{subckts}.end\n"),
    }
}

/// The "Plugin analyses" section of the sidebar.
pub fn section(ui: &mut Ui, app: &mut AppCtx, netlist: &str) {
    let host = app.plugins.host();
    let list = host.analyses();
    if list.is_empty() {
        return;
    }
    let p = pal(ui);
    let mut shown: BTreeMap<String, Shown> = ui
        .ctx()
        .data(|d| d.get_temp::<BTreeMap<String, Shown>>(key()))
        .unwrap_or_default();
    widgets::section_header(ui, "Plugin analyses");
    for (id, info) in list {
        widgets::inner_card_frame(p, false).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                // Run first (right edge), then the name cut into what is left
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = Btn::mini("Run").show(ui);
                    let l = format!("Run {}", info.name);
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &l));
                    if b.clicked() {
                        let opts =
                            serde_json::Value::Object(info.options.clone().into_iter().collect());
                        let r = host
                            .run_analysis(&id, &runnable(netlist), &opts)
                            .map(|o| (o.summary, o.table, o.text))
                            .map_err(|e| e.to_string());
                        shown.insert(id.clone(), r);
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&info.name)
                                    .font(font::semibold(13.0))
                                    .color(p.text),
                            )
                            .truncate(),
                        );
                    });
                });
            });
            // the plugin's own description: two lines here, all of it on hover
            if !info.description.is_empty() {
                widgets::wrapped_text(ui, &info.description, font::ui(12.0), p.text_muted, 2)
                    .on_hover_text(&info.description);
            }
            if !info.options.is_empty() {
                let o: Vec<String> = info
                    .options
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                ui.label(
                    egui::RichText::new(o.join(" · "))
                        .font(font::mono(11.0))
                        .color(p.text_faint),
                );
            }
            match shown.get(&id) {
                Some(Ok((summary, table, text))) => {
                    if !summary.is_empty() {
                        let r = ui.label(
                            egui::RichText::new(summary)
                                .font(font::mono_medium(12.0))
                                .color(p.accent_soft),
                        );
                        let s = format!("{} result: {summary}", info.name);
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &s));
                    }
                    if !table.is_empty() {
                        egui::Grid::new(("plugin-table", id.as_str()))
                            .spacing(egui::vec2(10.0, 2.0))
                            .show(ui, |ui| {
                                for (i, row) in table.iter().take(16).enumerate() {
                                    for c in row {
                                        ui.label(
                                            egui::RichText::new(c)
                                                .font(if i == 0 {
                                                    font::medium(11.0)
                                                } else {
                                                    font::mono(11.0)
                                                })
                                                .color(if i == 0 {
                                                    p.text_muted
                                                } else {
                                                    p.text_dim
                                                }),
                                        );
                                    }
                                    ui.end_row();
                                }
                            });
                    }
                    if !text.is_empty() {
                        ui.label(
                            egui::RichText::new(text)
                                .font(font::mono(11.0))
                                .color(p.text_dim),
                        );
                    }
                }
                Some(Err(e)) => {
                    ui.label(
                        egui::RichText::new(e)
                            .font(font::ui(12.0))
                            .color(p.danger_soft),
                    );
                }
                None => {}
            }
        });
    }
    ui.ctx().data_mut(|d| d.insert_temp(key(), shown));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_ins_are_inlined_for_plugins() {
        let t = "* x\nXU1 a b ATMEGA328P fw=x.hex\nR1 a 0 1k\n.end\n";
        let r = runnable(t);
        assert!(r.to_ascii_lowercase().contains(".subckt atmega328p"), "{r}");
        assert!(!r.contains("fw=x.hex"));
        assert!(r.trim_end().ends_with(".end"));
    }
}
