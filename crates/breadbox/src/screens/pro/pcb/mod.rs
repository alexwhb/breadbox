//! Pro → PCB (`PRO-7`): the project's footprints on a two-layer board with the ratsnest from
//! the circuit, placement (drag, R rotates, Auto-place), manual routing (click a pad, click
//! corners, click the target pad; V drops a via and switches layer; widths; Top / Bottom),
//! vias, the live design-rule check (clearance, width, drill, edge, shorts, unrouted) and
//! Gerber RS-274X + Excellon export. Everything is stored in the project (`PartInstance::pcb`,
//! `Project::pcb`) through the shared `CircuitDoc`, with its own undo stack.

use crate::app::AppCtx;
use crate::router::{Command, Screen};
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon, Seg};
use bb_circuit::geometry::Placement;
use bb_circuit::pcb::drc::{self, DrcIssue, DrcKind};
use bb_circuit::pcb::{self, Board, Copper, ItemKind, Layer, Trace, Via, footprint::PadShape};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

/// The project's fabrication files as a zip (Share → "PCB Gerbers + drill"); footprints that
/// were never placed get the auto-placement first (in the copy).
pub fn gerber_zip(
    project: &bb_circuit::Project,
    lib: &bb_circuit::Library,
    name: &str,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Write as _;
    let mut p = project.clone();
    pcb::auto_place(&mut p, lib, false);
    let board = Board::load(&p);
    let parts = pcb::placed_parts(&p, lib, false);
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (file, text) in pcb::gerber::export(&stem, &board, &parts) {
        zip.start_file(file, opts)?;
        zip.write_all(text.as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

/// What a click on the board does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PcbTool {
    #[default]
    Select,
    Route,
    Via,
}

/// px per mm + screen position of board (0, 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PcbView {
    pub scale: f32,
    pub origin: Pos2,
}

impl PcbView {
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        pos2(
            self.origin.x + x as f32 * self.scale,
            self.origin.y + y as f32 * self.scale,
        )
    }
    pub fn to_board(&self, p: Pos2) -> (f64, f64) {
        (
            ((p.x - self.origin.x) / self.scale) as f64,
            ((p.y - self.origin.y) / self.scale) as f64,
        )
    }
}

/// The egui temp-data key of the PCB view (tests map pads to screen points).
pub fn view_id() -> Id {
    Id::unique("pcb-view")
}

#[cfg(test)]
mod tests {
    #[test]
    fn share_zip_holds_the_seven_files() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/blink");
        let p = bb_circuit::Project::load(&dir).unwrap();
        let lib = bb_circuit::Library::builtin();
        let z = super::gerber_zip(&p, &lib, "blink").unwrap();
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(z)).unwrap();
        assert_eq!(a.len(), 7);
        let mut names: Vec<String> = (0..a.len())
            .map(|i| a.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        names.sort();
        assert!(
            names.contains(&"blink-F_Cu.gbr".to_string())
                && names.contains(&"blink.drl".to_string())
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Sel {
    Part(String),
    Trace(String),
    Via(String),
}

type Snapshot = (
    Vec<(String, Option<Placement>)>,
    bb_circuit::project::PcbLayout,
);

pub struct PcbScreen {
    pub tool: PcbTool,
    pub layer: Layer,
    pub width: f64,
    view: Option<PcbView>,
    fitted: Option<u64>,
    sel: Option<Sel>,
    /// Trace being routed: layer, width, points.
    routing: Option<Trace>,
    drag: Option<(String, (f64, f64), Placement)>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    issues: Vec<DrcIssue>,
    issues_gen: Option<u64>,
    pub last_export: Option<std::path::PathBuf>,
}

impl Default for PcbScreen {
    fn default() -> Self {
        PcbScreen {
            tool: PcbTool::Select,
            layer: Layer::Top,
            width: 0.4,
            view: None,
            fitted: None,
            sel: None,
            routing: None,
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            issues: Vec::new(),
            issues_gen: None,
            last_export: None,
        }
    }
}

const COPPER_TOP: Color32 = Color32::from_rgb(0xD0, 0x5A, 0x3C);
const COPPER_BOT: Color32 = Color32::from_rgb(0x3D, 0x7B, 0xE0);
const PAD: Color32 = Color32::from_rgb(0xC9, 0xA5, 0x4A);
const SILK: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xE0);
const MASK: Color32 = Color32::from_rgb(0x0F, 0x3A, 0x26);

fn snapshot(p: &bb_circuit::Project) -> Snapshot {
    (
        p.parts.iter().map(|x| (x.id.clone(), x.pcb)).collect(),
        p.pcb.clone(),
    )
}

fn restore(p: &mut bb_circuit::Project, s: &Snapshot) {
    for (id, at) in &s.0 {
        if let Some(x) = p.part_mut(id) {
            x.pcb = *at;
        }
    }
    p.pcb = s.1.clone();
}

impl PcbScreen {
    fn edit(
        &mut self,
        app: &mut AppCtx,
        f: impl FnOnce(&mut bb_circuit::Project, &bb_circuit::Library),
    ) {
        let Some(doc) = app.circuit.as_mut() else {
            return;
        };
        let lib = doc.lib.clone();
        self.undo.push(snapshot(&doc.editor.project));
        self.redo.clear();
        f(&mut doc.editor.project, &lib);
        doc.editor.generation += 1;
        doc.touched();
    }

    /// Places every footprint (re-places all with `all`).
    pub fn auto_place(&mut self, app: &mut AppCtx, all: bool) {
        self.edit(app, |p, lib| {
            pcb::auto_place(p, lib, all);
        });
        self.fitted = None;
    }

    fn board(app: &AppCtx) -> Option<Board> {
        app.circuit.as_ref().map(|d| Board::load(d.project()))
    }

    fn copper(app: &mut AppCtx) -> Option<(Board, Copper)> {
        let doc = app.circuit.as_mut()?;
        let conn = doc.connectivity();
        let board = Board::load(doc.project());
        let cu = Copper::build(doc.project(), &doc.lib, &board, &conn);
        Some((board, cu))
    }

    /// Writes Gerbers + drill into `dir`; returns the files.
    pub fn export_to(
        &mut self,
        app: &mut AppCtx,
        dir: &std::path::Path,
    ) -> anyhow::Result<Vec<std::path::PathBuf>> {
        let doc = app
            .circuit
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no project"))?;
        let board = Board::load(doc.project());
        let parts = pcb::placed_parts(doc.project(), &doc.lib, false);
        let name: String = app
            .project
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "board".into())
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        std::fs::create_dir_all(dir)?;
        let mut out = Vec::new();
        for (file, text) in pcb::gerber::export(&name, &board, &parts) {
            let path = dir.join(file);
            std::fs::write(&path, text)?;
            out.push(path);
        }
        self.last_export = Some(dir.to_path_buf());
        Ok(out)
    }

    fn side(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.spacing_mut().item_spacing.y = 12.0;
        widgets::section_header(ui, "Tools");
        ui.horizontal(|ui| {
            for (t, label, icon, tip) in [
                (
                    PcbTool::Select,
                    "Select",
                    Icon::Inspector,
                    "Drag footprints; R rotates, ⌫ deletes a trace or via",
                ),
                (
                    PcbTool::Route,
                    "Route",
                    Icon::Wire,
                    "Click a pad, the corners, then the target pad; V adds a via, Esc cancels",
                ),
                (
                    PcbTool::Via,
                    "Via",
                    Icon::Plus,
                    "Click the board to drop a via",
                ),
            ] {
                if Btn::ghost(label)
                    .icon(icon)
                    .selected(self.tool == t)
                    .tooltip(tip)
                    .show(ui)
                    .clicked()
                {
                    self.tool = t;
                    self.routing = None;
                }
            }
        });
        let mut layer = self.layer;
        if widgets::segmented(
            ui,
            "Layer",
            &mut layer,
            &[
                Seg::new(Layer::Top, "Top copper"),
                Seg::new(Layer::Bottom, "Bottom copper"),
            ],
        ) {
            self.layer = layer;
            if let Some(r) = self.routing.as_mut() {
                r.layer = layer;
            }
        }
        ui.label(
            egui::RichText::new("Trace width")
                .font(font::ui(12.0))
                .color(p.text_muted),
        );
        let mut w = (self.width * 100.0).round() as i32;
        widgets::mini_segmented(
            ui,
            "Trace width",
            &mut w,
            &[
                Seg::new(25, "0.25"),
                Seg::new(40, "0.40"),
                Seg::new(60, "0.60"),
                Seg::new(100, "1.00 mm"),
            ],
        );
        self.width = w as f64 / 100.0;
        widgets::section_header(ui, "Board");
        ui.horizontal(|ui| {
            if Btn::ghost("Auto-place")
                .tooltip("Re-place every footprint in rows and size the outline around them")
                .show(ui)
                .clicked()
            {
                self.auto_place(app, true);
            }
            if Btn::ghost("Fit outline")
                .tooltip("Size the outline around the footprints, traces and vias")
                .show(ui)
                .clicked()
            {
                self.edit(app, pcb::fit_outline);
            }
        });
        if let Some(b) = Self::board(app).and_then(|b| b.outline) {
            let t = format!(
                "Outline {:.1} × {:.1} mm · 2 layers · 1.6 mm FR-4",
                b.2 - b.0,
                b.3 - b.1
            );
            let r = ui.label(
                egui::RichText::new(&t)
                    .font(font::mono(11.5))
                    .color(p.text_dim),
            );
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &t));
        }
        widgets::section_header(ui, "Design rules");
        let s = drc::summary(&self.issues);
        let col = if self.issues.is_empty() {
            p.ok
        } else {
            p.accent_soft
        };
        let r = ui.label(
            egui::RichText::new(&s)
                .font(font::mono_medium(12.5))
                .color(col),
        );
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &s));
        ui.label(
            egui::RichText::new("clearance 0.20 · width ≥ 0.20 · drill ≥ 0.30 · edge 0.30 mm")
                .font(font::mono(10.5))
                .color(p.text_faint),
        );
        egui::ScrollArea::vertical()
            .id_salt("pcb-drc")
            .max_height(260.0)
            .show(ui, |ui| {
                for i in self.issues.iter().take(60) {
                    let c = match i.kind {
                        DrcKind::Unrouted => p.accent_soft,
                        _ => p.danger_soft,
                    };
                    let r = ui.label(
                        egui::RichText::new(format!("● {}", i.message))
                            .font(font::ui(11.5))
                            .color(c),
                    );
                    let m = i.message.clone();
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &m));
                }
            });
        widgets::section_header(ui, "Fabrication");
        if Btn::amber("Export Gerbers…")
            .icon(Icon::Download)
            .tooltip(
                "Gerber RS-274X copper, mask, silkscreen and outline, plus an Excellon drill file",
            )
            .show(ui)
            .clicked()
        {
            let dir = std::env::var_os("BREADBOX_EXPORT_DIR")
                .map(std::path::PathBuf::from)
                .map(|d| d.join("gerbers"))
                .or_else(|| crate::platform::pick_folder("Export Gerbers + drill into…"));
            if let Some(d) = dir {
                match self.export_to(app, &d) {
                    Ok(files) => app.toasts.success(format!(
                        "Wrote {} fabrication files to {}",
                        files.len(),
                        d.display()
                    )),
                    Err(e) => app.toasts.error(format!("{e:#}")),
                }
            }
        }
    }

    fn canvas(&mut self, ui: &mut Ui, app: &mut AppCtx, rect: Rect) {
        let p = pal(ui);
        crate::theme::paint_dots(ui.painter(), rect, p);
        let Some((board, cu)) = Self::copper(app) else {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No project open",
                font::ui(14.0),
                p.text_faint,
            );
            return;
        };
        // DRC once per revision
        let generation = app.circuit.as_ref().map(|d| d.generation()).unwrap_or(0);
        if self.issues_gen != Some(generation) {
            self.issues = drc::check(&board, &cu);
            self.issues_gen = Some(generation);
        }
        let outline = board.outline.unwrap_or((0.0, 0.0, 100.0, 80.0));
        if self.fitted != Some(app.project_generation) || self.view.is_none() {
            let (w, h) = (
                (outline.2 - outline.0) as f32,
                (outline.3 - outline.1) as f32,
            );
            let scale = ((rect.width() - 80.0) / w.max(1.0))
                .min((rect.height() - 80.0) / h.max(1.0))
                .clamp(1.0, 40.0);
            let c = rect.center();
            self.view = Some(PcbView {
                scale,
                origin: pos2(
                    c.x - (outline.0 as f32 + w / 2.0) * scale,
                    c.y - (outline.1 as f32 + h / 2.0) * scale,
                ),
            });
            self.fitted = Some(app.project_generation);
        }
        let mut view = self.view.unwrap_or(PcbView {
            scale: 6.0,
            origin: rect.min,
        });
        let resp = ui.interact(rect, Id::unique("pcb-canvas"), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "PCB canvas"));
        // pan / zoom
        if resp.contains_pointer() {
            let (scroll, zoom, cmd, hover) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers.command,
                    i.pointer.hover_pos(),
                )
            });
            let at = hover.unwrap_or(rect.center());
            let z = if zoom != 1.0 {
                zoom
            } else if cmd && scroll.y != 0.0 {
                (scroll.y * 0.004).exp()
            } else {
                1.0
            };
            if z != 1.0 {
                let before = view.to_board(at);
                view.scale = (view.scale * z).clamp(0.8, 80.0);
                let after = view.to_screen(before.0, before.1);
                view.origin += at - after;
            } else if scroll != egui::Vec2::ZERO {
                view.origin += scroll;
            }
        }
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let world = pointer.map(|q| view.to_board(q));
        let parts = app
            .circuit
            .as_ref()
            .map(|d| pcb::placed_parts(d.project(), &d.lib, false))
            .unwrap_or_default();
        // ---- input
        let keys = !ui.ctx().egui_wants_keyboard_input();
        if keys && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.routing = None;
            self.sel = None;
        }
        match self.tool {
            PcbTool::Select => {
                if resp.drag_started()
                    && let Some((x, y)) = ui
                        .input(|i| i.pointer.press_origin())
                        .map(|q| view.to_board(q))
                    && let Some(pl) = parts.iter().rev().find(|pl| {
                        let b = pl.bounds();
                        x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3
                    })
                {
                    self.sel = Some(Sel::Part(pl.part.clone()));
                    self.drag = Some((pl.part.clone(), (x, y), pl.at));
                }
                if resp.dragged()
                    && let (Some((id, start, at0)), Some((x, y))) = (self.drag.clone(), world)
                {
                    let snap = 0.635;
                    let nx = ((at0.x + x - start.0) / snap).round() * snap;
                    let ny = ((at0.y + y - start.1) / snap).round() * snap;
                    if let Some(doc) = app.circuit.as_mut()
                        && let Some(inst) = doc.editor.project.part_mut(&id)
                    {
                        let mut a = at0;
                        a.x = nx;
                        a.y = ny;
                        inst.pcb = Some(a);
                    }
                }
                if resp.drag_stopped()
                    && let Some((id, _, at0)) = self.drag.take()
                    && let Some(doc) = app.circuit.as_mut()
                {
                    let now = doc.project().part(&id).and_then(|x| x.pcb);
                    if now != Some(at0) {
                        let mut snap = snapshot(&doc.editor.project);
                        if let Some(e) = snap.0.iter_mut().find(|e| e.0 == id) {
                            e.1 = Some(at0);
                        }
                        self.undo.push(snap);
                        self.redo.clear();
                        doc.editor.generation += 1;
                        doc.touched();
                    }
                }
                if resp.clicked()
                    && let Some((x, y)) = world
                {
                    self.sel = hit(&board, &cu, &parts, x, y, view.scale);
                }
                if keys && let Some(sel) = self.sel.clone() {
                    if ui.input(|i| i.key_pressed(egui::Key::R))
                        && let Sel::Part(id) = &sel
                    {
                        let id = id.clone();
                        self.edit(app, |p, _| {
                            if let Some(x) = p.part_mut(&id) {
                                let mut a = x.pcb.unwrap_or_default();
                                a.rot = (a.rot + 90.0) % 360.0;
                                x.pcb = Some(a);
                            }
                        });
                    }
                    if ui.input(|i| {
                        i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)
                    }) {
                        match sel {
                            Sel::Trace(id) => self.edit(app, |p, _| {
                                let mut b = Board::load(p);
                                b.traces.retain(|t| t.id != id);
                                b.store(p);
                            }),
                            Sel::Via(id) => self.edit(app, |p, _| {
                                let mut b = Board::load(p);
                                b.vias.retain(|v| v.id != id);
                                b.store(p);
                            }),
                            Sel::Part(_) => {}
                        }
                        self.sel = None;
                    }
                }
            }
            PcbTool::Route => {
                if resp.clicked()
                    && let Some((x, y)) = world
                {
                    let pad = cu.pad_at(x, y);
                    match self.routing.as_mut() {
                        None => {
                            let start = pad.map(|i| (cu.pads[i].x, cu.pads[i].y)).unwrap_or((x, y));
                            self.routing = Some(Trace {
                                id: board.next_id("t"),
                                layer: self.layer,
                                width: self.width,
                                points: vec![start],
                            });
                        }
                        Some(t) => {
                            let end = pad.map(|i| (cu.pads[i].x, cu.pads[i].y)).unwrap_or((x, y));
                            t.points.push(end);
                            if pad.is_some() && t.points.len() >= 2 {
                                let done = self.routing.take().unwrap();
                                self.edit(app, move |p, _| {
                                    let mut b = Board::load(p);
                                    b.traces.push(done);
                                    b.store(p);
                                });
                            }
                        }
                    }
                }
                if resp.double_clicked()
                    && let Some(t) = self.routing.take()
                    && t.points.len() >= 2
                {
                    self.edit(app, move |p, _| {
                        let mut b = Board::load(p);
                        b.traces.push(t);
                        b.store(p);
                    });
                }
                // V: via at the last corner, continue on the other layer
                if keys
                    && ui.input(|i| i.key_pressed(egui::Key::V))
                    && let Some(t) = self.routing.take()
                {
                    let at = *t.points.last().unwrap_or(&(0.0, 0.0));
                    let next_layer = t.layer.other();
                    let width = t.width;
                    let via_id = board.next_id("v");
                    let next_trace = Trace {
                        id: format!("{}b", t.id),
                        layer: next_layer,
                        width,
                        points: vec![at],
                    };
                    self.edit(app, move |p, _| {
                        let mut b = Board::load(p);
                        if t.points.len() >= 2 {
                            b.traces.push(t);
                        }
                        b.vias.push(Via {
                            id: via_id,
                            x: at.0,
                            y: at.1,
                            drill: 0.4,
                            diameter: 0.8,
                        });
                        b.store(p);
                    });
                    self.layer = next_layer;
                    self.routing = Some(next_trace);
                }
            }
            PcbTool::Via => {
                if resp.clicked()
                    && let Some((x, y)) = world
                {
                    let id = board.next_id("v");
                    self.edit(app, move |p, _| {
                        let mut b = Board::load(p);
                        b.vias.push(Via {
                            id,
                            x,
                            y,
                            drill: 0.4,
                            diameter: 0.8,
                        });
                        b.store(p);
                    });
                }
            }
        }
        // re-read after edits
        let Some((board, cu)) = Self::copper(app) else {
            return;
        };
        let parts = app
            .circuit
            .as_ref()
            .map(|d| pcb::placed_parts(d.project(), &d.lib, false))
            .unwrap_or_default();
        // ---- draw
        let painter = ui.painter_at(rect);
        let r_out = Rect::from_two_pos(
            view.to_screen(outline.0, outline.1),
            view.to_screen(outline.2, outline.3),
        );
        painter.rect_filled(
            r_out.translate(vec2(0.0, 6.0)),
            CornerRadius::same(4),
            Color32::from_black_alpha(90),
        );
        painter.rect(
            r_out,
            CornerRadius::same(3),
            MASK,
            Stroke::new(1.5, Color32::from_rgb(0xD6, 0xC0, 0x8A)),
            StrokeKind::Middle,
        );
        let s = view.scale;
        let draw_traces = |layer: Layer, col: Color32, painter: &egui::Painter| {
            for t in board.traces.iter().filter(|t| t.layer == layer) {
                let pts: Vec<Pos2> = t.points.iter().map(|q| view.to_screen(q.0, q.1)).collect();
                let selected = self.sel == Some(Sel::Trace(t.id.clone()));
                let w = (t.width as f32 * s).max(1.0);
                for seg in pts.windows(2) {
                    painter.line_segment(
                        [seg[0], seg[1]],
                        Stroke::new(w, if selected { Color32::WHITE } else { col }),
                    );
                }
                for q in &pts {
                    painter.circle_filled(*q, w / 2.0, if selected { Color32::WHITE } else { col });
                }
            }
        };
        let (front, back) = match self.layer {
            Layer::Top => (
                (Layer::Top, COPPER_TOP),
                (Layer::Bottom, COPPER_BOT.gamma_multiply(0.55)),
            ),
            Layer::Bottom => (
                (Layer::Bottom, COPPER_BOT),
                (Layer::Top, COPPER_TOP.gamma_multiply(0.55)),
            ),
        };
        draw_traces(back.0, back.1, &painter);
        // silkscreen
        for pl in &parts {
            for (a, b) in pl.silk() {
                painter.line_segment(
                    [view.to_screen(a.0, a.1), view.to_screen(b.0, b.1)],
                    Stroke::new((0.15 * s).max(1.0), SILK),
                );
            }
            let bb = pl.bounds();
            painter.text(
                view.to_screen(bb.0 + 0.4, bb.1 + 0.2),
                Align2::LEFT_TOP,
                &pl.reference,
                font::mono_medium((1.2 * s).clamp(7.0, 16.0)),
                SILK,
            );
            if self.sel == Some(Sel::Part(pl.part.clone())) {
                painter.rect_stroke(
                    Rect::from_two_pos(view.to_screen(bb.0, bb.1), view.to_screen(bb.2, bb.3)),
                    CornerRadius::same(2),
                    Stroke::new(1.5, p.accent),
                    StrokeKind::Outside,
                );
            }
        }
        // pads
        for pd in &cu.pads {
            let c = view.to_screen(pd.x, pd.y);
            let size = vec2(pd.w as f32 * s, pd.h as f32 * s);
            match pd.shape {
                PadShape::Rect => {
                    painter.rect_filled(
                        Rect::from_center_size(c, size),
                        CornerRadius::same(1),
                        PAD,
                    );
                }
                PadShape::Round => {
                    painter.circle_filled(c, size.x.max(size.y) / 2.0, PAD);
                }
                PadShape::Oval => {
                    let r = size.x.min(size.y) / 2.0;
                    painter.rect_filled(
                        Rect::from_center_size(c, size),
                        CornerRadius::same(r.min(120.0) as u8),
                        PAD,
                    );
                }
            }
            if let Some(d) = pd.drill {
                painter.circle_filled(c, d as f32 * s / 2.0, Color32::from_rgb(0x10, 0x10, 0x10));
            }
        }
        draw_traces(front.0, front.1, &painter);
        for v in &board.vias {
            let c = view.to_screen(v.x, v.y);
            let selected = self.sel == Some(Sel::Via(v.id.clone()));
            painter.circle_filled(
                c,
                v.diameter as f32 * s / 2.0,
                if selected { Color32::WHITE } else { PAD },
            );
            painter.circle_filled(
                c,
                v.drill as f32 * s / 2.0,
                Color32::from_rgb(0x10, 0x10, 0x10),
            );
        }
        // ratsnest
        for (_, a, b) in cu.ratsnest() {
            let (pa, pb) = (&cu.pads[a], &cu.pads[b]);
            painter.extend(egui::Shape::dashed_line(
                &[view.to_screen(pa.x, pa.y), view.to_screen(pb.x, pb.y)],
                Stroke::new(1.0, Color32::from_rgb(0xF2, 0xD0, 0x3B)),
                4.0,
                3.0,
            ));
        }
        // DRC markers
        for i in self.issues.iter().filter(|i| i.kind != DrcKind::Unrouted) {
            let c = view.to_screen(i.at.0, i.at.1);
            painter.circle_stroke(c, 7.0, Stroke::new(1.5, p.danger_soft));
        }
        // routing preview
        if let Some(t) = &self.routing {
            let col = if t.layer == Layer::Top {
                COPPER_TOP
            } else {
                COPPER_BOT
            };
            let mut pts: Vec<Pos2> = t.points.iter().map(|q| view.to_screen(q.0, q.1)).collect();
            if let Some(h) = pointer {
                pts.push(h);
            }
            for seg in pts.windows(2) {
                painter.line_segment(
                    [seg[0], seg[1]],
                    Stroke::new((t.width as f32 * s).max(1.0), col.gamma_multiply(0.8)),
                );
            }
        }
        // hovered pad: its net
        if let Some((x, y)) = world
            && let Some(i) = cu.pad_at(x, y)
        {
            let pd = &cu.pads[i];
            let net = cu.pad_net[i].clone().unwrap_or_else(|| "no net".into());
            let t = format!("{}.{} · {net}", pd.reference, pd.pin);
            let c = view.to_screen(pd.x, pd.y);
            painter.text(
                c + vec2(10.0, -10.0),
                Align2::LEFT_BOTTOM,
                t,
                font::mono(11.0),
                p.text,
            );
        }
        let _ = ItemKind::Via(String::new());
        self.view = Some(view);
        ui.ctx().data_mut(|d| d.insert_temp(view_id(), view));
    }
}

/// What is under a board point (trace → via → footprint).
fn hit(
    board: &Board,
    cu: &Copper,
    parts: &[pcb::Placed],
    x: f64,
    y: f64,
    scale: f32,
) -> Option<Sel> {
    let tol = (4.0 / scale) as f64;
    for t in &board.traces {
        for w in t.points.windows(2) {
            if pcb::point_seg((x, y), w[0], w[1]) <= t.width / 2.0 + tol {
                return Some(Sel::Trace(t.id.clone()));
            }
        }
    }
    for v in &board.vias {
        if (v.x - x).hypot(v.y - y) <= v.diameter / 2.0 + tol {
            return Some(Sel::Via(v.id.clone()));
        }
    }
    let _ = cu;
    parts
        .iter()
        .rev()
        .find(|pl| {
            let b = pl.bounds();
            x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3
        })
        .map(|pl| Sel::Part(pl.part.clone()))
}

impl Screen for PcbScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some("PCB".into())
    }

    fn header(&mut self, ui: &mut Ui, _app: &mut AppCtx) {
        let p = pal(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let s = drc::summary(&self.issues);
            ui.label(egui::RichText::new(s).font(font::mono(12.0)).color(
                if self.issues.is_empty() {
                    p.ok
                } else {
                    p.accent_soft
                },
            ));
        });
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        // first visit: footprints without a place get one
        if let Some(doc) = app.circuit.as_ref() {
            let unplaced = pcb::placed_parts(doc.project(), &doc.lib, false)
                .iter()
                .any(|pl| {
                    doc.project()
                        .part(&pl.part)
                        .is_some_and(|i| i.pcb.is_none())
                });
            if unplaced {
                self.auto_place(app, false);
                self.undo.clear();
            }
        }
        let full = ui.max_rect();
        let side = Rect::from_min_size(full.min, vec2(300.0, full.height()));
        let canvas = Rect::from_min_max(pos2(side.right(), full.top()), full.max);
        ui.painter().rect_filled(side, 0.0, p.bg_panel);
        ui.painter().line_segment(
            [side.right_top(), side.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        self.canvas(ui, app, canvas);
        let mut c = widgets::child_ui(ui, side.shrink(16.0));
        egui::ScrollArea::vertical()
            .id_salt("pcb-side")
            .auto_shrink([false, false])
            .show(&mut c, |ui| self.side(ui, app));
    }

    fn command(&mut self, cmd: &Command, app: &mut AppCtx) -> bool {
        match cmd {
            Command::Undo | Command::Redo => {
                let Some(doc) = app.circuit.as_mut() else {
                    return false;
                };
                let (from, to) = if *cmd == Command::Undo {
                    (&mut self.undo, &mut self.redo)
                } else {
                    (&mut self.redo, &mut self.undo)
                };
                match from.pop() {
                    Some(s) => {
                        to.push(snapshot(&doc.editor.project));
                        restore(&mut doc.editor.project, &s);
                        doc.editor.generation += 1;
                        doc.touched();
                    }
                    None => app.toasts.info("Nothing to undo on the PCB"),
                }
                true
            }
            _ => false,
        }
    }

    fn project_changed(&mut self, _app: &mut AppCtx) {
        self.undo.clear();
        self.redo.clear();
        self.sel = None;
        self.routing = None;
        self.fitted = None;
        self.issues_gen = None;
    }
}
