//! The Workbench (design screen 1, `WB-1`…`WB-13`): header (Breadboard ⇄ Schematic, Run /
//! Pause, sim clock, speed, status, Share), parts palette, the bench or schematic canvas with
//! its status strip, the right rail (World, Instruments, Probes, Inspector, Waveforms) and the
//! bottom dock (Serial monitor, Probes & power).

mod dock;
pub mod inspector;
mod palette;
mod panels;
pub mod share;
pub mod world_program;

use crate::app::AppCtx;
use crate::bench::{SimSpeed, clock_label};
use crate::canvas::bench::{BenchCanvas, BenchOutput};
use crate::canvas::schematic::SchematicCanvas;
use crate::router::{Command, Screen};
use crate::theme::{self, font, pal};
use crate::widgets::{self, Btn, Icon, PanelMode, Seg};
use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};

/// Breadboard or schematic (`WB-1`, `WB-7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BenchView {
    #[default]
    Breadboard,
    Schematic,
}

/// Right-rail panels (`WB-8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RailPanel {
    World,
    Instruments,
    Probes,
    Inspector,
    Waveforms,
}

impl RailPanel {
    pub const ALL: [RailPanel; 5] = [
        RailPanel::World,
        RailPanel::Instruments,
        RailPanel::Probes,
        RailPanel::Inspector,
        RailPanel::Waveforms,
    ];
    pub fn label(self) -> &'static str {
        match self {
            RailPanel::World => "World",
            RailPanel::Instruments => "Instruments",
            RailPanel::Probes => "Probes",
            RailPanel::Inspector => "Inspector",
            RailPanel::Waveforms => "Waveforms",
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            RailPanel::World => Icon::World,
            RailPanel::Instruments => Icon::Multimeter,
            RailPanel::Probes => Icon::Probe,
            RailPanel::Inspector => Icon::Inspector,
            RailPanel::Waveforms => Icon::Waveform,
        }
    }
}

/// What the simulation menu asked for.
enum SimAction {
    Step(crate::router::StepSize),
    Reset,
    ResetMcu(String),
    Settings(bb_circuit::project::Settings),
}

/// Sync quanta offered in the simulation menu (µs, `SIM-1`).
const QUANTA_US: [(f64, &str); 4] = [
    (1.0, "1 µs"),
    (10.0, "10 µs"),
    (100.0, "100 µs"),
    (1000.0, "1 ms"),
];

/// The header's simulation menu (`WB-1`, `SIM-1`, `SIM-3`): speed, single steps (one quantum /
/// one millisecond), reset the bench or one MCU, and the seed + sync quantum the next run uses.
fn sim_menu(ui: &mut Ui, app: &mut AppCtx, chosen: &mut Option<SimSpeed>) -> Option<SimAction> {
    let p = pal(ui);
    let mut action = None;
    let caption = |ui: &mut Ui, t: &str| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(t.to_ascii_uppercase())
                .font(font::semibold(10.5))
                .color(p.text_faint),
        );
    };
    caption(ui, "Speed");
    for (l, s) in SPEEDS {
        if widgets::menu_item(ui, l, app.bench.speed == s, true) {
            *chosen = Some(s);
        }
    }
    let settings = app
        .circuit
        .as_ref()
        .map(|d| d.project().settings.clone())
        .unwrap_or_default();
    let q_label = QUANTA_US
        .iter()
        .find(|(v, _)| (v - settings.sync_quantum_us).abs() < 1e-9)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("{} µs", settings.sync_quantum_us));
    ui.separator();
    caption(ui, "Step (pauses first)");
    ui.horizontal(|ui| {
        if Btn::mini(&format!("Step {q_label}"))
            .icon(Icon::Play)
            .show(ui)
            .on_hover_text("Run one sync quantum, then pause")
            .clicked()
        {
            action = Some(SimAction::Step(crate::router::StepSize::Quantum));
        }
        if Btn::mini("Step 1 ms")
            .icon(Icon::Play)
            .show(ui)
            .on_hover_text("Run one millisecond, then pause")
            .clicked()
        {
            action = Some(SimAction::Step(crate::router::StepSize::Millisecond));
        }
    });
    ui.separator();
    caption(ui, "Reset");
    if widgets::menu_item(ui, "Reset bench", false, true) {
        action = Some(SimAction::Reset);
    }
    let targets = app.bench_link().targets();
    for t in &targets {
        if widgets::menu_item(ui, &format!("Reset MCU only · {}", t.id), false, true) {
            action = Some(SimAction::ResetMcu(t.id.clone()));
        }
    }
    ui.separator();
    caption(ui, "Determinism");
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Seed")
                .font(font::ui(12.5))
                .color(p.text_dim),
        );
        // the value being edited lives in temp memory until the drag / typing ends
        let id = egui::Id::unique("wb-seed-edit");
        let mut seed = ui.data(|d| d.get_temp::<u64>(id)).unwrap_or(settings.seed);
        let r = ui.add(egui::DragValue::new(&mut seed).range(0..=u32::MAX as u64));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Seed value"));
        ui.data_mut(|d| d.insert_temp(id, seed));
        let done =
            r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus());
        if done {
            ui.data_mut(|d| d.remove::<u64>(id));
            if seed != settings.seed {
                let mut st = settings.clone();
                st.seed = seed;
                action = Some(SimAction::Settings(st));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Sync quantum")
                .font(font::ui(12.5))
                .color(p.text_dim),
        );
        // index into QUANTA_US (a custom value from the project file shows none selected)
        let cur = QUANTA_US
            .iter()
            .position(|(v, _)| (v - settings.sync_quantum_us).abs() < 1e-9)
            .unwrap_or(usize::MAX);
        let mut pick = cur;
        let segs: Vec<Seg<usize>> = QUANTA_US
            .iter()
            .enumerate()
            .map(|(i, (_, l))| Seg::new(i, l))
            .collect();
        if widgets::mini_segmented(ui, "Sync quantum", &mut pick, &segs) && pick != cur {
            let mut st = settings.clone();
            st.sync_quantum_us = QUANTA_US[pick].0;
            action = Some(SimAction::Settings(st));
        }
    });
    ui.label(
        egui::RichText::new(
            "Changing the seed or quantum restarts the bench; the same seed replays the same run.",
        )
        .font(font::ui(11.0))
        .color(p.text_faint),
    );
    action
}

/// Speeds of the header menu.
pub const SPEEDS: [(&str, SimSpeed); 6] = [
    ("0.1×", SimSpeed::RealTime(0.1)),
    ("0.5×", SimSpeed::RealTime(0.5)),
    ("1× real-time", SimSpeed::RealTime(1.0)),
    ("2×", SimSpeed::RealTime(2.0)),
    ("10×", SimSpeed::RealTime(10.0)),
    ("Max", SimSpeed::Max),
];

fn speed_label(s: SimSpeed) -> &'static str {
    SPEEDS
        .iter()
        .find(|(_, v)| *v == s)
        .map(|(l, _)| *l)
        .unwrap_or("1× real-time")
}

/// Persisted per-screen state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkbenchState {
    pub view: BenchView,
    pub simple: bool,
    pub panels: Vec<RailPanel>,
    pub right: PanelMode,
    pub parts: PanelMode,
    pub dock_tab: dock::DockTab,
    pub annotations: bool,
    pub autoscroll: bool,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        WorkbenchState {
            view: BenchView::Breadboard,
            simple: true,
            panels: vec![RailPanel::World, RailPanel::Inspector],
            right: PanelMode::Open,
            parts: PanelMode::Open,
            dock_tab: dock::DockTab::Serial,
            annotations: true,
            autoscroll: true,
        }
    }
}

pub struct WorkbenchScreen {
    pub state: WorkbenchState,
    pub canvas: BenchCanvas,
    pub schematic: SchematicCanvas,
    palette: palette::Palette,
    panels: panels::Panels,
    dock: dock::Dock,
    share: share::ShareState,
    erc_open: bool,
    /// Project generation the screen last set up for.
    seen_project: u64,
    /// Name typed into the schematic's net-label popup.
    label_name: String,
}

impl Default for WorkbenchScreen {
    fn default() -> Self {
        WorkbenchScreen {
            state: WorkbenchState::default(),
            canvas: BenchCanvas::default(),
            schematic: SchematicCanvas::default(),
            palette: palette::Palette::default(),
            panels: panels::Panels::default(),
            dock: dock::Dock::default(),
            share: share::ShareState::default(),
            erc_open: false,
            seen_project: u64::MAX,
            label_name: String::new(),
        }
    }
}

impl WorkbenchScreen {
    /// "Bench · BB1 + U1 + pot 1 — drag wires from any pin".
    fn bench_title(app: &AppCtx) -> String {
        let Some(doc) = &app.circuit else {
            return "Bench".into();
        };
        let mut items: Vec<(u8, String)> = Vec::new();
        for p in &doc.project().parts {
            let Some(def) = doc.lib.get(&p.part) else {
                continue;
            };
            if def.is_breadboard() {
                items.push((0, p.reference.clone()));
            } else if def.is_mcu() {
                items.push((1, p.reference.clone()));
            } else if def.bench.art == "pot_soil" {
                items.push((2, p.reference.to_ascii_lowercase().replace("pot", "pot ")));
            }
        }
        items.sort_by_key(|(k, _)| *k);
        let items: Vec<String> = items.into_iter().map(|(_, s)| s).collect();
        if items.is_empty() {
            "Bench — drag parts from the palette".into()
        } else {
            format!("Bench · {} — drag wires from any pin", items.join(" + "))
        }
    }

    fn sheet_title(app: &AppCtx) -> String {
        let name = app
            .project
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "untitled".into());
        format!("Sheet 1 · {name}.sch — same netlist as the bench")
    }

    /// The 40 px strip above the canvas (`WB-12`).
    fn status_strip(&mut self, ui: &mut Ui, app: &mut AppCtx, rect: Rect) {
        let p = pal(ui);
        ui.painter().rect_filled(rect, 0.0, p.bg_header);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        let title = match self.state.view {
            BenchView::Breadboard => Self::bench_title(app),
            BenchView::Schematic => Self::sheet_title(app),
        };
        let mut child = widgets::child_ui(ui, rect.shrink2(vec2(14.0, 0.0)));
        child.horizontal_centered(|ui| {
            ui.label(
                egui::RichText::new(&title)
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                // zoom (+ the symbol standard on the schematic)
                if self.state.view == BenchView::Schematic
                    && let Some(doc) = app.circuit.as_mut()
                {
                    crate::screens::pro::schematic::net_label_control(
                        ui,
                        &mut self.schematic,
                        doc,
                        &mut self.label_name,
                    );
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
                let zoom = match self.state.view {
                    BenchView::Breadboard => self.canvas.view.zoom_pct(),
                    BenchView::Schematic => self.schematic.zoom_pct(),
                };
                let zr = ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{zoom:.0}%"))
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                    )
                    .sense(Sense::click()),
                );
                zr.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Zoom {zoom:.0}%"))
                });
                egui::Popup::menu(&zr).show(|ui| {
                    ui.set_min_width(140.0);
                    for (label, f) in [("Zoom in", 1.25f32), ("Zoom out", 0.8)] {
                        if widgets::menu_item(ui, label, false, true) {
                            match self.state.view {
                                BenchView::Breadboard => self.canvas.zoom(f),
                                BenchView::Schematic => self.schematic.zoom(f),
                            }
                        }
                    }
                    if widgets::menu_item(ui, "Fit to view", false, true) {
                        self.canvas.fit();
                        self.schematic.fit();
                    }
                });
                // ERC / DRC (click → list)
                let (erc, drc, items) = match app.circuit.as_mut() {
                    Some(doc) => {
                        let c = doc.checks();
                        let (e, d) = c.summary();
                        let all: Vec<_> = c.erc.iter().chain(c.drc.iter()).cloned().collect();
                        (e, d, all)
                    }
                    None => ("ERC —".into(), "DRC —".into(), Vec::new()),
                };
                let text = format!("{erc} · {drc}");
                let warn = !items.is_empty();
                let er = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&text)
                            .font(font::mono(12.0))
                            .color(if warn { p.accent_soft } else { p.text_muted }),
                    )
                    .sense(Sense::click()),
                );
                er.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
                if er.clicked() {
                    self.erc_open = !self.erc_open;
                }
                if self.erc_open {
                    let mut select: Option<Vec<String>> = None;
                    let area = egui::Area::new(egui::Id::unique("erc-list"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(er.rect.left_bottom() + vec2(-260.0, 8.0))
                        .show(ui.ctx(), |ui| {
                            widgets::card_frame(p).show(ui, |ui| {
                                ui.set_width(420.0);
                                widgets::section_header(ui, "Electrical & design rules");
                                ui.add_space(6.0);
                                if items.is_empty() {
                                    ui.label(
                                        egui::RichText::new("No findings — the circuit is clean.")
                                            .font(font::ui(12.0))
                                            .color(p.text_muted),
                                    );
                                }
                                for it in &items {
                                    let col = match it.severity {
                                        bb_circuit::ErcSeverity::Error => p.danger_soft,
                                        bb_circuit::ErcSeverity::Warning => p.accent,
                                    };
                                    let r = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("● {}", it.message))
                                                .font(font::ui(12.0))
                                                .color(col),
                                        )
                                        .wrap()
                                        .sense(Sense::click()),
                                    );
                                    if r.clicked() {
                                        select = Some(it.parts.clone());
                                    }
                                }
                            });
                        });
                    if let Some(parts) = select {
                        self.canvas.selection.clear();
                        self.canvas.selection.parts.extend(parts);
                        self.state.panels.retain(|p| *p != RailPanel::Inspector);
                        self.state.panels.push(RailPanel::Inspector);
                        self.erc_open = false;
                    }
                    if ui.input(|i| i.pointer.any_click())
                        && !area.response.contains_pointer()
                        && !er.contains_pointer()
                    {
                        self.erc_open = false;
                    }
                }
                if let Some(s) = app.bench.sim()
                    && let Some(e) = s.build_error()
                {
                    ui.label(
                        egui::RichText::new(format!("Bench: {e}"))
                            .font(font::ui(12.0))
                            .color(p.danger_soft),
                    );
                }
                if let Some(id) = &self.canvas.placing {
                    let name = app
                        .circuit
                        .as_ref()
                        .and_then(|d| d.lib.get(id).map(|d| d.name.clone()))
                        .unwrap_or_default();
                    ui.label(
                        egui::RichText::new(format!(
                            "Click on the bench to place {name} · Esc cancels"
                        ))
                        .font(font::ui(12.0))
                        .color(p.accent_soft),
                    );
                }
                if self.canvas.tool == crate::canvas::bench::Tool::Probe {
                    ui.label(
                        egui::RichText::new(
                            "Probe tool: click a pin to pin a live label · Esc cancels",
                        )
                        .font(font::ui(12.0))
                        .color(p.accent_soft),
                    );
                }
            });
        });
    }

    /// The right rail (icon buttons + collapse handles).
    fn rail(&mut self, ui: &mut Ui, rect: Rect) {
        let p = pal(ui);
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(14, 14, 14, 235));
        ui.painter().line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(1.0, p.border),
        );
        let mut child = widgets::child_ui(ui, rect.shrink2(vec2(8.0, 10.0)));
        child.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let open = self.state.right == PanelMode::Open;
            let (icon, label) = if open {
                (Icon::CollapseRight, "Hide panels")
            } else {
                (Icon::ExpandLeft, "Show panels")
            };
            if widgets::rail_button_sized(ui, icon, label, false, 40.0, 16.0).clicked() {
                self.state.right = if open {
                    PanelMode::Collapsed
                } else {
                    PanelMode::Open
                };
            }
            let (r, _) = ui.allocate_exact_size(vec2(28.0, 1.0), Sense::hover());
            ui.painter().rect_filled(r, 0.0, p.border_strong);
            for rp in RailPanel::ALL {
                let on = self.state.panels.contains(&rp) && open;
                if widgets::rail_button(ui, rp.icon(), rp.label(), on).clicked() {
                    if !open {
                        self.state.right = PanelMode::Open;
                        self.state.panels.retain(|x| *x != rp);
                        self.state.panels.insert(0, rp);
                    } else if on {
                        self.state.panels.retain(|x| *x != rp);
                    } else {
                        self.state.panels.insert(0, rp);
                    }
                }
            }
            // radio tools: virtual phone + MQTT explorer
            crate::widgets::radio_tools::rail_buttons(ui);
        });
    }

    fn apply_output(&mut self, app: &mut AppCtx, out: BenchOutput) {
        if out.selection_changed
            && self.canvas.selection.single_part().is_some()
            && !self.state.panels.contains(&RailPanel::Inspector)
        {
            self.state.panels.push(RailPanel::Inspector);
        }
        if out.sim.is_empty() && !out.live_edit {
            return;
        }
        let live = out.live_edit;
        if let Some(sim) = app.sim() {
            for c in out.sim {
                sim.send(c);
            }
        }
        if live {
            app.mark_bench_synced();
        }
    }
}

impl Screen for WorkbenchScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        None
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.add_space(12.0);
        let mut v = self.state.view;
        if widgets::segmented(
            ui,
            "View",
            &mut v,
            &[
                Seg::new(BenchView::Breadboard, "Breadboard").icon(Icon::SegBreadboard),
                Seg::new(BenchView::Schematic, "Schematic").icon(Icon::SegSchematic),
            ],
        ) {
            self.state.view = v;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // Share
            let share = Btn::ghost("Share").show(ui);
            self.share.menu(&share, app);
            ui.add_space(8.0);
            // status
            let tel = app.bench.sim().and_then(|s| s.telemetry());
            let (label, color) = match tel.as_ref().map(|t| &t.status) {
                Some(bb_sim::RunStatus::Running) => ("running", p.ok),
                Some(bb_sim::RunStatus::Error(_)) => ("error", p.danger),
                Some(bb_sim::RunStatus::Paused) => ("paused", p.text_muted),
                _ => ("stopped", p.text_faint),
            };
            let label = if app.bench.running { "running" } else { label };
            let color = if app.bench.running { p.ok } else { color };
            widgets::status_dot(ui, color, label, p.text_muted);
            // speed menu
            let sp = speed_label(app.bench.speed);
            let sr = ui.add(
                egui::Label::new(
                    egui::RichText::new(sp)
                        .font(font::mono(12.0))
                        .color(p.text_muted),
                )
                .sense(Sense::click()),
            );
            sr.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::ComboBox, true, format!("Speed: {sp}"))
            });
            let mut chosen = None;
            let mut action = None;
            egui::Popup::menu(&sr)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(220.0);
                    action = sim_menu(ui, app, &mut chosen);
                });
            if let Some(s) = chosen {
                app.bench.set_speed(s);
                egui::Popup::close_all(ui.ctx());
            }
            match action {
                Some(SimAction::Step(size)) => app.step_sim(size),
                Some(SimAction::Reset) => app.command(Command::ResetSim),
                Some(SimAction::ResetMcu(t)) => app.reset_mcu(Some(&t)),
                Some(SimAction::Settings(st)) => app.apply_sim_settings(st),
                None => {}
            }
            // clock
            let t = app.bench.sim_time;
            let clock = clock_label(t);
            let g1 = widgets::galley(ui, "t = ", font::mono(12.0), p.text_muted);
            let g2 = widgets::galley(ui, &clock, font::mono(12.0), p.text);
            let (r, resp) =
                ui.allocate_exact_size(vec2(g1.size().x + g2.size().x, 20.0), Sense::hover());
            ui.painter().galley(
                pos2(r.left(), r.center().y - g1.size().y / 2.0),
                g1.clone(),
                p.text_muted,
            );
            ui.painter().galley(
                pos2(r.left() + g1.size().x, r.center().y - g2.size().y / 2.0),
                g2,
                p.text,
            );
            let cl = format!("t = {clock}");
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &cl));
            ui.add_space(4.0);
            // Run / Pause
            let running = app.bench.running;
            let (icon, text) = if running {
                (Icon::Pause, "Pause")
            } else {
                (Icon::Play, "Run")
            };
            if Btn::amber(text).icon(icon).show(ui).clicked() {
                app.bench_link().set_running(!running);
                app.bench.running = !running;
            }
        });
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        if app.project.is_none() {
            return;
        }
        if self.seen_project != app.project_generation {
            self.seen_project = app.project_generation;
            self.canvas = BenchCanvas::default();
            self.schematic = SchematicCanvas::default();
            self.dock.reset();
        }
        // make sure the bench exists (paused) so the canvas shows live state
        let _ = app.sim();
        let p = pal(ui);
        // bottom dock (full width)
        egui::Panel::bottom("wb-dock")
            .exact_size(224.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(Color32::from_rgba_unmultiplied(6, 6, 6, 235)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter()
                    .line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, p.border));
                self.dock.ui(ui, app, &mut self.state);
            });
        // parts palette
        let mut parts_mode = self.state.parts;
        widgets::collapsible_side(
            ui,
            "wb-parts",
            widgets::Side::Left,
            "Parts",
            &mut parts_mode,
            theme::size::PARTS_PANEL,
            |ui, mode| {
                self.palette
                    .ui(ui, app, &mut self.state.simple, &mut self.canvas, mode);
            },
        );
        self.state.parts = parts_mode;
        // right panels
        if self.state.right == PanelMode::Open {
            egui::Panel::right("wb-right")
                .exact_size(theme::size::SIDE_RAIL)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(p.bg_panel))
                .show(ui, |ui| {
                    let r = ui.max_rect();
                    ui.painter()
                        .line_segment([r.left_top(), r.left_bottom()], Stroke::new(1.0, p.border));
                    self.panels.ui(ui, app, &mut self.state, &mut self.canvas);
                });
        }
        // centre: status strip, canvas, rail
        let full = ui.available_rect_before_wrap();
        let strip = Rect::from_min_size(full.min, vec2(full.width(), 40.0));
        let rail = Rect::from_min_max(pos2(full.right() - 56.0, strip.bottom()), full.max);
        let canvas = Rect::from_min_max(
            pos2(full.left(), strip.bottom()),
            pos2(rail.left(), full.bottom()),
        );
        self.share.bench_rect = (self.state.view == BenchView::Breadboard).then_some(canvas);
        self.status_strip(ui, app, strip);
        self.rail(ui, rail);
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        let gen_ = app.project_generation;
        match self.state.view {
            BenchView::Breadboard => {
                let out = match app.circuit.as_mut() {
                    Some(doc) => self.canvas.show(ui, canvas, doc, tel.as_deref(), gen_),
                    None => BenchOutput::default(),
                };
                self.apply_output(app, out);
            }
            BenchView::Schematic => {
                if let Some(doc) = app.circuit.as_mut() {
                    let sel = self
                        .schematic
                        .show(ui, canvas, doc, tel.as_deref(), gen_, app.theme);
                    if let Some(id) = sel {
                        self.canvas.selection.clear();
                        self.canvas.selection.parts.insert(id);
                    }
                }
            }
        }
        if let Some(doc) = app.circuit.as_ref()
            && doc.project().parts.is_empty()
            && self.state.view == BenchView::Breadboard
        {
            ui.painter().text(
                canvas.center(),
                Align2::CENTER_CENTER,
                "Drag a board and a breadboard from the palette to start",
                font::ui(14.0),
                p.text_faint,
            );
        }
        self.share.poll(app);
        ui.ctx().memory_mut(|_| {});
    }

    fn command(&mut self, cmd: &Command, app: &mut AppCtx) -> bool {
        match cmd {
            Command::Undo | Command::Redo => {
                if app.text_has_focus() {
                    return false;
                }
                let Some(doc) = app.circuit.as_mut() else {
                    return false;
                };
                let ok = if *cmd == Command::Undo {
                    doc.undo()
                } else {
                    doc.redo()
                };
                if !ok {
                    app.toasts.info(if *cmd == Command::Undo {
                        "Nothing to undo"
                    } else {
                        "Nothing to redo"
                    });
                }
                // drop selections of parts that no longer exist
                let ids: std::collections::HashSet<String> =
                    doc.project().parts.iter().map(|p| p.id.clone()).collect();
                self.canvas.selection.parts.retain(|p| ids.contains(p));
                let wires: std::collections::HashSet<String> =
                    doc.project().wires.iter().map(|w| w.id.clone()).collect();
                self.canvas.selection.wires.retain(|w| wires.contains(w));
                true
            }
            Command::Find => {
                self.palette.focus_search = true;
                true
            }
            _ => false,
        }
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn save_state(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.state).ok()
    }

    fn load_state(&mut self, state: &serde_json::Value) {
        if let Ok(s) = serde_json::from_value(state.clone()) {
            self.state = s;
        }
    }
}
