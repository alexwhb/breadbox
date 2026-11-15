//! Drawing in world units (0.1" = one breadboard pitch) on top of an egui painter: the view
//! transform, soft shadows, vertical gradients, glows, beziers and text that scale with zoom.
//! Every bench art function draws through a [`Pen`].

use bb_circuit::geometry::Placement;
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, epaint, pos2,
    vec2,
};

/// Pixels per 0.1" at 100 % zoom.
pub const BASE_SCALE: f32 = 8.0;

/// Pan / zoom: world (0.1" units) ↔ screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Screen position of world (0, 0).
    pub origin: Pos2,
    /// Points per world unit.
    pub scale: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            origin: pos2(0.0, 0.0),
            scale: BASE_SCALE,
        }
    }
}

impl View {
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        pos2(
            self.origin.x + x as f32 * self.scale,
            self.origin.y + y as f32 * self.scale,
        )
    }
    pub fn to_world(&self, p: Pos2) -> (f64, f64) {
        (
            ((p.x - self.origin.x) / self.scale) as f64,
            ((p.y - self.origin.y) / self.scale) as f64,
        )
    }
    pub fn len(&self, units: f64) -> f32 {
        units as f32 * self.scale
    }
    /// Zoom in percent (100 % = 8 px per 0.1").
    pub fn zoom_pct(&self) -> f32 {
        self.scale / BASE_SCALE * 100.0
    }
    /// Zooms about a screen point, clamped to 25–400 %.
    pub fn zoom_about(&mut self, at: Pos2, factor: f32) {
        let new = (self.scale * factor).clamp(BASE_SCALE * 0.25, BASE_SCALE * 4.0);
        let k = new / self.scale;
        self.origin = at + (self.origin - at) * k;
        self.scale = new;
    }
    /// Fits a world rectangle into `screen` (max 100 % unless `allow_zoom_in`).
    pub fn fit(screen: Rect, world: (f64, f64, f64, f64), max_scale: f32) -> View {
        let (x0, y0, x1, y1) = world;
        let w = ((x1 - x0) as f32).max(1.0);
        let h = ((y1 - y0) as f32).max(1.0);
        let scale = ((screen.width() - 48.0) / w)
            .min((screen.height() - 48.0) / h)
            .clamp(BASE_SCALE * 0.25, max_scale);
        let cx = (x0 + x1) as f32 / 2.0;
        let cy = (y0 + y1) as f32 / 2.0;
        View {
            origin: screen.center() - vec2(cx * scale, cy * scale),
            scale,
        }
    }
}

/// A world-space rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WRect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl WRect {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> WRect {
        WRect {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }
    pub fn xywh(x: f64, y: f64, w: f64, h: f64) -> WRect {
        WRect::new(x, y, x + w, y + h)
    }
    pub fn w(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f64 {
        self.y1 - self.y0
    }
    pub fn center(&self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn expand(&self, d: f64) -> WRect {
        WRect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }
    pub fn union(&self, o: &WRect) -> WRect {
        WRect::new(
            self.x0.min(o.x0),
            self.y0.min(o.y0),
            self.x1.max(o.x1),
            self.y1.max(o.y1),
        )
    }
    pub fn intersects(&self, o: &WRect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}

/// Colour helpers.
pub fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub fn rgba(hex: u32, a: f32) -> Color32 {
    let c = rgb(hex);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn lighten(c: Color32, t: f32) -> Color32 {
    crate::theme::mix(c, Color32::WHITE, t)
}

pub fn darken(c: Color32, t: f32) -> Color32 {
    let k = 1.0 - t.clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(
        (c.r() as f32 * k) as u8,
        (c.g() as f32 * k) as u8,
        (c.b() as f32 * k) as u8,
        c.a(),
    )
}

/// A light bloom queued to be drawn after wires (so LEDs glow over the jumpers).
#[derive(Debug, Clone, Copy)]
pub struct Glow {
    pub center: Pos2,
    pub radius: f32,
    pub color: Color32,
    pub intensity: f32,
}

/// Draws in world units through a view and an optional part-local placement.
#[derive(Clone)]
pub struct Pen<'a> {
    pub painter: &'a Painter,
    pub view: View,
    /// Local → world placement of the part being drawn (identity for scene items).
    pub local: Placement,
    /// Multiplies every colour's alpha (ghost previews while dragging).
    pub alpha: f32,
    /// When set, glows are queued here instead of drawn immediately.
    pub glows: Option<&'a std::cell::RefCell<Vec<Glow>>>,
}

impl<'a> Pen<'a> {
    pub fn new(painter: &'a Painter, view: View) -> Pen<'a> {
        Pen {
            painter,
            view,
            local: Placement::default(),
            alpha: 1.0,
            glows: None,
        }
    }

    /// Queues glows into `q` (drawn later with [`draw_glow`]).
    pub fn deferring(&self, q: &'a std::cell::RefCell<Vec<Glow>>) -> Pen<'a> {
        let mut p = self.clone();
        p.glows = Some(q);
        p
    }

    /// The same pen drawing in a part's local coordinates.
    pub fn at(&self, local: Placement) -> Pen<'a> {
        Pen {
            painter: self.painter,
            view: self.view,
            local,
            alpha: self.alpha,
            glows: self.glows,
        }
    }

    pub fn ghost(&self, alpha: f32) -> Pen<'a> {
        let mut p = self.clone();
        p.alpha = alpha;
        p
    }

    fn c(&self, c: Color32) -> Color32 {
        if self.alpha >= 1.0 {
            c
        } else {
            c.gamma_multiply(self.alpha)
        }
    }

    /// Local → screen.
    pub fn p(&self, x: f64, y: f64) -> Pos2 {
        let (wx, wy) = self.local.apply(x, y);
        self.view.to_screen(wx, wy)
    }

    /// Local rect → screen rect (rotations by multiples of 90° keep it axis-aligned).
    pub fn r(&self, r: WRect) -> Rect {
        let a = self.p(r.x0, r.y0);
        let b = self.p(r.x1, r.y1);
        Rect::from_two_pos(a, b)
    }

    pub fn len(&self, u: f64) -> f32 {
        self.view.len(u)
    }

    /// Pixel size for a world length, at least `min` px.
    pub fn px(&self, u: f64, min: f32) -> f32 {
        self.len(u).max(min)
    }

    pub fn visible(&self, r: WRect) -> bool {
        self.painter.clip_rect().intersects(self.r(r))
    }

    // ---------------------------------------------------------------- primitives

    pub fn rect(&self, r: WRect, radius: f64, fill: Color32) {
        let rr = self.r(r);
        self.painter.rect_filled(
            rr,
            CornerRadius::same(self.radius(radius, rr)),
            self.c(fill),
        );
    }

    pub fn rect_stroke(&self, r: WRect, radius: f64, width: f32, color: Color32) {
        let rr = self.r(r);
        self.painter.rect_stroke(
            rr,
            CornerRadius::same(self.radius(radius, rr)),
            Stroke::new(width, self.c(color)),
            egui::StrokeKind::Inside,
        );
    }

    pub fn rect_fs(&self, r: WRect, radius: f64, fill: Color32, width: f32, stroke: Color32) {
        self.rect(r, radius, fill);
        if width > 0.0 {
            self.rect_stroke(r, radius, width, stroke);
        }
    }

    fn radius(&self, radius: f64, rr: Rect) -> u8 {
        let px = self.len(radius).min(rr.width().min(rr.height()) / 2.0);
        px.clamp(0.0, 255.0) as u8
    }

    /// A soft drop shadow below a body (light from the top: offset down).
    pub fn shadow(&self, r: WRect, radius: f64, blur: f32, offset_y: f32, strength: f32) {
        let rr = self
            .r(r)
            .translate(vec2(0.0, offset_y * self.view.scale / 8.0));
        let blur = blur * (self.view.scale / 8.0).clamp(0.4, 3.0);
        let shape = epaint::RectShape::filled(
            rr,
            CornerRadius::same(self.radius(radius, rr)),
            self.c(Color32::from_black_alpha((strength * 255.0) as u8)),
        )
        .with_blur_width(blur);
        self.painter.add(shape);
    }

    /// Vertical gradient (screen top → bottom) in a rounded rectangle.
    pub fn grad(&self, r: WRect, radius: f64, top: Color32, bottom: Color32) {
        let rr = self.r(r);
        let rad = self.radius(radius, rr) as f32;
        let pts = rounded_rect_points(rr, rad);
        let mut mesh = Mesh::default();
        let c = rr.center();
        let col = |y: f32| {
            let t = ((y - rr.top()) / rr.height().max(1.0)).clamp(0.0, 1.0);
            self.c(crate::theme::mix(top, bottom, t))
        };
        mesh.colored_vertex(c, col(c.y));
        for p in &pts {
            mesh.colored_vertex(*p, col(p.y));
        }
        let n = pts.len() as u32;
        for i in 0..n {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
        }
        self.painter.add(Shape::mesh(mesh));
    }

    /// Horizontal gradient (screen left → right) in a rounded rectangle.
    pub fn hgrad(&self, r: WRect, radius: f64, left: Color32, right: Color32) {
        let rr = self.r(r);
        let rad = self.radius(radius, rr) as f32;
        let pts = rounded_rect_points(rr, rad);
        let mut mesh = Mesh::default();
        let c = rr.center();
        let col = |x: f32| {
            let t = ((x - rr.left()) / rr.width().max(1.0)).clamp(0.0, 1.0);
            self.c(crate::theme::mix(left, right, t))
        };
        mesh.colored_vertex(c, col(c.x));
        for p in &pts {
            mesh.colored_vertex(*p, col(p.x));
        }
        let n = pts.len() as u32;
        for i in 0..n {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
        }
        self.painter.add(Shape::mesh(mesh));
    }

    pub fn circle(&self, x: f64, y: f64, r: f64, fill: Color32) {
        self.painter
            .circle_filled(self.p(x, y), self.len(r).max(0.5), self.c(fill));
    }

    pub fn circle_stroke(&self, x: f64, y: f64, r: f64, width: f32, color: Color32) {
        self.painter.circle_stroke(
            self.p(x, y),
            self.len(r).max(0.5),
            Stroke::new(width, self.c(color)),
        );
    }

    /// A shaded sphere-ish disc: radial gradient from a highlight towards the rim.
    pub fn disc(&self, x: f64, y: f64, r: f64, center: Color32, rim: Color32) {
        let c = self.p(x, y);
        let rad = self.len(r).max(0.5);
        let hi = c + vec2(-rad * 0.3, -rad * 0.35);
        let mut mesh = Mesh::default();
        mesh.colored_vertex(hi, self.c(center));
        let n = 32;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(c + vec2(a.cos(), a.sin()) * rad, self.c(rim));
        }
        for i in 0..n as u32 {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n as u32);
        }
        self.painter.add(Shape::mesh(mesh));
    }

    /// A light bloom: `color` at the centre fading to transparent at radius `r`.
    pub fn glow(&self, x: f64, y: f64, r: f64, color: Color32, intensity: f32) {
        if intensity <= 0.01 {
            return;
        }
        let c = self.p(x, y);
        let rad = self.len(r).max(1.0);
        let g = Glow {
            center: c,
            radius: rad,
            color: self.c(color),
            intensity: intensity * self.alpha,
        };
        match self.glows {
            Some(q) => q.borrow_mut().push(g),
            None => draw_glow(self.painter, g),
        }
    }
}

/// Paints a radial bloom.
pub fn draw_glow(painter: &Painter, g: Glow) {
    {
        let (c, rad, color) = (g.center, g.radius, g.color);
        let intensity = g.intensity;
        let mut mesh = Mesh::default();
        let a = intensity.clamp(0.0, 1.0);
        mesh.colored_vertex(c, with_alpha(color, a));
        // two rings: a hot core and a soft falloff
        let n = 40;
        let mid = 0.38;
        for i in 0..n {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let d = vec2(t.cos(), t.sin());
            mesh.colored_vertex(c + d * rad * mid, with_alpha(color, a * 0.55));
        }
        for i in 0..n {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let d = vec2(t.cos(), t.sin());
            mesh.colored_vertex(c + d * rad, with_alpha(color, 0.0));
        }
        let n = n as u32;
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.add_triangle(0, 1 + i, 1 + j);
            mesh.add_triangle(1 + i, 1 + n + i, 1 + n + j);
            mesh.add_triangle(1 + i, 1 + n + j, 1 + j);
        }
        painter.add(Shape::mesh(mesh));
    }
}

impl<'a> Pen<'a> {
    pub fn ellipse(&self, x: f64, y: f64, rx: f64, ry: f64, fill: Color32) {
        let c = self.p(x, y);
        let (a, b) = (self.len(rx).max(0.5), self.len(ry).max(0.5));
        let pts: Vec<Pos2> = (0..36)
            .map(|i| {
                let t = i as f32 / 36.0 * std::f32::consts::TAU;
                c + vec2(t.cos() * a, t.sin() * b)
            })
            .collect();
        self.painter
            .add(Shape::convex_polygon(pts, self.c(fill), Stroke::NONE));
    }

    pub fn line(&self, a: (f64, f64), b: (f64, f64), width: f32, color: Color32) {
        self.painter.line_segment(
            [self.p(a.0, a.1), self.p(b.0, b.1)],
            Stroke::new(width, self.c(color)),
        );
    }

    /// A line whose width is in world units (scales with zoom).
    pub fn wline(&self, a: (f64, f64), b: (f64, f64), width: f64, color: Color32) {
        self.line(a, b, self.px(width, 0.6), color);
    }

    pub fn polyline(&self, pts: &[(f64, f64)], width: f32, color: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter
            .add(Shape::line(v, Stroke::new(width, self.c(color))));
    }

    /// Filled polygon (convex) in local coordinates.
    pub fn poly(&self, pts: &[(f64, f64)], fill: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter
            .add(Shape::convex_polygon(v, self.c(fill), Stroke::NONE));
    }

    pub fn poly_stroke(&self, pts: &[(f64, f64)], fill: Color32, width: f32, stroke: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::convex_polygon(
            v,
            self.c(fill),
            Stroke::new(width, self.c(stroke)),
        ));
    }

    /// Cubic bezier (local coordinates), stroke width in screen points.
    pub fn bezier(&self, p: [(f64, f64); 4], width: f32, color: Color32) {
        let pts = [
            self.p(p[0].0, p[0].1),
            self.p(p[1].0, p[1].1),
            self.p(p[2].0, p[2].1),
            self.p(p[3].0, p[3].1),
        ];
        self.painter
            .add(epaint::CubicBezierShape::from_points_stroke(
                pts,
                false,
                Color32::TRANSPARENT,
                Stroke::new(width, self.c(color)),
            ));
    }

    /// Text centred at a local point; `size` in world units (hidden when too small).
    pub fn text(
        &self,
        x: f64,
        y: f64,
        s: &str,
        size: f64,
        font: fn(f32) -> FontId,
        color: Color32,
    ) {
        self.text_at(x, y, s, size, font, color, egui::Align2::CENTER_CENTER);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_at(
        &self,
        x: f64,
        y: f64,
        s: &str,
        size: f64,
        font: fn(f32) -> FontId,
        color: Color32,
        align: egui::Align2,
    ) {
        let px = self.len(size);
        if px < 4.5 || s.is_empty() {
            return;
        }
        self.painter
            .text(self.p(x, y), align, s, font(px), self.c(color));
    }

    /// Silkscreen text along the part's local "up" direction (vertical pin labels). With
    /// `from_anchor` the text starts at `(x, y)` and runs up; otherwise it ends there (and so
    /// hangs below it). Rotated parts keep it readable (never upside down).
    #[allow(clippy::too_many_arguments)]
    pub fn text_up(
        &self,
        x: f64,
        y: f64,
        s: &str,
        size: f64,
        font: fn(f32) -> FontId,
        color: Color32,
        from_anchor: bool,
    ) {
        let px = self.len(size);
        if px < 4.5 || s.is_empty() {
            return;
        }
        let a = self.p(x, y);
        let u = (self.p(x, y - 1.0) - a).normalized();
        let galley = self
            .painter
            .layout_no_wrap(s.to_string(), font(px), self.c(color));
        let (w, h) = (galley.size().x, galley.size().y);
        // the text covers a + u·t for t in [0, w] (from the anchor) or [-w, 0]
        let (mut start, mut dir) = if from_anchor { (a, u) } else { (a - u * w, u) };
        if dir.x < -0.3 || (dir.x.abs() <= 0.3 && dir.y > 0.0) {
            start += dir * w;
            dir = -dir;
        }
        let n = egui::vec2(-dir.y, dir.x);
        let pos = start - n * (h / 2.0);
        let angle = dir.y.atan2(dir.x);
        self.painter
            .add(egui::epaint::TextShape::new(pos, galley, self.c(color)).with_angle(angle));
    }

    /// Text with a fixed screen size (labels that stay readable when zoomed out).
    #[allow(clippy::too_many_arguments)]
    pub fn label_px(
        &self,
        x: f64,
        y: f64,
        s: &str,
        px: f32,
        font: fn(f32) -> FontId,
        color: Color32,
        align: egui::Align2,
    ) -> Rect {
        self.painter
            .text(self.p(x, y), align, s, font(px), self.c(color))
    }
}

/// Points of a rounded rectangle outline (clockwise from top-left arc).
pub fn rounded_rect_points(r: Rect, rad: f32) -> Vec<Pos2> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    if rad < 0.5 {
        return vec![
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ];
    }
    let mut v = Vec::with_capacity(28);
    let seg = 6;
    let corners = [
        (pos2(r.left() + rad, r.top() + rad), 180.0f32),
        (pos2(r.right() - rad, r.top() + rad), 270.0),
        (pos2(r.right() - rad, r.bottom() - rad), 0.0),
        (pos2(r.left() + rad, r.bottom() - rad), 90.0),
    ];
    for (c, start) in corners {
        for i in 0..=seg {
            let a = (start + 90.0 * i as f32 / seg as f32).to_radians();
            v.push(c + vec2(a.cos(), a.sin()) * rad);
        }
    }
    v
}

/// Point on a cubic bezier.
pub fn bezier_point(p: [(f64, f64); 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let a = u * u * u;
    let b = 3.0 * u * u * t;
    let c = 3.0 * u * t * t;
    let d = t * t * t;
    (
        a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0,
        a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1,
    )
}

/// Distance from a point to a sampled cubic bezier.
pub fn bezier_distance(p: [(f64, f64); 4], q: (f64, f64)) -> f64 {
    let mut best = f64::INFINITY;
    let mut prev = p[0];
    for i in 1..=24 {
        let cur = bezier_point(p, i as f64 / 24.0);
        best = best.min(bb_circuit::geometry::point_segment_distance(q, prev, cur));
        prev = cur;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_round_trips_and_zoom_clamps() {
        let mut v = View {
            origin: pos2(100.0, 50.0),
            scale: 8.0,
        };
        let p = v.to_screen(10.0, 5.0);
        assert_eq!(p, pos2(180.0, 90.0));
        let w = v.to_world(p);
        assert!((w.0 - 10.0).abs() < 1e-6 && (w.1 - 5.0).abs() < 1e-6);
        v.zoom_about(p, 100.0);
        assert_eq!(v.zoom_pct(), 400.0);
        assert_eq!(v.to_screen(10.0, 5.0), p, "zoom keeps the anchor");
        v.zoom_about(p, 0.0001);
        assert_eq!(v.zoom_pct(), 25.0);
    }

    #[test]
    fn bezier_distance_is_zero_on_the_curve() {
        let b = [(0.0, 0.0), (0.0, 5.0), (10.0, 5.0), (10.0, 0.0)];
        let m = bezier_point(b, 0.5);
        assert!(bezier_distance(b, m) < 0.05);
        assert!(bezier_distance(b, (5.0, -3.0)) > 2.0);
    }
}
