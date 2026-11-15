//! Sensor / display / actuator modules, power sources, world objects (pot of soil, lamp,
//! hand) and the built-in bench instruments (mini scope, DMM placeholder).

use super::*;
use eframe::egui::{Align2, Mesh, Shape, pos2, vec2};

fn pins_row(p: &Pen, cx: &ArtCtx) {
    for l in &cx.def.bench.leads {
        header_pin(p, l.x, l.y);
    }
}

fn pcb(p: &Pen, r: WRect, top: Color32, bottom: Color32) {
    p.shadow(r, 0.3, 9.0, 5.0, 0.55);
    p.grad(r, 0.3, top, bottom);
    p.rect_stroke(r, 0.3, p.px(0.05, 0.6), darken(bottom, 0.25));
}

/// Capacitive soil sensor v1.2: black probe board, pins at the top, pointed tip.
pub fn capsoil(p: &Pen, cx: &ArtCtx) {
    let body = [
        (-0.6, -0.6),
        (2.6, -0.6),
        (2.6, 8.6),
        (1.0, 10.2),
        (-0.6, 8.6),
    ];
    let shadow: Vec<(f64, f64)> = body.iter().map(|(x, y)| (x + 0.3, y + 0.5)).collect();
    p.poly(&shadow, Color32::from_black_alpha(90));
    p.poly(&body, rgb(0x161616));
    p.rect(WRect::new(-0.6, -0.6, 2.6, 2.8), 0.2, rgb(0x1E1E1E));
    // components + the sensing pad outline
    p.rect(WRect::new(0.2, 1.2, 1.8, 2.3), 0.1, rgb(0x2E2E2E));
    p.rect_stroke(
        WRect::new(-0.2, 3.4, 2.2, 8.2),
        0.2,
        p.px(0.06, 0.6),
        rgb(0x3A3A3A),
    );
    p.line((-0.4, 3.0), (2.4, 3.0), p.px(0.08, 0.8), rgb(0xEDEDED));
    pins_row(p, cx);
    p.text(1.0, 6.0, &cx.part.reference, 0.6, font::mono, rgb(0x9AA6AE));
}

/// DHT11 / DHT22: a standing grille box.
pub fn dht(pen: &Pen, cx: &ArtCtx) {
    let (mx, _) = cx.lead_center();
    let top = cx.leads.iter().map(|l| l.2).fold(f64::INFINITY, f64::min);
    let base = top - 0.8;
    for (_, x, y) in cx.leads {
        leg(pen, (*x, *y), (*x, base));
    }
    let dht22 = cx.def.bench.art == "dht22";
    let h = if dht22 { 5.2 } else { 4.4 };
    let body = WRect::new(mx - 1.95, base - h, mx + 1.95, base);
    pen.shadow(body, 0.3, 6.0, 3.0, 0.5);
    let (c0, c1) = if dht22 {
        (rgb(0xF4F4F0), rgb(0xD6D6D0))
    } else {
        (rgb(0x3A8FD8), rgb(0x2266A8))
    };
    pen.grad(body, 0.3, c0, c1);
    let hole = darken(c1, 0.35);
    for r in 0..(if dht22 { 5 } else { 4 }) {
        for c in 0..4 {
            let x = mx - 1.2 + c as f64 * 0.8;
            let y = base - h + 0.7 + r as f64 * 0.75;
            pen.rect(WRect::xywh(x - 0.22, y - 0.16, 0.44, 0.32), 0.05, hole);
        }
    }
    if let Some(Look::Readout { text }) = cx.look()
        && pen.view.scale >= 6.0
    {
        pen.text_at(
            mx,
            base - h - 0.3,
            text,
            0.62,
            font::mono,
            rgb(0xF7C77A),
            Align2::CENTER_BOTTOM,
        );
    }
}

/// Small sensor breakouts (BME280, microphone).
pub fn breakout(p: &Pen, cx: &ArtCtx) {
    let (w, h) = cx.def.bench.size;
    let body = WRect::new(-0.6, -h + 0.4, w - 0.4, 0.6);
    let (c0, c1) = match cx.def.id.as_str() {
        "bme280" => (rgb(0x7A49B4), rgb(0x5A3290)),
        _ => (rgb(0xC8352B), rgb(0x8E2A22)),
    };
    pcb(p, body, c0, c1);
    let (mx, my) = body.center();
    if cx.def.id == "microphone" {
        p.disc(mx, my - 0.6, 1.0, rgb(0xC9CED4), rgb(0x6E747C));
        p.circle(mx, my - 0.6, 0.55, rgb(0x161616));
    } else {
        p.rect(
            WRect::new(mx - 0.6, my - 1.3, mx + 0.6, my - 0.1),
            0.05,
            rgb(0xB8BDC4),
        );
        p.circle(mx + 0.3, my - 1.0, 0.12, rgb(0x2A2A2A));
    }
    pins_row(p, cx);
    if let Some(Look::Readout { text }) = cx.look() {
        p.text_at(
            mx,
            -h + 0.1,
            text,
            0.55,
            font::mono,
            rgb(0xF7C77A),
            Align2::CENTER_BOTTOM,
        );
    }
}

pub fn pir(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-0.6, -4.7, 4.6, 0.6);
    pcb(p, body, rgb(0x2F7D3A), rgb(0x225E2C));
    let (mx, my) = (2.0, -2.0);
    let active = matches!(cx.look(), Some(Look::Switch { on: true }))
        || cx.reading("out").unwrap_or(0.0) > 0.5;
    p.shadow(
        WRect::new(mx - 2.2, my - 2.2, mx + 2.2, my + 2.2),
        2.2,
        6.0,
        2.0,
        0.4,
    );
    p.disc(mx, my, 2.2, rgb(0xFFFFFF), rgb(0xCFCFC8));
    for i in 0..6 {
        let a = i as f64 / 6.0 * std::f64::consts::TAU;
        p.line(
            (mx, my),
            (mx + a.cos() * 2.1, my + a.sin() * 2.1),
            p.px(0.04, 0.5),
            rgb(0xB8B8B0),
        );
    }
    if active {
        p.glow(mx, my, 3.5, rgb(0xF2A33A), 0.35);
    }
    pins_row(p, cx);
}

pub fn hc_sr04(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-2.6, -4.0, 5.6 + 0.6, 0.6);
    pcb(p, body, rgb(0x2D6BC4), rgb(0x1C4B92));
    for x in [-0.4, 4.0] {
        p.shadow(WRect::new(x - 1.7, -3.9, x + 1.7, -0.5), 1.7, 4.0, 2.0, 0.4);
        p.disc(x, -2.2, 1.7, rgb(0xE6E8EA), rgb(0x9DA3AB));
        p.circle(x, -2.2, 1.1, rgb(0x2A2A2A));
        // mesh
        for k in 0..4 {
            let y = -2.9 + k as f64 * 0.45;
            p.line((x - 0.8, y), (x + 0.8, y), p.px(0.03, 0.4), rgb(0x5A5A5A));
        }
    }
    p.text(1.8, -3.6, "HC-SR04", 0.5, font::mono_medium, rgb(0xEAF3F7));
    pins_row(p, cx);
}

/// SSD1306 OLED: the live 128×64 framebuffer.
pub fn oled(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-3.6, -10.4, 7.4, 0.6);
    pcb(p, body, rgb(0x1C3F7A), rgb(0x132D5A));
    let glass = WRect::new(-3.2, -8.6, 7.0, -1.4);
    p.rect(glass, 0.15, rgb(0x050608));
    let screen = WRect::new(-2.8, -7.9, 6.6, -2.6);
    if let Some(Look::Pixels {
        width,
        height,
        data,
        on,
        inverted,
        ..
    }) = cx.look()
        && *on
    {
        let sr = p.r(screen);
        let w = *width as usize;
        let h = *height as usize;
        if w > 0 && h > 0 && data.len() >= w * h {
            let px = sr.width() / w as f32;
            let py = sr.height() / h as f32;
            let lit = rgb(0x8FD8FF);
            let mut mesh = Mesh::default();
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let v = (data[y * w + x] != 0) ^ *inverted;
                    if !v {
                        x += 1;
                        continue;
                    }
                    let x0 = x;
                    while x < w && ((data[y * w + x] != 0) ^ *inverted) {
                        x += 1;
                    }
                    let r = eframe::egui::Rect::from_min_max(
                        pos2(sr.left() + x0 as f32 * px, sr.top() + y as f32 * py),
                        pos2(sr.left() + x as f32 * px, sr.top() + (y + 1) as f32 * py),
                    );
                    let i = mesh.vertices.len() as u32;
                    mesh.colored_vertex(r.left_top(), lit);
                    mesh.colored_vertex(r.right_top(), lit);
                    mesh.colored_vertex(r.right_bottom(), lit);
                    mesh.colored_vertex(r.left_bottom(), lit);
                    mesh.add_triangle(i, i + 1, i + 2);
                    mesh.add_triangle(i, i + 2, i + 3);
                }
            }
            p.painter.add(Shape::mesh(mesh));
        }
    }
    p.text(
        1.9,
        -9.6,
        "SSD1306 · 128×64",
        0.45,
        font::mono,
        rgb(0x9AB8E0),
    );
    pins_row(p, cx);
}

/// 16×2 character LCD with its live DDRAM text and custom characters.
pub fn lcd(p: &Pen, cx: &ArtCtx) {
    let i2c = cx.def.bench.art == "lcd_1602_i2c";
    let body = WRect::new(-0.6, -13.6, 31.6, 0.6);
    pcb(p, body, rgb(0x2F8A45), rgb(0x1F6A33));
    for (x, y) in [(0.6, -12.6), (30.4, -12.6), (0.6, -0.6 - 0.4), (30.4, -1.0)] {
        p.circle(x, y, 0.55, rgb(0xC9B37E));
        p.circle(x, y, 0.35, rgb(0x0A0A0A));
    }
    let bezel = WRect::new(1.6, -11.6, 29.4, -2.4);
    p.rect(bezel, 0.3, rgb(0x141414));
    let (text, codes, cgram, display_on, backlight, cursor) = match cx.look() {
        Some(Look::TextLcd {
            text,
            codes,
            cgram,
            display_on,
            backlight,
            cursor,
            ..
        }) => (
            text.clone(),
            codes.clone(),
            cgram.clone(),
            *display_on,
            *backlight,
            *cursor,
        ),
        _ => (Vec::new(), Vec::new(), Vec::new(), false, !i2c, None),
    };
    let screen = WRect::new(2.4, -10.8, 28.6, -3.2);
    let (bg, ink) = if backlight {
        (rgb(0x9CC23A), rgb(0x1E2A0E))
    } else {
        (rgb(0x5E7428), rgb(0x1E2A0E))
    };
    p.grad(screen, 0.15, lighten(bg, 0.08), bg);
    let cols = 16;
    let cw = screen.w() / cols as f64;
    let ch = screen.h() / 2.0;
    for row in 0..2 {
        for col in 0..cols {
            let x = screen.x0 + col as f64 * cw;
            let y = screen.y0 + row as f64 * ch;
            let cell = WRect::new(x + 0.12, y + 0.35, x + cw - 0.12, y + ch - 0.35);
            p.rect(cell, 0.05, darken(bg, 0.06));
            if !display_on {
                continue;
            }
            let code = codes
                .get(row)
                .and_then(|r| r.get(col))
                .copied()
                .unwrap_or(b' ');
            if code < 8 {
                if let Some(bits) = cgram.get(code as usize) {
                    let dx = cell.w() / 5.0;
                    let dy = cell.h() / 8.0;
                    for (ry, b) in bits.iter().enumerate() {
                        for bx in 0..5 {
                            if b & (0x10 >> bx) != 0 {
                                p.rect(
                                    WRect::xywh(
                                        cell.x0 + bx as f64 * dx + 0.03,
                                        cell.y0 + ry as f64 * dy + 0.03,
                                        dx - 0.06,
                                        dy - 0.06,
                                    ),
                                    0.0,
                                    ink,
                                );
                            }
                        }
                    }
                }
            } else {
                let s: String = text
                    .get(row)
                    .and_then(|r| r.chars().nth(col))
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                if !s.trim().is_empty() {
                    let (mx, my) = cell.center();
                    p.text(mx, my, &s, ch * 0.62, font::mono_medium, ink);
                }
            }
            if cursor == Some((col as u8, row as u8)) && (cx.t * 2.0).fract() < 0.5 {
                p.rect(
                    WRect::new(cell.x0, cell.y1 - 0.25, cell.x1, cell.y1),
                    0.0,
                    ink,
                );
            }
        }
    }
    if i2c {
        let back = WRect::new(-0.6, -1.6, 4.4, 0.6);
        p.rect(back, 0.1, rgb(0x161616));
        p.text(1.9, -0.5, "PCF8574", 0.45, font::mono, rgb(0x9AA6AE));
    }
    pins_row(p, cx);
}

pub fn relay(p: &Pen, cx: &ArtCtx) {
    let energized = matches!(cx.look(), Some(Look::Relay { energized: true }));
    let body = WRect::new(-0.6, -7.6, 9.4, 0.6);
    pcb(p, body, rgb(0x2D6BC4), rgb(0x1C4B92));
    let cube = WRect::new(0.6, -6.8, 6.4, -1.6);
    p.shadow(cube, 0.2, 6.0, 3.0, 0.5);
    p.grad(cube, 0.2, rgb(0x3A7BE0), rgb(0x2457B8));
    p.text(
        3.5,
        -4.6,
        "SRD-05VDC",
        0.5,
        font::mono_medium,
        rgb(0xEAF3F7),
    );
    p.text(
        3.5,
        -3.6,
        if energized { "ON" } else { "OFF" },
        0.45,
        font::mono,
        rgb(0xCFE3EC),
    );
    // screw terminal on the right
    let term = WRect::new(7.0, -6.8, 9.0, -1.6);
    p.rect(term, 0.1, rgb(0x2A8A4A));
    for k in 0..3 {
        let y = -6.0 + k as f64 * 1.7;
        p.circle(8.0, y, 0.55, rgb(0xC9CED4));
        p.line((7.7, y), (8.3, y), p.px(0.08, 0.8), rgb(0x5A5A5A));
    }
    let led_c = rgb(0xFF4A3A);
    p.circle(
        0.9,
        -0.9,
        0.3,
        lit(darken(led_c, 0.6), led_c, if energized { 1.0 } else { 0.0 }),
    );
    if energized {
        p.glow(0.9, -0.9, 1.6, led_c, 0.6);
    }
    pins_row(p, cx);
}

/// SG90 micro servo with its horn at the simulated angle.
pub fn servo(p: &Pen, cx: &ArtCtx) {
    // cable from the header to the body
    for (i, c) in [rgb(0x6B3F1E), rgb(0xC8352B), rgb(0xE8892B)]
        .iter()
        .enumerate()
    {
        let x = i as f64;
        p.bezier(
            [
                (x, 0.0),
                (x, 1.8),
                (2.6, 2.0 + i as f64 * 0.3),
                (3.4, 2.0 + i as f64 * 0.3),
            ],
            p.px(0.28, 1.5),
            *c,
        );
    }
    let body = WRect::new(3.4, 0.4, 11.2, 4.8);
    p.shadow(body, 0.3, 7.0, 4.0, 0.5);
    p.grad(body, 0.3, rgb(0x3A6FD0), rgb(0x2450A8));
    p.rect(WRect::new(2.4, 1.8, 12.2, 3.4), 0.2, rgb(0x2B5CB8));
    p.text(6.0, 3.7, "SG90", 0.55, font::mono_medium, rgb(0xDCE6F5));
    let hub = (8.8, 2.6);
    let ang = match cx.look() {
        Some(Look::Servo { angle_deg, .. }) => *angle_deg,
        _ => 90.0,
    };
    // 0° points right, 180° left (horn sweeps across the top)
    let a = (180.0 + ang).to_radians();
    let tip = (hub.0 + a.cos() * 3.2, hub.1 + a.sin() * 3.2);
    p.line(
        (hub.0 + 0.2, hub.1 + 0.3),
        (tip.0 + 0.2, tip.1 + 0.3),
        p.px(0.9, 3.0),
        Color32::from_black_alpha(70),
    );
    p.line(hub, tip, p.px(0.85, 3.0), rgb(0xF2F2F2));
    p.circle(tip.0, tip.1, 0.42, rgb(0xF2F2F2));
    p.disc(hub.0, hub.1, 0.75, rgb(0xFFFFFF), rgb(0xCFCFC8));
    p.circle(hub.0, hub.1, 0.2, rgb(0x8E949B));
    for k in 1..4 {
        let h = (
            hub.0 + a.cos() * k as f64 * 0.8,
            hub.1 + a.sin() * k as f64 * 0.8,
        );
        p.circle(h.0, h.1, 0.1, rgb(0xB8B8B0));
    }
    pins_row(p, cx);
}

/// 28BYJ-48 stepper with a marker on the shaft.
pub fn stepper(p: &Pen, cx: &ArtCtx) {
    let c = (6.0, 6.2);
    p.shadow(
        WRect::new(c.0 - 4.8, c.1 - 4.8, c.0 + 4.8, c.1 + 4.8),
        4.8,
        8.0,
        4.0,
        0.5,
    );
    p.disc(c.0, c.1, 4.8, rgb(0xDCE0E4), rgb(0x8E949B));
    p.rect(
        WRect::new(c.0 - 3.4, c.1 + 2.6, c.0 + 3.4, c.1 + 5.2),
        0.3,
        rgb(0x2D6BC4),
    );
    let ang = match cx.look() {
        Some(Look::Stepper { angle_deg, .. }) => *angle_deg,
        _ => 0.0,
    };
    let s = (c.0, c.1 - 1.6);
    p.disc(s.0, s.1, 1.1, rgb(0xEDEDED), rgb(0xA0A6AC));
    let a = (ang - 90.0).to_radians();
    p.line(
        s,
        (s.0 + a.cos() * 1.0, s.1 + a.sin() * 1.0),
        p.px(0.25, 1.5),
        rgb(0xC8352B),
    );
    p.text(
        c.0,
        c.1 + 1.6,
        "28BYJ-48",
        0.55,
        font::mono_medium,
        rgb(0x2C3135),
    );
    // cable to the header
    for (i, col) in [
        rgb(0xC8352B),
        rgb(0xE8892B),
        rgb(0xE7C531),
        rgb(0xE88AA8),
        rgb(0x3567C6),
    ]
    .iter()
    .enumerate()
    {
        let x = i as f64;
        p.bezier(
            [
                (x, 0.0),
                (x, 0.9),
                (c.0 - 1.0 + i as f64 * 0.5, 0.8),
                (c.0 - 1.0 + i as f64 * 0.5, 1.6),
            ],
            p.px(0.2, 1.2),
            *col,
        );
    }
    pins_row(p, cx);
}

/// DC motor / vibration motor / water pump.
pub fn motor(p: &Pen, cx: &ArtCtx) {
    let (rpm, ang) = match cx.look() {
        Some(Look::Motor { rpm, angle_deg }) => (*rpm, *angle_deg),
        _ => (0.0, 0.0),
    };
    let art = cx.def.bench.art.as_str();
    let (w, h) = cx.def.bench.size;
    let body = WRect::new(-0.6, -h + 0.4, w - 0.4, 0.6);
    match art {
        "water_pump" => {
            let can = WRect::new(0.0, -h + 1.0, w - 1.6, -0.4);
            p.shadow(can, 0.6, 6.0, 3.0, 0.5);
            p.grad(can, 0.6, rgb(0x3A3A3A), rgb(0x161616));
            p.rect(
                WRect::new(w - 1.8, -h + 2.0, w - 0.4, -h + 3.0),
                0.3,
                rgb(0x2A6FD0),
            );
            p.text(
                can.center().0,
                can.center().1,
                "PUMP",
                0.6,
                font::mono_medium,
                rgb(0xA3A3A3),
            );
            if rpm.abs() > 1.0 {
                let ph = (cx.t * 3.0).fract();
                p.circle(
                    w - 0.2,
                    -h + 2.5 + ph * 1.5,
                    0.25,
                    with_alpha(rgb(0x4CC3F0), 1.0 - ph as f32),
                );
            }
        }
        "vibration_motor" => {
            p.disc(1.0, -1.6, 1.5, rgb(0xDCE0E4), rgb(0x8E949B));
            let wob = if rpm.abs() > 1.0 {
                (cx.t * 60.0).sin() * 0.12
            } else {
                0.0
            };
            p.circle(1.0 + wob, -1.6, 0.5, rgb(0x6E747C));
        }
        _ => {
            let can = WRect::new(-0.2, -h + 1.0, w - 2.0, -0.8);
            p.shadow(can, 0.8, 7.0, 4.0, 0.5);
            p.grad(can, 0.8, rgb(0xE6E8EA), rgb(0x8E949B));
            p.rect(WRect::new(-0.4, -h + 1.4, 0.6, -1.2), 0.2, rgb(0xC9A54A));
            let (cxm, cym) = can.center();
            p.text(cxm, cym, "DC", 0.7, font::mono_medium, rgb(0x5A5F65));
            // shaft + a two-blade marker spinning at the simulated angle
            let s = (w - 1.2, cym);
            p.line((w - 2.0, cym), s, p.px(0.35, 1.5), rgb(0xB8BDC4));
            let a = ang.to_radians();
            let r = 1.6;
            let blur = rpm.abs() > 600.0;
            let col = if blur {
                with_alpha(rgb(0xF2A33A), 0.35)
            } else {
                rgb(0xF2A33A)
            };
            p.line(
                (s.0 - a.sin() * r * 0.25, s.1 - a.cos() * r),
                (s.0 + a.sin() * r * 0.25, s.1 + a.cos() * r),
                p.px(0.45, 2.0),
                col,
            );
            if blur {
                p.ellipse(s.0, s.1, 0.35, r, with_alpha(rgb(0xF2A33A), 0.18));
            }
        }
    }
    let _ = body;
    for (i, l) in cx.def.bench.leads.iter().enumerate() {
        let col = if i == 0 { rgb(0xC8352B) } else { rgb(0x1E1E1E) };
        p.circle(l.x, l.y, 0.32, col);
    }
}

pub fn battery(p: &Pen, cx: &ArtCtx) {
    let kind = cx.prop_text("kind").to_ascii_lowercase();
    let (w, h) = cx.def.bench.size;
    let body = WRect::new(-0.6, -h + 0.4, w - 0.6, -0.6);
    p.shadow(body, 0.3, 8.0, 4.0, 0.55);
    p.grad(body, 0.3, rgb(0x2A2A2A), rgb(0x141414));
    let cells = if kind.contains('4') {
        4
    } else if kind.contains("9v") || kind.contains("lipo") {
        1
    } else {
        2
    };
    if cells == 1 {
        let c = if kind.contains("lipo") {
            rgb(0x3A6FD0)
        } else {
            rgb(0xE8E8E8)
        };
        let r = body.expand(-0.5);
        p.grad(r, 0.3, c, darken(c, 0.25));
        let label = if kind.contains("lipo") {
            "LiPo 1S"
        } else {
            "9V"
        };
        p.text(
            r.center().0,
            r.center().1,
            label,
            0.9,
            font::mono_medium,
            rgb(0x2C3135),
        );
    } else {
        let ch = (body.h() - 0.6) / cells as f64;
        for i in 0..cells {
            let y = body.y0 + 0.3 + i as f64 * ch;
            let cell = WRect::new(body.x0 + 0.4, y + 0.08, body.x1 - 0.4, y + ch - 0.08);
            p.hgrad(cell, ch / 2.0, rgb(0xE07A1F), rgb(0x2A2A2A));
            p.text(
                cell.center().0,
                cell.center().1,
                "AA",
                ch * 0.45,
                font::mono_medium,
                rgb(0xEDEDED),
            );
        }
    }
    if let Some(v) = cx.reading("voltage").or_else(|| cx.reading("v")) {
        p.text_at(
            body.x1,
            body.y0 - 0.2,
            &format!("{v:.2} V"),
            0.6,
            font::mono,
            rgb(0xF7C77A),
            Align2::RIGHT_BOTTOM,
        );
    }
    for (i, l) in cx.def.bench.leads.iter().enumerate() {
        let col = if i == 0 { rgb(0xC8352B) } else { rgb(0x1E1E1E) };
        p.wline((l.x, l.y), (l.x, -0.6), 0.3, col);
        p.circle(l.x, l.y, 0.3, col);
    }
}

pub fn usb_supply(p: &Pen, cx: &ArtCtx) {
    let (w, h) = cx.def.bench.size;
    let jack = cx.def.bench.art == "barrel_jack";
    let body = WRect::new(-0.6, -h + 0.4, w - 0.4, -0.7);
    p.shadow(body, 0.5, 8.0, 4.0, 0.55);
    if jack {
        p.grad(body, 0.4, rgb(0x2A2A2A), rgb(0x141414));
        p.circle(body.center().0, body.center().1, 0.8, rgb(0x050505));
    } else {
        p.grad(body, 0.5, rgb(0xF4F4F0), rgb(0xCFCFC8));
        p.rect(
            WRect::new(body.x1 - 1.6, body.y0 + 1.0, body.x1 - 0.4, body.y0 + 2.2),
            0.1,
            rgb(0x8E949B),
        );
        let v = cx.prop_f64("voltage").unwrap_or(5.0);
        p.text(
            body.center().0 - 0.4,
            body.center().1,
            &format!("USB {v:.0}V"),
            0.55,
            font::mono_medium,
            rgb(0x5A5F65),
        );
    }
    for (i, l) in cx.def.bench.leads.iter().enumerate() {
        let col = if i == 0 { rgb(0xC8352B) } else { rgb(0x1E1E1E) };
        p.wline((l.x, l.y), (l.x, -0.7), 0.3, col);
        p.circle(l.x, l.y, 0.3, col);
    }
}

/// Pot of soil with a plant: soil colour + leaf droop follow the moisture; the front of the
/// pot is drawn after parts so sensors look pushed into the soil.
pub fn pot_soil(p: &Pen, cx: &ArtCtx, pass: Pass) {
    let m = cx.soil_vwc.unwrap_or(42.0).clamp(0.0, 60.0);
    let t = (m / 60.0) as f32;
    let soil = crate::theme::mix(rgb(0xA47E55), rgb(0x2E2018), t);
    if pass == Pass::Back {
        p.shadow(WRect::new(1.2, 2.0, 14.8, 14.6), 1.0, 18.0, 8.0, 0.55);
        p.rect(WRect::new(0.0, 0.4, 16.0, 2.4), 0.45, rgb(0x9E4A2A));
        p.ellipse(8.0, 1.4, 7.3, 0.85, soil);
        // the plant: a stem and broad leaves that droop as the soil dries
        let droop = (1.0 - m / 60.0) * 42.0;
        let green = crate::theme::mix(rgb(0x7E8F3E), rgb(0x3E8B45), t.powf(0.5));
        p.bezier(
            [(5.4, 1.3), (4.9, -2.6), (6.0, -6.0), (5.3, -10.5)],
            p.px(0.6, 2.5),
            green,
        );
        leaf(p, (5.3, -3.0), -12.0 + droop, false, 8.4, rgb(0x4FA357), t);
        leaf(p, (5.3, -1.4), -12.0 + droop, true, 8.0, rgb(0x459A4D), t);
        leaf(
            p,
            (5.3, -6.6),
            -58.0 + droop * 0.7,
            false,
            6.2,
            rgb(0x5BB563),
            t,
        );
        leaf(
            p,
            (5.4, -9.0),
            -112.0 + droop * 0.5,
            true,
            5.0,
            rgb(0x5BB563),
            t,
        );
        leaf(
            p,
            (5.3, -10.4),
            -76.0 + droop * 0.4,
            false,
            3.8,
            rgb(0x66C06E),
            t,
        );
    } else {
        // the pot body in front of whatever is in the soil
        let body = [(0.8, 2.0), (15.2, 2.0), (13.4, 13.8), (2.6, 13.8)];
        let mut mesh = Mesh::default();
        let top_c = rgb(0xC9693F);
        let bot_c = rgb(0x94442A);
        let pts: Vec<_> = body.iter().map(|&(x, y)| p.p(x, y)).collect();
        let ys = pts.iter().map(|q| q.y);
        let (y0, y1) = ys.fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), y| {
            (a.min(y), b.max(y))
        });
        for q in &pts {
            let k = ((q.y - y0) / (y1 - y0).max(1.0)).clamp(0.0, 1.0);
            mesh.colored_vertex(*q, crate::theme::mix(top_c, bot_c, k));
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        p.painter.add(Shape::mesh(mesh));
        // rim front lip + highlight
        p.rect(WRect::new(0.0, 1.5, 16.0, 2.6), 0.4, rgb(0xB85A33));
        p.line(
            (0.6, 1.8),
            (15.4, 1.8),
            p.px(0.1, 1.0),
            with_alpha(Color32::WHITE, 0.18),
        );
        p.text(
            8.0,
            8.6,
            &format!("{:.0} % VWC", m),
            0.8,
            font::mono_medium,
            with_alpha(rgb(0xF6E1D2), 0.85),
        );
        p.text(
            8.0,
            12.6,
            &cx.part
                .reference
                .to_ascii_lowercase()
                .replace("pot", "pot "),
            0.6,
            font::mono,
            with_alpha(rgb(0xF6E1D2), 0.6),
        );
    }
}

fn leaf(p: &Pen, at: (f64, f64), rot_deg: f64, mirror: bool, len: f64, color: Color32, wet: f32) {
    let a = rot_deg.to_radians();
    let (s, c) = a.sin_cos();
    let k = len / 6.6;
    let shape = [
        (0.0, 0.0),
        (1.8, -2.4),
        (5.2, -2.2),
        (6.6, -0.8),
        (4.8, 0.4),
        (2.0, 0.8),
    ];
    let pts: Vec<(f64, f64)> = shape
        .iter()
        .map(|&(x, y)| {
            let (x, y) = (x * k, y * k);
            let x = if mirror { -x } else { x };
            let (rx, ry) = if mirror {
                (x * c + y * s, -x * s + y * c)
            } else {
                (x * c - y * s, x * s + y * c)
            };
            (at.0 + rx, at.1 + ry)
        })
        .collect();
    let col = crate::theme::mix(rgb(0x9A9A4A), color, wet.powf(0.4));
    let sh: Vec<(f64, f64)> = pts.iter().map(|(x, y)| (x + 0.3, y + 0.5)).collect();
    p.poly(&sh, Color32::from_black_alpha(60));
    p.poly(&pts, col);
    p.line(pts[0], pts[3], p.px(0.08, 0.8), lighten(col, 0.25));
}

pub fn lamp(p: &Pen, cx: &ArtCtx) {
    let on = cx.prop_bool("on");
    let base = WRect::new(2.0, 10.6, 9.0, 12.0);
    p.shadow(base, 0.5, 8.0, 4.0, 0.5);
    p.grad(base, 0.5, rgb(0x3A3A3A), rgb(0x1A1A1A));
    p.wline((5.5, 10.8), (3.5, 5.0), 0.5, rgb(0x2A2A2A));
    p.wline((3.5, 5.0), (7.5, 2.4), 0.5, rgb(0x2A2A2A));
    p.poly(
        &[(6.0, 1.2), (10.4, 2.6), (9.4, 5.4), (5.6, 3.6)],
        rgb(0xD8D4C8),
    );
    if on {
        p.glow(9.4, 4.8, 9.0, rgb(0xFFE7A8), 0.75);
        p.circle(9.0, 4.4, 0.8, rgb(0xFFF4D6));
    }
}

pub fn hand(p: &Pen, _cx: &ArtCtx) {
    let skin = rgb(0xE8B98F);
    p.shadow(WRect::new(1.0, 3.0, 7.0, 10.0), 2.0, 10.0, 4.0, 0.45);
    p.rect(WRect::new(1.0, 3.6, 7.0, 9.6), 2.2, skin);
    for (i, h) in [3.6, 4.6, 4.4, 3.6].iter().enumerate() {
        let x = 1.6 + i as f64 * 1.45;
        p.rect(WRect::new(x, 3.8 - h, x + 1.2, 4.6), 0.6, skin);
    }
    p.rect(WRect::new(-0.6, 5.4, 1.6, 6.8), 0.7, skin);
}

/// The built-in 2-channel mini scope (CH1/CH2/GND jacks on top, live trace, caption).
pub fn mini_scope(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-1.0, -1.0, 19.0, 10.0);
    p.shadow(body, 0.7, 14.0, 7.0, 0.6);
    p.grad(body, 0.7, rgb(0x2C2C2C), rgb(0x1F1F1F));
    p.rect_stroke(body, 0.7, p.px(0.08, 1.0), rgb(0x3A3A3A));
    let screen = WRect::new(-0.2, 1.0, 13.8, 7.6);
    p.rect(screen, 0.25, rgb(0x040604));
    // graticule: 10 × 8 divisions
    let gc = rgb(0x1B2A1B);
    for i in 1..10 {
        let x = screen.x0 + screen.w() * i as f64 / 10.0;
        p.line((x, screen.y0), (x, screen.y1), p.px(0.03, 0.7), gc);
    }
    for i in 1..8 {
        let y = screen.y0 + screen.h() * i as f64 / 8.0;
        p.line((screen.x0, y), (screen.x1, y), p.px(0.03, 0.7), gc);
    }
    let iv = cx.instrument.cloned().unwrap_or_default();
    let colors = [rgb(0xF2D03B), rgb(0x4CC3F0)];
    for (ch, tr) in iv.traces.iter().enumerate().take(2) {
        if tr.len() < 2 {
            continue;
        }
        let pts: Vec<(f64, f64)> = tr
            .iter()
            .map(|(t, v)| {
                let x = screen.x0 + (*t as f64).clamp(0.0, 1.0) * screen.w();
                let y =
                    screen.y0 + screen.h() / 2.0 - (*v as f64).clamp(-4.2, 4.2) * screen.h() / 8.0;
                (x, y)
            })
            .collect();
        p.polyline(&pts, p.px(0.16, 1.5), colors[ch]);
    }
    // knobs
    for (y, ang) in [(2.6, -0.9f64), (5.6, 0.7)] {
        p.disc(16.4, y, 1.15, rgb(0x2E2E2E), rgb(0x141414));
        p.circle_stroke(16.4, y, 1.15, p.px(0.06, 0.8), rgb(0x454545));
        p.line(
            (16.4, y),
            (16.4 + ang.cos() * 0.9, y + ang.sin() * 0.9),
            p.px(0.14, 1.2),
            rgb(0xD9D9D9),
        );
    }
    p.text(16.4, 7.6, "CH1", 0.6, font::mono, rgb(0xF2D03B));
    // jacks
    for (l, c, name) in [
        (0usize, rgb(0xF2D03B), "CH1"),
        (1, rgb(0x4CC3F0), "CH2"),
        (2, rgb(0x6E6E6E), "⏚"),
    ] {
        if let Some(ld) = cx.def.bench.leads.get(l) {
            p.circle(ld.x, ld.y, 0.42, rgb(0x0A0A0A));
            p.circle_stroke(ld.x, ld.y, 0.42, p.px(0.12, 1.2), c);
            if p.view.scale >= 9.0 {
                p.text(ld.x, ld.y - 0.75, name, 0.4, font::mono, rgb(0xA3A3A3));
            }
        }
    }
    let cap = if iv.caption.is_empty() {
        "CH1 · 2 V/div · 1 ms/div".to_string()
    } else {
        iv.caption.clone()
    };
    p.text_at(
        -0.2,
        8.85,
        &cap,
        0.8,
        font::mono,
        rgb(0xEAEAEA),
        Align2::LEFT_CENTER,
    );
}

/// Handheld DMM (built-in placeholder until plugin panels render it): LCD with the live
/// reading, rotary dial at V⎓, VΩ / COM jacks where the probe leads attach.
pub fn dmm(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-1.0, -1.0, 12.0, 15.6);
    p.shadow(body, 1.3, 16.0, 8.0, 0.6);
    p.grad(body, 1.3, rgb(0x444444), rgb(0x2E2E2E));
    let inner = body.expand(-0.45);
    p.grad(inner, 1.0, rgb(0x222222), rgb(0x171717));
    let iv = cx.instrument.cloned().unwrap_or_default();
    // LCD
    let lcd = WRect::new(1.4, 0.3, 11.0, 4.0);
    p.grad(lcd, 0.3, rgb(0xB8C3A8), rgb(0xA2AE92));
    let mode = if iv.mode.is_empty() {
        "DC  AUTO".to_string()
    } else {
        iv.mode.to_uppercase()
    };
    p.text_at(
        1.8,
        0.9,
        &mode,
        0.5,
        font::mono,
        rgb(0x2C3627),
        Align2::LEFT_CENTER,
    );
    let disp = if iv.display.is_empty() {
        "----".to_string()
    } else {
        iv.display.clone()
    };
    p.text_at(
        9.9,
        2.5,
        &disp,
        1.9,
        font::mono_medium,
        rgb(0x18200F),
        Align2::RIGHT_CENTER,
    );
    let unit = if iv.unit.is_empty() {
        "V".to_string()
    } else {
        iv.unit.clone()
    };
    p.text_at(
        10.1,
        3.0,
        &unit,
        0.7,
        font::mono,
        rgb(0x18200F),
        Align2::LEFT_CENTER,
    );
    // dial
    let c = (6.2, 9.4);
    p.disc(c.0, c.1, 3.0, rgb(0x2A2A2A), rgb(0x101010));
    p.circle_stroke(c.0, c.1, 3.0, p.px(0.06, 0.8), rgb(0x2E2E2E));
    p.disc(c.0, c.1, 2.0, rgb(0x505050), rgb(0x2E2E2E));
    let a = (-130.0f64).to_radians();
    p.line(
        c,
        (c.0 + a.cos() * 1.7, c.1 + a.sin() * 1.7),
        p.px(0.4, 2.5),
        rgb(0xEAEAEA),
    );
    for (lab, ang, col) in [
        ("OFF", -90.0f64, rgb(0xC7C7C7)),
        ("V⎓", -140.0, rgb(0xF2A33A)),
        ("V~", -190.0, rgb(0xC7C7C7)),
        ("Ω", -40.0, rgb(0xC7C7C7)),
        ("A", 10.0, rgb(0xC7C7C7)),
        ("Hz", 55.0, rgb(0xC7C7C7)),
    ] {
        let r = ang.to_radians();
        p.text(
            c.0 + r.cos() * 3.7,
            c.1 + r.sin() * 3.7,
            lab,
            0.5,
            font::mono,
            col,
        );
    }
    // jacks (the leads) + 10A
    for (i, ld) in cx.def.bench.leads.iter().enumerate() {
        let col = if i == 0 { rgb(0xC8352B) } else { rgb(0x6E6E6E) };
        p.circle(ld.x, ld.y, 0.45, rgb(0x070707));
        p.circle_stroke(ld.x, ld.y, 0.45, p.px(0.16, 1.5), col);
    }
    p.circle(0.0, 13.6, 0.45, rgb(0x070707));
    p.circle_stroke(0.0, 13.6, 0.45, p.px(0.16, 1.5), rgb(0x6E6E6E));
    if p.view.scale >= 8.0 {
        p.text(0.0, 1.0, "VΩ", 0.4, font::mono, rgb(0xA3A3A3));
        p.text(1.0, 1.0, "COM", 0.32, font::mono, rgb(0xA3A3A3));
        p.text(0.0, 12.7, "10A", 0.4, font::mono, rgb(0xA3A3A3));
    }
    let _ = vec2(0.0, 0.0);
}
