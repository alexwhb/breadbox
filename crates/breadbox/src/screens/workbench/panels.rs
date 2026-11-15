//! Right-rail panels (`WB-8`): World (`WB-9`), Instruments (bench tools + probe clipping),
//! Probes (`WB-6`), Inspector (`WB-10`, in `inspector.rs`) and Waveforms (the mini scope).

use super::{RailPanel, WorkbenchState, inspector};
use crate::app::AppCtx;
use crate::canvas::bench::{BenchCanvas, Tool, probe_text};
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon, Stop, StopSlider};
use bb_circuit::edit::{Op, Transaction};
use bb_circuit::project::{Endpoint, InstrumentDef};
use bb_sim::Command;
use eframe::egui::{self, Color32, Sense, Stroke, Ui, vec2};

#[derive(Default)]
pub struct Panels {
    world: WorldUi,
    program: super::world_program::WorldProgramUi,
    inspector: inspector::Inspector,
}

/// Slider values being edited (the live telemetry wins when idle).
#[derive(Default)]
struct WorldUi {
    moist: f64,
    lux: f64,
    temp: f64,
    rh: f64,
    active: Option<&'static str>,
    /// Humidity, time-warp and location shown.
    more: bool,
}

/// Well-known places for the day/night cycle.
pub const LOCATIONS: [(&str, f64, f64, f64); 6] = [
    ("Ashland, OR", 42.1946, -122.7095, -8.0),
    ("San Francisco, CA", 37.7749, -122.4194, -8.0),
    ("New York, NY", 40.7128, -74.0060, -5.0),
    ("London, UK", 51.5072, -0.1276, 0.0),
    ("Tokyo, JP", 35.6762, 139.6503, 9.0),
    ("Sydney, AU", -33.8688, 151.2093, 10.0),
];

impl Panels {
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        app: &mut AppCtx,
        state: &mut WorkbenchState,
        canvas: &mut BenchCanvas,
    ) {
        egui::ScrollArea::vertical()
            .id_salt("wb-right-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(16).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 18.0;
                    let order: Vec<RailPanel> = RailPanel::ALL
                        .into_iter()
                        .filter(|p| state.panels.contains(p))
                        .collect();
                    if order.is_empty() {
                        ui.label(
                            egui::RichText::new("Pick a panel from the rail →")
                                .font(font::ui(12.0))
                                .color(pal(ui).text_faint),
                        );
                    }
                    for (i, p) in order.iter().enumerate() {
                        if i > 0 {
                            widgets::hline(ui, pal(ui).border);
                        }
                        match p {
                            RailPanel::World => self.world(ui, app),
                            RailPanel::Instruments => instruments(ui, app, canvas),
                            RailPanel::Probes => probes(ui, app, canvas),
                            RailPanel::Inspector => self.inspector.ui(ui, app, canvas),
                            RailPanel::Waveforms => waveforms(ui, app),
                        }
                    }
                });
            });
    }

    fn world(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let Some(doc) = app.circuit.as_ref() else {
            return;
        };
        let cfg = doc.project().world.clone();
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        let pot = cfg.pots.first().cloned();
        let snap_pot = tel.as_ref().and_then(|t| t.world.pots.first().cloned());
        // header
        let caption = pot
            .as_ref()
            .map(|p| {
                format!(
                    "{} · {}",
                    p.name,
                    if p.indoor { "indoor" } else { "outdoor" }
                )
            })
            .unwrap_or_default();
        widgets::section_header_row(ui, "World", &caption);
        // sync from the simulator when not dragging
        let w = &mut self.world;
        if w.active.is_none() {
            w.moist = snap_pot
                .as_ref()
                .map(|p| p.vwc)
                .or(pot.as_ref().map(|p| p.vwc))
                .unwrap_or(0.0);
            w.lux = tel
                .as_ref()
                .map(|t| t.world.ambient_lux)
                .unwrap_or(cfg.light.ambient_lux);
            w.temp = tel
                .as_ref()
                .map(|t| t.world.temp_c)
                .unwrap_or(cfg.air.temp_c);
            w.rh = tel.as_ref().map(|t| t.world.rh).unwrap_or(cfg.air.rh);
        }
        let mut sends: Vec<(String, f64)> = Vec::new();
        let mut commit = false;
        let mut track =
            |resp: &egui::Response, key: &'static str, active: &mut Option<&'static str>| {
                if resp.drag_started() || resp.is_pointer_button_down_on() {
                    *active = Some(key);
                }
                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                    commit = true;
                    *active = None;
                }
            };
        if let Some(pt) = &pot {
            let sat = pt.saturation.max(60.0);
            // named stops spread like the design (left / centre / right)
            let stops = [
                Stop {
                    value: 0.0,
                    label: "Bone dry",
                },
                Stop {
                    value: sat / 2.0,
                    label: "Field capacity",
                },
                Stop {
                    value: sat,
                    label: "Saturated",
                },
            ];
            let before = w.moist;
            let r = StopSlider::new("Soil moisture", 0.0..=sat, &|v| format!("{v:.0} % VWC"))
                .stops(&stops)
                .step(1.0)
                .show(ui, &mut w.moist);
            track(&r, "moist", &mut w.active);
            if w.moist != before {
                sends.push((format!("{}.vwc", pt.id), w.moist));
            }
        }
        let stops = [
            Stop {
                value: 0.0,
                label: "Night",
            },
            Stop {
                value: 500.0,
                label: "Living room",
            },
            Stop {
                value: 1000.0,
                label: "Bright office",
            },
        ];
        let before = w.lux;
        let r = StopSlider::new("Ambient light", 0.0..=1000.0, &|v| format!("{v:.0} lx"))
            .stops(&stops)
            .step(10.0)
            .show(ui, &mut w.lux);
        track(&r, "lux", &mut w.active);
        if w.lux != before {
            sends.push(("light.ambient_lux".into(), w.lux));
        }
        let before = w.temp;
        let r = StopSlider::new("Air temperature", -10.0..=45.0, &|v| format!("{v:.0} °C"))
            .step(1.0)
            .show(ui, &mut w.temp);
        track(&r, "temp", &mut w.active);
        if w.temp != before {
            sends.push(("air.temp_c".into(), w.temp));
        }
        if w.more {
            let before = w.rh;
            let r = StopSlider::new("Humidity", 0.0..=100.0, &|v| format!("{v:.0} %RH"))
                .step(1.0)
                .show(ui, &mut w.rh);
            track(&r, "rh", &mut w.active);
            if w.rh != before {
                sends.push(("air.rh".into(), w.rh));
            }
        }
        let more = w.more;
        // checkboxes
        let mut new_cfg = cfg.clone();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            if let Some(pt) = new_cfg.pots.first_mut() {
                let rate = snap_pot
                    .as_ref()
                    .map(|p| p.evaporation_pct_h)
                    .unwrap_or(1.4);
                let mut evap = pt.evaporation;
                if widgets::checkbox(ui, &mut evap, &format!("Evaporation · ≈ {rate:.1} %/h"))
                    .changed()
                {
                    pt.evaporation = evap;
                    sends.push((format!("{}.evaporation", pt.id), evap as u8 as f64));
                    commit = true;
                }
                // scheduled watering (WORLD-1): 100 ml every N world hours
                let cur = pt.schedule.map(|s| s.every_h);
                let shown = match pt.schedule {
                    Some(s) => format!("every {} h · {:.0} ml", s.every_h, s.ml),
                    None => "off".into(),
                };
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Water automatically")
                            .font(crate::theme::font::ui(13.0))
                            .color(pal(ui).text_dim),
                    );
                    let (_, picked) =
                        widgets::select(ui, "Water automatically", &shown, 150.0, |ui| {
                            let mut pick = None;
                            for (l, v) in [
                                ("Off", 0.0),
                                ("Every 6 h", 6.0),
                                ("Every 12 h", 12.0),
                                ("Every 24 h", 24.0),
                                ("Every 48 h", 48.0),
                            ] {
                                let on = cur.map_or(v == 0.0, |c| (c - v).abs() < 1e-9);
                                if widgets::menu_item(ui, l, on, true) {
                                    pick = Some(v);
                                }
                            }
                            pick
                        });
                    if let Some(Some(v)) = picked {
                        pt.schedule = (v > 0.0).then_some(bb_world::WaterSchedule {
                            every_h: v,
                            ml: 100.0,
                            at_h: 0.0,
                        });
                        sends.push((format!("{}.water_every_h", pt.id), v));
                        commit = true;
                    }
                });
            }
            let mut solar = new_cfg.light.mode == bb_world::LightMode::Solar;
            let loc = new_cfg.light.location.name.clone();
            if widgets::checkbox(ui, &mut solar, &format!("Day/night cycle ({loc})")).changed() {
                new_cfg.light.mode = if solar {
                    bb_world::LightMode::Solar
                } else {
                    bb_world::LightMode::Manual
                };
                sends.push(("light.solar".into(), solar as u8 as f64));
                commit = true;
            }
            if solar {
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    let (_, picked) = widgets::select(ui, "Location", &loc, 200.0, |ui| {
                        let mut pick = None;
                        for (name, lat, lon, off) in LOCATIONS {
                            if widgets::menu_item(ui, name, name == loc, true) {
                                pick = Some((name, lat, lon, off));
                            }
                        }
                        pick
                    });
                    if let Some(Some((name, lat, lon, off))) = picked {
                        new_cfg.light.location = bb_world::sun::Location {
                            name: name.into(),
                            lat,
                            lon,
                            utc_offset_h: off,
                        };
                        commit = true;
                        relocate(app, new_cfg.clone());
                    }
                });
            }
            let mut noise = new_cfg.noise;
            // WORLD-6: what on / off mean, in the Inspector's words (hover text: the panel has
            // no height to spare)
            if widgets::checkbox(ui, &mut noise, "Sensor noise & ADC quantization")
                .on_hover_text(format!(
                    "On — {}\nOff — {}",
                    measurement_text(true),
                    measurement_text(false)
                ))
                .changed()
            {
                new_cfg.noise = noise;
                sends.push(("world.noise".into(), noise as u8 as f64));
                commit = true;
            }
        });
        let mut toggle_more = false;
        ui.horizontal(|ui| {
            if let Some(pt) = &pot
                && Btn::ghost("Water the pot")
                    .icon(Icon::Plus)
                    .small()
                    .show(ui)
                    .clicked()
                && let Some(sim) = app.sim()
            {
                sim.send(Command::Water {
                    pot: pt.id.clone(),
                    ml: 100.0,
                });
            }
            let more_label = if more {
                "Fewer settings"
            } else {
                "More settings"
            };
            if Btn::mini(more_label).show(ui).clicked() {
                toggle_more = true;
            }
            if !more {
                return;
            }
            let warp = tel
                .as_ref()
                .map(|t| t.world.time_warp)
                .unwrap_or(cfg.time_warp);
            let label = format!("World time ×{}", warp.round() as i64);
            let (_, picked) = widgets::select(ui, "World time-warp", &label, 150.0, |ui| {
                let mut pick = None;
                for (l, v) in [
                    ("×1 (real time)", 1.0),
                    ("×60", 60.0),
                    ("×600", 600.0),
                    ("×3600", 3600.0),
                ] {
                    if widgets::menu_item(ui, l, (warp - v).abs() < 0.5, true) {
                        pick = Some(v);
                    }
                }
                pick
            });
            if let Some(Some(v)) = picked {
                sends.push(("world.time_warp".into(), v));
                new_cfg.time_warp = v;
                commit = true;
            }
        });
        if toggle_more {
            self.world.more = !self.world.more;
        }
        let _ = p;
        // apply
        if !sends.is_empty()
            && let Some(sim) = app.sim()
        {
            for (k, v) in &sends {
                sim.send(Command::SetWorld {
                    key: k.clone(),
                    value: *v,
                });
            }
        }
        if commit && let Some(doc) = app.circuit.as_mut() {
            if let Some(pt) = new_cfg.pots.first_mut() {
                pt.vwc = self.world.moist;
            }
            new_cfg.light.ambient_lux = self.world.lux;
            new_cfg.air.temp_c = self.world.temp;
            new_cfg.air.rh = self.world.rh;
            doc.editor.set_world(new_cfg);
            doc.touched();
        }
        // movable objects (WORLD-4) and the programmable world (WORLD-5)
        super::world_program::objects(ui, app);
        super::world_program::program(ui, app, &mut self.program);
    }
}

/// Changing the day/night location rebuilds the world model at the same moisture.
fn relocate(app: &mut AppCtx, cfg: bb_world::WorldConfig) {
    if let Some(sim) = app.sim() {
        sim.send(Command::With(bb_sim::handle::BenchFn(std::sync::Arc::new(
            move |b| {
                let mut c = cfg.clone();
                for pot in c.pots.iter_mut() {
                    if let Some(s) = b.world().snapshot().pots.iter().find(|p| p.id == pot.id) {
                        pot.vwc = s.vwc;
                    }
                }
                if let Ok(w) = bb_world::World::new(c) {
                    *b.world_mut() = w;
                }
            },
        ))));
    }
}

/// Bench tools: built-in mini scope and DMM, plugin instruments (placed from the Plugins
/// screen). Each probe can be clipped onto any pin.
fn instruments(ui: &mut Ui, app: &mut AppCtx, canvas: &mut BenchCanvas) {
    let p = pal(ui);
    widgets::section_header_row(ui, "Instruments", "drag leads onto pins");
    let Some(doc) = app.circuit.as_mut() else {
        return;
    };
    let defs = doc.project().instruments.clone();
    let lib = doc.lib.clone();
    let tools: [(&str, &str, Icon, &str, &[&str]); 2] = [
        (
            "mini_scope",
            "Oscilloscope",
            Icon::Oscilloscope,
            "2 channels · trigger · duty",
            &["ch1", "ch2"],
        ),
        (
            "dmm",
            "Multimeter",
            Icon::Multimeter,
            "DC volts · auto · 6000 counts",
            &["plus", "com"],
        ),
    ];
    for (kind, name, icon, sub, keys) in tools {
        let placed = defs.iter().find(|d| d.kind == kind).cloned();
        widgets::inner_card_frame(p, placed.is_some()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icon.paint(
                    ui.painter(),
                    egui::Rect::from_min_size(ui.cursor().min + vec2(0.0, 4.0), vec2(22.0, 22.0)),
                    if placed.is_some() { p.accent_soft } else { p.text_dim },
                );
                ui.add_space(30.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(egui::RichText::new(name).font(font::medium(13.0)).color(p.text));
                    let s = if placed.is_some() { "On bench".to_string() } else { sub.to_string() };
                    ui.label(egui::RichText::new(s).font(font::ui(11.0)).color(p.text_muted));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let act = if placed.is_some() { "Remove" } else { "Add" };
                    if Btn::ghost(&format!("{act} {name}")).small().show(ui).clicked() {
                        let mut tx = Transaction::new(&format!("{act} {name}"));
                        match &placed {
                            Some(d) => {
                                tx.ops.push(Op::Instrument { before: Some(d.clone()), after: None, index: None });
                                if let Some(part) = &d.part
                                    && let Some(pi) = doc.project().part(part).cloned()
                                {
                                    tx.ops.push(Op::Part { before: Some(pi), after: None, index: None });
                                }
                            }
                            None => {
                                // place the instrument above / beside the bench
                                let e = canvas_extent(doc);
                                let (x, y) = if kind == "mini_scope" { (e.0, e.1 - 12.0) } else { (e.2 + 4.0, e.1) };
                                let pid = doc.project().next_id(kind);
                                let reference = doc.project().next_ref(if kind == "dmm" { "DMM" } else { "SCOPE" });
                                let part = bb_circuit::PartInstance::new(&pid, &reference, kind).at(x.round(), y.round());
                                let id = doc.project().next_id(&format!("{kind}_i"));
                                let settings = if kind == "mini_scope" {
                                    serde_json::json!({"timebase": 0.001, "vdiv": 2.0, "trigger": {"edge": "rising", "level": 2.5, "mode": "auto", "source": "ch1"}})
                                } else {
                                    serde_json::json!({"mode": "dc_v"})
                                };
                                tx.ops.push(Op::Part { before: None, after: Some(part), index: None });
                                tx.ops.push(Op::Instrument {
                                    before: None,
                                    after: Some(InstrumentDef { id, kind: kind.into(), part: Some(pid), probes: Default::default(), settings }),
                                    index: None,
                                });
                            }
                        }
                        doc.editor.commit(tx);
                        doc.touched();
                    }
                });
            });
            if let Some(d) = &placed {
                for key in keys {
                    ui.horizontal(|ui| {
                        ui.add_space(30.0);
                        let target = d.probes.get(*key).map(|e| endpoint_label(doc, e)).unwrap_or_else(|| "not clipped".into());
                        ui.label(
                            egui::RichText::new(format!("{} → {target}", key.to_uppercase()))
                                .font(font::mono(11.5))
                                .color(if d.probes.contains_key(*key) { p.accent_soft } else { p.text_faint }),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let armed = matches!(&canvas.tool, Tool::Clip { instrument, key: k } if instrument == &d.id && k == key);
                            if Btn::mini(&format!("Clip {}", key.to_uppercase())).selected(armed).show(ui).clicked() {
                                canvas.tool = if armed { Tool::Select } else { Tool::Clip { instrument: d.id.clone(), key: key.to_string() } };
                            }
                        });
                    });
                }
            }
        });
    }
    let _ = lib;
    let plugin: Vec<_> = defs
        .iter()
        .filter(|d| d.kind != "mini_scope" && d.kind != "dmm")
        .collect();
    for d in plugin {
        ui.label(
            egui::RichText::new(format!("{} · {}", d.kind, d.id))
                .font(font::mono(12.0))
                .color(p.text_dim),
        );
    }
    ui.label(
        egui::RichText::new("Clip a lead, then click any pin, wire end or hole on the bench. Logic analyzer, bench supply and function generator come from the Plugins screen.")
            .font(font::ui(11.0))
            .color(p.text_faint),
    );
}

fn canvas_extent(doc: &crate::circuit::CircuitDoc) -> (f64, f64, f64, f64) {
    let mut e = (0.0f64, 0.0f64, 40.0f64, 20.0f64);
    let mut first = true;
    for p in &doc.project().parts {
        if let Some(b) = &p.bench {
            let s = doc
                .lib
                .get(&p.part)
                .map(|d| d.bench.size)
                .unwrap_or((4.0, 4.0));
            let r = (b.at.x, b.at.y, b.at.x + s.0, b.at.y + s.1);
            if first {
                e = r;
                first = false;
            } else {
                e = (e.0.min(r.0), e.1.min(r.1), e.2.max(r.2), e.3.max(r.3));
            }
        }
    }
    e
}

pub fn endpoint_label(doc: &crate::circuit::CircuitDoc, e: &Endpoint) -> String {
    match e {
        Endpoint::Pin { part, pin } => {
            let r = doc
                .project()
                .part(part)
                .map(|p| p.reference.clone())
                .unwrap_or_else(|| part.clone());
            format!("{r}.{pin}")
        }
        Endpoint::Free { x, y } => format!("({x:.0}, {y:.0})"),
    }
}

/// Live probes (`WB-6`): value / min / max / avg, add with the probe tool, remove.
fn probes(ui: &mut Ui, app: &mut AppCtx, canvas: &mut BenchCanvas) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        widgets::section_header(ui, "Probes");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let on = canvas.tool == Tool::Probe;
            if Btn::mini("Add probe").selected(on).show(ui).clicked() {
                canvas.tool = if on { Tool::Select } else { Tool::Probe };
            }
        });
    });
    let tel = app.bench.sim().and_then(|s| s.telemetry());
    let Some(doc) = app.circuit.as_mut() else {
        return;
    };
    let probes = doc.project().probes.clone();
    if probes.is_empty() {
        ui.label(
            egui::RichText::new("No probes yet — Add probe, then click a pin.")
                .font(font::ui(12.0))
                .color(p.text_faint),
        );
    }
    let mut remove = None;
    for pr in &probes {
        let r = tel
            .as_ref()
            .and_then(|t| t.probes.iter().find(|x| x.id == pr.id));
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    egui::RichText::new(&pr.label)
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
                let v = r.map(probe_text).unwrap_or_else(|| "—".into());
                ui.label(
                    egui::RichText::new(v)
                        .font(font::mono_medium(15.0))
                        .color(p.text),
                );
                if let Some(r) = r {
                    ui.label(
                        egui::RichText::new(format!(
                            "min {} · max {} · avg {}",
                            bb_core::si::format(r.min, &r.unit),
                            bb_core::si::format(r.max, &r.unit),
                            bb_core::si::format(r.avg, &r.unit)
                        ))
                        .font(font::mono(10.5))
                        .color(p.text_faint),
                    );
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if Btn::plain(&format!("Remove probe {}", pr.label))
                    .icon_only(Icon::Close)
                    .height(22.0)
                    .show(ui)
                    .clicked()
                {
                    remove = Some(pr.id.clone());
                }
            });
        });
    }
    if let Some(id) = remove {
        doc.editor.remove_probe(&id);
        doc.touched();
    }
}

/// The mini scope's screen, large, with timebase / V/div / trigger controls.
fn waveforms(ui: &mut Ui, app: &mut AppCtx) {
    let p = pal(ui);
    widgets::section_header_row(ui, "Waveforms", "mini scope");
    let tel = app.bench.sim().and_then(|s| s.telemetry());
    let Some(t) = tel else {
        return;
    };
    if t.scopes.is_empty() {
        ui.label(
            egui::RichText::new("Add the oscilloscope from Instruments to see waveforms here.")
                .font(font::ui(12.0))
                .color(p.text_faint),
        );
        return;
    }
    for f in &t.scopes {
        let w = ui.available_width();
        let (r, resp) = ui.allocate_exact_size(vec2(w, w * 0.62), Sense::hover());
        let label = format!("Scope {} · {}", f.id, f.label);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &label));
        ui.painter().rect_filled(r, 6.0, Color32::from_rgb(4, 6, 4));
        let grid = Color32::from_rgb(0x1B, 0x2A, 0x1B);
        for i in 1..10 {
            let x = r.left() + r.width() * i as f32 / 10.0;
            ui.painter().line_segment(
                [egui::pos2(x, r.top()), egui::pos2(x, r.bottom())],
                Stroke::new(1.0, grid),
            );
        }
        for i in 1..8 {
            let y = r.top() + r.height() * i as f32 / 8.0;
            ui.painter().line_segment(
                [egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
                Stroke::new(1.0, grid),
            );
        }
        let span = (f.timebase * 10.0).max(1e-9);
        for (ch, tr) in f.traces.iter().enumerate() {
            let vdiv = f.vdiv.get(ch).copied().unwrap_or(1.0).max(1e-9);
            let col = if ch == 0 { p.trace_yellow } else { p.info };
            let pts: Vec<egui::Pos2> = tr
                .iter()
                .map(|(tt, v)| {
                    let x = r.left() + (tt / span).clamp(0.0, 1.0) as f32 * r.width();
                    let y = r.center().y
                        - ((v / vdiv - 2.0).clamp(-4.2, 4.2) as f32) * r.height() / 8.0;
                    egui::pos2(x, y)
                })
                .collect();
            if pts.len() > 1 {
                ui.painter()
                    .add(egui::Shape::line(pts, Stroke::new(1.6, col)));
            }
        }
        ui.label(
            egui::RichText::new(format!(
                "{} · f {} · duty {:.1} % · Vpp {} · avg {}",
                f.label,
                bb_core::si::format(f.frequency, "Hz"),
                f.duty * 100.0,
                bb_core::si::format(f.vpp, "V"),
                bb_core::si::format(f.vavg, "V")
            ))
            .font(font::mono(11.0))
            .color(p.text_dim),
        );
        ui.horizontal_wrapped(|ui| {
            let mut send = None;
            for (l, tb) in [
                ("0.1 ms/div", 1e-4),
                ("1 ms/div", 1e-3),
                ("10 ms/div", 1e-2),
            ] {
                if Btn::mini(l)
                    .selected((f.timebase - tb).abs() < tb * 0.01)
                    .show(ui)
                    .clicked()
                {
                    send = Some(Command::Scope {
                        id: f.id.clone(),
                        timebase: Some(tb),
                        vdiv: None,
                        trigger: None,
                    });
                }
            }
            for (l, v) in [("1 V/div", 1.0), ("2 V/div", 2.0)] {
                if Btn::mini(l)
                    .selected((f.vdiv[0] - v).abs() < 1e-6)
                    .show(ui)
                    .clicked()
                {
                    send = Some(Command::Scope {
                        id: f.id.clone(),
                        timebase: None,
                        vdiv: Some([v, f.vdiv[1]]),
                        trigger: None,
                    });
                }
            }
            if let Some(c) = send
                && let Some(sim) = app.sim()
            {
                sim.send(c);
            }
        });
    }
}

/// `WORLD-6` in words, shared by the World panel and the Inspector: on = realistic sensors and
/// ADC, off = ideal ones. Firmware reads integer codes either way; probes and meters always
/// show the continuous voltage.
pub fn measurement_text(realistic: bool) -> &'static str {
    if realistic {
        "realistic: sensor noise + ADC offset / gain error, INL / DNL and ±1 LSB noise (seeded)"
    } else {
        "ideal: noise-free sensors, exact ADC codes floor(V / Vref · 1024)"
    }
}
