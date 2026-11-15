//! Instrument front panels (`PLUG-4`, `PLUG-6`): renders a plugin's declarative
//! `bb_plugin::panel::Panel` with its live `PanelState` — LCD / 7-segment displays with
//! annunciators and bar graph, buttons, the rotary dial, knobs, slide switches, LEDs, probe
//! jacks and scope / logic screens (graticule, traces, markers, cursors) — and turns clicks /
//! drags on its controls into control values (`UserInput::Value { key: control id, value }`).
//!
//! The look follows the design's Handheld DMM (`docs/design/source/4-plugins.html`): a
//! rounded body in the panel colour with a darker inset face, a grey-green LCD with dark
//! digits, dark rounded buttons, a large dial with labels on its ring and the selected
//! position in amber, black jacks with coloured rings.
//!
//! Coordinates: panel elements are in millimetres on the face; [`Face`] maps them into a
//! screen rect (uniform scale, centred).

use crate::theme::font;
use bb_plugin::panel::{Element, Panel, PanelState};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2,
    vec2,
};

/// `#RRGGBB` → colour (fallback on bad input).
pub fn hex(s: &str, fallback: Color32) -> Color32 {
    bb_plugin::theme::parse_color(s)
        .map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
        .unwrap_or(fallback)
}

fn rgb(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

/// Symbols plugin panels use that no bundled font has: [`ptext`] draws them as shapes and
/// [`glyph_safe`] spells them out for plain UI text (`tests/glyphs.rs` checks the rest).
pub const SHAPED_SYMBOLS: &[(char, &str)] = &[
    ('⎓', "DC"),
    ('∿', "sine"),
    ('⊓', "square"),
    ('⋀', "triangle"),
    ('⎍', "pulse"),
];

fn shaped(c: char) -> bool {
    SHAPED_SYMBOLS.iter().any(|(s, _)| *s == c)
}

/// Width of a shaped symbol at `size`.
fn symbol_width(c: char, size: f32) -> f32 {
    if c == '⎓' { size * 0.62 } else { size * 0.9 }
}

/// Draws a shaped symbol in the box starting at `x` (text line centred on `cy`).
fn paint_symbol(painter: &egui::Painter, c: char, x: f32, cy: f32, size: f32, color: Color32) {
    let w = symbol_width(c, size);
    let l = x + w * 0.1;
    let r = x + w * 0.9;
    let sw = (size * 0.09).max(1.0);
    let stroke = Stroke::new(sw, color);
    let a = size * 0.24; // waveform amplitude
    let line = |pts: Vec<Pos2>| painter.add(egui::Shape::line(pts, stroke));
    match c {
        '⎓' => {
            painter.line_segment(
                [pos2(l, cy - size * 0.12), pos2(r, cy - size * 0.12)],
                stroke,
            );
            let dash = (r - l) / 5.0;
            for k in [0.0, 2.0, 4.0] {
                let d = l + dash * k;
                painter.line_segment(
                    [pos2(d, cy + size * 0.12), pos2(d + dash, cy + size * 0.12)],
                    stroke,
                );
            }
        }
        '∿' => {
            let pts = (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0;
                    pos2(l + (r - l) * t, cy - a * (t * std::f32::consts::TAU).sin())
                })
                .collect();
            line(pts);
        }
        '⊓' => {
            let m = (l + r) / 2.0;
            line(vec![
                pos2(l, cy + a),
                pos2(l, cy - a),
                pos2(m, cy - a),
                pos2(m, cy + a),
                pos2(r, cy + a),
                pos2(r, cy - a),
            ]);
        }
        '⋀' => {
            let q = (r - l) / 4.0;
            line(vec![
                pos2(l, cy),
                pos2(l + q, cy - a),
                pos2(l + 3.0 * q, cy + a),
                pos2(r, cy),
            ]);
        }
        _ => {
            // '⎍' a narrow pulse
            let p0 = l + (r - l) * 0.35;
            let p1 = l + (r - l) * 0.6;
            line(vec![
                pos2(l, cy + a),
                pos2(p0, cy + a),
                pos2(p0, cy - a),
                pos2(p1, cy - a),
                pos2(p1, cy + a),
                pos2(r, cy + a),
            ]);
        }
    }
}

/// `painter.text` that also draws the [`SHAPED_SYMBOLS`] (`V⎓`, waveform icons) as shapes.
pub fn ptext(
    painter: &egui::Painter,
    pos: Pos2,
    align: Align2,
    text: impl ToString,
    font: FontId,
    color: Color32,
) -> Rect {
    let text = text.to_string();
    if !text.chars().any(shaped) {
        return painter.text(pos, align, text, font, color);
    }
    // runs of text and single symbols
    let mut runs: Vec<Result<String, char>> = Vec::new();
    for c in text.chars() {
        match (shaped(c), runs.last_mut()) {
            (true, _) => runs.push(Err(c)),
            (false, Some(Ok(t))) => t.push(c),
            (false, _) => runs.push(Ok(c.to_string())),
        }
    }
    let laid: Vec<Result<std::sync::Arc<egui::Galley>, char>> = runs
        .into_iter()
        .map(|r| r.map(|t| painter.layout_no_wrap(t, font.clone(), color)))
        .collect();
    let h = laid
        .iter()
        .filter_map(|g| g.as_ref().ok().map(|g| g.size().y))
        .fold(font.size, f32::max);
    let w: f32 = laid
        .iter()
        .map(|g| match g {
            Ok(g) => g.size().x,
            Err(c) => symbol_width(*c, font.size),
        })
        .sum();
    let rect = align.anchor_size(pos, vec2(w, h));
    let mut x = rect.left();
    for g in laid {
        match g {
            Ok(g) => {
                let gw = g.size().x;
                painter.galley(pos2(x, rect.top()), g, color);
                x += gw;
            }
            Err(c) => {
                paint_symbol(painter, c, x, rect.center().y, font.size, color);
                x += symbol_width(c, font.size);
            }
        }
    }
    rect
}

/// Look-alikes [`glyph_safe`] swaps in for characters Inter lacks (third-party plugin text).
pub const GLYPH_SUBSTITUTES: &[(char, &str)] = &[('∥', "‖"), ('▯', "□")];

/// Text from plugin data shown in the UI fonts: the [`SHAPED_SYMBOLS`] are spelled out
/// (`V⎓` → `V DC`, `∿` → `sine`) and [`GLYPH_SUBSTITUTES`] swapped in (`∥` → `‖`).
pub fn glyph_safe(s: &str) -> String {
    let mut out = s.replace("V⎓", "V DC");
    for (c, word) in SHAPED_SYMBOLS.iter().chain(GLYPH_SUBSTITUTES) {
        out = out.replace(*c, word);
    }
    out
}

/// A panel placed on screen.
#[derive(Debug, Clone, Copy)]
pub struct Face {
    /// Screen rect of the whole face.
    pub rect: Rect,
    /// Pixels per millimetre.
    pub scale: f32,
}

impl Face {
    /// Fits the panel into `area` (uniform scale, centred).
    pub fn fit(panel: &Panel, area: Rect) -> Face {
        let (w, h) = (panel.size[0].max(1.0), panel.size[1].max(1.0));
        let scale = (area.width() / w).min(area.height() / h);
        let size = vec2(w * scale, h * scale);
        Face {
            rect: Rect::from_center_size(area.center(), size),
            scale,
        }
    }
    /// Exactly `rect` (callers that already keep the aspect ratio).
    pub fn exact(panel: &Panel, rect: Rect) -> Face {
        Face {
            rect,
            scale: rect.width() / panel.size[0].max(1.0),
        }
    }
    pub fn p(&self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.rect.left() + x * self.scale,
            self.rect.top() + y * self.scale,
        )
    }
    pub fn r(&self, r: [f32; 4]) -> Rect {
        Rect::from_min_size(
            self.p(r[0], r[1]),
            vec2(r[2] * self.scale, r[3] * self.scale),
        )
    }
    pub fn mm(&self, v: f32) -> f32 {
        v * self.scale
    }
}

/// A rotary position's angle (degrees, 0 = up, clockwise); spread over 270° if unset.
pub fn position_angle(positions: &[bb_plugin::panel::Position], i: usize) -> f32 {
    if let Some(a) = positions.get(i).and_then(|p| p.angle) {
        return a;
    }
    let n = positions.len().max(1);
    if n == 1 {
        return 0.0;
    }
    -135.0 + 270.0 * i as f32 / (n - 1) as f32
}

fn dir(angle_deg: f32) -> egui::Vec2 {
    let a = angle_deg.to_radians();
    vec2(a.sin(), -a.cos())
}

/// The position nearest to a pointer angle around the dial centre.
pub fn nearest_position(
    positions: &[bb_plugin::panel::Position],
    center: Pos2,
    at: Pos2,
) -> Option<usize> {
    let d = at - center;
    if d.length() < 1.0 {
        return None;
    }
    let ang = d.x.atan2(-d.y).to_degrees();
    (0..positions.len()).min_by(|&a, &b| {
        let da = angle_diff(position_angle(positions, a), ang);
        let db = angle_diff(position_angle(positions, b), ang);
        da.total_cmp(&db)
    })
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn control_value(state: Option<&PanelState>, id: &str, default: f64) -> f64 {
    state
        .and_then(|s| s.controls.get(id).copied())
        .unwrap_or(default)
}

/// Paints the whole panel. `live` = false shows the display blank (`----`) when no state.
pub fn paint(painter: &egui::Painter, face: &Face, panel: &Panel, state: Option<&PanelState>) {
    let body = hex(panel.color.as_deref().unwrap_or("#3A3A3A"), rgb(0x3A3A3A));
    let r = face.rect;
    let short = r.width().min(r.height());
    // soft shadow
    for i in 0..4 {
        let e = 2.0 + i as f32 * 3.0;
        painter.rect_filled(
            r.translate(vec2(0.0, e * 0.6)).expand(e * 0.5),
            CornerRadius::same((short * 0.11 + e) as u8),
            Color32::from_black_alpha(22),
        );
    }
    painter.rect_filled(r, CornerRadius::same((short * 0.11) as u8), body);
    let inset = (short * 0.053).max(2.0);
    let inner = r.shrink(inset);
    let inner_col = crate::theme::mix(body, rgb(0x0A0A0A), 0.62);
    painter.rect_filled(inner, CornerRadius::same((short * 0.08) as u8), inner_col);
    for el in &panel.elements {
        paint_element(painter, face, el, state);
    }
}

fn mono(px: f32) -> FontId {
    font::mono(px.max(4.0))
}

fn paint_element(painter: &egui::Painter, face: &Face, el: &Element, state: Option<&PanelState>) {
    match el {
        Element::Display {
            id,
            rect,
            style,
            bar,
            ..
        } => {
            let r = face.r(*rect);
            let ds = state.and_then(|s| s.displays.get(id));
            paint_display(painter, face, r, style, *bar, ds);
        }
        Element::Button {
            id,
            rect,
            label,
            color,
            ..
        } => {
            let r = face.r(*rect);
            let on = control_value(state, id, 0.0) > 0.5;
            let fill = color
                .as_deref()
                .map(|c| hex(c, rgb(0x2A313B)))
                .unwrap_or(rgb(0x2A313B));
            painter.rect(
                r,
                CornerRadius::same((r.height() * 0.23) as u8),
                fill,
                Stroke::new(1.0, if on { rgb(0xF2A33A) } else { rgb(0x454545) }),
                StrokeKind::Inside,
            );
            ptext(
                painter,
                r.center(),
                Align2::CENTER_CENTER,
                label,
                font::ui((r.height() * 0.42).clamp(4.0, 13.0)),
                rgb(0xD9D9D9),
            );
        }
        Element::Rotary {
            id,
            rect,
            positions,
            default,
        } => {
            let r = face.r(*rect);
            let c = r.center();
            let outer = r.width().min(r.height()) / 2.0;
            let knob = outer * 0.58;
            painter.circle(c, outer, rgb(0x141414), Stroke::new(1.5, rgb(0x2E2E2E)));
            // knob with a soft highlight
            painter.circle_filled(c + vec2(0.0, outer * 0.04), knob, rgb(0x262626));
            painter.circle_filled(c, knob, rgb(0x3A3A3A));
            let cur = control_value(state, id, *default as f64).round().max(0.0) as usize;
            let cur = cur.min(positions.len().saturating_sub(1));
            let fs = (outer * 0.17).clamp(4.0, 12.0);
            for (i, p) in positions.iter().enumerate() {
                let a = position_angle(positions, i);
                let at = c + dir(a) * (outer * 0.84);
                let col = if i == cur {
                    rgb(0xF2A33A)
                } else {
                    rgb(0xD9D9D9)
                };
                ptext(painter, at, Align2::CENTER_CENTER, &p.label, mono(fs), col);
            }
            // pointer bar
            let a = position_angle(positions, cur);
            let d = dir(a);
            let n = vec2(-d.y, d.x);
            let w = knob * 0.22;
            let tip = c + d * (knob * 1.02);
            let tail = c - d * (knob * 0.1);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tail + n * (w / 2.0),
                    tip + n * (w / 2.0),
                    tip - n * (w / 2.0),
                    tail - n * (w / 2.0),
                ],
                rgb(0xEAEAEA),
                Stroke::NONE,
            ));
            painter.circle_filled(tip, w / 2.0, rgb(0xEAEAEA));
            painter.circle_filled(tail, w / 2.0, rgb(0xEAEAEA));
        }
        Element::Knob {
            id,
            rect,
            label,
            min,
            max,
            default,
            log,
            unit,
            ..
        } => {
            let r = face.r(*rect);
            let rad = r.width().min(r.height() * 0.78) / 2.0;
            let c = pos2(r.center().x, r.top() + rad);
            let v = control_value(state, id, *default);
            let t = knob_fraction(v, *min, *max, *log);
            painter.circle(c, rad, rgb(0x2C2C2C), Stroke::new(1.0, rgb(0x4A4A4A)));
            painter.circle_filled(c, rad * 0.78, rgb(0x3E3E3E));
            let a = -135.0 + 270.0 * t as f32;
            painter.line_segment(
                [c, c + dir(a) * rad * 0.8],
                Stroke::new((rad * 0.14).max(1.0), rgb(0xEAEAEA)),
            );
            let fs = (rad * 0.42).clamp(4.0, 11.0);
            ptext(
                painter,
                pos2(c.x, r.bottom()),
                Align2::CENTER_BOTTOM,
                format!("{label} {}", fmt_value(v, unit)),
                mono(fs),
                rgb(0xA3A3A3),
            );
        }
        Element::Switch {
            id,
            rect,
            label,
            options,
            default,
        } => {
            let r = face.r(*rect);
            let cur = control_value(state, id, *default as f64).round().max(0.0) as usize;
            let n = options.len().max(1);
            let lab_h = r.height() * 0.38;
            let track = Rect::from_min_max(pos2(r.left(), r.top() + lab_h), r.max);
            painter.rect_filled(
                track,
                CornerRadius::same((track.height() * 0.3) as u8),
                rgb(0x111111),
            );
            let seg_w = track.width() / n as f32;
            for (i, o) in options.iter().enumerate() {
                let s = Rect::from_min_size(
                    pos2(track.left() + seg_w * i as f32, track.top()),
                    vec2(seg_w, track.height()),
                )
                .shrink(1.0);
                if i == cur {
                    painter.rect_filled(
                        s,
                        CornerRadius::same((s.height() * 0.3) as u8),
                        rgb(0x2A1F0E),
                    );
                }
                ptext(
                    painter,
                    s.center(),
                    Align2::CENTER_CENTER,
                    o,
                    mono((s.height() * 0.5).clamp(4.0, 11.0)),
                    if i == cur {
                        rgb(0xF7C77A)
                    } else {
                        rgb(0xA3A3A3)
                    },
                );
            }
            ptext(
                painter,
                pos2(r.left(), r.top()),
                Align2::LEFT_TOP,
                label,
                font::ui((lab_h * 0.85).clamp(4.0, 11.0)),
                rgb(0xA3A3A3),
            );
        }
        Element::Jack {
            rect, label, color, ..
        } => {
            let r = face.r(*rect);
            let c = r.center();
            let rad = r.width().min(r.height()) / 2.0;
            let ring = color
                .as_deref()
                .map(|c| hex(c, rgb(0x5C5C5C)))
                .unwrap_or(rgb(0x5C5C5C));
            let ring = if ring.r() < 40 && ring.g() < 40 && ring.b() < 40 {
                rgb(0x5C5C5C)
            } else {
                ring
            };
            painter.circle(
                c,
                rad,
                rgb(0x070707),
                Stroke::new((rad * 0.16).max(1.2), ring),
            );
            painter.circle_filled(c, rad * 0.35, rgb(0x1A1A1A));
            ptext(
                painter,
                pos2(c.x, r.bottom() + face.mm(0.3)),
                Align2::CENTER_TOP,
                label,
                mono((rad * 0.6).clamp(4.0, 10.0)),
                rgb(0xA3A3A3),
            );
        }
        Element::Led {
            id,
            rect,
            label,
            color,
        } => {
            let r = face.r(*rect);
            let c = r.center();
            let rad = r.width().min(r.height()) / 2.0;
            let b = state.and_then(|s| s.leds.get(id).copied()).unwrap_or(0.0);
            let col = hex(color, rgb(0x6FD48F));
            if b > 0.02 {
                for k in 1..4 {
                    painter.circle_filled(
                        c,
                        rad * (1.0 + k as f32 * 0.8),
                        col.gamma_multiply(0.12 * b),
                    );
                }
            }
            painter.circle(
                c,
                rad,
                crate::theme::mix(rgb(0x202020), col, 0.25 + 0.75 * b),
                Stroke::new(1.0, rgb(0x111111)),
            );
            if !label.is_empty() {
                ptext(
                    painter,
                    pos2(r.right() + face.mm(1.0), c.y),
                    Align2::LEFT_CENTER,
                    label,
                    mono((rad * 1.4).clamp(4.0, 9.0)),
                    rgb(0xA3A3A3),
                );
            }
        }
        Element::Screen {
            id,
            rect,
            kind,
            grid,
            channels,
        } => {
            let r = face.r(*rect);
            let ss = state.and_then(|s| s.screens.get(id));
            paint_screen(painter, r, kind, *grid, channels, ss);
        }
        Element::Label { rect, text, color } => {
            let r = face.r(*rect);
            ptext(
                painter,
                r.left_center(),
                Align2::LEFT_CENTER,
                text,
                font::ui((r.height() * 0.7).clamp(4.0, 12.0)),
                color
                    .as_deref()
                    .map(|c| hex(c, rgb(0xA3A3A3)))
                    .unwrap_or(rgb(0xA3A3A3)),
            );
        }
    }
}

fn knob_fraction(v: f64, min: f64, max: f64, log: bool) -> f64 {
    if log && min > 0.0 && max > min {
        ((v.max(min)).ln() - min.ln()) / (max.ln() - min.ln())
    } else if max > min {
        (v - min) / (max - min)
    } else {
        0.0
    }
    .clamp(0.0, 1.0)
}

fn knob_value(t: f64, min: f64, max: f64, log: bool, step: Option<f64>) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let v = if log && min > 0.0 && max > min {
        (min.ln() + t * (max.ln() - min.ln())).exp()
    } else {
        min + t * (max - min)
    };
    match step {
        Some(s) if s > 0.0 => (v / s).round() * s,
        _ => v,
    }
    .clamp(min.min(max), max.max(min))
}

fn fmt_value(v: f64, unit: &str) -> String {
    let s = bb_core::si::format(v, unit);
    s.trim().to_string()
}

fn paint_display(
    painter: &egui::Painter,
    face: &Face,
    r: Rect,
    style: &str,
    bar: u32,
    ds: Option<&bb_plugin::panel::DisplayState>,
) {
    let (bg, ink, frame) = match style {
        "seven_segment" => (rgb(0x140606), rgb(0xFF5A48), rgb(0x050505)),
        "vfd" => (rgb(0x06120F), rgb(0x5CF2D0), rgb(0x050505)),
        _ => (rgb(0xAEB9A0), rgb(0x18200F), rgb(0x0F1310)),
    };
    let rad = CornerRadius::same((r.height() * 0.08) as u8);
    painter.rect(
        r,
        rad,
        bg,
        Stroke::new((face.mm(0.8)).clamp(1.0, 3.0), frame),
        StrokeKind::Inside,
    );
    if style == "lcd" {
        // a subtle top-to-bottom shade like a real LCD
        let lower = Rect::from_min_max(pos2(r.left(), r.center().y), r.max).shrink(1.5);
        painter.rect_filled(lower, rad, Color32::from_black_alpha(10));
    }
    let faint = ink.gamma_multiply(0.75);
    let pad = r.height() * 0.12;
    let ann_fs = (r.height() * 0.12).clamp(4.0, 12.0);
    let (text, unit, anns, barv) = match ds {
        Some(d) => (
            if d.text.is_empty() {
                "----".to_string()
            } else {
                d.text.clone()
            },
            d.unit.clone(),
            d.annunciators.clone(),
            d.bar,
        ),
        None => ("----".to_string(), String::new(), Vec::new(), None),
    };
    // annunciators: AUTO/MANU on the left, the rest on the right (design: AUTO … DC)
    let (left, right): (Vec<&String>, Vec<&String>) = anns
        .iter()
        .partition(|a| matches!(a.as_str(), "AUTO" | "MANU" | "HOLD"));
    ptext(
        painter,
        pos2(r.left() + pad, r.top() + pad * 0.8),
        Align2::LEFT_TOP,
        left.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        mono(ann_fs),
        faint,
    );
    ptext(
        painter,
        pos2(r.right() - pad, r.top() + pad * 0.8),
        Align2::RIGHT_TOP,
        right
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        mono(ann_fs),
        faint,
    );
    // digits right-aligned, unit after them
    let unit_w = if unit.is_empty() {
        0.0
    } else {
        r.height() * 0.32
    };
    // as large as fits: ~50 % of the display height, narrower for long readings
    let mut digits_fs = (r.height() * 0.5).clamp(6.0, 64.0);
    let room = (r.width() - pad * 2.0 - unit_w).max(10.0);
    let gw = painter
        .layout_no_wrap(text.clone(), font::mono_medium(digits_fs), ink)
        .size()
        .x;
    if gw > room {
        digits_fs = (digits_fs * room / gw).max(5.0);
    }
    ptext(
        painter,
        pos2(r.right() - pad - unit_w, r.top() + r.height() * 0.62),
        Align2::RIGHT_CENTER,
        &text,
        font::mono_medium(digits_fs),
        ink,
    );
    if !unit.is_empty() {
        ptext(
            painter,
            pos2(r.right() - pad * 0.7, r.top() + r.height() * 0.78),
            Align2::RIGHT_CENTER,
            &unit,
            mono((r.height() * 0.14).clamp(4.0, 16.0)),
            ink,
        );
    }
    // bar graph along the bottom-left
    if bar > 0 {
        let bw = r.width() * 0.56;
        let by = r.bottom() - pad * 0.9;
        let bh = (r.height() * 0.05).max(1.5);
        let v = barv.unwrap_or(0.0).clamp(0.0, 1.0);
        painter.rect_filled(
            Rect::from_min_size(pos2(r.left() + pad, by - bh), vec2(bw * v.max(0.02), bh)),
            CornerRadius::same(1),
            faint,
        );
    }
}

fn paint_screen(
    painter: &egui::Painter,
    r: Rect,
    kind: &str,
    grid: [u32; 2],
    channels: &[bb_plugin::panel::Channel],
    ss: Option<&bb_plugin::panel::ScreenState>,
) {
    painter.rect(
        r,
        CornerRadius::same(3),
        rgb(0x0A110E),
        Stroke::new(1.5, rgb(0x050505)),
        StrokeKind::Inside,
    );
    let gx = grid[0].max(1) as f32;
    let gy = grid[1].max(1) as f32;
    let gcol = rgb(0x1C3328);
    for i in 1..grid[0] {
        let x = r.left() + r.width() * i as f32 / gx;
        painter.line_segment(
            [pos2(x, r.top()), pos2(x, r.bottom())],
            Stroke::new(0.6, gcol),
        );
    }
    for j in 1..grid[1] {
        let y = r.top() + r.height() * j as f32 / gy;
        painter.line_segment(
            [pos2(r.left(), y), pos2(r.right(), y)],
            Stroke::new(0.6, gcol),
        );
    }
    // centre axes a little brighter
    let cx = r.center().x;
    let cy = r.center().y;
    let acol = rgb(0x2A4A3B);
    painter.line_segment(
        [pos2(cx, r.top()), pos2(cx, r.bottom())],
        Stroke::new(0.8, acol),
    );
    painter.line_segment(
        [pos2(r.left(), cy), pos2(r.right(), cy)],
        Stroke::new(0.8, acol),
    );
    let Some(ss) = ss else {
        return;
    };
    let to_screen = |p: [f32; 2]| {
        pos2(
            r.left() + p[0] / gx * r.width(),
            cy - p[1] / gy * r.height(),
        )
    };
    let clip = painter.with_clip_rect(r.shrink(1.0));
    for (ti, t) in ss.traces.iter().enumerate() {
        let col = channels
            .iter()
            .find(|c| c.id == t.channel)
            .map(|c| hex(&c.color, rgb(0xF2D03B)))
            .unwrap_or_else(|| match ti {
                0 => rgb(0xF2D03B),
                1 => rgb(0x4CC3F0),
                2 => rgb(0xE05BD8),
                _ => rgb(0x6FD48F),
            });
        let mut pts: Vec<Pos2> = Vec::with_capacity(t.points.len() * 2);
        for (i, p) in t.points.iter().enumerate() {
            let s = to_screen(*p);
            if t.digital
                && i > 0
                && let Some(prev) = pts.last().copied()
            {
                pts.push(pos2(s.x, prev.y));
            }
            pts.push(s);
        }
        if pts.len() >= 2 {
            clip.add(egui::Shape::line(pts, Stroke::new(1.4, col)));
        }
    }
    for c in &ss.cursors {
        let x = r.left() + c / gx * r.width();
        clip.extend(egui::Shape::dashed_line(
            &[pos2(x, r.top()), pos2(x, r.bottom())],
            Stroke::new(1.0, rgb(0xF7C77A)),
            3.0,
            2.0,
        ));
    }
    let fs = (r.height() * 0.06).clamp(5.0, 11.0);
    for m in &ss.markers {
        let p = to_screen([m.x, m.y]);
        ptext(
            &clip,
            p,
            Align2::LEFT_BOTTOM,
            &m.text,
            mono(fs),
            rgb(0xEAEAEA),
        );
    }
    if !ss.header.is_empty() {
        ptext(
            &clip,
            r.left_top() + vec2(4.0, 3.0),
            Align2::LEFT_TOP,
            &ss.header,
            mono(fs),
            rgb(0xB5D88A),
        );
    }
    let _ = kind;
}

/// Display text for AccessKit / tests: `1.882 V AUTO DC`.
pub fn display_text(panel: &Panel, state: Option<&PanelState>) -> Option<String> {
    let id = panel.elements.iter().find_map(|e| match e {
        Element::Display { id, .. } => Some(id.clone()),
        _ => None,
    })?;
    let d = state.and_then(|s| s.displays.get(&id));
    Some(match d {
        Some(d) => {
            let mut s = format!("{} {}", d.text, d.unit).trim().to_string();
            if !d.annunciators.is_empty() {
                s.push(' ');
                s.push_str(&d.annunciators.join(" "));
            }
            s
        }
        None => "----".into(),
    })
}

/// The current position label of the first rotary (`V⎓`).
pub fn dial_label(panel: &Panel, state: Option<&PanelState>) -> Option<String> {
    panel.elements.iter().find_map(|e| match e {
        Element::Rotary {
            id,
            positions,
            default,
            ..
        } => {
            let i = control_value(state, id, *default as f64).round().max(0.0) as usize;
            positions.get(i).map(|p| p.label.clone())
        }
        _ => None,
    })
}

/// Live interaction with a panel's controls: one AccessKit widget per control (named
/// `<name> HOLD`, `<name> dial`, `<name> dial Ω`, …) laid over the painted face. Returns the
/// control values to send (`(control id, value)`), in order.
pub fn controls(
    ui: &mut Ui,
    face: &Face,
    panel: &Panel,
    state: Option<&PanelState>,
    name: &str,
    base: egui::Id,
) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for (k, el) in panel.elements.iter().enumerate() {
        match el {
            Element::Button {
                id,
                rect,
                label,
                momentary,
                ..
            } => {
                let r = face.r(*rect);
                let resp = ui.interact(r, base.with(("btn", k)), Sense::click());
                let text = format!("{name} {label}");
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
                if *momentary {
                    if resp.is_pointer_button_down_on()
                        && !ui.data(|d| d.get_temp::<bool>(resp.id).unwrap_or(false))
                    {
                        ui.data_mut(|d| d.insert_temp(resp.id, true));
                        out.push((id.clone(), 1.0));
                    }
                    if !resp.is_pointer_button_down_on()
                        && ui.data(|d| d.get_temp::<bool>(resp.id).unwrap_or(false))
                    {
                        ui.data_mut(|d| d.insert_temp(resp.id, false));
                        out.push((id.clone(), 0.0));
                    }
                    if resp.clicked() && !ui.data(|d| d.get_temp::<bool>(resp.id).unwrap_or(false))
                    {
                        // keyboard / AccessKit click: press + release
                        out.push((id.clone(), 1.0));
                        out.push((id.clone(), 0.0));
                    }
                } else if resp.clicked() {
                    out.push((id.clone(), 1.0));
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
            Element::Rotary {
                id,
                rect,
                positions,
                default,
            } => {
                let r = face.r(*rect);
                let c = r.center();
                let outer = r.width().min(r.height()) / 2.0;
                let cur = control_value(state, id, *default as f64).round().max(0.0) as usize;
                // position labels: click to turn there
                let fs = (outer * 0.17).clamp(4.0, 12.0);
                for (i, p) in positions.iter().enumerate() {
                    let at = c + dir(position_angle(positions, i)) * (outer * 0.84);
                    let lr = Rect::from_center_size(at, vec2(fs * 2.2, fs * 1.6));
                    let resp = ui.interact(lr, base.with(("pos", k, i)), Sense::click());
                    let text = format!("{name} dial {}", p.label);
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::RadioButton, true, i == cur, &text)
                    });
                    if resp.clicked() && i != cur {
                        out.push((id.clone(), i as f64));
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
                // the knob: click = next position, drag = turn
                let kr = Rect::from_center_size(c, vec2(outer * 1.3, outer * 1.3));
                let resp = ui.interact(kr, base.with(("dial", k)), Sense::click_and_drag());
                let cur_label = positions
                    .get(cur)
                    .map(|p| p.label.clone())
                    .unwrap_or_default();
                let text = format!("{name} dial: {cur_label}");
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, &text));
                let n = positions.len();
                if n > 0 {
                    if resp.dragged()
                        && let Some(pp) = resp.interact_pointer_pos()
                        && let Some(i) = nearest_position(positions, c, pp)
                        && i != cur
                    {
                        out.push((id.clone(), i as f64));
                    } else if resp.clicked() {
                        out.push((id.clone(), ((cur + 1) % n) as f64));
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                }
            }
            Element::Knob {
                id,
                rect,
                label,
                min,
                max,
                default,
                step,
                log,
                unit,
            } => {
                let r = face.r(*rect);
                let resp = ui.interact(r, base.with(("knob", k)), Sense::click_and_drag());
                let v = control_value(state, id, *default);
                let text = format!("{name} {label}: {}", fmt_value(v, unit));
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, &text));
                if resp.dragged() {
                    let d = resp.drag_delta();
                    let dt = ((d.x - d.y) / 160.0) as f64;
                    if dt != 0.0 {
                        let t = knob_fraction(v, *min, *max, *log) + dt;
                        out.push((id.clone(), knob_value(t, *min, *max, *log, *step)));
                    }
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
            }
            Element::Switch {
                id,
                rect,
                label,
                options,
                default,
            } => {
                let r = face.r(*rect);
                let resp = ui.interact(r, base.with(("sw", k)), Sense::click());
                let cur = control_value(state, id, *default as f64).round().max(0.0) as usize;
                let text = format!(
                    "{name} {label}: {}",
                    options.get(cur).cloned().unwrap_or_default()
                );
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
                if resp.clicked() && !options.is_empty() {
                    // click the option under the pointer, else the next one
                    let n = options.len();
                    let i = resp
                        .interact_pointer_pos()
                        .map(|p| (((p.x - r.left()) / r.width()) * n as f32).floor() as usize)
                        .map(|i| i.min(n - 1))
                        .filter(|i| *i != cur)
                        .unwrap_or((cur + 1) % n);
                    out.push((id.clone(), i as f64));
                }
            }
            Element::Display { rect, .. } => {
                let r = face.r(*rect);
                let resp = ui.interact(r, base.with(("disp", k)), Sense::hover());
                let text = format!(
                    "{name} display: {}",
                    display_text(panel, state).unwrap_or_default()
                );
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
            }
            _ => {}
        }
    }
    out
}

/// A self-contained instrument widget (Plugins screen detail art, lesson panels): allocates
/// `size`, paints the face and handles its controls.
pub fn instrument_widget(
    ui: &mut Ui,
    panel: &Panel,
    state: Option<&PanelState>,
    size: egui::Vec2,
    name: &str,
) -> Vec<(String, f64)> {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let face = Face::fit(panel, rect);
    paint(ui.painter(), &face, panel, state);
    controls(
        ui,
        &face,
        panel,
        state,
        name,
        egui::Id::unique(("instrument", name)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dmm() -> Panel {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/dmm-handheld/panel.json");
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn symbols_without_glyphs_are_spelled_out_or_drawn() {
        assert_eq!(
            glyph_safe("V⎓ · ∿ ⊓ ⋀ ⎍ · 10 MΩ ∥ 100 pF"),
            "V DC · sine square triangle pulse · 10 MΩ ‖ 100 pF"
        );
        // ptext lays a shaped symbol out between its text runs
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            let f = font::mono(12.0);
            let plain = ptext(
                painter,
                pos2(0.0, 0.0),
                Align2::LEFT_TOP,
                "AB",
                f.clone(),
                Color32::WHITE,
            );
            let with = ptext(
                painter,
                pos2(0.0, 0.0),
                Align2::LEFT_TOP,
                "A∿B",
                f.clone(),
                Color32::WHITE,
            );
            assert!((with.width() - plain.width() - symbol_width('∿', 12.0)).abs() < 0.5);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn dial_positions_and_display_text() {
        let p = dmm();
        let positions = p
            .elements
            .iter()
            .find_map(|e| match e {
                Element::Rotary { positions, .. } => Some(positions.clone()),
                _ => None,
            })
            .unwrap();
        let c = pos2(100.0, 100.0);
        // straight up-left (−75°) is V⎓
        let at = c + dir(-75.0) * 30.0;
        assert_eq!(nearest_position(&positions, c, at), Some(2));
        let st: PanelState = serde_json::from_value(serde_json::json!({
            "displays": {"lcd": {"text": "1.882", "unit": "V", "annunciators": ["AUTO", "DC"]}},
            "controls": {"dial": 4}
        }))
        .unwrap();
        assert_eq!(display_text(&p, Some(&st)).unwrap(), "1.882 V AUTO DC");
        assert_eq!(dial_label(&p, Some(&st)).unwrap(), "Ω");
        assert_eq!(dial_label(&p, None).unwrap(), "V⎓");
        assert!(
            (knob_value(knob_fraction(1000.0, 1.0, 1e6, true), 1.0, 1e6, true, None) - 1000.0)
                .abs()
                < 1e-6
        );
        let f = Face::fit(&p, Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)));
        assert!((f.rect.height() - 200.0).abs() < 0.01 && (f.rect.width() - 100.0).abs() < 0.01);
    }
}
