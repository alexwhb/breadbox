//! Solderless breadboards (830 / 400 tie points): cream body, terminal-strip holes in rows
//! a–j, the centre channel, two power-rail pairs with red/blue stripes, column numbers and row
//! letters.

use super::*;
use bb_circuit::breadboard::{ROWS, Rail, rail_x, row_y};
use bb_circuit::part::Layout;
use eframe::egui::{Align2, Mesh, Shape, pos2};

/// Holes are drawn individually above this zoom (points per 0.1").
pub fn breadboard_holes_visible(scale: f32) -> bool {
    scale >= 3.2
}

pub fn breadboard(p: &Pen, cx: &ArtCtx) {
    let (w, h) = cx.def.bench.size;
    let (cols, rail_holes) = match &cx.def.bench.layout {
        Some(Layout::Breadboard {
            columns,
            rail_holes,
        }) => (*columns, *rail_holes),
        _ => (63, 50),
    };
    let body = WRect::new(0.0, 0.0, w, h);
    // shadow + body
    p.shadow(body, 0.9, 14.0, 8.0, 0.55);
    p.grad(body, 0.9, rgb(0xF4F1EA), rgb(0xE6E1D6));
    p.rect_stroke(body, 0.9, p.px(0.08, 1.0), rgb(0xD2CBBE));
    // centre channel
    p.grad(
        WRect::new(0.35, 9.1, w - 0.35, 9.9),
        0.15,
        rgb(0xCFC8BA),
        rgb(0xE0DACE),
    );
    // rail stripes: red above each + row, blue below each − row
    let stripe = p.px(0.16, 1.2);
    let rx0 = rail_x(cols, rail_holes, 1) - 0.8;
    let rx1 = rail_x(cols, rail_holes, rail_holes) + 0.8;
    for rail in Rail::ALL {
        let y = rail.y();
        let (sy, col) = if rail.is_plus() {
            (y - 0.62, rgb(0xD64545))
        } else {
            (y + 0.62, rgb(0x2F6FD6))
        };
        p.line((rx0, sy), (rx1, sy), stripe, col);
        // + / − marks at both ends
        let mark = if rail.is_plus() { "+" } else { "−" };
        for x in [rx0 - 0.55, rx1 + 0.55] {
            p.text(x, y, mark, 0.95, font::mono_medium, col);
        }
    }
    // rail separators (between the rail block and the terminal strips)
    for y in [3.0, 16.0] {
        p.line(
            (0.5, y),
            (w - 0.5, y),
            p.px(0.04, 0.6),
            with_alpha(rgb(0xC9C1B2), 0.6),
        );
    }
    if breadboard_holes_visible(p.view.scale) {
        holes(p, cols, rail_holes);
    } else {
        // zoomed far out: a faint hole texture
        for (y0, y1) in [(0.6, 2.4), (3.6, 8.4), (10.6, 15.4), (16.6, 18.4)] {
            p.rect(
                WRect::new(1.0 - 0.3, y0, cols as f64 + 0.3, y1),
                0.1,
                with_alpha(rgb(0x8D8576), 0.35),
            );
        }
    }
    // column numbers + row letters
    if p.view.scale >= 4.5 {
        let num_c = rgb(0x8D8576);
        for c in 1..=cols {
            if c == 1 || c % 5 == 0 {
                p.text(c as f64, 3.45, &c.to_string(), 0.62, font::mono, num_c);
                p.text(c as f64, 15.55, &c.to_string(), 0.62, font::mono, num_c);
            }
        }
        for r in ROWS {
            if let Some(y) = row_y(r) {
                let s = r.to_string();
                p.text(0.35, y, &s, 0.62, font::mono, num_c);
                p.text(w - 0.35, y, &s, 0.62, font::mono, num_c);
            }
        }
    }
    // label
    let tie = if cols >= 60 { "830" } else { "400" };
    p.text_at(
        w - 1.0,
        h - 0.25,
        &format!("{} · {tie} tie-points", cx.part.reference),
        0.75,
        font::mono,
        rgb(0x7A7264),
        Align2::RIGHT_BOTTOM,
    );
}

/// All holes as one mesh (fast: ~830 quads).
fn holes(p: &Pen, cols: u32, rail_holes: u32) {
    let s = p.len(0.36).max(1.2);
    let hole = rgb(0x6F685C);
    let rim = rgb(0xFBF9F4);
    let mut mesh = Mesh::default();
    let mut quad = |x: f64, y: f64| {
        let c = p.p(x, y);
        let r = eframe::egui::Rect::from_center_size(c, eframe::egui::vec2(s, s));
        // light rim (bottom-right) then the dark hole
        let rr = r
            .translate(eframe::egui::vec2(s * 0.12, s * 0.14))
            .expand(s * 0.08);
        add_quad(&mut mesh, rr, rim);
        add_quad(&mut mesh, r, hole);
    };
    for c in 1..=cols {
        for r in ROWS {
            if let Some(y) = row_y(r) {
                quad(c as f64, y);
            }
        }
    }
    for rail in Rail::ALL {
        for k in 1..=rail_holes {
            quad(rail_x(cols, rail_holes, k), rail.y());
        }
    }
    p.painter.add(Shape::mesh(mesh));
}

fn add_quad(mesh: &mut Mesh, r: eframe::egui::Rect, c: Color32) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(r.left_top(), c);
    mesh.colored_vertex(pos2(r.right(), r.top()), c);
    mesh.colored_vertex(r.right_bottom(), c);
    mesh.colored_vertex(pos2(r.left(), r.bottom()), c);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}
