//! Building blocks, semiconductors and indicators: axial parts drawn between their (possibly
//! bent) leads, standing parts above their leads, DIP packages, switches and LED displays.

use super::*;
use eframe::egui::Align2;

type P2 = (f64, f64);

fn sub(a: P2, b: P2) -> P2 {
    (a.0 - b.0, a.1 - b.1)
}

fn norm(v: P2) -> (P2, f64) {
    let l = (v.0 * v.0 + v.1 * v.1).sqrt();
    if l < 1e-9 {
        ((1.0, 0.0), 0.0)
    } else {
        ((v.0 / l, v.1 / l), l)
    }
}

/// Label colour: dark on a breadboard, muted on the bench mat.
fn label_color(cx: &ArtCtx) -> Color32 {
    if cx.on_board {
        rgb(0x3A352D)
    } else {
        rgb(0xA3A3A3)
    }
}

/// `10k`, `220Ω`, `4.7k`, `1M`.
pub fn ohms_label(v: f64) -> String {
    let (n, suf) = if v >= 1e6 {
        (v / 1e6, "M")
    } else if v >= 1e3 {
        (v / 1e3, "k")
    } else {
        (v, "Ω")
    };
    let s = if (n - n.round()).abs() < 1e-6 {
        format!("{}", n.round() as i64)
    } else {
        let t = format!("{n:.1}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    format!("{s}{suf}")
}

/// A capsule body along `dir` centred at `mid` with a top-lit highlight.
fn capsule(pen: &Pen, mid: P2, dir: P2, len: f64, rad: f64, fill: Color32) {
    let n = (-dir.1, dir.0);
    let hl = len / 2.0 - rad * 0.6;
    let a = (mid.0 - dir.0 * hl, mid.1 - dir.1 * hl);
    let b = (mid.0 + dir.0 * hl, mid.1 + dir.1 * hl);
    let quad = [
        (a.0 + n.0 * rad, a.1 + n.1 * rad),
        (b.0 + n.0 * rad, b.1 + n.1 * rad),
        (b.0 - n.0 * rad, b.1 - n.1 * rad),
        (a.0 - n.0 * rad, a.1 - n.1 * rad),
    ];
    // drop shadow
    let sh: Vec<P2> = quad.iter().map(|q| (q.0 + 0.25, q.1 + 0.45)).collect();
    pen.poly(&sh, Color32::from_black_alpha(70));
    pen.circle(a.0 + 0.25, a.1 + 0.45, rad, Color32::from_black_alpha(70));
    pen.circle(b.0 + 0.25, b.1 + 0.45, rad, Color32::from_black_alpha(70));
    pen.poly(&quad, fill);
    pen.circle(a.0, a.1, rad, fill);
    pen.circle(b.0, b.1, rad, fill);
    // highlight toward the screen top
    let up = if n.1 <= 0.0 { n } else { (-n.0, -n.1) };
    let h0 = (a.0 + up.0 * rad * 0.45, a.1 + up.1 * rad * 0.45);
    let h1 = (b.0 + up.0 * rad * 0.45, b.1 + up.1 * rad * 0.45);
    pen.line(
        h0,
        h1,
        pen.px(rad * 0.38, 1.0),
        with_alpha(Color32::WHITE, 0.28),
    );
    // bottom shade
    let d0 = (a.0 - up.0 * rad * 0.55, a.1 - up.1 * rad * 0.55);
    let d1 = (b.0 - up.0 * rad * 0.55, b.1 - up.1 * rad * 0.55);
    pen.line(
        d0,
        d1,
        pen.px(rad * 0.4, 1.0),
        with_alpha(Color32::BLACK, 0.18),
    );
}

/// A band across a capsule at offset `t` (units from the middle).
fn band(pen: &Pen, mid: P2, dir: P2, t: f64, width: f64, rad: f64, color: Color32) {
    let n = (-dir.1, dir.0);
    let c = (mid.0 + dir.0 * t, mid.1 + dir.1 * t);
    let hw = width / 2.0;
    let q = [
        (c.0 - dir.0 * hw + n.0 * rad, c.1 - dir.1 * hw + n.1 * rad),
        (c.0 + dir.0 * hw + n.0 * rad, c.1 + dir.1 * hw + n.1 * rad),
        (c.0 + dir.0 * hw - n.0 * rad, c.1 + dir.1 * hw - n.1 * rad),
        (c.0 - dir.0 * hw - n.0 * rad, c.1 - dir.1 * hw - n.1 * rad),
    ];
    pen.poly(&q, color);
}

/// Leads in holes + a straight wire between them under an axial body.
fn axial_wires(pen: &Pen, a: P2, b: P2) {
    pen.wline(
        (a.0 + 0.15, a.1 + 0.25),
        (b.0 + 0.15, b.1 + 0.25),
        0.22,
        Color32::from_black_alpha(60),
    );
    pen.wline(a, b, 0.22, rgb(0x9AA0A6));
    pen.wline((a.0, a.1 - 0.06), (b.0, b.1 - 0.06), 0.07, rgb(0xD5DADF));
    for q in [a, b] {
        pen.circle(q.0, q.1, 0.16, rgb(0x6E747C));
    }
}

fn axial_frame(cx: &ArtCtx) -> Option<(P2, P2, P2, P2, f64)> {
    let (a, b) = cx.ends()?;
    let (dir, len) = norm(sub(b, a));
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    Some((a, b, mid, dir, len))
}

pub(super) fn above_label(pen: &Pen, cx: &ArtCtx, x: f64, y: f64, text: &str) {
    if pen.view.scale < 5.0 {
        return;
    }
    pen.text_at(
        x,
        y,
        text,
        0.68,
        font::mono,
        label_color(cx),
        Align2::CENTER_BOTTOM,
    );
}

pub fn resistor(pen: &Pen, cx: &ArtCtx) {
    let Some((a, b, mid, dir, len)) = axial_frame(cx) else {
        return;
    };
    axial_wires(pen, a, b);
    let body_len = (len * 0.62).clamp(1.6, 2.8);
    let rad = 0.46;
    let body = if cx.damaged() {
        rgb(0x5A4632)
    } else {
        rgb(0xE2C896)
    };
    capsule(pen, mid, dir, body_len, rad, body);
    let ohms = cx.prop_f64("resistance").unwrap_or(1000.0);
    let tol = cx.prop_text("tolerance").parse::<f64>().unwrap_or(5.0);
    let bands = resistor_bands(ohms, tol);
    let n = bands.len();
    let span = body_len * 0.62;
    for (i, d) in bands.iter().enumerate() {
        let t = if i == n - 1 {
            span / 2.0 + 0.05
        } else {
            -span / 2.0 + i as f64 * (span * 0.72 / (n - 2).max(1) as f64)
        };
        band(pen, mid, dir, t, 0.2, rad * 0.98, band_color(*d));
    }
    let top = a.1.min(b.1) - 0.75;
    above_label(
        pen,
        cx,
        mid.0,
        top,
        &format!("{} {}", cx.part.reference, ohms_label(ohms)),
    );
}

pub fn diode(pen: &Pen, cx: &ArtCtx) {
    let Some((a, b, mid, dir, len)) = axial_frame(cx) else {
        return;
    };
    axial_wires(pen, a, b);
    let model = cx.prop_text("model").to_ascii_lowercase();
    let glass = model.contains("4148");
    let body_len = (len * 0.55).clamp(1.4, 2.4);
    let (body, mark) = if glass {
        (rgb(0xE88A3A), rgb(0x1A1A1A))
    } else {
        (rgb(0x1E1E1E), rgb(0xC9CED4))
    };
    capsule(
        pen,
        mid,
        dir,
        body_len,
        if glass { 0.32 } else { 0.42 },
        body,
    );
    // cathode band at the k end (second lead)
    band(pen, mid, dir, body_len / 2.0 - 0.45, 0.24, 0.42, mark);
    let top = a.1.min(b.1) - 0.7;
    let label = if model.is_empty() {
        cx.part.reference.clone()
    } else {
        format!("{} {}", cx.part.reference, cx.prop_text("model"))
    };
    above_label(pen, cx, mid.0, top, &label);
}

pub fn inductor(pen: &Pen, cx: &ArtCtx) {
    let Some((a, b, mid, _, _)) = axial_frame(cx) else {
        return;
    };
    let base = a.1.min(b.1) - 1.0;
    leg(pen, a, (a.0, base));
    leg(pen, b, (b.0, base));
    let r = WRect::new(mid.0 - 1.5, base - 2.4, mid.0 + 1.5, base + 0.1);
    pen.shadow(r, 0.4, 5.0, 3.0, 0.45);
    pen.grad(r, 0.5, rgb(0x3A3A3A), rgb(0x1E1E1E));
    for i in 0..5 {
        let y = base - 2.0 + i as f64 * 0.45;
        pen.rect(
            WRect::new(mid.0 - 1.3, y, mid.0 + 1.3, y + 0.28),
            0.12,
            rgb(0xB87333),
        );
    }
    above_label(pen, cx, mid.0, base - 2.6, &cx.part.reference);
}

pub fn soil_fork(pen: &Pen, cx: &ArtCtx) {
    let Some((a, b, mid, _, _)) = axial_frame(cx) else {
        return;
    };
    let top = a.1.min(b.1);
    let r = WRect::new(mid.0 - 1.6, top - 1.2, mid.0 + 1.6, top + 0.4);
    pen.shadow(r, 0.2, 4.0, 3.0, 0.4);
    pen.rect(r, 0.2, rgb(0x1F4E8C));
    for x in [mid.0 - 0.8, mid.0 + 0.8] {
        let prong = WRect::new(x - 0.35, top + 0.3, x + 0.35, top + 4.5);
        pen.rect(prong, 0.2, rgb(0x1F4E8C));
        pen.rect(prong.expand(-0.12), 0.15, rgb(0xC9A54A));
    }
    for q in [a, b] {
        pen.circle(q.0, q.1, 0.2, rgb(0xC9A54A));
    }
}

pub fn speaker(pen: &Pen, cx: &ArtCtx) {
    let Some((a, b, mid, _, _)) = axial_frame(cx) else {
        return;
    };
    let cy = a.1.min(b.1) - 3.2;
    pen.shadow(
        WRect::new(mid.0 - 2.6, cy - 2.6, mid.0 + 2.6, cy + 2.6),
        2.6,
        8.0,
        4.0,
        0.5,
    );
    pen.disc(mid.0, cy, 2.6, rgb(0x4A4A4A), rgb(0x1A1A1A));
    pen.circle(mid.0, cy, 1.7, rgb(0x262626));
    pen.circle(mid.0, cy, 0.7, rgb(0x3A3A3A));
    for q in [a, b] {
        leg(pen, q, (q.0, cy + 2.2));
    }
    buzz_waves(pen, cx, mid.0 + 2.8, cy);
}

/// Sound arcs next to an active buzzer / speaker.
fn buzz_waves(pen: &Pen, cx: &ArtCtx, x: f64, y: f64) {
    let Some(Look::Buzzer { level, freq_hz }) = cx.look() else {
        return;
    };
    if *level <= 0.02 || *freq_hz <= 0.0 {
        return;
    }
    for i in 0..3 {
        let ph = ((cx.t * 2.5 + i as f64 / 3.0) % 1.0) as f32;
        let r = 0.6 + ph as f64 * 2.4;
        let a = level.min(1.0) * (1.0 - ph) * 0.9;
        let pts: Vec<P2> = (0..9)
            .map(|k| {
                let t = (-50.0 + k as f64 * 12.5f64).to_radians();
                (x + r * t.cos(), y + r * t.sin())
            })
            .collect();
        pen.polyline(&pts, pen.px(0.14, 1.2), with_alpha(rgb(0x4CC3F0), a));
    }
}

/// Standing body frame: (centre x, base y = where legs leave the body).
fn stand(cx: &ArtCtx, lift: f64) -> (f64, f64) {
    let (mx, _) = cx.lead_center();
    let top = cx.leads.iter().map(|l| l.2).fold(f64::INFINITY, f64::min);
    (mx, if top.is_finite() { top - lift } else { 0.0 })
}

fn legs_to(pen: &Pen, cx: &ArtCtx, base: f64, spread: f64) {
    let (mx, _) = cx.lead_center();
    for (_, x, y) in cx.leads {
        let bx = mx + (x - mx).clamp(-spread, spread);
        leg(pen, (*x, *y), (bx, base));
    }
}

pub fn led(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 1.6);
    legs_to(pen, cx, base, 0.5);
    let color = led_color(&cx.prop_text("color"));
    let b = match cx.look() {
        Some(Look::Led { brightness, .. }) => *brightness,
        _ => 0.0,
    };
    let b = if cx.damaged() { 0.0 } else { b.clamp(0.0, 1.0) };
    // perceived brightness: a few mA already look bright
    let lum = if b > 0.002 { b.powf(0.45) } else { 0.0 };
    let dim = darken(color, 0.58);
    let body_c = if cx.damaged() {
        rgb(0x3A2E28)
    } else {
        lit(dim, color, lum)
    };
    let dome = WRect::new(mx - 1.0, base - 3.1, mx + 1.0, base + 0.05);
    pen.shadow(dome, 1.0, 5.0, 3.0, 0.5);
    pen.rect(dome, 1.0, body_c);
    pen.rect_stroke(dome, 1.0, pen.px(0.08, 0.8), darken(color, 0.35));
    // flange
    pen.rect(
        WRect::new(mx - 1.35, base - 0.42, mx + 1.35, base + 0.05),
        0.12,
        darken(body_c, 0.12),
    );
    // specular highlight
    pen.ellipse(
        mx - 0.4,
        base - 2.35,
        0.28,
        0.55,
        with_alpha(Color32::WHITE, 0.45),
    );
    if lum > 0.02 {
        pen.glow(
            mx,
            base - 1.8,
            3.6 + 8.0 * lum as f64,
            color,
            (1.1 * lum).min(1.0),
        );
    }
    above_label(pen, cx, mx, base - 3.4, &cx.part.reference);
}

pub fn rgb_led(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 2.4);
    legs_to(pen, cx, base, 0.9);
    let (r, g, b) = match cx.look() {
        Some(Look::RgbLed { r, g, b }) => (*r, *g, *b),
        _ => (0.0, 0.0, 0.0),
    };
    let lvl = r.max(g).max(b);
    let mixc = Color32::from_rgb(
        (255.0 * r.clamp(0.0, 1.0)) as u8,
        (255.0 * g.clamp(0.0, 1.0)) as u8,
        (255.0 * b.clamp(0.0, 1.0)) as u8,
    );
    let body = crate::theme::mix(rgb(0xD8D8D2), lighten(mixc, 0.3), lvl);
    let dome = WRect::new(mx - 1.15, base - 3.3, mx + 1.15, base + 0.05);
    pen.shadow(dome, 1.15, 5.0, 3.0, 0.5);
    pen.rect(dome, 1.15, body);
    pen.rect(
        WRect::new(mx - 1.45, base - 0.42, mx + 1.45, base + 0.05),
        0.12,
        darken(body, 0.15),
    );
    pen.ellipse(
        mx - 0.45,
        base - 2.5,
        0.3,
        0.55,
        with_alpha(Color32::WHITE, 0.5),
    );
    if lvl > 0.02 {
        pen.glow(mx, base - 1.9, 2.4 + 4.0 * lvl as f64, mixc, 0.9 * lvl);
    }
    above_label(pen, cx, mx, base - 3.6, &cx.part.reference);
}

pub fn cap_ceramic(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 1.2);
    legs_to(pen, cx, base, 0.6);
    let cy = base - 0.9;
    pen.shadow(
        WRect::new(mx - 1.0, cy - 0.9, mx + 1.0, cy + 0.9),
        0.9,
        4.0,
        2.0,
        0.45,
    );
    pen.disc(mx, cy, 0.95, rgb(0xF0C860), rgb(0xB8862E));
    let c = cx.prop_f64("capacitance").unwrap_or(100e-9);
    pen.text(mx, cy, &cap_code(c), 0.5, font::mono_medium, rgb(0x5A3F12));
    above_label(pen, cx, mx, cy - 1.15, &cx.part.reference);
}

/// `104` style code from farads.
fn cap_code(f: f64) -> String {
    let pf = f * 1e12;
    if pf < 10.0 {
        return format!("{pf:.0}");
    }
    let e = pf.log10().floor() as i32 - 1;
    let m = (pf / 10f64.powi(e)).round() as i64;
    format!("{m}{e}")
}

pub fn cap_electrolytic(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.9);
    legs_to(pen, cx, base, 0.4);
    let popped = cx.damaged();
    let body = WRect::new(mx - 0.95, base - 3.6, mx + 0.95, base);
    pen.shadow(body, 0.3, 5.0, 3.0, 0.5);
    pen.hgrad(body, 0.3, rgb(0x2C55A0), rgb(0x16305E));
    // minus stripe on the − lead side (second lead)
    let minus_right = cx.leads.len() == 2 && cx.leads[1].1 >= cx.leads[0].1;
    let sx = if minus_right { mx + 0.35 } else { mx - 0.65 };
    pen.rect(
        WRect::new(sx, base - 3.5, sx + 0.3, base - 0.1),
        0.0,
        rgb(0xB8C4D8),
    );
    for k in 0..4 {
        let y = base - 3.0 + k as f64 * 0.8;
        pen.text(sx + 0.15, y, "−", 0.4, font::mono, rgb(0x16305E));
    }
    // top: vent cross or a burst
    let top = WRect::new(mx - 0.95, base - 3.95, mx + 0.95, base - 3.25);
    pen.rect(
        top,
        0.35,
        if popped { rgb(0x3A3A3A) } else { rgb(0xC9CED4) },
    );
    if !popped {
        pen.line(
            (mx - 0.5, base - 3.6),
            (mx + 0.5, base - 3.6),
            pen.px(0.06, 0.6),
            rgb(0x8E949B),
        );
    }
    above_label(pen, cx, mx, base - 4.2, &cx.part.reference);
}

pub fn ldr(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 1.6);
    legs_to(pen, cx, base, 0.6);
    let cy = base - 1.4;
    pen.shadow(
        WRect::new(mx - 1.4, cy - 1.4, mx + 1.4, cy + 1.4),
        1.4,
        5.0,
        3.0,
        0.45,
    );
    pen.disc(mx, cy, 1.45, rgb(0xF0C878), rgb(0xC08A3A));
    pen.circle_stroke(mx, cy, 1.45, pen.px(0.1, 1.0), rgb(0xA27A2C));
    // the CdS serpentine track
    let mut track = Vec::new();
    for i in 0..5 {
        let x = mx - 1.0 + i as f64 * 0.5;
        let (y0, y1) = if i % 2 == 0 {
            (cy - 0.6, cy + 0.6)
        } else {
            (cy + 0.6, cy - 0.6)
        };
        track.push((x, y0));
        track.push((x, y1));
    }
    pen.polyline(&track, pen.px(0.18, 1.2), rgb(0x8A3B1E));
    above_label(pen, cx, mx, cy - 1.7, &cx.part.reference);
}

pub fn thermistor(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 1.4);
    legs_to(pen, cx, base, 0.4);
    let cy = base - 0.8;
    pen.disc(mx, cy, 0.8, rgb(0x3A5EA8), rgb(0x1A2F5A));
    pen.text(mx, cy, "NTC", 0.42, font::mono_medium, rgb(0xDCE6F5));
    above_label(pen, cx, mx, cy - 1.0, &cx.part.reference);
}

pub fn to92(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.9);
    legs_to(pen, cx, base, 1.0);
    let body = WRect::new(mx - 1.35, base - 2.4, mx + 1.35, base);
    pen.shadow(body, 0.6, 4.0, 2.0, 0.5);
    pen.grad(body, 0.6, rgb(0x2E2E2E), rgb(0x141414));
    pen.rect(
        WRect::new(mx - 1.35, base - 0.7, mx + 1.35, base),
        0.1,
        rgb(0x1A1A1A),
    );
    let pn = if !cx.def.part_number.is_empty() {
        cx.def.part_number.clone()
    } else {
        cx.prop_text("model")
    };
    let short = pn.trim_start_matches("2N").to_string();
    pen.text(mx, base - 1.4, &short, 0.55, font::mono, rgb(0x9AA6AE));
    above_label(pen, cx, mx, base - 2.6, &cx.part.reference);
}

pub fn to220(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.8);
    legs_to(pen, cx, base, 1.0);
    let tab = WRect::new(mx - 1.5, base - 5.0, mx + 1.5, base - 2.6);
    pen.shadow(tab, 0.15, 5.0, 3.0, 0.45);
    pen.grad(tab, 0.15, rgb(0xD4D8DD), rgb(0x9DA3AB));
    pen.circle(mx, base - 4.0, 0.5, rgb(0x2A2A2A));
    let body = WRect::new(mx - 1.5, base - 2.8, mx + 1.5, base);
    pen.grad(body, 0.1, rgb(0x2E2E2E), rgb(0x141414));
    pen.text(
        mx,
        base - 1.4,
        &cx.def.part_number,
        0.5,
        font::mono,
        rgb(0x9AA6AE),
    );
    above_label(pen, cx, mx, base - 5.2, &cx.part.reference);
}

pub fn small_module(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.7);
    legs_to(pen, cx, base, 1.0);
    let body = WRect::new(mx - 1.5, base - 2.2, mx + 1.5, base);
    pen.shadow(body, 0.2, 4.0, 2.0, 0.45);
    pen.grad(body, 0.2, rgb(0x2B6E4F), rgb(0x1E5139));
    pen.rect(
        WRect::new(mx - 0.7, base - 1.7, mx + 0.7, base - 0.5),
        0.05,
        rgb(0x161616),
    );
    above_label(pen, cx, mx, base - 2.4, &cx.part.reference);
}

/// Knob centre (world) and radius for a pot (the canvas makes it draggable).
pub fn pot_knob(cx_center: (f64, f64), base: f64, trimmer: bool) -> ((f64, f64), f64) {
    if trimmer {
        ((cx_center.0, base - 1.25), 0.8)
    } else {
        ((cx_center.0, base - 1.7), 1.25)
    }
}

pub fn pot(pen: &Pen, cx: &ArtCtx) {
    let trimmer = cx.def.bench.art == "trimmer";
    let (mx, base) = stand(cx, 0.6);
    legs_to(pen, cx, base, 1.2);
    let pos = match cx.look() {
        Some(Look::Knob { position }) => *position,
        _ => cx.prop_f64("position").unwrap_or(0.5),
    }
    .clamp(0.0, 1.0);
    let (k, kr) = pot_knob((mx, 0.0), base, trimmer);
    let half = if trimmer { 1.2 } else { 1.65 };
    let body = WRect::new(mx - half, base - 2.0 * half, mx + half, base);
    pen.shadow(body, 0.2, 5.0, 3.0, 0.5);
    pen.grad(body, 0.25, rgb(0x2D6BC4), rgb(0x1C4B92));
    pen.shadow(
        WRect::new(k.0 - kr, k.1 - kr, k.0 + kr, k.1 + kr),
        kr,
        3.0,
        1.5,
        0.5,
    );
    if trimmer {
        pen.disc(k.0, k.1, kr, rgb(0xF2F2F2), rgb(0xB8BDC4));
    } else {
        pen.disc(k.0, k.1, kr, rgb(0x4A4A4A), rgb(0x1E1E1E));
        // knurling
        for i in 0..16 {
            let a = i as f64 / 16.0 * std::f64::consts::TAU;
            pen.line(
                (k.0 + a.cos() * kr * 0.82, k.1 + a.sin() * kr * 0.82),
                (k.0 + a.cos() * kr, k.1 + a.sin() * kr),
                pen.px(0.05, 0.6),
                rgb(0x5A5A5A),
            );
        }
    }
    let ang = (-225.0 + 270.0 * pos).to_radians();
    let ind = if trimmer {
        rgb(0x3A3A3A)
    } else {
        rgb(0xF2A33A)
    };
    pen.line(
        (k.0 + ang.cos() * kr * 0.15, k.1 + ang.sin() * kr * 0.15),
        (k.0 + ang.cos() * kr * 0.85, k.1 + ang.sin() * kr * 0.85),
        pen.px(0.16, 1.4),
        ind,
    );
    above_label(pen, cx, mx, base - 2.0 * half - 0.2, &cx.part.reference);
}

pub fn slide_switch(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.4);
    legs_to(pen, cx, base, 1.0);
    let on = match cx.look() {
        Some(Look::Switch { on }) => *on,
        _ => cx.prop_bool("on"),
    };
    let body = WRect::new(mx - 1.6, base - 1.2, mx + 1.6, base);
    pen.shadow(body, 0.15, 4.0, 2.0, 0.45);
    pen.grad(body, 0.15, rgb(0x2E2E2E), rgb(0x161616));
    let kx = if on { mx + 0.45 } else { mx - 0.45 };
    pen.rect(
        WRect::new(kx - 0.45, base - 1.6, kx + 0.45, base - 0.6),
        0.1,
        rgb(0xB8BDC4),
    );
    above_label(pen, cx, mx, base - 1.8, &cx.part.reference);
}

pub fn tilt_switch(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.6);
    legs_to(pen, cx, base, 0.5);
    let tilted = matches!(cx.look(), Some(Look::Switch { on: true })) || cx.prop_bool("tilted");
    let body = WRect::new(mx - 0.7, base - 2.2, mx + 0.7, base);
    pen.shadow(body, 0.6, 4.0, 2.0, 0.4);
    pen.hgrad(body, 0.6, rgb(0x6E8FB8), rgb(0x2A4466));
    let by = if tilted { base - 0.6 } else { base - 1.6 };
    pen.disc(mx, by, 0.4, rgb(0xEDEDED), rgb(0x8E949B));
    above_label(pen, cx, mx, base - 2.4, &cx.part.reference);
}

pub fn buzzer(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.5);
    legs_to(pen, cx, base, 1.2);
    let cy = base - 1.6;
    pen.shadow(
        WRect::new(mx - 1.6, cy - 1.6, mx + 1.6, cy + 1.6),
        1.6,
        6.0,
        3.0,
        0.5,
    );
    pen.disc(mx, cy, 1.6, rgb(0x3A3A3A), rgb(0x121212));
    pen.circle(mx, cy, 0.35, rgb(0x050505));
    pen.text(
        mx - 0.9,
        cy - 0.9,
        "+",
        0.6,
        font::mono_medium,
        rgb(0xA3A3A3),
    );
    buzz_waves(pen, cx, mx + 1.9, cy);
    above_label(pen, cx, mx, cy - 1.8, &cx.part.reference);
}

pub fn source(pen: &Pen, cx: &ArtCtx) {
    let (mx, base) = stand(cx, 0.6);
    // a SPICE card's row of up to 16 nodes: a SIP-style body over all of them
    let span = cx.leads.iter().map(|l| l.1).fold(f64::MIN, f64::max)
        - cx.leads.iter().map(|l| l.1).fold(f64::MAX, f64::min);
    let body = if cx.def.bench.art == "spice_box" && span > 2.5 {
        let used = cx.prop_f64("nodes").unwrap_or(4.0) as usize;
        for (i, (_, x, y)) in cx.leads.iter().enumerate() {
            let a = if i < used { 1.0 } else { 0.35 };
            let mut p2 = pen.clone();
            p2.alpha *= a;
            leg(&p2, (*x, *y), (*x, base));
        }
        WRect::new(
            mx - span / 2.0 - 0.6,
            base - 2.4,
            mx + span / 2.0 + 0.6,
            base,
        )
    } else {
        legs_to(pen, cx, base, 1.0);
        WRect::new(mx - 1.7, base - 2.4, mx + 1.7, base)
    };
    pen.shadow(body, 0.3, 5.0, 3.0, 0.5);
    pen.grad(body, 0.3, rgb(0x2A2A2A), rgb(0x161616));
    let label = if cx.def.bench.art == "spice_box" {
        "SPICE"
    } else if cx.def.id == "isource" {
        "I"
    } else {
        "V"
    };
    pen.text(mx, base - 1.2, label, 0.8, font::mono_medium, rgb(0x82AAFF));
    above_label(pen, cx, mx, base - 2.6, &cx.part.reference);
}

/// Tactile push button (local coordinates; leads at the four corners).
pub fn button(p: &Pen, cx: &ArtCtx) {
    let pressed = matches!(cx.look(), Some(Look::Switch { on: true }));
    let body = WRect::new(-0.2, -0.2, 3.2, 2.2);
    p.shadow(body, 0.2, 5.0, 3.0, 0.5);
    p.grad(body, 0.2, rgb(0x3A3A3A), rgb(0x1E1E1E));
    for l in &cx.def.bench.leads {
        p.rect(
            WRect::xywh(l.x - 0.25, l.y - 0.25, 0.5, 0.5),
            0.05,
            rgb(0xB8BDC4),
        );
    }
    let (r, c) = if pressed {
        (0.78, rgb(0x6E1A14))
    } else {
        (0.95, rgb(0xC8352B))
    };
    p.disc(1.5, 1.0, r, lighten(c, 0.25), c);
    p.text(
        1.5,
        -0.75,
        &cx.part.reference,
        0.62,
        font::mono,
        label_color(cx),
    );
}

/// DIP IC (local; pin 1 bottom-left, notch on the left).
pub fn dip(p: &Pen, cx: &ArtCtx) {
    let leads = &cx.def.bench.leads;
    let maxx = leads.iter().map(|l| l.x).fold(0.0, f64::max);
    let maxy = leads.iter().map(|l| l.y).fold(0.0, f64::max);
    for l in leads {
        let (y0, y1) = if l.y < 0.5 {
            (l.y, l.y + 0.6)
        } else {
            (l.y - 0.6, l.y)
        };
        p.rect(
            WRect::new(l.x - 0.18, y0, l.x + 0.18, y1),
            0.0,
            rgb(0xB8BDC4),
        );
    }
    let body = WRect::new(-0.55, 0.5, maxx + 0.55, maxy - 0.5);
    p.shadow(body, 0.15, 5.0, 3.0, 0.5);
    p.grad(body, 0.15, rgb(0x2C2C2C), rgb(0x141414));
    p.circle(-0.55, (maxy) / 2.0, 0.35, rgb(0x0A0A0A));
    p.circle(0.1, maxy - 1.0, 0.18, rgb(0x3A3A3A));
    let name = if cx.def.part_number.is_empty() {
        cx.def.name.clone()
    } else {
        cx.def.part_number.clone()
    };
    p.text(
        maxx / 2.0,
        maxy / 2.0,
        &name,
        0.62,
        font::mono,
        rgb(0xA3ACB4),
    );
    p.text(
        maxx / 2.0,
        -0.7,
        &cx.part.reference,
        0.6,
        font::mono,
        label_color(cx),
    );
}

pub fn dip_switch(p: &Pen, cx: &ArtCtx) {
    let on = match cx.look() {
        Some(Look::Switches { on }) => on.clone(),
        _ => vec![false; 4],
    };
    let body = WRect::new(-0.5, 0.45, 3.5, 2.55);
    p.shadow(body, 0.15, 5.0, 3.0, 0.5);
    p.grad(body, 0.15, rgb(0xD0473C), rgb(0xA0302A));
    for i in 0..4 {
        let x = i as f64;
        p.rect(WRect::new(x - 0.3, 0.8, x + 0.3, 2.2), 0.06, rgb(0x5A1A14));
        let up = on.get(i).copied().unwrap_or(false);
        let ky = if up { 0.9 } else { 1.5 };
        p.rect(
            WRect::new(x - 0.25, ky, x + 0.25, ky + 0.6),
            0.05,
            rgb(0xF2F2F2),
        );
    }
}

pub fn led_bar(p: &Pen, cx: &ArtCtx) {
    let leds = match cx.look() {
        Some(Look::LedArray { leds }) => leds.clone(),
        _ => Vec::new(),
    };
    let body = WRect::new(-0.55, 0.45, 9.55, 2.55);
    p.shadow(body, 0.1, 5.0, 3.0, 0.5);
    p.rect(body, 0.1, rgb(0x1A1A1A));
    for i in 0..10 {
        let x = i as f64;
        let c = leds
            .get(i)
            .map(|c| Color32::from_rgb(c.r, c.g, c.b))
            .unwrap_or(Color32::BLACK);
        let lvl = (c.r().max(c.g()).max(c.b())) as f32 / 255.0;
        let seg = WRect::new(x - 0.3, 0.8, x + 0.3, 2.2);
        p.rect(seg, 0.05, lit(rgb(0x3A1A16), lighten(c, 0.2), lvl));
        if lvl > 0.05 {
            p.glow(x, 1.5, 1.2, c, 0.5 * lvl);
        }
    }
}

const SEGS: [(f64, f64, f64, f64); 7] = [
    // a b c d e f g as (x0, y0, x1, y1) in a 3×5 cell
    (0.5, 0.0, 2.5, 0.0),
    (2.7, 0.2, 2.7, 2.3),
    (2.7, 2.7, 2.7, 4.8),
    (0.5, 5.0, 2.5, 5.0),
    (0.3, 2.7, 0.3, 4.8),
    (0.3, 0.2, 0.3, 2.3),
    (0.5, 2.5, 2.5, 2.5),
];

pub fn seven_segment(p: &Pen, cx: &ArtCtx) {
    let (bits, lvl) = match cx.look() {
        Some(Look::SevenSeg { digits, brightness }) => {
            (digits.first().copied().unwrap_or(0), *brightness)
        }
        _ => (0, 0.0),
    };
    let body = WRect::new(-0.5, -0.5, 4.5, 6.5);
    p.shadow(body, 0.15, 6.0, 3.0, 0.5);
    p.grad(body, 0.15, rgb(0x2A2A2A), rgb(0x111111));
    let on_c = rgb(0xFF4A3A);
    let off_c = rgb(0x3A1A16);
    let (ox, oy) = (0.6, 0.5);
    for (i, s) in SEGS.iter().enumerate() {
        let on = bits & (1 << i) != 0;
        let c = if on {
            lit(off_c, on_c, lvl.max(0.6))
        } else {
            off_c
        };
        p.line(
            (ox + s.0, oy + s.1),
            (ox + s.2, oy + s.3),
            p.px(0.42, 2.0),
            c,
        );
    }
    let dp = bits & 0x80 != 0;
    p.circle(ox + 3.3, oy + 5.0, 0.25, if dp { on_c } else { off_c });
    if bits != 0 && lvl > 0.0 {
        p.glow(2.0, 3.0, 4.0, on_c, 0.25 * lvl);
    }
}

pub fn ws2812(pen: &Pen, cx: &ArtCtx) {
    let leds = match cx.look() {
        Some(Look::LedArray { leds }) => leds.clone(),
        _ => Vec::new(),
    };
    let n = cx.prop_f64("count").unwrap_or(8.0).clamp(1.0, 60.0) as usize;
    let (x0, y0) = cx.leads.first().map(|l| (l.1, l.2)).unwrap_or((0.0, 0.0));
    let strip = WRect::new(x0 - 0.5, y0 - 2.2, x0 + 4.0 + n as f64 * 1.6, y0 - 0.6);
    for (_, x, y) in cx.leads {
        leg(pen, (*x, *y), (*x, y0 - 0.7));
    }
    pen.shadow(strip, 0.2, 5.0, 3.0, 0.45);
    pen.rect(strip, 0.2, rgb(0xEDEDED));
    for i in 0..n {
        let x = x0 + 1.2 + i as f64 * 1.6;
        let c = leds
            .get(i)
            .map(|c| Color32::from_rgb(c.r, c.g, c.b))
            .unwrap_or(Color32::BLACK);
        let lvl = (c.r().max(c.g()).max(c.b())) as f32 / 255.0;
        pen.rect(
            WRect::new(x - 0.55, y0 - 1.95, x + 0.55, y0 - 0.85),
            0.1,
            rgb(0xF6F6F0),
        );
        // unlit: the pale lens of a 5050 package with its die showing
        pen.circle(x, y0 - 1.4, 0.38, lit(rgb(0xDCD6C2), lighten(c, 0.3), lvl));
        if lvl < 0.3 {
            pen.rect(
                WRect::new(x - 0.12, y0 - 1.52, x + 0.12, y0 - 1.28),
                0.02,
                rgb(0x6B6658),
            );
        }
        if lvl > 0.05 {
            pen.glow(x, y0 - 1.4, 1.6 + 1.5 * lvl as f64, c, 0.8 * lvl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_labels_read_like_the_design() {
        assert_eq!(ohms_label(10_000.0), "10k");
        assert_eq!(ohms_label(220.0), "220Ω");
        assert_eq!(ohms_label(4_700.0), "4.7k");
        assert_eq!(ohms_label(1e6), "1M");
        assert_eq!(cap_code(100e-9), "104");
        assert_eq!(cap_code(22e-12), "220");
    }
}
