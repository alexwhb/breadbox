//! Plugin parts on the bench (`PLUG-6`): instruments draw their declarative front panel
//! (`bb_plugin::panel`, live `PanelState` from `PartTelemetry::panel`) with working controls
//! (dial, buttons, knobs, switches → `Command::Input`), other plugin parts draw their SVG art
//! (`plugin://<id>/<file>.svg`, rasterised with resvg), and wires plugged into an instrument's
//! jacks are drawn as probe leads whose tips you drag onto any pin to clip them on (or drop
//! anywhere to leave them dangling).
//!
//! The plugin service registers what the canvas needs ([`register`]) whenever the active
//! plugins change; the canvas calls [`draw`] from the art dispatcher, [`interact`] right after
//! its own interaction rect and [`draw_leads`] after the instruments.

use super::art::ArtCtx;
use super::pen::{Pen, WRect, rgb};
use super::scene::Scene;
use crate::circuit::CircuitDoc;
use crate::widgets::instrument::{self, Face};
use bb_circuit::project::{Endpoint, Wire};
use bb_core::UserInput;
use bb_plugin::panel::{Element, Panel, PanelState};
use bb_sim::{Command, Telemetry};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, pos2, vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// What the canvas knows about the active plugins' parts.
#[derive(Default)]
struct Registered {
    /// Part definition id → front panel.
    panels: HashMap<String, Arc<Panel>>,
    /// `plugin://…` art URL → file.
    art: HashMap<String, PathBuf>,
}

static REGISTERED: RwLock<Option<Registered>> = RwLock::new(None);

/// Registers the active plugins' panels and art (called by the plugin service).
pub fn register(host: &bb_plugin::PluginHost) {
    let mut r = Registered::default();
    for i in host.instruments() {
        if let Some(p) = i.panel {
            r.panels.insert(i.part.clone(), Arc::new(p));
        }
    }
    let lib = host.library();
    for def in lib.parts() {
        let art = &def.bench.art;
        if art.starts_with(bb_plugin::host::ASSET_SCHEME)
            && let Some(path) = host.asset_path(art)
        {
            r.art.insert(art.clone(), path);
        }
    }
    if let Ok(mut g) = REGISTERED.write() {
        *g = Some(r);
    }
}

/// The front panel of a plugin instrument part definition.
pub fn panel_for(def_id: &str) -> Option<Arc<Panel>> {
    REGISTERED
        .read()
        .ok()?
        .as_ref()?
        .panels
        .get(def_id)
        .cloned()
}

fn art_path(url: &str) -> Option<PathBuf> {
    REGISTERED.read().ok()?.as_ref()?.art.get(url).cloned()
}

/// World rect of a panel part's face (its placement ignoring rotation).
fn face_world(def: &bb_circuit::PartDef, part: &bb_circuit::PartInstance) -> Option<WRect> {
    let at = part.bench.as_ref()?.at;
    let (w, h) = def.bench.size;
    Some(WRect::new(at.x, at.y, at.x + w, at.y + h))
}

/// Draws a plugin part. Returns `false` when it has neither a panel nor loadable art.
pub fn draw(lp: &Pen, cx: &ArtCtx) -> bool {
    if let Some(panel) = panel_for(&cx.def.id) {
        let (w, h) = cx.def.bench.size;
        let screen = lp.r(WRect::new(0.0, 0.0, w, h));
        let face = Face::exact(&panel, screen);
        let state = cx
            .instrument
            .and_then(|v| v.panel.as_ref())
            .and_then(PanelState::from_json);
        instrument::paint(lp.painter, &face, &panel, state.as_ref());
        return true;
    }
    let art = &cx.def.bench.art;
    if let Some(path) = art_path(art) {
        let (w, h) = cx.def.bench.size;
        let screen = lp.r(WRect::new(0.0, 0.0, w.max(1.0), h.max(1.0)));
        if let Some(tex) = svg_texture(lp.painter.ctx(), art, &path, screen.size()) {
            lp.painter.image(
                tex.id(),
                screen,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE.gamma_multiply(lp.alpha),
            );
            return true;
        }
    }
    false
}

type TexCache = HashMap<(String, u32), egui::TextureHandle>;
static TEXTURES: Mutex<Option<TexCache>> = Mutex::new(None);

/// The SVG rasterised at about `size` (cached per 64 px size bucket).
fn svg_texture(
    ctx: &egui::Context,
    url: &str,
    path: &std::path::Path,
    size: egui::Vec2,
) -> Option<egui::TextureHandle> {
    let bucket = ((size.x.max(size.y) * ctx.pixels_per_point()) / 64.0)
        .ceil()
        .clamp(1.0, 24.0) as u32;
    let key = (url.to_string(), bucket);
    let mut g = TEXTURES.lock().ok()?;
    let cache = g.get_or_insert_with(HashMap::new);
    if let Some(t) = cache.get(&key) {
        return Some(t.clone());
    }
    let text = std::fs::read_to_string(path).ok()?;
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&text, &opt).ok()?;
    let ts = tree.size();
    let target = bucket as f32 * 64.0;
    let scale = target / ts.width().max(ts.height()).max(1.0);
    let w = (ts.width() * scale).ceil().max(1.0) as u32;
    let h = (ts.height() * scale).ceil().max(1.0) as u32;
    let mut pix = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pix.as_mut(),
    );
    let img = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pix.data());
    let tex = ctx.load_texture(
        format!("plugin-art {url} {bucket}"),
        img,
        egui::TextureOptions::LINEAR,
    );
    cache.insert(key, tex.clone());
    Some(tex)
}

/// A wire plugged into an instrument jack: (instrument part id, jack pin, which end is the
/// tip: `false` = `b`, `true` = `a`).
pub fn lead_of(doc: &CircuitDoc, w: &Wire) -> Option<(String, String, bool)> {
    let jack = |e: &Endpoint| -> Option<(String, String)> {
        let Endpoint::Pin { part, pin } = e else {
            return None;
        };
        let inst = doc.project().part(part)?;
        let panel = panel_for(&inst.part)?;
        panel
            .elements
            .iter()
            .any(|el| matches!(el, Element::Jack { pin: p, .. } if p == pin))
            .then(|| (part.clone(), pin.clone()))
    };
    match (jack(&w.a), jack(&w.b)) {
        (Some((p, j)), None) => Some((p, j, false)),
        (None, Some((p, j))) => Some((p, j, true)),
        _ => None,
    }
}

/// Probe lead colour of a jack (red / black ring).
fn jack_color(def_id: &str, pin: &str) -> Color32 {
    let c = panel_for(def_id).and_then(|p| {
        p.elements.iter().find_map(|e| match e {
            Element::Jack { pin: jp, color, .. } if jp == pin => color.clone(),
            _ => None,
        })
    });
    match c {
        Some(c) => {
            let col = instrument::hex(&c, rgb(0xC8352B));
            if col.r() < 60 && col.g() < 60 && col.b() < 60 {
                rgb(0x2B2B2B)
            } else {
                rgb(0xC8352B)
            }
        }
        None => rgb(0xC8352B),
    }
}

/// Lead-tip drag state (kept in the bench canvas).
#[derive(Debug, Clone, Default)]
pub struct LeadDrag {
    /// Wire being re-clipped and the tip's current world position.
    pub dragging: Option<(String, (f64, f64))>,
}

/// Draws every probe lead (after the instruments, so cables leave the jacks on top).
pub fn draw_leads(pen: &Pen, doc: &CircuitDoc, scene: &Scene, drag: &LeadDrag) {
    for w in &doc.project().wires {
        let Some((part, pin, tip_is_a)) = lead_of(doc, w) else {
            continue;
        };
        let jack_end = if tip_is_a { &w.b } else { &w.a };
        let tip_end = if tip_is_a { &w.a } else { &w.b };
        let Some(a) = scene.endpoint_pos(jack_end) else {
            continue;
        };
        let mut b = match scene.endpoint_pos(tip_end) {
            Some(b) => b,
            None => continue,
        };
        if let Some((id, pos)) = &drag.dragging
            && *id == w.id
        {
            b = *pos;
        }
        let def = doc
            .project()
            .part(&part)
            .map(|p| p.part.clone())
            .unwrap_or_default();
        let color = jack_color(&def, &pin);
        // plug in the jack
        pen.circle(a.0, a.1, 0.45, crate::canvas::pen::darken(color, 0.2));
        super::bench::probe_lead(pen, a, b, color);
        // a probe tip / clip at the far end
        let free = matches!(tip_end, Endpoint::Free { .. });
        pen.circle(b.0, b.1, if free { 0.55 } else { 0.5 }, color);
        pen.circle(b.0, b.1, 0.22, rgb(0xC9C9C9));
    }
}

/// Live controls + lead tips. Call right after the canvas's own interaction rect so these
/// sit on top of it. Returns commands for the bench.
#[allow(clippy::too_many_arguments)]
pub fn interact(
    ui: &mut Ui,
    view: &super::pen::View,
    clip: Rect,
    doc: &mut CircuitDoc,
    scene: &Scene,
    tel: Option<&Telemetry>,
    drag: &mut LeadDrag,
) -> Vec<Command> {
    let mut out = Vec::new();
    let painter = ui.painter_at(clip);
    let pen = Pen::new(&painter, *view);
    // instrument faces
    let parts: Vec<(String, String, String)> = doc
        .project()
        .parts
        .iter()
        .map(|p| (p.id.clone(), p.reference.clone(), p.part.clone()))
        .collect();
    for (id, reference, def_id) in &parts {
        let Some(panel) = panel_for(def_id) else {
            continue;
        };
        let (Some(def), Some(inst)) = (doc.lib.get(def_id), doc.project().part(id)) else {
            continue;
        };
        let Some(wr) = face_world(def, inst) else {
            continue;
        };
        let screen = pen.r(wr);
        if !clip.intersects(screen) {
            continue;
        }
        let face = Face::exact(&panel, screen);
        let state = tel
            .and_then(|t| t.parts.iter().find(|p| &p.part == id))
            .and_then(|p| p.panel.as_ref())
            .and_then(PanelState::from_json);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(clip));
        child.set_clip_rect(clip);
        for (key, value) in instrument::controls(
            &mut child,
            &face,
            &panel,
            state.as_ref(),
            reference,
            egui::Id::unique(("bench-panel", id.as_str())),
        ) {
            out.push(Command::Input {
                part: id.clone(),
                input: UserInput::Value { key, value },
            });
        }
    }
    // lead tips
    let leads: Vec<(Wire, String, String, bool)> = doc
        .project()
        .wires
        .iter()
        .filter_map(|w| lead_of(doc, w).map(|(p, j, a)| (w.clone(), p, j, a)))
        .collect();
    for (w, part, jack, tip_is_a) in leads {
        let tip_end = if tip_is_a { &w.a } else { &w.b };
        let Some(b) = scene.endpoint_pos(tip_end) else {
            continue;
        };
        let pos = match &drag.dragging {
            Some((id, p)) if *id == w.id => *p,
            _ => b,
        };
        let s = view.to_screen(pos.0, pos.1);
        let r = Rect::from_center_size(s, vec2(16.0, 16.0));
        let resp = ui.interact(
            r,
            egui::Id::unique(("lead-tip", w.id.as_str())),
            Sense::drag(),
        );
        let reference = doc
            .project()
            .part(&part)
            .map(|p| p.reference.clone())
            .unwrap_or_default();
        let label = format!("{reference} {} probe tip", jack_label(doc, &part, &jack));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            drag.dragging = Some((w.id.clone(), view.to_world(p)));
        }
        if resp.drag_stopped()
            && let Some((id, at)) = drag.dragging.take()
            && id == w.id
        {
            clip_tip(doc, scene, &w.id, tip_is_a, &part, at);
        }
    }
    out
}

/// The printed label of an instrument jack (`VΩ`, `COM`).
fn jack_label(doc: &CircuitDoc, part: &str, pin: &str) -> String {
    doc.project()
        .part(part)
        .and_then(|p| panel_for(&p.part))
        .and_then(|panel| {
            panel.elements.iter().find_map(|e| match e {
                Element::Jack { pin: jp, label, .. } if jp == pin => Some(label.clone()),
                _ => None,
            })
        })
        .unwrap_or_else(|| pin.to_string())
}

/// Clips a lead tip onto the pin nearest to `at` (or leaves it dangling there).
pub fn clip_tip(
    doc: &mut CircuitDoc,
    scene: &Scene,
    wire: &str,
    tip_is_a: bool,
    instrument: &str,
    at: (f64, f64),
) {
    let Some(mut w) = doc.project().wire(wire).cloned() else {
        return;
    };
    let target = scene
        .pin_at(at.0, at.1, 0.6)
        .map(|p| {
            let part = &scene.parts[p.part];
            (part.id.clone(), p.pin.clone())
        })
        .filter(|(p, _)| p != instrument)
        .map(|(p, pin)| Endpoint::pin(&p, &pin))
        .unwrap_or(Endpoint::Free { x: at.0, y: at.1 });
    if tip_is_a {
        w.a = target;
    } else {
        w.b = target;
    }
    if doc.editor.update_wire(w).is_ok() {
        doc.touched();
    }
}

/// Screen position of an instrument control element's centre (tests, lessons).
pub fn control_screen_pos(
    view: &super::pen::View,
    doc: &CircuitDoc,
    part: &str,
    element: &str,
) -> Option<Pos2> {
    let inst = doc.project().part(part)?;
    let def = doc.lib.get(&inst.part)?;
    let panel = panel_for(&inst.part)?;
    let wr = face_world(def, inst)?;
    let pen_rect = Rect::from_two_pos(view.to_screen(wr.x0, wr.y0), view.to_screen(wr.x1, wr.y1));
    let face = Face::exact(&panel, pen_rect);
    panel.elements.iter().find_map(|e| {
        (e.id() == Some(element)).then(|| match e {
            Element::Display { rect, .. }
            | Element::Button { rect, .. }
            | Element::Rotary { rect, .. }
            | Element::Knob { rect, .. }
            | Element::Switch { rect, .. }
            | Element::Jack { rect, .. }
            | Element::Led { rect, .. }
            | Element::Screen { rect, .. }
            | Element::Label { rect, .. } => face.r(*rect).center(),
        })
    })
}
