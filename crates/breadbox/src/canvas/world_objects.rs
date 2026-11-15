//! World objects on the bench (`WORLD-4`): a hand, a person walking by, a wall, a magnet.
//! They live in the world model in metres (`project.world.objects`); the bench draws them at
//! `metres / 0.00254` bench units (the same mapping parts use for their world position) and
//! the user drags them. Distance (HC-SR04), motion (PIR) and magnetic (hall / reed) sensors
//! read them live through `Command::SetWorld { "<id>.x" / "<id>.y" }`.

use super::pen::{Pen, WRect};
use crate::theme::font;
use bb_sim::Telemetry;
use bb_world::{ObjectConfig, ObjectKind};
use eframe::egui::Color32;

/// Metres per bench unit (0.1").
pub const M_PER_UNIT: f64 = 0.00254;

/// An object as the bench shows it (bench units).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectView {
    pub id: String,
    pub kind: ObjectKind,
    pub x: f64,
    pub y: f64,
    /// Drawn / hit radius (bench units).
    pub r: f64,
}

/// Bench-unit radius an object is drawn with (its world size, clamped so a hand stays
/// grabbable and a person does not cover the bench).
fn draw_radius(o: &ObjectConfig) -> f64 {
    let base = (o.radius / M_PER_UNIT).clamp(6.0, 40.0);
    match o.kind {
        ObjectKind::Magnet => base.max(8.0),
        _ => base,
    }
}

/// The project's present objects at their live positions (scripts and timelines can move
/// them: the telemetry snapshot wins over the saved position).
pub fn objects(cfg: &[ObjectConfig], tel: Option<&Telemetry>) -> Vec<ObjectView> {
    cfg.iter()
        .filter(|o| o.present)
        .map(|o| {
            let pos = tel
                .and_then(|t| t.world.objects.iter().find(|(id, _)| id == &o.id))
                .map(|(_, p)| *p)
                .unwrap_or(o.pos);
            ObjectView {
                id: o.id.clone(),
                kind: o.kind,
                x: pos.x / M_PER_UNIT,
                y: pos.y / M_PER_UNIT,
                r: draw_radius(o),
            }
        })
        .collect()
}

/// The topmost object under a bench point.
pub fn hit(objs: &[ObjectView], x: f64, y: f64) -> Option<&ObjectView> {
    objs.iter()
        .rev()
        .find(|o| (o.x - x).powi(2) + (o.y - y).powi(2) <= (o.r * 1.05).powi(2))
}

fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Draws every object; `dragging` overrides one object's position while it is dragged.
pub fn draw(
    pen: &Pen,
    objs: &[ObjectView],
    dragging: Option<(&str, (f64, f64))>,
    hover: Option<&str>,
) {
    for o in objs {
        let (x, y) = match dragging {
            Some((id, p)) if id == o.id => p,
            _ => (o.x, o.y),
        };
        let r = o.r;
        if !pen.visible(WRect::new(
            x - r * 1.6,
            y - r * 1.6,
            x + r * 1.6,
            y + r * 1.6,
        )) {
            continue;
        }
        let lifted = dragging.is_some_and(|(id, _)| id == o.id);
        // soft shadow (higher while lifted)
        let off = if lifted { r * 0.35 } else { r * 0.15 };
        pen.ellipse(
            x + off * 0.4,
            y + off,
            r * 1.05,
            r * 0.95,
            Color32::from_black_alpha(if lifted { 70 } else { 90 }),
        );
        match o.kind {
            ObjectKind::Hand => hand(pen, x, y, r),
            ObjectKind::Person => person(pen, x, y, r),
            ObjectKind::Wall => wall(pen, x, y, r),
            ObjectKind::Magnet => magnet(pen, x, y, r),
        }
        if hover == Some(o.id.as_str()) || lifted {
            pen.circle_stroke(x, y, r * 1.12, 1.5, rgb(0xF2A33A));
        }
        pen.text(
            x,
            y + r * 1.25,
            &o.id,
            (r * 0.32).clamp(2.2, 5.0),
            font::mono,
            Color32::from_rgba_unmultiplied(230, 230, 230, 200),
        );
    }
}

/// An open hand seen from above: palm, four fingers, thumb.
fn hand(pen: &Pen, x: f64, y: f64, r: f64) {
    let skin = rgb(0xE2A983);
    let edge = rgb(0xA8714F);
    let fw = r * 0.26;
    // fingers (pointing up, the way an HC-SR04 on the bench faces)
    for (i, len) in [0.62, 0.78, 0.74, 0.58].iter().enumerate() {
        let fx = x - r * 0.42 + i as f64 * r * 0.28;
        let top = y - r * 0.25 - r * len;
        pen.wline((fx, y - r * 0.2), (fx, top), fw + 0.6, edge);
        pen.wline((fx, y - r * 0.2), (fx, top), fw, skin);
    }
    // thumb
    pen.wline(
        (x - r * 0.45, y + r * 0.15),
        (x - r * 0.9, y - r * 0.25),
        fw + 0.6,
        edge,
    );
    pen.wline(
        (x - r * 0.45, y + r * 0.15),
        (x - r * 0.9, y - r * 0.25),
        fw,
        skin,
    );
    // palm
    let palm = WRect::new(x - r * 0.58, y - r * 0.35, x + r * 0.5, y + r * 0.6);
    pen.rect_fs(palm, r * 0.3, skin, 0.8, edge);
    pen.grad(
        palm.expand(-r * 0.06),
        r * 0.26,
        Color32::from_white_alpha(28),
        Color32::from_black_alpha(18),
    );
}

/// A person from above: shoulders + head.
fn person(pen: &Pen, x: f64, y: f64, r: f64) {
    pen.ellipse(x, y + r * 0.1, r, r * 0.55, rgb(0x3D5A80));
    pen.ellipse(x, y + r * 0.1, r * 0.92, r * 0.47, rgb(0x4A6C97));
    pen.circle(x, y - r * 0.05, r * 0.38, rgb(0x6B4A33));
    pen.circle(x, y - r * 0.08, r * 0.32, rgb(0x7E5A3F));
}

/// A wall / box (reflects ultrasound): a brick bar.
fn wall(pen: &Pen, x: f64, y: f64, r: f64) {
    let w = WRect::new(x - r * 1.5, y - r * 0.35, x + r * 1.5, y + r * 0.35);
    pen.rect_fs(w, r * 0.06, rgb(0x9C5B45), 0.8, rgb(0x6E3E2F));
    let rows = 2;
    for k in 1..rows {
        let yy = w.y0 + w.h() * k as f64 / rows as f64;
        pen.line((w.x0, yy), (w.x1, yy), 1.0, rgb(0xC9B8A6));
    }
    let n = 6;
    for k in 1..n {
        let xx = w.x0 + w.w() * k as f64 / n as f64;
        let (a, b) = if k % 2 == 0 {
            (w.y0, w.y0 + w.h() / 2.0)
        } else {
            (w.y0 + w.h() / 2.0, w.y1)
        };
        pen.line((xx, a), (xx, b), 1.0, rgb(0xC9B8A6));
    }
}

/// A bar magnet: red N, blue S.
fn magnet(pen: &Pen, x: f64, y: f64, r: f64) {
    let n = WRect::new(x - r, y - r * 0.42, x, y + r * 0.42);
    let s = WRect::new(x, y - r * 0.42, x + r, y + r * 0.42);
    pen.rect(n, r * 0.08, rgb(0xC8352B));
    pen.rect(s, r * 0.08, rgb(0x3567C6));
    pen.grad(
        WRect::new(x - r, y - r * 0.42, x + r, y),
        r * 0.08,
        Color32::from_white_alpha(40),
        Color32::from_white_alpha(0),
    );
    let sz = (r * 0.5).clamp(2.0, 8.0);
    pen.text(x - r * 0.5, y, "N", sz, font::semibold, Color32::WHITE);
    pen.text(x + r * 0.5, y, "S", sz, font::semibold, Color32::WHITE);
}

/// A new object of `kind` near the first sensor that reads it (30 cm in front of an HC-SR04
/// or PIR, beside a hall sensor), else near the middle of the bench (`at`, bench units).
pub fn new_object(
    kind: ObjectKind,
    existing: &[ObjectConfig],
    sensor_at: Option<(f64, f64)>,
    at: (f64, f64),
) -> ObjectConfig {
    let base = match kind {
        ObjectKind::Hand => "hand",
        ObjectKind::Person => "person",
        ObjectKind::Wall => "wall",
        ObjectKind::Magnet => "magnet",
    };
    let mut id = base.to_string();
    let mut n = 2;
    while existing.iter().any(|o| o.id == id) {
        id = format!("{base}{n}");
        n += 1;
    }
    let (radius, offset_m) = match kind {
        ObjectKind::Hand => (0.04, (0.0, -0.30)),
        ObjectKind::Person => (0.25, (0.0, -1.2)),
        ObjectKind::Wall => (0.3, (0.0, -0.8)),
        ObjectKind::Magnet => (0.01, (0.0, -0.03)),
    };
    let (x, y) = match sensor_at {
        Some((sx, sy)) => (sx * M_PER_UNIT + offset_m.0, sy * M_PER_UNIT + offset_m.1),
        None => (at.0 * M_PER_UNIT, at.1 * M_PER_UNIT),
    };
    ObjectConfig {
        id,
        kind,
        pos: bb_core::Pos::new(x, y),
        radius,
        strength_mt: 50.0,
        present: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_map_metres_to_bench_units_and_hit_test() {
        let cfg = vec![ObjectConfig {
            id: "hand".into(),
            pos: bb_core::Pos::new(0.254, 0.0254),
            radius: 0.04,
            ..Default::default()
        }];
        let v = objects(&cfg, None);
        assert!((v[0].x - 100.0).abs() < 1e-9 && (v[0].y - 10.0).abs() < 1e-9);
        assert!(hit(&v, 105.0, 12.0).is_some());
        assert!(hit(&v, 150.0, 10.0).is_none());
    }

    #[test]
    fn new_objects_get_unique_ids_in_front_of_the_sensor() {
        let a = new_object(ObjectKind::Hand, &[], Some((100.0, 50.0)), (0.0, 0.0));
        assert_eq!(a.id, "hand");
        assert!((a.pos.y - (50.0 * M_PER_UNIT - 0.30)).abs() < 1e-9);
        let b = new_object(ObjectKind::Hand, &[a], None, (10.0, 10.0));
        assert_eq!(b.id, "hand2");
    }
}
