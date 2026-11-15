//! The breadboard bench canvas (`WB-3`…`WB-6`, `WB-13`): draws the scene with live state from
//! the simulator and edits the circuit through the shared `CircuitDoc` (every change is an
//! undoable `bb_circuit::edit` transaction).
//!
//! Interaction: drag from any pin or hole to another to add a jumper wire (auto colour, or the
//! picked one); drag a part's body to move it (0.1" snapping); click / ⇧-click / box-drag to
//! select; R rotate, F flip, ⌫ delete, ⌘D duplicate, ⌘C / ⌘V; double-click a wire to add a
//! bend point, drag bend points, double-click one to remove it; drag a pot knob, press
//! buttons, flip switches; probe tool adds live labels; scroll pans, ⌘-scroll / pinch zooms,
//! space- or middle-drag pans. Parts dropped from the palette land with their first lead on
//! the grid.

use super::art::{self, ArtCtx, InstrumentView, Pass};
use super::instruments::InstrumentRegistry;
use super::pen::{BASE_SCALE, Glow, Pen, View, WRect, bezier_distance, draw_glow, rgb, with_alpha};
use super::scene::{Scene, snap_drop};
use crate::circuit::CircuitDoc;
use crate::theme::{self, font, pal};
use bb_circuit::edit::{Op, Transaction};
use bb_circuit::project::{Endpoint, Quantity};
use bb_circuit::{Connectivity, NetId, PropValue};
use bb_core::UserInput;
use bb_sim::{Command, Telemetry};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Stroke, Ui,
    pos2, vec2,
};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Payload of a part dragged from the palette.
#[derive(Debug, Clone, PartialEq)]
pub struct PartDrop(pub String);

/// The bench's selection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub parts: BTreeSet<String>,
    pub wires: BTreeSet<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.wires.is_empty()
    }
    pub fn clear(&mut self) {
        self.parts.clear();
        self.wires.clear();
    }
    pub fn single_part(&self) -> Option<&str> {
        (self.parts.len() == 1 && self.wires.is_empty())
            .then(|| self.parts.iter().next().map(String::as_str))
            .flatten()
    }
    pub fn single_wire(&self) -> Option<&str> {
        (self.wires.len() == 1 && self.parts.is_empty())
            .then(|| self.wires.iter().next().map(String::as_str))
            .flatten()
    }
}

/// What a click does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    /// Click a pin to pin a live probe label to it (`WB-6`).
    Probe,
    /// Click a pin to clip an instrument's probe lead onto it (`"ch1"`, `"plus"`, …).
    Clip { instrument: String, key: String },
}

#[derive(Debug, Clone, PartialEq)]
enum Drag {
    None,
    Pan {
        origin: Pos2,
        start: Pos2,
    },
    Move {
        ids: Vec<String>,
        start: (f64, f64),
        delta: (f64, f64),
    },
    Wire {
        from: Endpoint,
        from_pos: (f64, f64),
        to: (f64, f64),
    },
    Box {
        start: (f64, f64),
        end: (f64, f64),
    },
    Bend {
        wire: String,
        idx: usize,
        pos: (f64, f64),
    },
    Knob {
        part: String,
        start: f64,
        value: f64,
        origin: Pos2,
    },
    Label {
        probe: String,
        pos: (f64, f64),
    },
}

/// Per-frame result for the screen.
#[derive(Debug, Default)]
pub struct BenchOutput {
    /// Commands for the running bench (inputs, live parameters, probes).
    pub sim: Vec<Command>,
    pub selection_changed: bool,
    /// A live property was changed through `SetPartParam` (no rebuild needed).
    pub live_edit: bool,
}

/// The hovered thing (tooltips, net highlight).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoverInfo {
    pub part: Option<String>,
    pub pin: Option<(String, String)>,
    pub wire: Option<String>,
    pub net: Option<NetId>,
}

pub struct BenchCanvas {
    pub view: View,
    /// Generation of the project the view was fitted to.
    fitted: Option<u64>,
    pub selection: Selection,
    pub tool: Tool,
    /// Colour for new wires (`None` = auto: red for +, black for GND).
    pub wire_color: Option<String>,
    /// A palette part waiting to be placed with a click.
    pub placing: Option<String>,
    drag: Drag,
    scene: Option<Arc<Scene>>,
    pressed: Option<String>,
    pub instruments: InstrumentRegistry,
    pub hover: HoverInfo,
    /// Screen rect of the canvas last frame.
    pub rect: Rect,
    /// Fit the view on the next frame.
    pub refit: bool,
    t0: std::time::Instant,
    /// Plugin instruments: probe-lead tip being dragged (`canvas::plugin_parts`).
    pub lead_drag: super::plugin_parts::LeadDrag,
}

impl Default for BenchCanvas {
    fn default() -> Self {
        BenchCanvas {
            view: View::default(),
            fitted: None,
            selection: Selection::default(),
            tool: Tool::Select,
            wire_color: None,
            placing: None,
            drag: Drag::None,
            scene: None,
            pressed: None,
            instruments: InstrumentRegistry::builtin(),
            hover: HoverInfo::default(),
            rect: Rect::NOTHING,
            refit: false,
            t0: std::time::Instant::now(),
            lead_drag: Default::default(),
        }
    }
}

/// Dupont wire colours (the design's brighter bench palette).
pub fn wire_color(name: &str) -> Color32 {
    match name {
        "red" => rgb(0xE0483D),
        "black" => rgb(0x2B2B2B),
        "orange" => rgb(0xF08A24),
        "yellow" => rgb(0xF2D03B),
        "green" => rgb(0x3FBF6A),
        "blue" => rgb(0x3D7BE0),
        "purple" => rgb(0x9B6BE0),
        "grey" | "gray" => rgb(0x8A8A8A),
        "white" => rgb(0xE6E6E6),
        "brown" => rgb(0x7A4A21),
        other => bb_core::Rgb::hex(other)
            .map(|c| Color32::from_rgb(c.r, c.g, c.b))
            .unwrap_or(rgb(0xF08A24)),
    }
}

/// The sagging bezier of a wire segment (jumpers arc up off the board).
pub fn wire_curve(a: (f64, f64), b: (f64, f64)) -> [(f64, f64); 4] {
    let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let lift = (d * 0.24 + 0.35).min(11.0);
    let k = 0.12;
    [
        a,
        (a.0 + (b.0 - a.0) * k, a.1 - lift),
        (b.0 - (b.0 - a.0) * k, b.1 - lift),
        b,
    ]
}

/// Bezier segments through a wire's bend points.
pub fn wire_segments(a: (f64, f64), bends: &[(f64, f64)], b: (f64, f64)) -> Vec<[(f64, f64); 4]> {
    let mut pts = vec![a];
    pts.extend_from_slice(bends);
    pts.push(b);
    if pts.len() == 2 {
        return vec![wire_curve(a, b)];
    }
    // with bends: gentle curves that pass through every bend (Catmull-Rom → bezier)
    let n = pts.len();
    (0..n - 1)
        .map(|i| {
            let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
            let p1 = pts[i];
            let p2 = pts[i + 1];
            let p3 = if i + 2 < n { pts[i + 2] } else { pts[n - 1] };
            let t = 1.0 / 6.0;
            [
                p1,
                (p1.0 + (p2.0 - p0.0) * t, p1.1 + (p2.1 - p0.1) * t - 0.4),
                (p2.0 - (p3.0 - p1.0) * t, p2.1 - (p3.1 - p1.1) * t - 0.4),
                p2,
            ]
        })
        .collect()
}

fn hypot(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Knob centre + radius of a potentiometer part (world).
fn knob_of(it: &super::scene::PartItem) -> Option<((f64, f64), f64)> {
    let art = it.def.bench.art.as_str();
    if art != "potentiometer" && art != "trimmer" {
        return None;
    }
    let n = it.leads.len().max(1) as f64;
    let mx = it.leads.iter().map(|l| l.1).sum::<f64>() / n;
    let top = it.leads.iter().map(|l| l.2).fold(f64::INFINITY, f64::min);
    Some(art::passives_pot_knob(
        (mx, 0.0),
        top - 0.6,
        art == "trimmer",
    ))
}

impl BenchCanvas {
    /// The scene for the document's current revision (cached).
    pub fn scene(&mut self, doc: &mut CircuitDoc) -> Arc<Scene> {
        let g = doc.generation();
        if let Some(s) = &self.scene
            && s.generation == g
        {
            return s.clone();
        }
        let conn = doc.connectivity();
        let s = Arc::new(Scene::build(doc.project(), &doc.lib, &conn, g));
        self.scene = Some(s.clone());
        s
    }

    /// Zoom by a factor about the canvas centre.
    pub fn zoom(&mut self, factor: f32) {
        let c = self.rect.center();
        self.view.zoom_about(c, factor);
    }

    pub fn fit(&mut self) {
        self.refit = true;
    }

    fn time(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// Draws and handles the canvas in `rect`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        doc: &mut CircuitDoc,
        tel: Option<&Telemetry>,
        project_generation: u64,
    ) -> BenchOutput {
        let mut out = BenchOutput::default();
        self.rect = rect;
        let scene = self.scene(doc);
        if self.fitted != Some(project_generation) || self.refit {
            self.view = View::fit(rect, padded(scene.extent), BASE_SCALE);
            self.fitted = Some(project_generation);
            self.refit = false;
        }
        let resp = ui.interact(rect, Id::unique("bench-canvas"), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Bench canvas"));
        ui.ctx()
            .data_mut(|d| d.insert_temp(Id::unique("bench-view"), self.view));
        // plugin instrument controls + probe-lead tips sit on top of the canvas
        out.sim.extend(super::plugin_parts::interact(
            ui,
            &self.view,
            rect,
            doc,
            &scene,
            tel,
            &mut self.lead_drag,
        ));
        let scene = self.scene(doc);
        let conn = doc.connectivity();
        self.handle_view_input(ui, &resp);
        self.handle_input(ui, &resp, doc, &scene, &conn, tel, &mut out);
        let scene = self.scene(doc);
        let painter = ui.painter_at(rect);
        self.draw(ui, &painter, rect, doc, &scene, &conn, tel);
        self.tooltip(ui, &resp, doc, &scene, &conn, tel);
        if self.needs_animation(tel) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        out
    }

    fn needs_animation(&self, tel: Option<&Telemetry>) -> bool {
        tel.is_some_and(|t| {
            t.parts.iter().any(|p| {
                p.visual.smoke > 0.02
                    || matches!(p.visual.look, bb_core::Look::Buzzer { level, .. } if level > 0.02)
            })
        })
    }

    // ---------------------------------------------------------------- input

    fn handle_view_input(&mut self, ui: &Ui, resp: &Response) {
        if !resp.contains_pointer() {
            return;
        }
        let (scroll, zoom, cmd, hover) = ui.input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.modifiers.command,
                i.pointer.hover_pos(),
            )
        });
        let at = hover.unwrap_or(self.rect.center());
        if zoom != 1.0 {
            self.view.zoom_about(at, zoom);
        } else if cmd && scroll.y != 0.0 {
            self.view.zoom_about(at, (scroll.y * 0.004).exp());
        } else if scroll != egui::Vec2::ZERO {
            self.view.origin += scroll;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_input(
        &mut self,
        ui: &Ui,
        resp: &Response,
        doc: &mut CircuitDoc,
        scene: &Scene,
        conn: &Connectivity,
        tel: Option<&Telemetry>,
        out: &mut BenchOutput,
    ) {
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let world = pointer.map(|p| self.view.to_world(p));
        let lib = doc.lib.clone();
        // hover
        self.hover = HoverInfo::default();
        if let Some((x, y)) = world
            && resp.contains_pointer()
        {
            if let Some(pin) = scene.pin_at(x, y, 0.42) {
                let part = &scene.parts[pin.part];
                self.hover.pin = Some((part.id.clone(), pin.pin.clone()));
                self.hover.net = conn.net_of(&part.id, &pin.pin);
            } else if let Some(w) = self.wire_at(doc, scene, (x, y)) {
                self.hover.net = doc.project().wire(&w).and_then(|w| match &w.a {
                    Endpoint::Pin { part, pin } => conn.net_of(part, pin),
                    _ => None,
                });
                self.hover.wire = Some(w);
            } else if let Some(i) = scene.part_at(x, y) {
                self.hover.part = Some(scene.parts[i].id.clone());
            }
        }
        // drop from the palette
        if let Some(p) = resp.dnd_release_payload::<PartDrop>()
            && let Some((x, y)) = world
        {
            self.place(doc, &p.0, x, y, out);
        }
        // keyboard
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing && resp.contains_pointer() || !typing && !self.selection.is_empty() {
            self.keys(ui, doc, out);
        }
        let alt = ui.input(|i| i.modifiers.alt);
        let shift = ui.input(|i| i.modifiers.shift);
        let space = ui.input(|i| i.key_down(Key::Space));
        let middle = ui.input(|i| i.pointer.middle_down());

        // ---- press / drag start (what is under the press origin, not where the pointer is
        // once egui recognises the drag)
        let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
        if resp.drag_started()
            && let Some(p) = origin
        {
            let (x, y) = self.view.to_world(p);
            self.drag = if space || middle {
                Drag::Pan {
                    origin: self.view.origin,
                    start: p,
                }
            } else if let Some(probe) = self.label_at(doc, scene, p) {
                let pos = doc
                    .project()
                    .probes
                    .iter()
                    .find(|x| x.id == probe)
                    .and_then(|x| x.pinned)
                    .unwrap_or((x, y));
                Drag::Label { probe, pos }
            } else if let Some((w, idx)) = self.bend_at(doc, p) {
                Drag::Bend {
                    wire: w,
                    idx,
                    pos: (x, y),
                }
            } else if let Some((part, v)) = self.knob_at(scene, tel, doc, (x, y)) {
                Drag::Knob {
                    part,
                    start: v,
                    value: v,
                    origin: p,
                }
            } else if let Some(pin) = scene.pin_at(x, y, 0.42).filter(|_| !alt) {
                let part = &scene.parts[pin.part];
                Drag::Wire {
                    from: Endpoint::pin(&part.id, &pin.pin),
                    from_pos: (pin.x, pin.y),
                    to: (x, y),
                }
            } else if let Some(i) = scene.part_at(x, y) {
                let id = scene.parts[i].id.clone();
                if !self.selection.parts.contains(&id) {
                    if !shift {
                        self.selection.clear();
                    }
                    self.selection.parts.insert(id);
                    out.selection_changed = true;
                }
                if let Some(pid) = self.pressed.take() {
                    out.sim.push(Command::Input {
                        part: pid,
                        input: UserInput::Press(false),
                    });
                }
                Drag::Move {
                    ids: self.selection.parts.iter().cloned().collect(),
                    start: (x, y),
                    delta: (0.0, 0.0),
                }
            } else {
                Drag::Box {
                    start: (x, y),
                    end: (x, y),
                }
            };
        }
        // ---- dragging
        if resp.dragged()
            && let (Some(p), Some((x, y))) = (pointer, world)
        {
            match &mut self.drag {
                Drag::Pan { origin, start } => self.view.origin = *origin + (p - *start),
                Drag::Move { start, delta, .. } => {
                    *delta = ((x - start.0).round(), (y - start.1).round());
                }
                Drag::Wire { to, .. } => *to = (x, y),
                Drag::Box { end, .. } => *end = (x, y),
                Drag::Bend { pos, .. } => *pos = (x.round(), y.round()),
                Drag::Label { pos, .. } => *pos = (x, y),
                Drag::Knob {
                    part,
                    start,
                    value,
                    origin,
                } => {
                    let d = (p.x - origin.x) - (p.y - origin.y);
                    let v = (*start + d as f64 / 160.0).clamp(0.0, 1.0);
                    if (v - *value).abs() > 1e-4 {
                        *value = v;
                        out.sim.push(Command::Input {
                            part: part.clone(),
                            input: UserInput::Position(v),
                        });
                    }
                }
                Drag::None => {}
            }
        }
        // ---- release
        if resp.drag_stopped() {
            let drag = std::mem::replace(&mut self.drag, Drag::None);
            match drag {
                Drag::Move { ids, delta, .. } => {
                    if delta != (0.0, 0.0) {
                        doc.editor.move_parts(&ids, delta.0, delta.1, true);
                        doc.touched();
                    }
                }
                Drag::Wire { from, from_pos, to } => {
                    if let Some(pin) = scene.pin_at(to.0, to.1, 0.45) {
                        let part = &scene.parts[pin.part];
                        let b = Endpoint::pin(&part.id, &pin.pin);
                        if b != from && hypot(from_pos, (pin.x, pin.y)) > 0.1 {
                            let id = doc.editor.add_wire(&lib, from, b);
                            if let Some(c) = &self.wire_color
                                && let Some(mut w) = doc.project().wire(&id).cloned()
                            {
                                w.color = c.clone();
                                let _ = doc.editor.update_wire(w);
                            }
                            doc.touched();
                            self.selection.clear();
                            self.selection.wires.insert(id);
                            out.selection_changed = true;
                        }
                    }
                }
                Drag::Box { start, end } => {
                    if !shift {
                        self.selection.clear();
                    }
                    for id in scene.parts_in(WRect::new(start.0, start.1, end.0, end.1)) {
                        self.selection.parts.insert(id);
                    }
                    out.selection_changed = true;
                }
                Drag::Bend { wire, idx, pos } => {
                    if let Some(mut w) = doc.project().wire(&wire).cloned()
                        && idx < w.bends.len()
                    {
                        w.bends[idx] = pos;
                        let _ = doc.editor.update_wire(w);
                        doc.touched();
                    }
                }
                Drag::Label { probe, pos } => {
                    if let Some(before) =
                        doc.project().probes.iter().find(|p| p.id == probe).cloned()
                    {
                        let mut after = before.clone();
                        after.pinned = Some(pos);
                        let mut tx = Transaction::new("Move label");
                        tx.ops.push(Op::Probe {
                            before: Some(before),
                            after: Some(after),
                            index: None,
                        });
                        doc.editor.commit(tx);
                        doc.touched();
                    }
                }
                Drag::Knob { part, value, .. } => {
                    if doc
                        .editor
                        .set_prop(&lib, &part, "position", PropValue::Num(value))
                        .is_ok()
                    {
                        doc.touched();
                        out.sim.push(Command::SetPartParam {
                            part,
                            key: "position".into(),
                            value,
                        });
                        out.live_edit = true;
                    }
                }
                Drag::Pan { .. } | Drag::None => {}
            }
        }
        // ---- press-and-hold controls (buttons) + clicks
        if resp.is_pointer_button_down_on()
            && self.pressed.is_none()
            && matches!(self.drag, Drag::None)
            && let Some((x, y)) = world
            && let Some(i) = scene.part_at(x, y)
            && scene.parts[i].def.bench.art == "tactile_button"
            && scene.pin_at(x, y, 0.3).is_none()
        {
            let id = scene.parts[i].id.clone();
            out.sim.push(Command::Input {
                part: id.clone(),
                input: UserInput::Press(true),
            });
            self.pressed = Some(id);
        }
        if !resp.is_pointer_button_down_on()
            && let Some(id) = self.pressed.take()
        {
            out.sim.push(Command::Input {
                part: id,
                input: UserInput::Press(false),
            });
        }
        if resp.double_clicked()
            && let Some((x, y)) = world
        {
            self.double_click(doc, scene, (x, y));
        } else if resp.clicked()
            && let Some((x, y)) = world
        {
            self.click(doc, scene, conn, (x, y), shift, out);
        }
        if resp.secondary_clicked()
            && let Some((x, y)) = world
        {
            if let Some(w) = self.wire_at(doc, scene, (x, y)) {
                if !self.selection.wires.contains(&w) {
                    self.selection.clear();
                    self.selection.wires.insert(w);
                    out.selection_changed = true;
                }
            } else if let Some(i) = scene.part_at(x, y) {
                let id = scene.parts[i].id.clone();
                if !self.selection.parts.contains(&id) {
                    self.selection.clear();
                    self.selection.parts.insert(id);
                    out.selection_changed = true;
                }
            }
        }
        self.context_menu(resp, doc, out);
        // cursor
        let cursor = match (&self.drag, &self.hover) {
            (Drag::Pan { .. }, _) => Some(CursorIcon::Grabbing),
            (Drag::Move { .. }, _) => Some(CursorIcon::Grabbing),
            (Drag::Wire { .. }, _) => Some(CursorIcon::Crosshair),
            (_, h) if h.pin.is_some() => Some(CursorIcon::Crosshair),
            (_, h) if h.part.is_some() || h.wire.is_some() => Some(CursorIcon::PointingHand),
            _ if self.placing.is_some() => Some(CursorIcon::Copy),
            _ => None,
        };
        if let Some(c) = cursor
            && resp.contains_pointer()
        {
            ui.ctx().set_cursor_icon(c);
        }
    }

    /// Places a library part at a world point (palette drop or click-to-place).
    pub fn place(
        &mut self,
        doc: &mut CircuitDoc,
        def_id: &str,
        x: f64,
        y: f64,
        out: &mut BenchOutput,
    ) {
        let lib = doc.lib.clone();
        let Some(def) = lib.get(def_id) else { return };
        if def_id == "jumper_wire" {
            // the palette's wire tile: arm wire drawing (drag from a pin)
            return;
        }
        let (px, py) = snap_drop(def, x, y);
        if let Ok(id) = doc.editor.add_part(&lib, def_id, px, py) {
            doc.touched();
            self.selection.clear();
            self.selection.parts.insert(id);
            out.selection_changed = true;
        }
    }

    fn click(
        &mut self,
        doc: &mut CircuitDoc,
        scene: &Scene,
        conn: &Connectivity,
        (x, y): (f64, f64),
        shift: bool,
        out: &mut BenchOutput,
    ) {
        if let Some(def) = self.placing.take() {
            self.place(doc, &def, x, y, out);
            return;
        }
        if let Tool::Clip { instrument, key } = self.tool.clone() {
            if let Some(pin) = scene.pin_at(x, y, 0.45) {
                let part = &scene.parts[pin.part];
                if let Some(before) = doc
                    .project()
                    .instruments
                    .iter()
                    .find(|i| i.id == instrument)
                    .cloned()
                {
                    let mut after = before.clone();
                    after
                        .probes
                        .insert(key.clone(), Endpoint::pin(&part.id, &pin.pin));
                    let mut tx = Transaction::new("Clip probe");
                    tx.ops.push(Op::Instrument {
                        before: Some(before),
                        after: Some(after),
                        index: None,
                    });
                    doc.editor.commit(tx);
                    doc.touched();
                }
                self.tool = Tool::Select;
            }
            return;
        }
        if self.tool == Tool::Probe {
            if let Some(pin) = scene.pin_at(x, y, 0.45) {
                let part = &scene.parts[pin.part];
                let net = conn
                    .net_of(&part.id, &pin.pin)
                    .map(|n| net_display(&conn.net(n).name))
                    .unwrap_or_default();
                let label = if part.def.is_breadboard() {
                    format!("Net {net}")
                } else {
                    format!("{} · {}", part.reference, pin.pin)
                };
                let target = Endpoint::pin(&part.id, &pin.pin);
                let id = doc
                    .editor
                    .add_probe(&label, target.clone(), Quantity::Voltage);
                // pin the label up and to the right of the pin
                if let Some(before) = doc.project().probes.iter().find(|p| p.id == id).cloned() {
                    let mut after = before.clone();
                    after.pinned = Some((pin.x + 2.0, pin.y - 3.0));
                    let mut tx = Transaction::new("Pin probe label");
                    tx.ops.push(Op::Probe {
                        before: Some(before),
                        after: Some(after.clone()),
                        index: None,
                    });
                    doc.editor.commit(tx);
                    out.sim.push(Command::AddProbe(after));
                }
                doc.touched();
                self.tool = Tool::Select;
            }
            return;
        }
        // switches toggle on click
        if let Some(i) = scene.part_at(x, y) {
            let it = &scene.parts[i];
            match it.def.bench.art.as_str() {
                "slide_switch" | "tilt_switch" if scene.pin_at(x, y, 0.3).is_none() => {
                    out.sim.push(Command::Input {
                        part: it.id.clone(),
                        input: UserInput::Toggle { index: 0 },
                    });
                }
                "dip_switch" => {
                    let at = doc
                        .project()
                        .part(&it.id)
                        .and_then(|p| p.bench.as_ref())
                        .map(|b| b.at);
                    if let Some(at) = at {
                        let (lx, _) = at.inverse(x, y);
                        let idx = lx.round().clamp(0.0, 3.0) as usize;
                        out.sim.push(Command::Input {
                            part: it.id.clone(),
                            input: UserInput::Toggle { index: idx },
                        });
                    }
                }
                _ => {}
            }
        }
        let hit_wire = self.wire_at(doc, scene, (x, y));
        let hit_part = scene.part_at(x, y).map(|i| scene.parts[i].id.clone());
        if !shift {
            self.selection.clear();
        }
        if let Some(w) = hit_wire {
            if !self.selection.wires.remove(&w) {
                self.selection.wires.insert(w);
            }
        } else if let Some(p) = hit_part
            && !self.selection.parts.remove(&p)
        {
            self.selection.parts.insert(p);
        }
        out.selection_changed = true;
    }

    fn double_click(&mut self, doc: &mut CircuitDoc, scene: &Scene, (x, y): (f64, f64)) {
        // remove a bend
        if let Some((w, idx)) = self.bend_at(doc, self.view.to_screen(x, y))
            && let Some(mut wire) = doc.project().wire(&w).cloned()
        {
            wire.bends.remove(idx);
            let _ = doc.editor.update_wire(wire);
            doc.touched();
            return;
        }
        // add a bend where the wire was clicked
        if let Some(w) = self.wire_at(doc, scene, (x, y))
            && let Some(mut wire) = doc.project().wire(&w).cloned()
            && let (Some(a), Some(b)) = (scene.endpoint_pos(&wire.a), scene.endpoint_pos(&wire.b))
        {
            let segs = wire_segments(a, &wire.bends, b);
            let mut best = (f64::INFINITY, 0);
            for (i, s) in segs.iter().enumerate() {
                let d = bezier_distance(*s, (x, y));
                if d < best.0 {
                    best = (d, i);
                }
            }
            wire.bends
                .insert(best.1.min(wire.bends.len()), (x.round(), y.round()));
            let _ = doc.editor.update_wire(wire);
            doc.touched();
            self.selection.clear();
            self.selection.wires.insert(w);
        }
    }

    fn keys(&mut self, ui: &Ui, doc: &mut CircuitDoc, out: &mut BenchOutput) {
        let lib = doc.lib.clone();
        let (r, f, del, dup, copy, paste, esc, arrows) = ui.input_mut(|i| {
            let none = Modifiers::NONE;
            let cmd = Modifiers::COMMAND;
            let k = |i: &mut egui::InputState, m: Modifiers, key: Key| {
                i.consume_shortcut(&egui::KeyboardShortcut::new(m, key))
            };
            let mut arrows = (0.0, 0.0);
            for (key, d) in [
                (Key::ArrowLeft, (-1.0, 0.0)),
                (Key::ArrowRight, (1.0, 0.0)),
                (Key::ArrowUp, (0.0, -1.0)),
                (Key::ArrowDown, (0.0, 1.0)),
            ] {
                if k(i, none, key) {
                    arrows = d;
                }
            }
            (
                k(i, none, Key::R),
                k(i, none, Key::F),
                k(i, none, Key::Delete) || k(i, none, Key::Backspace),
                k(i, cmd, Key::D),
                k(i, cmd, Key::C),
                k(i, cmd, Key::V),
                k(i, none, Key::Escape),
                arrows,
            )
        });
        let parts: Vec<String> = self.selection.parts.iter().cloned().collect();
        let wires: Vec<String> = self.selection.wires.iter().cloned().collect();
        if r {
            for id in &parts {
                let _ = doc.editor.rotate(id);
            }
            doc.touched();
        }
        if f {
            for id in &parts {
                let _ = doc.editor.flip(id);
            }
            doc.touched();
        }
        if del && !self.selection.is_empty() {
            doc.editor.delete(&parts, &wires);
            doc.touched();
            self.selection.clear();
            out.selection_changed = true;
        }
        if dup && !parts.is_empty() {
            let new = doc.editor.duplicate(&lib, &parts);
            doc.touched();
            self.selection.clear();
            self.selection.parts.extend(new);
            out.selection_changed = true;
        }
        if copy && !parts.is_empty() {
            doc.clipboard = doc.editor.copy(&parts);
        }
        if paste && !doc.clipboard.parts.is_empty() {
            let clip = doc.clipboard.clone();
            let new = doc.editor.paste(&lib, &clip, 2.0, 2.0);
            doc.touched();
            self.selection.clear();
            self.selection.parts.extend(new);
            out.selection_changed = true;
        }
        if esc {
            self.placing = None;
            self.tool = Tool::Select;
            self.drag = Drag::None;
            self.selection.clear();
            out.selection_changed = true;
        }
        if arrows != (0.0, 0.0) && !parts.is_empty() {
            doc.editor.move_parts(&parts, arrows.0, arrows.1, true);
            doc.touched();
        }
    }

    fn context_menu(&mut self, resp: &Response, doc: &mut CircuitDoc, out: &mut BenchOutput) {
        let lib = doc.lib.clone();
        resp.context_menu(|ui| {
            ui.set_min_width(170.0);
            let parts: Vec<String> = self.selection.parts.iter().cloned().collect();
            let wires: Vec<String> = self.selection.wires.iter().cloned().collect();
            if !parts.is_empty() {
                if ui.button("Rotate  (R)").clicked() {
                    for id in &parts {
                        let _ = doc.editor.rotate(id);
                    }
                    doc.touched();
                    ui.close();
                }
                if ui.button("Flip  (F)").clicked() {
                    for id in &parts {
                        let _ = doc.editor.flip(id);
                    }
                    doc.touched();
                    ui.close();
                }
                if ui.button("Duplicate  (⌘D)").clicked() {
                    let new = doc.editor.duplicate(&lib, &parts);
                    doc.touched();
                    self.selection.clear();
                    self.selection.parts.extend(new);
                    out.selection_changed = true;
                    ui.close();
                }
                if ui.button("Copy  (⌘C)").clicked() {
                    doc.clipboard = doc.editor.copy(&parts);
                    ui.close();
                }
            }
            if let Some(w) = self.selection.single_wire().map(str::to_string) {
                ui.label(egui::RichText::new("Wire colour").font(font::ui(11.0)));
                ui.horizontal_wrapped(|ui| {
                    for c in bb_circuit::project::WIRE_COLORS {
                        let (r, rr) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 7.0, wire_color(c));
                        rr.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                format!("Wire colour {c}"),
                            )
                        });
                        if rr.clicked()
                            && let Some(mut wire) = doc.project().wire(&w).cloned()
                        {
                            wire.color = c.to_string();
                            let _ = doc.editor.update_wire(wire);
                            doc.touched();
                            ui.close();
                        }
                    }
                });
            }
            if (!parts.is_empty() || !wires.is_empty()) && ui.button("Delete  (⌫)").clicked() {
                doc.editor.delete(&parts, &wires);
                doc.touched();
                self.selection.clear();
                out.selection_changed = true;
                ui.close();
            }
            if !doc.clipboard.parts.is_empty() && ui.button("Paste  (⌘V)").clicked() {
                let clip = doc.clipboard.clone();
                let new = doc.editor.paste(&lib, &clip, 2.0, 2.0);
                doc.touched();
                self.selection.clear();
                self.selection.parts.extend(new);
                out.selection_changed = true;
                ui.close();
            }
            if ui.button("Fit to view").clicked() {
                self.refit = true;
                ui.close();
            }
        });
    }

    fn wire_at(&self, doc: &CircuitDoc, scene: &Scene, q: (f64, f64)) -> Option<String> {
        let tol = (5.0 / self.view.scale as f64).max(0.3);
        let mut best: Option<(f64, String)> = None;
        for w in &doc.project().wires {
            let (Some(a), Some(b)) = (scene.endpoint_pos(&w.a), scene.endpoint_pos(&w.b)) else {
                continue;
            };
            for s in wire_segments(a, &w.bends, b) {
                let d = bezier_distance(s, q);
                if d < tol && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, w.id.clone()));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// A bend handle of a selected wire under the screen point.
    fn bend_at(&self, doc: &CircuitDoc, p: Pos2) -> Option<(String, usize)> {
        for id in &self.selection.wires {
            let Some(w) = doc.project().wire(id) else {
                continue;
            };
            for (i, b) in w.bends.iter().enumerate() {
                if self.view.to_screen(b.0, b.1).distance(p) < 8.0 {
                    return Some((id.clone(), i));
                }
            }
        }
        None
    }

    fn knob_at(
        &self,
        scene: &Scene,
        tel: Option<&Telemetry>,
        doc: &CircuitDoc,
        q: (f64, f64),
    ) -> Option<(String, f64)> {
        for it in scene.parts.iter().rev() {
            if let Some((c, r)) = knob_of(it)
                && hypot(c, q) <= r * 1.1
            {
                let v = tel
                    .and_then(|t| t.parts.iter().find(|p| p.part == it.id))
                    .and_then(|p| match p.visual.look {
                        bb_core::Look::Knob { position } => Some(position),
                        _ => None,
                    })
                    .or_else(|| {
                        doc.project()
                            .part(&it.id)
                            .and_then(|p| bb_circuit::Project::prop_f64(&it.def, p, "position"))
                    })
                    .unwrap_or(0.5);
                return Some((it.id.clone(), v));
            }
        }
        None
    }

    /// A floating probe label under the screen point.
    fn label_at(&self, doc: &CircuitDoc, scene: &Scene, p: Pos2) -> Option<String> {
        let _ = scene;
        for pr in &doc.project().probes {
            if let Some((x, y)) = pr.pinned {
                let a = self.view.to_screen(x, y);
                let r = pill_rect(a, 190.0);
                if r.contains(p) {
                    return Some(pr.id.clone());
                }
            }
        }
        None
    }

    // ---------------------------------------------------------------- drawing

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        doc: &CircuitDoc,
        scene: &Scene,
        conn: &Connectivity,
        tel: Option<&Telemetry>,
    ) {
        let p = pal(ui);
        theme::paint_dots(painter, rect, p);
        let glows = RefCell::new(Vec::<Glow>::new());
        let pen = Pen::new(painter, self.view).deferring(&glows);
        let project = doc.project();
        let t = self.time();
        let lux = tel
            .map(|t| t.world.ambient_lux)
            .unwrap_or(project.world.light.ambient_lux);
        let ambient = (lux / 1000.0).clamp(0.0, 1.0) as f32;
        let parts_tel: HashMap<&str, &bb_sim::PartTelemetry> = tel
            .map(|t| t.parts.iter().map(|p| (p.part.as_str(), p)).collect())
            .unwrap_or_default();
        let inst_views = self.instrument_views(doc, tel);
        let moving: Option<(&[String], (f64, f64))> = match &self.drag {
            Drag::Move { ids, delta, .. } if *delta != (0.0, 0.0) => Some((ids, *delta)),
            _ => None,
        };
        let empty: Vec<String> = Vec::new();
        let draw_part = |it: &super::scene::PartItem, pass: Pass, pen: &Pen| {
            let Some(inst) = project.part(&it.id) else {
                return;
            };
            let pt = parts_tel.get(it.id.as_str());
            let soil = it
                .def
                .props
                .iter()
                .any(|p| p.key == "pot")
                .then(|| {
                    let pot = inst
                        .props
                        .get("pot")
                        .map(|v| v.as_text())
                        .unwrap_or_else(|| "pot1".into());
                    tel.and_then(|t| t.world.pots.iter().find(|p| p.id == pot).map(|p| p.vwc))
                        .or_else(|| {
                            project
                                .world
                                .pots
                                .iter()
                                .find(|p| p.id == pot)
                                .map(|p| p.vwc)
                        })
                })
                .flatten();
            // moving: draw at the preview offset
            let shifted;
            let (inst, leads) = match moving {
                Some((ids, d)) if ids.contains(&it.id) => {
                    let mut i2 = inst.clone();
                    if let Some(b) = i2.bench.as_mut() {
                        b.at.x += d.0;
                        b.at.y += d.1;
                    }
                    shifted = it
                        .leads
                        .iter()
                        .map(|(p, x, y)| (p.clone(), x + d.0, y + d.1))
                        .collect::<Vec<_>>();
                    (std::borrow::Cow::Owned(i2), shifted.as_slice())
                }
                _ => (std::borrow::Cow::Borrowed(inst), it.leads.as_slice()),
            };
            let cx = ArtCtx {
                def: &it.def,
                part: &inst,
                leads,
                vis: pt.map(|p| &p.visual),
                readings: pt.map(|p| p.readings.as_slice()).unwrap_or(&[]),
                t,
                instrument: inst_views.get(it.id.as_str()),
                soil_vwc: soil,
                ambient,
                wired: scene.wired.get(&it.id).unwrap_or(&empty),
                on_board: it.on_board,
            };
            if !pen.visible(it.bounds.expand(8.0)) {
                return;
            }
            art::draw(pen, &cx, pass);
        };
        // 1. breadboards + boards
        for it in scene.parts.iter().filter(|p| p.layer <= 1) {
            draw_part(it, Pass::Body, &pen);
        }
        // 2. world objects' backs (pots), parts, other off-board things, then pot fronts
        let pots: Vec<_> = scene
            .parts
            .iter()
            .filter(|p| art::passes(&p.def.bench.art).contains(&Pass::Back))
            .collect();
        for it in &pots {
            draw_part(it, Pass::Back, &pen);
        }
        for it in scene.parts.iter().filter(|p| p.layer == 2) {
            draw_part(it, Pass::Body, &pen);
        }
        for it in &pots {
            draw_part(it, Pass::Front, &pen);
        }
        for it in scene.parts.iter().filter(|p| p.layer == 3) {
            if !art::passes(&p_art(it)).contains(&Pass::Back) {
                draw_part(it, Pass::Body, &pen);
            }
        }
        // 3. ratsnest (connections made in the schematic but not wired on the bench)
        for r in &conn.ratsnest_bench {
            dashed(&pen, r.from, r.to, rgb(0x4CC3F0));
        }
        // 4. wires
        let hl_net = self.hover.net;
        for w in &project.wires {
            if super::plugin_parts::lead_of(doc, w).is_some() {
                continue; // drawn as probe leads after the instruments
            }
            let (Some(mut a), Some(mut b)) = (scene.endpoint_pos(&w.a), scene.endpoint_pos(&w.b))
            else {
                continue;
            };
            if let Some((ids, d)) = moving {
                if w.a.part().is_some_and(|p| ids.iter().any(|i| i == p)) {
                    a = (a.0 + d.0, a.1 + d.1);
                }
                if w.b.part().is_some_and(|p| ids.iter().any(|i| i == p)) {
                    b = (b.0 + d.0, b.1 + d.1);
                }
            }
            let mut bends = w.bends.clone();
            if let Drag::Bend { wire, idx, pos } = &self.drag
                && wire == &w.id
                && *idx < bends.len()
            {
                bends[*idx] = *pos;
            }
            let selected = self.selection.wires.contains(&w.id);
            let on_net = hl_net.is_some()
                && match &w.a {
                    Endpoint::Pin { part, pin } => conn.net_of(part, pin) == hl_net,
                    _ => false,
                };
            draw_wire(&pen, a, &bends, b, wire_color(&w.color), selected, on_net);
            if selected {
                for bp in &bends {
                    let s = self.view.to_screen(bp.0, bp.1);
                    painter.circle(s, 5.0, p.bg_panel, Stroke::new(1.5, p.accent));
                }
            }
        }
        // 5. instrument probe leads
        for ins in &project.instruments {
            let Some(part) = &ins.part else { continue };
            let imp = self.instruments.get(&ins.kind);
            for (key, target) in &ins.probes {
                let lead = imp
                    .map(|i| i.lead_pin(key))
                    .unwrap_or_else(|| key.to_ascii_uppercase());
                let (Some(a), Some(mut b)) =
                    (scene.pin_pos(part, &lead), scene.endpoint_pos(target))
                else {
                    continue;
                };
                // a lead clipped into a breadboard strip / rail may sit in any hole of it:
                // draw it into the one nearest the instrument
                if let Endpoint::Pin {
                    part: tp,
                    pin: tpin,
                } = target
                    && scene.part(tp).is_some_and(|it| it.def.is_breadboard())
                    && let Some(n) = conn.net_of(tp, tpin)
                {
                    let same_strip = |h: &bb_circuit::TermRef| {
                        h.part == *tp
                            && match (
                                bb_circuit::breadboard::parse_hole(&h.pin),
                                bb_circuit::breadboard::parse_hole(tpin),
                            ) {
                                (Some(x), Some(y)) => {
                                    bb_circuit::breadboard::strip_key(&x, 50, false)
                                        == bb_circuit::breadboard::strip_key(&y, 50, false)
                                }
                                _ => false,
                            }
                    };
                    if let Some(p) = conn
                        .net(n)
                        .holes
                        .iter()
                        .filter(|h| same_strip(h))
                        .filter_map(|h| scene.pin_pos(&h.part, &h.pin))
                        .min_by(|p, q| hypot(*p, a).total_cmp(&hypot(*q, a)))
                    {
                        b = p;
                    }
                }
                let color = imp.map(|i| i.lead_color(key)).unwrap_or(rgb(0xC8352B));
                probe_lead(&pen, a, b, color);
                if let Some(text) = imp.and_then(|i| {
                    let net = match target {
                        Endpoint::Pin { part, pin } => conn
                            .net_of(part, pin)
                            .map(|n| net_display(&conn.net(n).name))
                            .unwrap_or_default(),
                        _ => String::new(),
                    };
                    i.tip_label(key, &net)
                }) {
                    pen.text_at(
                        b.0 + 0.9,
                        b.1 - 0.4,
                        &text,
                        0.72,
                        font::mono,
                        rgb(0x8D8576),
                        Align2::LEFT_BOTTOM,
                    );
                }
            }
        }
        // 6. instruments themselves (on top), then plugin instruments' probe leads
        for it in scene.parts.iter().filter(|p| p.layer >= 4) {
            draw_part(it, Pass::Body, &pen);
        }
        super::plugin_parts::draw_leads(&pen, doc, scene, &self.lead_drag);
        // 7. night: the bench dims with ambient light; LEDs then glow over it
        let night = (1.0 - ambient).powi(2) * 0.36;
        if night > 0.01 {
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha((night * 255.0) as u8));
        }
        for g in glows.borrow().iter() {
            draw_glow(painter, *g);
        }
        // magic smoke + damage badges on top of everything
        let plain = Pen::new(painter, self.view);
        for it in &scene.parts {
            let Some(pt) = parts_tel.get(it.id.as_str()) else {
                continue;
            };
            if pt.visual.smoke > 0.02 {
                art::smoke(&plain, it.bounds, pt.visual.smoke, t, it.id.len() as f64);
            }
            if pt.visual.health.is_damaged() {
                let c = self.view.to_screen(it.bounds.x1, it.bounds.y0);
                painter.circle(
                    c,
                    7.0,
                    p.danger,
                    Stroke::new(1.5, Color32::from_rgb(20, 8, 6)),
                );
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    "!",
                    font::mono_medium(11.0),
                    Color32::WHITE,
                );
            }
        }
        // 8. net highlight on hovered pins
        if let Some(net) = hl_net {
            let n = conn.net(net);
            for tr in n.pins.iter().chain(n.holes.iter()) {
                if let Some((x, y)) = scene.pin_pos(&tr.part, &tr.pin) {
                    let s = self.view.to_screen(x, y);
                    painter.circle_stroke(
                        s,
                        (self.view.scale * 0.42).max(3.0),
                        Stroke::new(1.4, with_alpha(p.info, 0.85)),
                    );
                }
            }
        }
        if let Some((part, pin)) = &self.hover.pin
            && let Some((x, y)) = scene.pin_pos(part, pin)
        {
            let s = self.view.to_screen(x, y);
            painter.circle_stroke(
                s,
                (self.view.scale * 0.55).max(4.0),
                Stroke::new(2.0, p.accent),
            );
        }
        // 9. selection outlines
        for id in &self.selection.parts {
            if let Some(it) = scene.part(id) {
                let mut b = it.bounds;
                if let Some((ids, d)) = moving
                    && ids.contains(id)
                {
                    b = WRect::new(b.x0 + d.0, b.y0 + d.1, b.x1 + d.0, b.y1 + d.1);
                }
                let r = pen.r(b.expand(0.3));
                painter.rect_stroke(
                    r,
                    6.0,
                    Stroke::new(1.5, p.accent),
                    egui::StrokeKind::Outside,
                );
            }
        }
        // 10. probe labels
        self.draw_probe_labels(&pen, painter, doc, scene, conn, tel);
        // 11. drag previews
        match &self.drag {
            Drag::Wire { from_pos, to, .. } => {
                let snap = scene
                    .pin_at(to.0, to.1, 0.45)
                    .map(|p| (p.x, p.y))
                    .unwrap_or(*to);
                let c = self
                    .wire_color
                    .as_deref()
                    .map(wire_color)
                    .unwrap_or(p.accent);
                draw_wire(
                    &pen,
                    *from_pos,
                    &[],
                    snap,
                    with_alpha(c, 0.85),
                    false,
                    false,
                );
            }
            Drag::Box { start, end } => {
                let r = Rect::from_two_pos(
                    self.view.to_screen(start.0, start.1),
                    self.view.to_screen(end.0, end.1),
                );
                painter.rect(
                    r,
                    2.0,
                    with_alpha(p.accent, 0.08),
                    Stroke::new(1.0, p.accent),
                    egui::StrokeKind::Inside,
                );
            }
            _ => {}
        }
        // palette drag / click-to-place preview
        let ghost = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|h| rect.contains(*h))
            .and_then(|h| {
                let id = egui::DragAndDrop::payload::<PartDrop>(ui.ctx())
                    .map(|p| p.0.clone())
                    .or_else(|| self.placing.clone())?;
                Some((id, self.view.to_world(h)))
            });
        if let Some((id, (x, y))) = ghost
            && let Some(def) = doc.lib.get(&id)
        {
            let (px, py) = snap_drop(def, x, y);
            let mut inst = bb_circuit::PartInstance::new("ghost", &def.ref_prefix, &id).at(px, py);
            inst.reference = format!("{}?", def.ref_prefix);
            let leads = bb_circuit::Project::bench_leads(def, &inst).unwrap_or_default();
            let cx = ArtCtx {
                def,
                part: &inst,
                leads: &leads,
                vis: None,
                readings: &[],
                t,
                instrument: None,
                soil_vwc: None,
                ambient,
                wired: &empty,
                on_board: true,
            };
            let gp = Pen::new(painter, self.view).ghost(0.6);
            art::draw(&gp, &cx, Pass::Body);
            // the holes the leads would land in
            for (_, lx, ly) in &leads {
                if let Some(h) = scene.hole_at(*lx, *ly, 0.3) {
                    let s = self.view.to_screen(h.x, h.y);
                    painter.circle_stroke(
                        s,
                        (self.view.scale * 0.4).max(3.0),
                        Stroke::new(1.5, p.ok),
                    );
                }
            }
        }
    }

    fn instrument_views<'a>(
        &self,
        doc: &'a CircuitDoc,
        tel: Option<&Telemetry>,
    ) -> HashMap<&'a str, InstrumentView> {
        let mut m = HashMap::new();
        let Some(tel) = tel else { return m };
        for ins in &doc.project().instruments {
            if let (Some(part), Some(imp)) = (&ins.part, self.instruments.get(&ins.kind)) {
                m.insert(part.as_str(), imp.view(ins, tel));
            }
        }
        // plugin instruments: their live front panel
        for p in &doc.project().parts {
            if let Some(pt) = tel.parts.iter().find(|t| t.part == p.id)
                && let Some(panel) = &pt.panel
            {
                m.entry(p.id.as_str()).or_default().panel = Some(panel.clone());
            }
        }
        m
    }

    fn draw_probe_labels(
        &self,
        pen: &Pen,
        painter: &egui::Painter,
        doc: &CircuitDoc,
        scene: &Scene,
        conn: &Connectivity,
        tel: Option<&Telemetry>,
    ) {
        let project = doc.project();
        for pr in &project.probes {
            let Some(target) = scene.endpoint_pos(&pr.target) else {
                continue;
            };
            let pinned = match &self.drag {
                Drag::Label { probe, pos } if probe == &pr.id => *pos,
                _ => pr.pinned.unwrap_or((target.0 + 2.0, target.1 - 3.0)),
            };
            // colour of a wire on the probed net (as in the design), else amber
            let net = match &pr.target {
                Endpoint::Pin { part, pin } => conn.net_of(part, pin),
                _ => None,
            };
            let color = net
                .and_then(|n| {
                    project.wires.iter().find(|w| match &w.a {
                        Endpoint::Pin { part, pin } => conn.net_of(part, pin) == Some(n),
                        _ => false,
                    })
                })
                .map(|w| wire_color(&w.color))
                .filter(|c| *c != wire_color("black"))
                .unwrap_or(rgb(0xF2A33A));
            let reading = tel.and_then(|t| t.probes.iter().find(|p| p.id == pr.id));
            let value = reading.map(probe_text).unwrap_or_else(|| "—".into());
            let a = pen.view.to_screen(pinned.0, pinned.1);
            // pill floats above its pinned point
            let f = font::mono(11.0);
            let g1 = painter.layout_no_wrap(format!("{} ", pr.label), f.clone(), rgb(0xEAEAEA));
            let g2 = painter.layout_no_wrap(value, f, rgb(0xF7C77A));
            let w = g1.size().x + g2.size().x + 24.0;
            let mut r = pill_rect(a, w);
            // a pill that would sit on its own target moves to the other side of the pin
            let st = pen.view.to_screen(target.0, target.1);
            if r.expand(4.0).contains(st) {
                r = r.translate(vec2(-w, 0.0));
            }
            // the leader goes to the nearest point of the probed net (a hole next to the LED
            // rather than the far-away MCU pin)
            let near = net
                .map(|n| {
                    let nn = conn.net(n);
                    nn.pins
                        .iter()
                        .chain(nn.holes.iter())
                        .filter_map(|t| scene.pin_pos(&t.part, &t.pin))
                        .min_by(|p, q| {
                            let dp = pen.view.to_screen(p.0, p.1).distance(r.center());
                            let dq = pen.view.to_screen(q.0, q.1).distance(r.center());
                            dp.total_cmp(&dq)
                        })
                        .unwrap_or(target)
                })
                .unwrap_or(target);
            let s_target = pen.view.to_screen(near.0, near.1);
            let from = pos2(
                s_target.x.clamp(r.left() + 14.0, r.right() - 14.0),
                if s_target.y > r.center().y {
                    r.bottom()
                } else {
                    r.top()
                },
            );
            dashed_screen(painter, from, s_target, color);
            painter.circle_stroke(s_target, 6.0, Stroke::new(2.0, color));
            painter.circle_filled(s_target, 1.8, color);
            painter.rect(
                r,
                11.0,
                Color32::from_rgba_unmultiplied(14, 14, 14, 240),
                Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
            let y = r.center().y - g1.size().y / 2.0;
            let x2 = r.left() + 12.0 + g1.size().x;
            painter.galley(pos2(r.left() + 12.0, y), g1, Color32::WHITE);
            painter.galley(pos2(x2, y), g2, Color32::WHITE);
        }
    }

    fn tooltip(
        &self,
        ui: &Ui,
        resp: &Response,
        doc: &CircuitDoc,
        scene: &Scene,
        conn: &Connectivity,
        tel: Option<&Telemetry>,
    ) {
        if !matches!(self.drag, Drag::None) || !resp.hovered() {
            return;
        }
        let volts = |net: NetId| -> Option<f64> {
            let name = net_display(&conn.net(net).name);
            tel?.nets.iter().find(|(n, _)| n == &name).map(|(_, v)| *v)
        };
        let text: Option<String> = (|| {
            if let Some((part, pin)) = &self.hover.pin {
                let it = scene.part(part)?;
                let head = if it.def.is_breadboard() {
                    let desc = bb_circuit::breadboard::parse_hole(pin)
                        .map(|h| bb_circuit::breadboard::describe(&h))
                        .unwrap_or_default();
                    format!("{} {pin} · {desc}", it.reference)
                } else {
                    let label = it
                        .def
                        .pin(pin)
                        .map(|d| d.label().to_string())
                        .unwrap_or(pin.clone());
                    format!("{} · {label}", it.reference)
                };
                let net = self.hover.net.map(|n| {
                    let v = volts(n)
                        .map(|v| format!(" · {}", bb_core::si::format(v, "V")))
                        .unwrap_or_default();
                    format!("\nnet {}{v}", net_display(&conn.net(n).name))
                });
                let current = tel
                    .and_then(|t| t.parts.iter().find(|p| &p.part == part))
                    .and_then(|p| match &p.visual.look {
                        bb_core::Look::Led { current, .. } => {
                            Some(format!(" · I {}", bb_core::si::format(*current, "A")))
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                Some(format!("{head}{}{current}", net.unwrap_or_default()))
            } else if let Some(w) = &self.hover.wire {
                let wire = doc.project().wire(w)?;
                let net = self.hover.net.map(|n| {
                    let v = volts(n)
                        .map(|v| format!(" · {}", bb_core::si::format(v, "V")))
                        .unwrap_or_default();
                    format!("net {}{v}", net_display(&conn.net(n).name))
                });
                Some(format!("{} wire\n{}", wire.color, net.unwrap_or_default()))
            } else if let Some(part) = &self.hover.part {
                let it = scene.part(part)?;
                let health = tel
                    .and_then(|t| t.parts.iter().find(|p| &p.part == part))
                    .map(|p| match &p.visual.health {
                        bb_core::Health::Ok => String::new(),
                        h => format!(" · {}", h.label()),
                    })
                    .unwrap_or_default();
                Some(format!("{} · {}{health}", it.reference, it.def.name))
            } else {
                None
            }
        })();
        if let Some(t) = text {
            let _ = ui;
            resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(egui::RichText::new(t).font(font::mono(11.5)));
            });
        }
    }
}

fn p_art(it: &super::scene::PartItem) -> String {
    it.def.bench.art.clone()
}

/// Screen rect of a floating probe label pinned at `a` (it floats just above the point).
fn pill_rect(a: Pos2, w: f32) -> Rect {
    Rect::from_min_size(a - vec2(0.0, 26.0), vec2(w, 22.0))
}

/// Net names as the user sees them (`0` → `GND`).
pub fn net_display(name: &str) -> String {
    if name == "0" {
        "GND".into()
    } else {
        name.to_string()
    }
}

fn padded(r: WRect) -> (f64, f64, f64, f64) {
    (r.x0 - 2.0, r.y0 - 2.0, r.x1 + 2.0, r.y1 + 2.0)
}

/// `2.598 V`, `48 %`, `HIGH` as the design's labels show them (2 decimals for volts).
pub fn probe_text(r: &bb_sim::ProbeReading) -> String {
    match r.quantity {
        Quantity::Voltage => format!("{:.2} V", r.value),
        Quantity::Duty => format!("{:.0}%", r.value),
        Quantity::Logic => {
            if r.value > 0.5 {
                "ON".into()
            } else {
                "OFF".into()
            }
        }
        _ => r.text.clone(),
    }
}

/// A jumper wire: soft shadow, coloured core, specular highlight; plugs at both ends.
pub fn draw_wire(
    pen: &Pen,
    a: (f64, f64),
    bends: &[(f64, f64)],
    b: (f64, f64),
    color: Color32,
    selected: bool,
    highlight: bool,
) {
    let w = pen.px(0.62, 2.2);
    let segs = wire_segments(a, bends, b);
    for s in &segs {
        let sh: [(f64, f64); 4] = [
            (s[0].0 + 0.45, s[0].1 + 0.85),
            (s[1].0 + 0.45, s[1].1 + 0.85),
            (s[2].0 + 0.45, s[2].1 + 0.85),
            (s[3].0 + 0.45, s[3].1 + 0.85),
        ];
        pen.bezier(sh, w * 1.1, Color32::from_black_alpha(82));
    }
    for s in &segs {
        if selected || highlight {
            let c = if selected {
                rgb(0xF2A33A)
            } else {
                rgb(0x4CC3F0)
            };
            pen.bezier(*s, w + 4.0, with_alpha(c, 0.55));
        }
        pen.bezier(*s, w, color);
        let hi: [(f64, f64); 4] = [
            (s[0].0 - 0.06, s[0].1 - 0.12),
            (s[1].0 - 0.06, s[1].1 - 0.12),
            (s[2].0 - 0.06, s[2].1 - 0.12),
            (s[3].0 - 0.06, s[3].1 - 0.12),
        ];
        pen.bezier(hi, (w * 0.28).max(1.0), with_alpha(Color32::WHITE, 0.28));
    }
    // dupont plug ends
    for q in [a, b] {
        pen.circle(q.0, q.1, 0.26, crate::canvas::pen::darken(color, 0.35));
        pen.circle(q.0, q.1, 0.12, rgb(0x9AA0A6));
    }
}

/// An instrument probe lead: a thin cable with a clip at the target.
pub fn probe_lead(pen: &Pen, a: (f64, f64), b: (f64, f64), color: Color32) {
    let d = hypot(a, b);
    let sag = (d * 0.18).min(6.0);
    let c = [
        a,
        (a.0 + (b.0 - a.0) * 0.25, a.1 + sag),
        (b.0 - (b.0 - a.0) * 0.3, b.1 + sag * 0.6),
        b,
    ];
    let w = pen.px(0.3, 1.6);
    pen.bezier(
        [
            (c[0].0 + 0.3, c[0].1 + 0.6),
            (c[1].0 + 0.3, c[1].1 + 0.6),
            (c[2].0 + 0.3, c[2].1 + 0.6),
            (c[3].0 + 0.3, c[3].1 + 0.6),
        ],
        w,
        Color32::from_black_alpha(70),
    );
    pen.bezier(c, w, color);
    pen.bezier(
        [
            (c[0].0, c[0].1 - 0.05),
            (c[1].0, c[1].1 - 0.05),
            (c[2].0, c[2].1 - 0.05),
            (c[3].0, c[3].1 - 0.05),
        ],
        (w * 0.3).max(0.8),
        with_alpha(Color32::WHITE, 0.22),
    );
    pen.circle(b.0, b.1, 0.42, color);
    pen.circle(b.0, b.1, 0.18, rgb(0x0A0A0A));
}

fn dashed(pen: &Pen, a: (f64, f64), b: (f64, f64), color: Color32) {
    dashed_screen(
        pen.painter,
        pen.view.to_screen(a.0, a.1),
        pen.view.to_screen(b.0, b.1),
        with_alpha(color, 0.85),
    );
}

fn dashed_screen(painter: &egui::Painter, a: Pos2, b: Pos2, color: Color32) {
    painter.extend(egui::Shape::dashed_line(
        &[a, b],
        Stroke::new(1.2, color),
        3.0,
        2.0,
    ));
}
