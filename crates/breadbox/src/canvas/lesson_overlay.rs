//! Lesson overlays on the bench canvas: what "Test circuit" and "Show me" point at.
//!
//! * green glow on what's already connected to a goal's source ([`Tone::Ok`]);
//! * pulsing amber rings on target holes / pins, amber outlines on parts to use
//!   ([`Tone::Target`], [`Tone::Focus`]);
//! * a red marker with a tooltip on a wrong wire end or part ([`Tone::Wrong`]);
//! * a red dashed outline + × on a wire to remove ([`Tone::Remove`]);
//! * "Show me" ghost jumpers: a dashed, marching jumper from source to destination with a
//!   short label;
//! * the success pulse: a green ring that swells once around the parts a step lit up;
//! * while dragging a wire in a lesson, the hole under the pointer says which net it joins.

use super::bench::{wire_color, wire_curve, wire_segments};
use super::pen::{Pen, View, bezier_point, rgb, with_alpha};
use super::scene::Scene;
use crate::theme::font;
use bb_circuit::project::Project;
use bb_circuit::{Connectivity, Library};
use bb_lessons::diagnose::{Mark, Spot, Tone};
use bb_lessons::guide::Ghost;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Stroke, vec2};

const GREEN: Color32 = Color32::from_rgb(0x6F, 0xD4, 0x8F);
const AMBER: Color32 = Color32::from_rgb(0xF2, 0xA3, 0x3A);
const RED: Color32 = Color32::from_rgb(0xF0, 0x5A, 0x4A);

/// What a lesson asks the bench canvas to draw on top of the circuit.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub marks: Vec<Mark>,
    pub ghosts: Vec<Ghost>,
    /// Parts to pulse once (green), with the seconds since the success.
    pub celebrate: Vec<String>,
    pub celebrate_age: f64,
    /// Seconds since "Show me" opened (the ghost jumper grows in once).
    pub ghost_age: f64,
    /// While dragging a wire, the hole under the pointer says which net it joins.
    pub net_hints: bool,
}

impl Overlay {
    /// Drops repeated marks (two findings pointing at the same holes) — the first one wins.
    pub fn dedup(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.marks
            .retain(|m| seen.insert((m.spot.clone(), m.tone as u8)));
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty() && self.ghosts.is_empty() && self.celebrate.is_empty()
    }
    /// Needs repainting for its animation.
    pub fn animating(&self) -> bool {
        !self.marks.is_empty()
            || !self.ghosts.is_empty()
            || (!self.celebrate.is_empty() && self.celebrate_age < 1.6)
    }
}

fn pulse(t: f64, speed: f64) -> f32 {
    (0.5 + 0.5 * (t * speed).sin()) as f32
}

fn pill(painter: &egui::Painter, at: Pos2, text: &str, color: Color32, align_above: bool) -> Rect {
    let g = painter.layout_no_wrap(text.to_string(), font::medium(11.5), Color32::WHITE);
    let size = g.size() + vec2(16.0, 8.0);
    let min = if align_above {
        Pos2::new(at.x - size.x / 2.0, at.y - size.y - 6.0)
    } else {
        Pos2::new(at.x + 10.0, at.y - size.y / 2.0)
    };
    let r = Rect::from_min_size(min, size);
    painter.rect(
        r,
        size.y / 2.0,
        Color32::from_rgba_unmultiplied(14, 14, 14, 235),
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    painter.galley(r.min + vec2(8.0, 4.0), g, Color32::WHITE);
    r
}

/// Screen position + radius of a hole / pin.
fn pin_screen(view: &View, scene: &Scene, part: &str, pin: &str) -> Option<(Pos2, f32)> {
    let (x, y) = scene.pin_pos(part, pin)?;
    Some((view.to_screen(x, y), (view.scale * 0.42).max(3.5)))
}

fn wire_screen_points(scene: &Scene, project: &Project, id: &str) -> Option<Vec<[(f64, f64); 4]>> {
    let w = project.wire(id)?;
    let (a, b) = (scene.endpoint_pos(&w.a)?, scene.endpoint_pos(&w.b)?);
    Some(wire_segments(a, &w.bends, b))
}

/// Draws the overlay. Returns the note of a red marker under the pointer (for a tooltip).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ov: &Overlay,
    painter: &egui::Painter,
    view: &View,
    scene: &Scene,
    project: &Project,
    t: f64,
    pointer: Option<Pos2>,
) -> Option<String> {
    let pen = Pen::new(painter, *view);
    let mut hover_note: Option<String> = None;
    let beat = pulse(t, 4.2);
    // 1. green glow under everything else
    for m in ov.marks.iter().filter(|m| m.tone == Tone::Ok) {
        if let Spot::Pin { part, pin } = &m.spot
            && let Some((c, r)) = pin_screen(view, scene, part, pin)
        {
            painter.circle_filled(c, r * 1.25, with_alpha(GREEN, 0.18));
            painter.circle_stroke(c, r * 0.95, Stroke::new(1.4, with_alpha(GREEN, 0.75)));
        }
    }
    // 2. wires and parts
    for m in &ov.marks {
        match (&m.spot, m.tone) {
            (Spot::Wire(id), tone) => {
                let Some(segs) = wire_screen_points(scene, project, id) else {
                    continue;
                };
                let (col, width) = match tone {
                    Tone::Remove | Tone::Wrong => (RED, 3.0),
                    Tone::Ok => (GREEN, 2.0),
                    _ => (AMBER, 2.0),
                };
                for s in &segs {
                    let pts: Vec<Pos2> = (0..=24)
                        .map(|i| {
                            let p = bezier_point(*s, i as f64 / 24.0);
                            view.to_screen(p.0, p.1)
                        })
                        .collect();
                    painter.extend(egui::Shape::dashed_line_with_offset(
                        &pts,
                        Stroke::new(width, with_alpha(col, 0.55 + 0.4 * beat)),
                        &[7.0],
                        &[5.0],
                        (t * 18.0) as f32 % 12.0,
                    ));
                }
                if tone == Tone::Remove
                    && let Some(mid) = segs.get(segs.len() / 2)
                {
                    let p = bezier_point(*mid, 0.5);
                    let c = view.to_screen(p.0, p.1);
                    painter.circle(c, 9.0, RED, Stroke::new(1.5, Color32::from_rgb(20, 8, 6)));
                    let d = 3.6;
                    let st = Stroke::new(2.0, Color32::WHITE);
                    painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], st);
                    painter.line_segment([c + vec2(d, -d), c + vec2(-d, d)], st);
                    if !m.note.is_empty() {
                        pill(painter, c - vec2(0.0, 8.0), &m.note, RED, true);
                    }
                    if pointer.is_some_and(|q| q.distance(c) < 12.0) {
                        hover_note = Some(m.note.clone());
                    }
                }
            }
            (Spot::Part(id), tone) if tone != Tone::Ok => {
                let Some(it) = scene.part(id) else { continue };
                let r = pen.r(it.bounds.expand(0.5));
                let col = match tone {
                    Tone::Wrong | Tone::Remove => RED,
                    _ => AMBER,
                };
                let grow = 2.0 + 3.0 * beat;
                painter.rect_stroke(
                    r.expand(grow),
                    8.0,
                    Stroke::new(2.0, with_alpha(col, 0.45 + 0.45 * beat)),
                    egui::StrokeKind::Outside,
                );
                if tone == Tone::Wrong {
                    let c = r.right_top() + vec2(2.0, -2.0);
                    painter.circle(c, 8.0, RED, Stroke::new(1.5, Color32::from_rgb(20, 8, 6)));
                    painter.text(
                        c,
                        Align2::CENTER_CENTER,
                        "!",
                        font::mono_medium(11.0),
                        Color32::WHITE,
                    );
                    if pointer.is_some_and(|q| r.expand(4.0).contains(q)) && !m.note.is_empty() {
                        hover_note = Some(m.note.clone());
                    }
                }
                if !m.note.is_empty() && tone != Tone::Wrong {
                    pill(
                        painter,
                        r.center_top() - vec2(0.0, grow),
                        &m.note,
                        col,
                        true,
                    );
                }
            }
            _ => {}
        }
    }
    // 3. ghost jumpers ("Show me")
    for g in &ov.ghosts {
        let (Some(a), Some(b)) = (
            scene.pin_pos(&g.a.part, &g.a.pin),
            scene.pin_pos(&g.b.part, &g.b.pin),
        ) else {
            continue;
        };
        let curve = wire_curve(a, b);
        // the jumper grows from the source once, then its dashes march toward the target
        let grown = (ov.ghost_age / 0.9).clamp(0.0, 1.0);
        let grown = 1.0 - (1.0 - grown).powi(3);
        let n = 40;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let p = bezier_point(curve, grown * i as f64 / n as f64);
                view.to_screen(p.0, p.1)
            })
            .collect();
        let col = wire_color(&g.color);
        let width = view.len(0.55).clamp(2.0, 8.0);
        painter.add(egui::Shape::line(
            pts.clone(),
            Stroke::new(width + 3.0, Color32::from_black_alpha(90)),
        ));
        painter.extend(egui::Shape::dashed_line_with_offset(
            &pts,
            Stroke::new(width, with_alpha(col, 0.9)),
            &[9.0],
            &[5.0],
            -(t * 22.0) as f32 % 14.0,
        ));
        for (end, p) in [(true, a), (false, b)] {
            let c = view.to_screen(p.0, p.1);
            let r = (view.scale * 0.5).max(5.0);
            let k = if end { 1.0 } else { 0.6 + 0.4 * beat };
            painter.circle_stroke(
                c,
                r * (1.0 + 0.35 * beat),
                Stroke::new(2.2, with_alpha(AMBER, k)),
            );
            painter.circle_filled(c, r * 0.35, with_alpha(AMBER, 0.9));
        }
        let mid = bezier_point(curve, 0.5);
        let m = view.to_screen(mid.0, mid.1);
        pill(painter, m - vec2(0.0, 4.0), &g.label, AMBER, true);
    }
    // 4. pins: targets pulse, wrong ends get a red marker
    for m in &ov.marks {
        let Spot::Pin { part, pin } = &m.spot else {
            continue;
        };
        let Some((c, r)) = pin_screen(view, scene, part, pin) else {
            continue;
        };
        match m.tone {
            Tone::Target | Tone::Focus => {
                painter.circle_filled(c, r * 0.55, with_alpha(AMBER, 0.35 + 0.35 * beat));
                painter.circle_stroke(
                    c,
                    r * (0.8 + 0.3 * beat),
                    Stroke::new(1.6, with_alpha(AMBER, 0.55 + 0.45 * (1.0 - beat))),
                );
            }
            Tone::Wrong => {
                painter.circle(
                    c,
                    r * 1.2,
                    with_alpha(RED, 0.9),
                    Stroke::new(1.5, Color32::from_rgb(20, 8, 6)),
                );
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    "!",
                    font::mono_medium((r * 1.6).clamp(9.0, 14.0)),
                    Color32::WHITE,
                );
                painter.circle_stroke(
                    c,
                    r * (1.5 + 0.5 * beat),
                    Stroke::new(1.5, with_alpha(RED, 0.6 * (1.0 - beat))),
                );
                if pointer.is_some_and(|q| q.distance(c) < r * 1.8) && !m.note.is_empty() {
                    hover_note = Some(m.note.clone());
                }
            }
            _ => {}
        }
    }
    // 5. the success pulse
    if ov.celebrate_age < 1.6 {
        let k = (ov.celebrate_age / 1.6) as f32;
        for id in &ov.celebrate {
            if let Some(it) = scene.part(id) {
                let r = pen.r(it.bounds.expand(0.4));
                painter.rect_stroke(
                    r.expand(4.0 + 22.0 * k),
                    10.0 + 12.0 * k,
                    Stroke::new(3.0 * (1.0 - k) + 0.5, with_alpha(GREEN, 0.9 * (1.0 - k))),
                    egui::StrokeKind::Outside,
                );
                painter.rect_filled(r.expand(2.0), 8.0, with_alpha(GREEN, 0.12 * (1.0 - k)));
            }
        }
    }
    hover_note
}

/// "joins column 5 · R1's lead in column 5" — what a hole under a dragged wire connects to.
pub fn net_hint(
    project: &Project,
    lib: &Library,
    conn: &Connectivity,
    part: &str,
    pin: &str,
) -> String {
    let n = bb_lessons::names::Namer::new(project, lib, conn);
    let place = n.where_is(part, pin);
    let who: Vec<String> = n
        .net_pins(part, pin)
        .iter()
        .filter(|t| !(t.part == part && t.pin == pin))
        .take(2)
        .map(|t| n.pin(&t.part, &t.pin))
        .collect();
    if who.is_empty() {
        let empty = conn
            .net_of(part, pin)
            .map(|id| conn.net(id).pins.is_empty())
            .unwrap_or(true);
        if empty && n.is_breadboard(part) {
            format!("{place} · nothing here yet")
        } else {
            place
        }
    } else if n.is_breadboard(part) {
        format!("{place} · {}", who.join(", "))
    } else {
        who.join(", ")
    }
}

/// Draws the net hint pill next to the pointer.
pub fn draw_net_hint(painter: &egui::Painter, at: Pos2, text: &str) {
    pill(
        painter,
        at + vec2(6.0, -18.0),
        &format!("joins {text}"),
        rgb(0x4CC3F0),
        false,
    );
}
