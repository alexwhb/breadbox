//! Development boards and radio modules: Arduino UNO R3 / Mega 2560 / Nano, ESP32 DevKit,
//! Raspberry Pi Pico, ESP-01, HM-10 / HC-05. Header pins come from the part's lead table, so
//! the art always matches where wires attach.

use super::*;
use eframe::egui::Align2;

fn board_led(p: &Pen, x: f64, y: f64, color: Color32, on: f32) {
    p.rect(
        WRect::xywh(x - 0.38, y - 0.24, 0.76, 0.48),
        0.06,
        lit(darken(color, 0.65), color, on),
    );
    if on > 0.05 {
        p.glow(x, y, 1.4 + on as f64 * 1.0, color, 0.55 * on);
    }
}

fn mounting_hole(p: &Pen, x: f64, y: f64) {
    p.circle(x, y, 0.62, rgb(0xC9B37E));
    p.circle(x, y, 0.42, rgb(0x0A0A0A));
}

/// Labels for wired pins (or all pins when zoomed in).
fn pin_labels(p: &Pen, cx: &ArtCtx, row_y: f64, below: bool) {
    let all = p.view.scale >= 13.0;
    if p.view.scale < 7.0 {
        return;
    }
    for l in &cx.def.bench.leads {
        if (l.y - row_y).abs() > 0.01 {
            continue;
        }
        if !all && !cx.wired.iter().any(|w| w == &l.pin) {
            continue;
        }
        let name = cx
            .def
            .pin(&l.pin)
            .map(|d| d.label().to_string())
            .unwrap_or_else(|| l.pin.clone());
        let y = if below { l.y + 1.0 } else { l.y - 1.0 };
        p.text(l.x, y, &name, 0.62, font::mono_medium, rgb(0xCFE3EC));
    }
}

/// Header blocks under groups of consecutive leads on one row.
fn headers(p: &Pen, cx: &ArtCtx, female: bool) {
    let mut rows: Vec<(f64, Vec<f64>)> = Vec::new();
    for l in &cx.def.bench.leads {
        match rows.iter_mut().find(|(y, _)| (*y - l.y).abs() < 0.01) {
            Some((_, xs)) => xs.push(l.x),
            None => rows.push((l.y, vec![l.x])),
        }
    }
    for (y, mut xs) in rows {
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // split into runs of 1-pitch steps
        let mut start = xs[0];
        let mut prev = xs[0];
        let mut runs = Vec::new();
        for &x in &xs[1..] {
            if (x - prev - 1.0).abs() > 0.05 {
                runs.push((start, prev));
                start = x;
            }
            prev = x;
        }
        runs.push((start, prev));
        for (a, b) in runs {
            let r = WRect::new(a - 0.5, y - 0.5, b + 0.5, y + 0.5);
            if female {
                p.shadow(r, 0.1, 3.0, 2.0, 0.5);
                p.grad(r, 0.12, rgb(0x262626), rgb(0x111111));
            } else {
                p.rect(r, 0.12, rgb(0x161616));
            }
        }
    }
    for l in &cx.def.bench.leads {
        if female {
            socket(p, l.x, l.y);
        } else {
            header_pin(p, l.x, l.y);
        }
    }
}

fn board_look(cx: &ArtCtx) -> (bool, f32, bool, bool) {
    match cx.look() {
        Some(Look::Board { power, l, tx, rx }) => (*power, *l, *tx, *rx),
        _ => (false, 0.0, false, false),
    }
}

fn dip_chip(p: &Pen, r: WRect, pins_per_side: usize, label: &str) {
    // legs
    let step = r.w() / pins_per_side as f64;
    for i in 0..pins_per_side {
        let x = r.x0 + step * (i as f64 + 0.5);
        for (y0, y1) in [(r.y0 - 0.45, r.y0), (r.y1, r.y1 + 0.45)] {
            p.rect(WRect::new(x - 0.16, y0, x + 0.16, y1), 0.0, rgb(0xB8BDC4));
        }
    }
    p.shadow(r, 0.15, 4.0, 2.0, 0.5);
    p.grad(r, 0.15, rgb(0x262626), rgb(0x141414));
    p.circle(r.x0 + 0.6, r.y0 + r.h() / 2.0, 0.28, rgb(0x2E2E2E));
    let (mx, my) = r.center();
    p.text(mx, my, label, 0.72, font::mono, rgb(0x9AA6AE));
}

/// Arduino UNO R3 (and the Mega 2560 with `mega`).
pub fn uno(p: &Pen, cx: &ArtCtx, mega: bool) {
    let w = if mega { 40.0 } else { 27.0 };
    let body = WRect::new(0.0, 0.0, w, 21.0);
    let (power, l, tx, rx) = board_look(cx);
    p.shadow(body, 1.1, 16.0, 9.0, 0.6);
    p.grad(body, 1.1, rgb(0x2470A0), rgb(0x1A5578));
    p.rect_stroke(body, 1.1, p.px(0.12, 1.0), rgb(0x164A61));
    // silkscreen outline
    p.rect_stroke(
        body.expand(-0.6),
        0.8,
        p.px(0.05, 0.6),
        with_alpha(rgb(0x7FB6D3), 0.25),
    );
    for (x, y) in [(1.6, 1.6), (w - 1.6, 19.4), (1.6, 19.4)] {
        mounting_hole(p, x, y);
    }
    // USB-B jack
    let usb = WRect::new(-1.6, 2.6, 5.0, 8.4);
    p.shadow(usb, 0.2, 5.0, 3.0, 0.5);
    p.grad(usb, 0.2, rgb(0xD4D8DD), rgb(0x9DA3AB));
    p.rect_stroke(usb, 0.2, p.px(0.05, 0.6), rgb(0x8A9099));
    p.rect(WRect::new(-1.6, 4.0, 0.2, 7.0), 0.1, rgb(0x6E747C));
    // DC barrel jack
    let dc = WRect::new(-1.0, 13.4, 4.4, 18.4);
    p.shadow(dc, 0.3, 5.0, 3.0, 0.5);
    p.grad(dc, 0.35, rgb(0x2A2A2A), rgb(0x121212));
    p.circle(0.6, 15.9, 1.0, rgb(0x050505));
    // reset button
    p.rect(WRect::new(6.6, 1.8, 8.4, 3.6), 0.2, rgb(0xB8BDC4));
    p.circle(7.5, 2.7, 0.55, rgb(0x8E2A22));
    // crystal
    p.rect(WRect::new(9.2, 10.6, 12.6, 11.9), 0.6, rgb(0xC8CDD3));
    // regulator + caps
    p.rect(WRect::new(5.6, 13.2, 8.0, 15.0), 0.1, rgb(0x1A1A1A));
    p.circle(6.0, 10.2, 0.85, rgb(0x2A2A2A));
    p.circle(6.0, 10.2, 0.55, rgb(0x6E6E6E));
    // the MCU
    if mega {
        let chip = WRect::new(19.0, 8.0, 25.0, 14.0);
        p.shadow(chip, 0.1, 4.0, 2.0, 0.4);
        p.rect(chip, 0.1, rgb(0x191919));
        p.text(22.0, 11.0, "ATmega2560", 0.62, font::mono, rgb(0x9AA6AE));
    } else {
        dip_chip(p, WRect::new(12.2, 12.6, 25.2, 15.6), 14, "ATmega328P");
    }
    // name
    let name = if mega { "MEGA 2560" } else { "UNO R3" };
    let (nx, ny) = if mega { (12.0, 9.0) } else { (17.6, 8.6) };
    p.text(nx, ny, name, 2.1, font::brand, rgb(0xEAF3F7));
    p.text(
        nx,
        ny + 1.6,
        "ARDUINO",
        0.7,
        font::mono_medium,
        with_alpha(rgb(0xCFE3EC), 0.7),
    );
    // status LEDs: ON, L, TX, RX
    board_led(p, 25.3, 9.2, rgb(0x5CFF8A), if power { 1.0 } else { 0.0 });
    p.text_at(
        24.5,
        9.2,
        "ON",
        0.55,
        font::mono,
        rgb(0xCFE3EC),
        Align2::RIGHT_CENTER,
    );
    board_led(p, 10.0, 5.0, rgb(0xFFB547), l);
    board_led(p, 10.0, 6.2, rgb(0xFFB547), if tx { 1.0 } else { 0.0 });
    board_led(p, 10.0, 7.4, rgb(0xFFB547), if rx { 1.0 } else { 0.0 });
    for (y, s) in [(5.0, "L"), (6.2, "TX"), (7.4, "RX")] {
        p.text_at(
            10.7,
            y,
            s,
            0.5,
            font::mono,
            rgb(0xCFE3EC),
            Align2::LEFT_CENTER,
        );
    }
    // header captions
    let cap = rgb(0xCFE3EC);
    p.text_at(
        8.0,
        2.5,
        "DIGITAL (PWM~)",
        0.62,
        font::mono,
        cap,
        Align2::LEFT_CENTER,
    );
    p.text_at(
        10.0,
        18.5,
        "POWER",
        0.62,
        font::mono,
        cap,
        Align2::LEFT_CENTER,
    );
    p.text_at(
        19.0,
        18.5,
        "ANALOG IN",
        0.62,
        font::mono,
        cap,
        Align2::LEFT_CENTER,
    );
    headers(p, cx, true);
    pin_labels(p, cx, 1.0, true);
    pin_labels(p, cx, 20.0, false);
}

pub fn nano(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-1.2, -0.7, 15.2, 6.7);
    let (power, l, tx, rx) = board_look(cx);
    p.shadow(body, 0.5, 10.0, 6.0, 0.55);
    p.grad(body, 0.5, rgb(0x2470A0), rgb(0x1A5578));
    let usb = WRect::new(-2.0, 1.8, 1.0, 4.2);
    p.grad(usb, 0.15, rgb(0xD4D8DD), rgb(0x9DA3AB));
    // TQFP rotated look: a square chip
    p.poly(
        &[(7.0, 1.4), (8.6, 3.0), (7.0, 4.6), (5.4, 3.0)],
        rgb(0x191919),
    );
    p.text(10.8, 3.0, "NANO", 1.0, font::brand, rgb(0xEAF3F7));
    board_led(p, 3.0, 2.2, rgb(0x5CFF8A), if power { 1.0 } else { 0.0 });
    board_led(p, 3.0, 3.8, rgb(0xFFB547), l);
    board_led(p, 13.0, 2.4, rgb(0xFFB547), if tx { 1.0 } else { 0.0 });
    board_led(p, 13.0, 3.6, rgb(0xFFB547), if rx { 1.0 } else { 0.0 });
    for ld in &cx.def.bench.leads {
        header_pin(p, ld.x, ld.y);
    }
    pin_labels(p, cx, 0.0, true);
    pin_labels(p, cx, 6.0, false);
}

pub fn esp32(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-1.0, -1.0, 18.8, 11.0);
    let (power, l, _, _) = board_look(cx);
    p.shadow(body, 0.6, 12.0, 7.0, 0.6);
    p.grad(body, 0.6, rgb(0x262626), rgb(0x141414));
    // module shield + antenna
    let shield = WRect::new(9.6, 1.4, 15.6, 8.6);
    p.shadow(shield, 0.2, 4.0, 2.0, 0.4);
    p.grad(shield, 0.2, rgb(0xC9CED4), rgb(0x8E949B));
    p.text(12.6, 4.4, "ESP32", 0.95, font::mono_medium, rgb(0x2C3135));
    p.text(12.6, 5.8, "WROOM-32", 0.6, font::mono, rgb(0x3A4045));
    let ant = WRect::new(15.6, 1.4, 18.4, 8.6);
    p.rect(ant, 0.15, rgb(0x1C1C1C));
    let mut zig = Vec::new();
    for i in 0..7 {
        let y = 2.2 + i as f64 * 0.95;
        zig.push((16.0, y));
        zig.push((18.0, y));
    }
    p.polyline(&zig, p.px(0.14, 1.0), rgb(0xC9A54A));
    // usb + buttons
    p.grad(
        WRect::new(-1.6, 3.6, 1.2, 6.4),
        0.15,
        rgb(0xD4D8DD),
        rgb(0x9DA3AB),
    );
    for (x, lab) in [(3.2, "EN"), (3.2, "BOOT")] {
        let y = if lab == "EN" { 2.6 } else { 7.4 };
        p.rect(
            WRect::new(x - 0.7, y - 0.6, x + 0.7, y + 0.6),
            0.15,
            rgb(0xB8BDC4),
        );
        p.circle(x, y, 0.38, rgb(0x2A2A2A));
        p.text_at(
            x + 1.0,
            y,
            lab,
            0.5,
            font::mono,
            silk(),
            Align2::LEFT_CENTER,
        );
    }
    board_led(p, 6.2, 3.4, rgb(0xFF4A3A), if power { 1.0 } else { 0.0 });
    board_led(p, 6.2, 6.6, rgb(0x4A8BFF), l);
    p.text(
        6.8,
        5.0,
        "DEVKIT V1",
        0.55,
        font::mono,
        with_alpha(silk(), 0.7),
    );
    for ld in &cx.def.bench.leads {
        header_pin(p, ld.x, ld.y);
    }
    pin_labels(p, cx, 0.0, true);
    pin_labels(p, cx, 10.0, false);
}

pub fn pico(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-1.0, -0.7, 19.7, 7.7);
    let (power, l, _, _) = board_look(cx);
    p.shadow(body, 0.5, 10.0, 6.0, 0.55);
    p.grad(body, 0.5, rgb(0x33893A), rgb(0x256B2B));
    // castellated edges
    for ld in &cx.def.bench.leads {
        p.circle(ld.x, ld.y, 0.42, rgb(0xC9A54A));
        p.circle(ld.x, ld.y, 0.22, rgb(0x0E2A10));
    }
    let usb = WRect::new(-1.8, 2.3, 0.8, 4.7);
    p.grad(usb, 0.15, rgb(0xD4D8DD), rgb(0x9DA3AB));
    let chip = WRect::new(7.8, 2.0, 11.0, 5.2);
    p.shadow(chip, 0.1, 3.0, 2.0, 0.45);
    p.rect(chip, 0.1, rgb(0x191919));
    p.text(9.4, 3.6, "RP2040", 0.55, font::mono, rgb(0x9AA6AE));
    p.rect(WRect::new(3.2, 1.6, 4.8, 3.0), 0.2, rgb(0xEDEDED));
    p.text_at(
        2.0,
        5.4,
        "BOOTSEL",
        0.45,
        font::mono,
        silk(),
        Align2::LEFT_CENTER,
    );
    board_led(p, 2.0, 1.6, rgb(0x5CFF8A), if power { l } else { 0.0 });
    p.text(14.8, 3.5, "Raspberry Pi Pico", 0.62, font::medium, silk());
    pin_labels(p, cx, 0.0, true);
    pin_labels(p, cx, 7.0, false);
}

/// ESP-01 (ESP8266): the 2×4 header at the left, antenna top-left, chip on the right.
pub fn esp01(p: &Pen, cx: &ArtCtx) {
    let body = WRect::new(-0.9, -3.4, 9.5, 1.8);
    p.shadow(body, 0.25, 9.0, 5.0, 0.6);
    p.grad(body, 0.25, rgb(0x252525), rgb(0x161616));
    p.rect_stroke(body, 0.25, p.px(0.05, 0.6), rgb(0x333333));
    // antenna meander
    let mut zig = vec![(-0.3, -1.3)];
    for i in 0..5 {
        let x = -0.3 + i as f64 * 1.0;
        zig.push((x, -2.7));
        zig.push((x + 0.5, -2.7));
        zig.push((x + 0.5, -1.7));
        zig.push((x + 1.0, -1.7));
    }
    p.polyline(&zig, p.px(0.2, 1.2), rgb(0xC9A54A));
    // ESP8266 + flash
    let chip = WRect::new(5.2, -2.6, 8.6, 0.8);
    p.shadow(chip, 0.15, 3.0, 2.0, 0.45);
    p.grad(chip, 0.15, rgb(0x3A3A3A), rgb(0x262626));
    p.text(6.9, -0.9, "ESP8266", 0.55, font::mono, rgb(0xCFD5DB));
    p.rect(WRect::new(3.4, -1.0, 4.8, 0.9), 0.1, rgb(0x2E2E2E));
    // power LED (red) + activity (blue)
    let powered = cx.reading("vcc").is_none_or(|v| v > 2.5);
    board_led(p, 4.0, 1.25, rgb(0x4CC3F0), if powered { 0.8 } else { 0.0 });
    for ld in &cx.def.bench.leads {
        header_pin(p, ld.x, ld.y);
    }
    let name = format!("{} Wi-Fi", cx.part.reference);
    p.text_at(
        4.3,
        -3.9,
        &name,
        0.68,
        font::mono,
        rgb(0x625C51),
        Align2::CENTER_BOTTOM,
    );
}

/// HM-10 / HC-05 Bluetooth breakouts: blue board, module, antenna, state LED.
pub fn bt_module(p: &Pen, cx: &ArtCtx) {
    let h = cx.def.bench.size.1;
    let body = WRect::new(-0.7, -h + 0.4, 5.7, 0.6);
    p.shadow(body, 0.3, 9.0, 5.0, 0.55);
    p.grad(body, 0.3, rgb(0x2457B8), rgb(0x1A3F8A));
    let module = WRect::new(0.2, -h + 1.0, 4.8, -2.0);
    p.shadow(module, 0.1, 3.0, 2.0, 0.4);
    p.grad(module, 0.1, rgb(0x2F7D3A), rgb(0x225E2C));
    let mut zig = Vec::new();
    for i in 0..4 {
        let x = 0.8 + i as f64 * 1.0;
        zig.push((x, -h + 1.6));
        zig.push((x, -h + 2.8));
    }
    p.polyline(&zig, p.px(0.14, 1.0), rgb(0xC9A54A));
    let title = if cx.def.bench.art == "hm10" {
        "HM-10"
    } else {
        "HC-05"
    };
    p.text(
        2.5,
        -h * 0.45,
        title,
        0.75,
        font::mono_medium,
        rgb(0xEAF3F7),
    );
    let connected = cx.reading("connected").unwrap_or(0.0) > 0.5;
    let blink = if connected {
        1.0
    } else if (cx.t * 2.0).fract() < 0.5 {
        0.9
    } else {
        0.0
    };
    board_led(p, 4.6, -1.2, rgb(0x4A8BFF), blink);
    for ld in &cx.def.bench.leads {
        header_pin(p, ld.x, ld.y);
    }
    pin_labels(p, cx, 0.0, false);
}
