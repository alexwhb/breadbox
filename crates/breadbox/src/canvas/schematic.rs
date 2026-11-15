//! The schematic view (`WB-7`, Pro → Schematic): the same circuit drawn with standard
//! symbols (ANSI or IEC), reference designators and values, explicit orthogonal schematic
//! wires plus every physical connection from the bench (auto-routed, so "physical wiring
//! always shows up in the schematic"), junction dots, net labels, power and ground symbols,
//! live net values while running and the ERC finding of the sheet.
//!
//! Drawing goes through [`SchDraw`] so the same code renders on screen (egui) and to SVG
//! (Share → schematic SVG / PNG).

use super::pen::{View, WRect, rgb};
use crate::circuit::CircuitDoc;
use bb_circuit::part::{PinDir, PinRole};
use bb_circuit::project::Project;
use bb_circuit::{Connectivity, Library, PartDef, PartInstance};
use bb_sim::Telemetry;
use eframe::egui::{
    self, Align2, Color32, FontId, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// Points per grid unit at 100 %.
pub const SCH_SCALE: f32 = 12.0;

const INK: u32 = 0x1C1C1C;
const PAPER: u32 = 0xF6F5F0;
const BODY: u32 = 0xFFFDF6;
const GREY: u32 = 0x5C5C5C;
const DOT: u32 = 0xD4D0C4;

/// Drawing backend in grid units.
pub trait SchDraw {
    fn line(&mut self, a: (f64, f64), b: (f64, f64), w: f64, c: Color32);
    fn polyline(&mut self, pts: &[(f64, f64)], w: f64, c: Color32);
    fn polygon(
        &mut self,
        pts: &[(f64, f64)],
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    );
    fn circle(
        &mut self,
        c: (f64, f64),
        r: f64,
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    );
    fn rect(
        &mut self,
        r: WRect,
        radius: f64,
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    );
    /// `size` in grid units.
    fn text(
        &mut self,
        p: (f64, f64),
        s: &str,
        size: f64,
        bold: bool,
        color: Color32,
        align: Align2,
    );
    /// Text centred at `p`, rotated −90° (reads bottom to top).
    fn text_up(&mut self, p: (f64, f64), s: &str, size: f64, bold: bool, color: Color32);
}

/// egui backend.
pub struct EguiSch<'a> {
    pub painter: &'a Painter,
    pub view: View,
}

impl EguiSch<'_> {
    fn p(&self, q: (f64, f64)) -> Pos2 {
        self.view.to_screen(q.0, q.1)
    }
    fn w(&self, w: f64) -> f32 {
        (w as f32 * self.view.scale).max(0.8)
    }
}

impl SchDraw for EguiSch<'_> {
    fn line(&mut self, a: (f64, f64), b: (f64, f64), w: f64, c: Color32) {
        self.painter
            .line_segment([self.p(a), self.p(b)], Stroke::new(self.w(w), c));
    }
    fn polyline(&mut self, pts: &[(f64, f64)], w: f64, c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|q| self.p(*q)).collect();
        self.painter.add(Shape::line(v, Stroke::new(self.w(w), c)));
    }
    fn polygon(
        &mut self,
        pts: &[(f64, f64)],
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    ) {
        let v: Vec<Pos2> = pts.iter().map(|q| self.p(*q)).collect();
        self.painter.add(Shape::convex_polygon(
            v,
            fill.unwrap_or(Color32::TRANSPARENT),
            stroke
                .map(|(w, c)| Stroke::new(self.w(w), c))
                .unwrap_or(Stroke::NONE),
        ));
    }
    fn circle(
        &mut self,
        c: (f64, f64),
        r: f64,
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    ) {
        let rr = (r as f32 * self.view.scale).max(0.6);
        if let Some(f) = fill {
            self.painter.circle_filled(self.p(c), rr, f);
        }
        if let Some((w, s)) = stroke {
            self.painter
                .circle_stroke(self.p(c), rr, Stroke::new(self.w(w), s));
        }
    }
    fn rect(
        &mut self,
        r: WRect,
        radius: f64,
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    ) {
        let rr = Rect::from_two_pos(self.p((r.x0, r.y0)), self.p((r.x1, r.y1)));
        let rad = (radius as f32 * self.view.scale).clamp(0.0, 255.0) as u8;
        if let Some(f) = fill {
            self.painter.rect_filled(rr, rad, f);
        }
        if let Some((w, s)) = stroke {
            self.painter
                .rect_stroke(rr, rad, Stroke::new(self.w(w), s), egui::StrokeKind::Middle);
        }
    }
    fn text(
        &mut self,
        p: (f64, f64),
        s: &str,
        size: f64,
        bold: bool,
        color: Color32,
        align: Align2,
    ) {
        let px = size as f32 * self.view.scale;
        if px < 4.0 {
            return;
        }
        let f: FontId = if bold {
            crate::theme::font::mono_medium(px)
        } else {
            crate::theme::font::mono(px)
        };
        self.painter.text(self.p(p), align, s, f, color);
    }
    fn text_up(&mut self, p: (f64, f64), s: &str, size: f64, bold: bool, color: Color32) {
        let px = size as f32 * self.view.scale;
        if px < 4.0 {
            return;
        }
        let f: FontId = if bold {
            crate::theme::font::mono_medium(px)
        } else {
            crate::theme::font::mono(px)
        };
        let g = self.painter.layout_no_wrap(s.to_string(), f, color);
        let c = self.p(p);
        // rotate about the galley's centre: its top-left goes to c + (−h/2, +w/2)
        let pos = c + vec2(-g.size().y / 2.0, g.size().x / 2.0);
        self.painter.add(
            egui::epaint::TextShape::new(pos, g, color).with_angle(-std::f32::consts::FRAC_PI_2),
        );
    }
}

/// SVG backend (10 user units per grid unit).
pub struct SvgSch {
    pub out: String,
}

const SVG_K: f64 = 10.0;

fn hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl SchDraw for SvgSch {
    fn line(&mut self, a: (f64, f64), b: (f64, f64), w: f64, c: Color32) {
        let _ = writeln!(
            self.out,
            r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="{:.2}" stroke-linecap="round"/>"#,
            a.0 * SVG_K,
            a.1 * SVG_K,
            b.0 * SVG_K,
            b.1 * SVG_K,
            hex(c),
            w * SVG_K
        );
    }
    fn polyline(&mut self, pts: &[(f64, f64)], w: f64, c: Color32) {
        let d: Vec<String> = pts
            .iter()
            .map(|q| format!("{:.1},{:.1}", q.0 * SVG_K, q.1 * SVG_K))
            .collect();
        let _ = writeln!(
            self.out,
            r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="{:.2}" stroke-linejoin="round" stroke-linecap="round"/>"#,
            d.join(" "),
            hex(c),
            w * SVG_K
        );
    }
    fn polygon(
        &mut self,
        pts: &[(f64, f64)],
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    ) {
        let d: Vec<String> = pts
            .iter()
            .map(|q| format!("{:.1},{:.1}", q.0 * SVG_K, q.1 * SVG_K))
            .collect();
        let (s, sw) = stroke
            .map(|(w, c)| (hex(c), w * SVG_K))
            .unwrap_or(("none".into(), 0.0));
        let _ = writeln!(
            self.out,
            r#"<polygon points="{}" fill="{}" stroke="{}" stroke-width="{:.2}" stroke-linejoin="round"/>"#,
            d.join(" "),
            fill.map(hex).unwrap_or("none".into()),
            s,
            sw
        );
    }
    fn circle(
        &mut self,
        c: (f64, f64),
        r: f64,
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    ) {
        let (s, sw) = stroke
            .map(|(w, c)| (hex(c), w * SVG_K))
            .unwrap_or(("none".into(), 0.0));
        let _ = writeln!(
            self.out,
            r#"<circle cx="{:.1}" cy="{:.1}" r="{:.2}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"#,
            c.0 * SVG_K,
            c.1 * SVG_K,
            r * SVG_K,
            fill.map(hex).unwrap_or("none".into()),
            s,
            sw
        );
    }
    fn rect(
        &mut self,
        r: WRect,
        radius: f64,
        fill: Option<Color32>,
        stroke: Option<(f64, Color32)>,
    ) {
        let (s, sw) = stroke
            .map(|(w, c)| (hex(c), w * SVG_K))
            .unwrap_or(("none".into(), 0.0));
        let _ = writeln!(
            self.out,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"#,
            r.x0 * SVG_K,
            r.y0 * SVG_K,
            r.w() * SVG_K,
            r.h() * SVG_K,
            radius * SVG_K,
            fill.map(hex).unwrap_or("none".into()),
            s,
            sw
        );
    }
    fn text(
        &mut self,
        p: (f64, f64),
        s: &str,
        size: f64,
        bold: bool,
        color: Color32,
        align: Align2,
    ) {
        let anchor = match align.x() {
            egui::Align::Min => "start",
            egui::Align::Center => "middle",
            egui::Align::Max => "end",
        };
        let base = match align.y() {
            egui::Align::Min => "hanging",
            egui::Align::Center => "central",
            egui::Align::Max => "auto",
        };
        let _ = writeln!(
            self.out,
            r#"<text x="{:.1}" y="{:.1}" font-family="JetBrains Mono" font-size="{:.1}" font-weight="{}" fill="{}" text-anchor="{anchor}" dominant-baseline="{base}">{}</text>"#,
            p.0 * SVG_K,
            p.1 * SVG_K,
            size * SVG_K,
            if bold { 500 } else { 400 },
            hex(color),
            esc(s)
        );
    }
    fn text_up(&mut self, p: (f64, f64), s: &str, size: f64, bold: bool, color: Color32) {
        let (x, y) = (p.0 * SVG_K, p.1 * SVG_K);
        let _ = writeln!(
            self.out,
            r#"<text x="{x:.1}" y="{y:.1}" font-family="JetBrains Mono" font-size="{:.1}" font-weight="{}" fill="{}" text-anchor="middle" dominant-baseline="central" transform="rotate(-90 {x:.1} {y:.1})">{}</text>"#,
            size * SVG_K,
            if bold { 500 } else { 400 },
            hex(color),
            esc(s)
        );
    }
}

// ------------------------------------------------------------------------------------------
// the sheet model

/// One drawn connection segment run (an explicit or auto-routed wire).
#[derive(Debug, Clone, PartialEq)]
pub struct SchSegment {
    pub pts: Vec<(f64, f64)>,
    pub net: Option<String>,
    /// Drawn by the user in the schematic (vs. mirrored from the bench).
    pub explicit: bool,
    pub id: Option<String>,
}

/// A power / ground flag at a pin.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    pub at: (f64, f64),
    pub dir: PinDir,
    pub text: String,
    pub ground: bool,
}

/// A pin anchor on the sheet: (part id, pin, x, y, direction, net name).
pub type Anchor = (String, String, f64, f64, PinDir, Option<String>);

/// Everything to draw for one revision.
#[derive(Debug, Clone, Default)]
pub struct Sheet {
    pub segments: Vec<SchSegment>,
    pub junctions: Vec<(f64, f64)>,
    pub flags: Vec<Flag>,
    /// Net-label flags for long-distance signal nets.
    pub labels: Vec<Flag>,
    pub extent: WRect,
    /// Pin anchors (part id, pin, x, y, dir, net).
    pub anchors: Vec<Anchor>,
}

fn dir_vec(d: PinDir) -> (f64, f64) {
    match d {
        PinDir::Left => (-1.0, 0.0),
        PinDir::Right => (1.0, 0.0),
        PinDir::Up => (0.0, -1.0),
        PinDir::Down => (0.0, 1.0),
    }
}

/// Pin direction after the part's rotation / flip.
fn rotate_dir(d: PinDir, at: &bb_circuit::geometry::Placement) -> PinDir {
    let (dx, dy) = dir_vec(d);
    let (x0, y0) = at.apply(0.0, 0.0);
    let (x1, y1) = at.apply(dx, dy);
    let (vx, vy) = (x1 - x0, y1 - y0);
    if vx.abs() > vy.abs() {
        if vx > 0.0 {
            PinDir::Right
        } else {
            PinDir::Left
        }
    } else if vy > 0.0 {
        PinDir::Down
    } else {
        PinDir::Up
    }
}

/// Is this net a supply rail (named like one or fed by a power output)?
fn supply_name(
    conn: &Connectivity,
    project: &Project,
    lib: &Library,
    net: &bb_circuit::Net,
) -> Option<String> {
    if net.is_ground {
        return None;
    }
    let n = net.name.to_ascii_uppercase();
    if [
        "5V", "3V3", "VCC", "VDD", "VIN", "VBUS", "3.3V", "12V", "9V",
    ]
    .contains(&n.as_str())
    {
        return Some(n);
    }
    let fed = net.pins.iter().any(|t| {
        project
            .part(&t.part)
            .and_then(|p| lib.get(&p.part))
            .and_then(|d| d.pin(&t.pin).map(|pd| pd.role == PinRole::PowerOut))
            .unwrap_or(false)
    });
    let _ = conn;
    fed.then(|| net.name.clone())
}

fn flag_text(name: &str) -> String {
    let n = name.trim_start_matches('+');
    if n.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("+{n}")
    } else {
        n.to_string()
    }
}

impl Sheet {
    pub fn build(project: &Project, lib: &Library, conn: &Connectivity) -> Sheet {
        let mut sheet = Sheet::default();
        let mut ext: Option<WRect> = None;
        let grow = |e: &mut Option<WRect>, r: WRect| {
            *e = Some(match *e {
                Some(x) => x.union(&r),
                None => r,
            })
        };
        // anchors + body extents
        for p in &project.parts {
            let (Some(def), Some(at)) = (lib.get(&p.part), p.schematic.as_ref()) else {
                continue;
            };
            if def.schematic.symbol == "none" {
                continue;
            }
            let (w, h) = def.schematic.size;
            let a = at.apply(-w / 2.0 - 2.5, -h / 2.0 - 2.5);
            let b = at.apply(w / 2.0 + 2.5, h / 2.0 + 2.5);
            grow(&mut ext, WRect::new(a.0, a.1, b.0, b.1));
            for sp in &def.schematic.pins {
                let (x, y) = at.apply(sp.x, sp.y);
                let net = conn
                    .net_of(&p.id, &sp.pin)
                    .map(|n| conn.net(n).name.clone());
                sheet.anchors.push((
                    p.id.clone(),
                    sp.pin.clone(),
                    x,
                    y,
                    rotate_dir(sp.dir, at),
                    net,
                ));
                grow(&mut ext, WRect::new(x - 1.0, y - 1.0, x + 1.0, y + 1.0));
            }
        }
        // explicit wires
        let mut deg: HashMap<(i64, i64), u32> = HashMap::new();
        let key = |q: (f64, f64)| ((q.0 * 2.0).round() as i64, (q.1 * 2.0).round() as i64);
        for w in &project.schematic.wires {
            for q in &w.points {
                grow(
                    &mut ext,
                    WRect::new(q.0 - 1.0, q.1 - 1.0, q.0 + 1.0, q.1 + 1.0),
                );
            }
            let net = w
                .points
                .first()
                .and_then(|q| {
                    sheet
                        .anchors
                        .iter()
                        .find(|a| (a.2 - q.0).abs() < 0.26 && (a.3 - q.1).abs() < 0.26)
                })
                .and_then(|a| a.5.clone());
            for (i, q) in w.points.iter().enumerate() {
                let inc = if i == 0 || i + 1 == w.points.len() {
                    1
                } else {
                    2
                };
                *deg.entry(key(*q)).or_default() += inc;
            }
            sheet.segments.push(SchSegment {
                pts: w.points.clone(),
                net,
                explicit: true,
                id: Some(w.id.clone()),
            });
        }
        // nets mirrored from the bench: power/ground flags, routed signal nets
        let placed: HashSet<&str> = project
            .parts
            .iter()
            .filter(|p| p.schematic.is_some())
            .map(|p| p.id.as_str())
            .collect();
        for net in &conn.nets {
            let pins: Vec<&Anchor> = sheet
                .anchors
                .iter()
                .filter(|a| {
                    net.pins.iter().any(|t| t.part == a.0 && t.pin == a.1)
                        && placed.contains(a.0.as_str())
                })
                .collect();
            if pins.is_empty() {
                continue;
            }
            // pins already joined in the schematic count as one
            let mut groups: Vec<Vec<usize>> = Vec::new();
            for (i, a) in pins.iter().enumerate() {
                match groups.iter_mut().find(|g| {
                    let b = pins[g[0]];
                    conn.connected_in_schematic((&a.0, &a.1), (&b.0, &b.1))
                }) {
                    Some(g) => g.push(i),
                    None => groups.push(vec![i]),
                }
            }
            // an unconnected supply pin (IOREF, VIN) needs no flag
            if !net.is_ground && net.pins.len() + net.holes.len() < 2 {
                continue;
            }
            if net.is_ground || supply_name(conn, project, lib, net).is_some() {
                let text = if net.is_ground {
                    "GND".to_string()
                } else if let Some(v) = net.pins.iter().find_map(|t| match t.pin.as_str() {
                    "5V" | "3V3" | "VBUS" | "VIN" => Some(t.pin.clone()),
                    _ => None,
                }) {
                    flag_text(&v)
                } else {
                    flag_text(&supply_name(conn, project, lib, net).unwrap_or_default())
                };
                // one flag per pin that is not wired to another pin in the schematic
                for g in &groups {
                    for &i in g {
                        let a = pins[i];
                        // power symbols of their own (gnd / power_symbol parts) need no flag
                        let own = project
                            .part(&a.0)
                            .and_then(|p| lib.get(&p.part))
                            .is_some_and(|d| {
                                d.schematic.symbol == "ground" || d.schematic.symbol == "vcc"
                            });
                        if own || (g.len() > 1 && i != g[0]) {
                            continue;
                        }
                        let (dx, dy) = dir_vec(a.4);
                        let crowded = sheet.flags.iter().any(|f| {
                            f.dir == a.4
                                && f.text != text
                                && (f.at.0 - a.2).abs() < 3.0
                                && (f.at.1 - (a.3 + dy * 1.5)).abs() < 1.6
                        });
                        let len = if crowded { 4.0 } else { 1.5 };
                        let end = (a.2 + dx * len, a.3 + dy * len);
                        sheet.segments.push(SchSegment {
                            pts: vec![(a.2, a.3), end],
                            net: Some(net.name.clone()),
                            explicit: false,
                            id: None,
                        });
                        sheet.flags.push(Flag {
                            at: end,
                            dir: a.4,
                            text: text.clone(),
                            ground: net.is_ground,
                        });
                    }
                }
                continue;
            }
            if groups.len() < 2 {
                continue;
            }
            // representative point per group; MST over Manhattan distance
            let reps: Vec<usize> = groups.iter().map(|g| g[0]).collect();
            let far = reps.iter().any(|&i| {
                reps.iter()
                    .any(|&j| (pins[i].2 - pins[j].2).abs() + (pins[i].3 - pins[j].3).abs() > 20.0)
            });
            if far {
                for &i in &reps {
                    let a = pins[i];
                    let (dx, dy) = dir_vec(a.4);
                    let end = (a.2 + dx * 2.0, a.3 + dy * 2.0);
                    sheet.segments.push(SchSegment {
                        pts: vec![(a.2, a.3), end],
                        net: Some(net.name.clone()),
                        explicit: false,
                        id: None,
                    });
                    sheet.labels.push(Flag {
                        at: end,
                        dir: a.4,
                        text: crate::canvas::bench::net_display(&net.name),
                        ground: false,
                    });
                }
                continue;
            }
            let n = reps.len();
            let mut in_tree = vec![false; n];
            in_tree[0] = true;
            let dist = |i: usize, j: usize| {
                (pins[reps[i]].2 - pins[reps[j]].2).abs()
                    + (pins[reps[i]].3 - pins[reps[j]].3).abs()
            };
            for _ in 1..n {
                let mut best = (f64::INFINITY, 0, 0);
                for i in 0..n {
                    if !in_tree[i] {
                        continue;
                    }
                    for (j, &done) in in_tree.iter().enumerate() {
                        if done {
                            continue;
                        }
                        let d = dist(i, j);
                        if d < best.0 {
                            best = (d, i, j);
                        }
                    }
                }
                in_tree[best.2] = true;
                let (a, b) = (pins[reps[best.1]], pins[reps[best.2]]);
                let pts = route((a.2, a.3), a.4, (b.2, b.3), b.4);
                for (k, q) in pts.iter().enumerate() {
                    let inc = if k == 0 || k + 1 == pts.len() { 1 } else { 0 };
                    *deg.entry(key(*q)).or_default() += inc;
                }
                sheet.segments.push(SchSegment {
                    pts,
                    net: Some(net.name.clone()),
                    explicit: false,
                    id: None,
                });
            }
        }
        // junctions: wire ends meeting at a point with three or more branches, or a wire end
        // landing on the middle of another segment of the same net (T)
        for (k, d) in &deg {
            if *d >= 3 {
                sheet.junctions.push((k.0 as f64 / 2.0, k.1 as f64 / 2.0));
            }
        }
        let segs = sheet.segments.clone();
        for s in &segs {
            for end in s.pts.iter() {
                for o in &segs {
                    if std::ptr::eq(o, s) || o.net != s.net || o.net.is_none() {
                        continue;
                    }
                    for w in o.pts.windows(2) {
                        if on_segment(*end, w[0], w[1])
                            && !close(*end, w[0])
                            && !close(*end, w[1])
                            && !sheet.junctions.iter().any(|j| close(*j, *end))
                        {
                            sheet.junctions.push(*end);
                        }
                    }
                }
            }
        }
        // flags whose texts would overlap: lift every second one on a longer stub
        let n = sheet.flags.len();
        for i in 0..n {
            for j in 0..i {
                let (a, b) = (&sheet.flags[i], &sheet.flags[j]);
                if a.ground || b.ground || a.dir != b.dir || a.text == b.text {
                    continue;
                }
                let horizontal = matches!(a.dir, PinDir::Up | PinDir::Down);
                let half = (a.text.chars().count() + b.text.chars().count()) as f64 * 0.27;
                let _ = horizontal;
                let overlap = (a.at.0 - b.at.0).abs() < half && (a.at.1 - b.at.1).abs() < 1.2;
                if overlap {
                    let (dx, dy) = dir_vec(a.dir);
                    let old = sheet.flags[i].at;
                    let new = (old.0 + dx * 2.2, old.1 + dy * 2.2);
                    sheet.flags[i].at = new;
                    if let Some(seg) = sheet
                        .segments
                        .iter_mut()
                        .find(|s| s.pts.last() == Some(&old))
                    {
                        seg.pts.push(new);
                    }
                }
            }
        }
        sheet.extent = ext.unwrap_or(WRect::new(0.0, 0.0, 60.0, 40.0));
        sheet
    }
}

fn close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05
}

fn on_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> bool {
    bb_circuit::geometry::point_segment_distance(p, a, b) < 0.05
}

/// Orthogonal route between two pin anchors leaving along their pin directions.
pub fn route(a: (f64, f64), da: PinDir, b: (f64, f64), db: PinDir) -> Vec<(f64, f64)> {
    if (a.0 - b.0).abs() < 1e-6 || (a.1 - b.1).abs() < 1e-6 {
        return vec![a, b];
    }
    let (ax, ay) = dir_vec(da);
    let (bx, by) = dir_vec(db);
    let a1 = (a.0 + ax * 1.0, a.1 + ay * 1.0);
    let b1 = (b.0 + bx * 1.0, b.1 + by * 1.0);
    let horizontal_first = ax != 0.0;
    let corner = if horizontal_first {
        (b1.0, a1.1)
    } else {
        (a1.0, b1.1)
    };
    let mut pts = vec![a, a1, corner, b1, b];
    pts.dedup_by(|x, y| close(*x, *y));
    // drop collinear middle points
    let mut out: Vec<(f64, f64)> = Vec::new();
    for q in pts {
        if out.len() >= 2 {
            let p0 = out[out.len() - 2];
            let p1 = out[out.len() - 1];
            if ((p0.0 - p1.0).abs() < 1e-9 && (p1.0 - q.0).abs() < 1e-9)
                || ((p0.1 - p1.1).abs() < 1e-9 && (p1.1 - q.1).abs() < 1e-9)
            {
                out.pop();
            }
        }
        out.push(q);
    }
    out
}

// ------------------------------------------------------------------------------------------
// symbols

/// Draws one part's symbol, designator and value.
pub fn draw_symbol(
    d: &mut dyn SchDraw,
    def: &PartDef,
    part: &PartInstance,
    iec: bool,
    selected: bool,
) {
    let Some(at) = part.schematic.as_ref() else {
        return;
    };
    let ink = if selected { rgb(0xC77A12) } else { rgb(INK) };
    let body = rgb(BODY);
    let w = 0.16;
    let t = |x: f64, y: f64| at.apply(x, y);
    let line = |d: &mut dyn SchDraw, a: (f64, f64), b: (f64, f64)| {
        d.line(t(a.0, a.1), t(b.0, b.1), w, ink)
    };
    let poly = |d: &mut dyn SchDraw, pts: &[(f64, f64)]| {
        let v: Vec<(f64, f64)> = pts.iter().map(|q| t(q.0, q.1)).collect();
        d.polyline(&v, w, ink)
    };
    let shape = |d: &mut dyn SchDraw, pts: &[(f64, f64)], fill: Option<Color32>| {
        let v: Vec<(f64, f64)> = pts.iter().map(|q| t(q.0, q.1)).collect();
        d.polygon(&v, fill, Some((w, ink)))
    };
    let sym = def.schematic.symbol.as_str();
    let value = value_text(def, part);
    // label placement (outside the body, upright)
    let (lw, lh) = def.schematic.size;
    let vertical = {
        let a = at.apply(-1.0, 0.0);
        let b = at.apply(1.0, 0.0);
        (a.0 - b.0).abs() < 0.5
    };
    match sym {
        "resistor" | "photoresistor" | "thermistor" | "potentiometer" => {
            line(d, (-3.0, 0.0), (-1.5, 0.0));
            line(d, (1.5, 0.0), (3.0, 0.0));
            if iec {
                shape(
                    d,
                    &[(-1.5, -0.55), (1.5, -0.55), (1.5, 0.55), (-1.5, 0.55)],
                    Some(body),
                );
            } else {
                let zz = [
                    (-1.5, 0.0),
                    (-1.25, -0.6),
                    (-0.75, 0.6),
                    (-0.25, -0.6),
                    (0.25, 0.6),
                    (0.75, -0.6),
                    (1.25, 0.6),
                    (1.5, 0.0),
                ];
                poly(d, &zz);
            }
            if sym == "photoresistor" {
                for k in [-0.6, 0.2] {
                    line(d, (k - 1.0, -2.2), (k, -1.1));
                    shape(
                        d,
                        &[(k, -1.1), (k - 0.45, -1.25), (k - 0.15, -1.55)],
                        Some(ink),
                    );
                }
            }
            if sym == "thermistor" {
                poly(d, &[(-1.6, 1.0), (1.2, -1.0), (1.8, -1.0)]);
            }
            if sym == "potentiometer" {
                line(d, (0.0, 2.0), (0.0, 0.8));
                shape(d, &[(0.0, 0.7), (-0.35, 1.25), (0.35, 1.25)], Some(ink));
            }
        }
        "capacitor" | "capacitor_polarized" => {
            line(d, (-3.0, 0.0), (-0.35, 0.0));
            line(d, (0.35, 0.0), (3.0, 0.0));
            line(d, (-0.35, -1.1), (-0.35, 1.1));
            if sym == "capacitor_polarized" {
                poly(d, &[(0.55, -1.1), (0.3, -0.5), (0.3, 0.5), (0.55, 1.1)]);
                d.text(t(-1.2, -1.2), "+", 0.9, false, ink, Align2::CENTER_CENTER);
            } else {
                line(d, (0.35, -1.1), (0.35, 1.1));
            }
        }
        "inductor" => {
            line(d, (-3.0, 0.0), (-1.6, 0.0));
            line(d, (1.6, 0.0), (3.0, 0.0));
            let mut pts = Vec::new();
            for k in 0..4 {
                let cx = -1.2 + k as f64 * 0.8;
                for s in 0..=8 {
                    let a = std::f64::consts::PI * (1.0 - s as f64 / 8.0);
                    pts.push((cx + 0.4 * a.cos(), -0.4 * a.sin()));
                }
            }
            poly(d, &pts);
        }
        "diode" | "led" => {
            line(d, (-3.0, 0.0), (-0.7, 0.0));
            line(d, (0.7, 0.0), (3.0, 0.0));
            shape(d, &[(-0.7, -0.8), (-0.7, 0.8), (0.7, 0.0)], Some(body));
            line(d, (0.7, -0.8), (0.7, 0.8));
            if sym == "led" {
                for k in [-0.1, 0.6] {
                    line(d, (k, -1.0), (k + 0.9, -1.9));
                    shape(
                        d,
                        &[(k + 0.9, -1.9), (k + 0.45, -1.75), (k + 0.75, -1.45)],
                        Some(ink),
                    );
                }
            }
        }
        "switch" | "button" => {
            line(d, (-3.0, 0.0), (-1.2, 0.0));
            line(d, (1.2, 0.0), (3.0, 0.0));
            d.circle(t(-1.2, 0.0), 0.18, Some(body), Some((w, ink)));
            d.circle(t(1.2, 0.0), 0.18, Some(body), Some((w, ink)));
            if sym == "button" {
                line(d, (-1.3, -0.9), (1.3, -0.9));
                line(d, (0.0, -0.9), (0.0, -1.6));
            } else {
                line(d, (-1.1, -0.1), (1.1, -0.9));
            }
        }
        "spdt" => {
            line(d, (-3.0, 0.0), (-1.0, 0.0));
            line(d, (1.0, -1.0), (3.0, -1.0));
            line(d, (1.0, 1.0), (3.0, 1.0));
            line(d, (-1.0, 0.0), (1.0, -0.9));
        }
        "npn" | "pnp" | "nmos" | "pmos" => {
            d.circle(t(0.3, 0.0), 1.9, Some(body), Some((w, ink)));
            if sym.ends_with("mos") {
                line(d, (-3.0, 0.0), (-0.8, 0.0));
                line(d, (-0.8, -1.0), (-0.8, 1.0));
                line(d, (-0.4, -1.2), (-0.4, 1.2));
                line(d, (-0.4, -0.9), (1.0, -0.9));
                line(d, (-0.4, 0.9), (1.0, 0.9));
                line(d, (1.0, -0.9), (1.0, -3.0));
                line(d, (1.0, 0.9), (1.0, 3.0));
                shape(d, &[(-0.4, 0.9), (0.15, 0.6), (0.15, 1.2)], Some(ink));
            } else {
                line(d, (-3.0, 0.0), (-0.6, 0.0));
                line(d, (-0.6, -1.0), (-0.6, 1.0));
                line(d, (-0.6, -0.45), (1.0, -1.4));
                line(d, (-0.6, 0.45), (1.0, 1.4));
                line(d, (1.0, -1.4), (1.0, -3.0));
                line(d, (1.0, 1.4), (1.0, 3.0));
                if sym == "npn" {
                    shape(d, &[(1.0, 1.4), (0.35, 1.3), (0.7, 0.85)], Some(ink));
                } else {
                    shape(d, &[(-0.6, -0.45), (0.0, -0.55), (-0.3, -1.0)], Some(ink));
                }
            }
        }
        "opamp" => {
            shape(d, &[(-2.5, -2.4), (-2.5, 2.4), (2.6, 0.0)], Some(body));
            line(d, (-4.0, -1.0), (-2.5, -1.0));
            line(d, (-4.0, 1.0), (-2.5, 1.0));
            line(d, (2.6, 0.0), (4.0, 0.0));
            d.text(t(-1.9, -1.0), "−", 0.9, false, ink, Align2::CENTER_CENTER);
            d.text(t(-1.9, 1.0), "+", 0.9, false, ink, Align2::CENTER_CENTER);
        }
        "vsource" | "isource" | "battery" => {
            line(d, (0.0, -3.0), (0.0, -1.3));
            line(d, (0.0, 1.3), (0.0, 3.0));
            if sym == "battery" {
                line(d, (-1.0, -0.35), (1.0, -0.35));
                line(d, (-0.5, 0.35), (0.5, 0.35));
                line(d, (0.0, -1.3), (0.0, -0.35));
                line(d, (0.0, 0.35), (0.0, 1.3));
            } else {
                d.circle(t(0.0, 0.0), 1.3, Some(body), Some((w, ink)));
                if sym == "vsource" {
                    d.text(t(0.0, -0.5), "+", 0.8, false, ink, Align2::CENTER_CENTER);
                    d.text(t(0.0, 0.55), "−", 0.8, false, ink, Align2::CENTER_CENTER);
                } else {
                    line(d, (0.0, 0.7), (0.0, -0.7));
                    shape(d, &[(0.0, -0.8), (-0.35, -0.2), (0.35, -0.2)], Some(ink));
                }
            }
        }
        "buzzer" | "speaker" | "motor" => {
            line(d, (-3.0, 0.0), (-1.3, 0.0));
            line(d, (1.3, 0.0), (3.0, 0.0));
            d.circle(t(0.0, 0.0), 1.3, Some(body), Some((w, ink)));
            let letter = match sym {
                "motor" => "M",
                "speaker" => "SP",
                _ => "BZ",
            };
            d.text(t(0.0, 0.0), letter, 0.8, true, ink, Align2::CENTER_CENTER);
        }
        "ground" => {
            line(d, (0.0, -1.0), (0.0, 0.0));
            line(d, (-0.9, 0.0), (0.9, 0.0));
            line(d, (-0.55, 0.35), (0.55, 0.35));
            line(d, (-0.2, 0.7), (0.2, 0.7));
            return;
        }
        "vcc" => {
            line(d, (0.0, 1.0), (0.0, 0.0));
            line(d, (-0.8, 0.0), (0.8, 0.0));
            let net = Project::prop(def, part, "net")
                .map(|v| v.as_text())
                .unwrap_or_else(|| "VCC".into());
            d.text(
                t(0.0, -0.5),
                &flag_text(&net),
                0.9,
                false,
                ink,
                Align2::CENTER_BOTTOM,
            );
            return;
        }
        _ => {
            // box: body (wider than the declared size, so pin names fit), stubs, pin names
            let has_lr = def
                .schematic
                .pins
                .iter()
                .any(|q| matches!(q.dir, PinDir::Left | PinDir::Right));
            let has_tb = def
                .schematic
                .pins
                .iter()
                .any(|q| matches!(q.dir, PinDir::Up | PinDir::Down));
            let hw = lw / 2.0 + if has_lr { 1.2 } else { 0.0 };
            let hh = lh / 2.0 + if has_tb { 1.2 } else { 0.0 };
            let a = t(-hw, -hh);
            let b = t(hw, hh);
            let body_r = WRect::new(a.0, a.1, b.0, b.1);
            d.rect(body_r, 0.0, Some(body), Some((w, ink)));
            for sp in &def.schematic.pins {
                let (dx, dy) = dir_vec(sp.dir);
                // stub from the anchor to the body edge
                let edge = match sp.dir {
                    PinDir::Left => (-hw, sp.y),
                    PinDir::Right => (hw, sp.y),
                    PinDir::Up => (sp.x, -hh),
                    PinDir::Down => (sp.x, hh),
                };
                line(d, (sp.x, sp.y), edge);
                let name = def
                    .pin(&sp.pin)
                    .map(|p| p.label().to_string())
                    .unwrap_or(sp.pin.clone());
                let inner = (edge.0 - dx * 0.35, edge.1 - dy * 0.35);
                let dir = rotate_dir(sp.dir, at);
                let (tx, ty) = t(inner.0, inner.1);
                match dir {
                    PinDir::Left => d.text((tx, ty), &name, 0.68, false, ink, Align2::LEFT_CENTER),
                    PinDir::Right => {
                        d.text((tx, ty), &name, 0.68, false, ink, Align2::RIGHT_CENTER)
                    }
                    PinDir::Up | PinDir::Down => {
                        let len = name.chars().count() as f64 * 0.42 * 0.68 / 0.7;
                        let off = if dir == PinDir::Up {
                            len / 2.0 + 0.1
                        } else {
                            -len / 2.0 - 0.1
                        };
                        d.text_up((tx, ty + off), &name, 0.68, false, ink);
                    }
                }
            }
            // title + name read up the middle of tall boxes, across short ones
            let (cx, cy) = t(0.0, 0.0);
            let title = box_title(def);
            let (bw, bh) = (body_r.w(), body_r.h());
            let tall = bh >= bw;
            let name = if def.name.len() > 22 && !def.part_number.is_empty() {
                def.part_number.clone()
            } else {
                def.name.clone()
            };
            if tall {
                d.text_up((cx - 0.35, cy), &title, 0.82, true, ink);
                if name != title {
                    d.text_up(
                        (cx + 0.6, cy),
                        &fit(&name, bh - 1.0, 0.6),
                        0.6,
                        false,
                        rgb(GREY),
                    );
                }
            } else {
                d.text(
                    (cx, cy - 0.3),
                    &fit(&title, bw - 4.6, 0.8),
                    0.8,
                    true,
                    ink,
                    Align2::CENTER_CENTER,
                );
                if name != title && bh >= 3.0 {
                    d.text(
                        (cx, cy + 0.7),
                        &fit(&name, bw - 4.6, 0.58),
                        0.58,
                        false,
                        rgb(GREY),
                        Align2::CENTER_CENTER,
                    );
                }
            }
            // designator outside the top-right corner
            let (rx, ry) = (body_r.x1, body_r.y0);
            d.text(
                (rx, ry - 0.35),
                &part.reference,
                0.85,
                true,
                ink,
                Align2::RIGHT_BOTTOM,
            );
            return;
        }
    }
    // designator + value next to two-terminal symbols
    let (cx, cy) = t(0.0, 0.0);
    if vertical {
        let dx = if matches!(sym, "led" | "photoresistor" | "thermistor") {
            2.6
        } else {
            1.2
        };
        d.text(
            (cx + dx, cy - 0.5),
            &part.reference,
            0.85,
            true,
            ink,
            Align2::LEFT_CENTER,
        );
        d.text(
            (cx + dx, cy + 0.6),
            &value,
            0.75,
            false,
            rgb(GREY),
            Align2::LEFT_CENTER,
        );
    } else {
        let above = if matches!(sym, "npn" | "pnp" | "nmos" | "pmos" | "opamp") {
            2.6
        } else {
            1.4
        };
        d.text(
            (cx, cy - above),
            format!("{} {}", part.reference, value).trim(),
            0.8,
            false,
            ink,
            Align2::CENTER_BOTTOM,
        );
    }
    let _ = lh;
}

/// Truncates text to fit `width` grid units at `size`.
fn fit(s: &str, width: f64, size: f64) -> String {
    let max = ((width / (size * 0.6)).floor() as usize).max(3);
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", t.trim_end())
    }
}

fn box_title(def: &PartDef) -> String {
    if let Some(m) = &def.model.mcu
        && !m.label.is_empty()
    {
        return m.label.split(" @").next().unwrap_or(&m.label).to_string();
    }
    if !def.part_number.is_empty() {
        def.part_number.clone()
    } else {
        def.name.clone()
    }
}

/// `220Ω`, `100nF`, `amber`, `2N2222`.
pub fn value_text(def: &PartDef, part: &PartInstance) -> String {
    let num = |k: &str, u: &str| {
        Project::prop_f64(def, part, k).map(|v| bb_core::si::format(v, u).replace(' ', ""))
    };
    match def.schematic.symbol.as_str() {
        "resistor" | "potentiometer" if def.prop("resistance").is_some() => {
            num("resistance", "Ω").unwrap_or_default()
        }
        "capacitor" | "capacitor_polarized" => num("capacitance", "F").unwrap_or_default(),
        "inductor" => num("inductance", "H").unwrap_or_default(),
        "led" => Project::prop(def, part, "color")
            .map(|v| v.as_text())
            .unwrap_or_default(),
        "diode" | "npn" | "pnp" | "nmos" => Project::prop(def, part, "model")
            .map(|v| v.as_text())
            .unwrap_or(def.part_number.clone()),
        _ => def.part_number.clone(),
    }
}

/// Draws a power / ground flag.
fn draw_flag(d: &mut dyn SchDraw, f: &Flag) {
    let ink = rgb(INK);
    let w = 0.16;
    let (x, y) = f.at;
    if f.ground {
        // three bars, pointing away from the pin
        let (dx, dy) = dir_vec(f.dir);
        let (nx, ny) = (dy, -dx);
        for (k, half) in [(0.0, 0.9), (0.35, 0.55), (0.7, 0.2)] {
            let c = (x + dx * k, y + dy * k);
            d.line(
                (c.0 - nx * half, c.1 - ny * half),
                (c.0 + nx * half, c.1 + ny * half),
                w,
                ink,
            );
        }
    } else {
        let (dx, dy) = dir_vec(f.dir);
        let (nx, ny) = (dy, -dx);
        d.line(
            (x - nx * 0.8, y - ny * 0.8),
            (x + nx * 0.8, y + ny * 0.8),
            w,
            ink,
        );
        let tp = (x + dx * 0.4, y + dy * 0.4);
        let align = match f.dir {
            PinDir::Up => Align2::CENTER_BOTTOM,
            PinDir::Down => Align2::CENTER_TOP,
            PinDir::Left => Align2::RIGHT_CENTER,
            PinDir::Right => Align2::LEFT_CENTER,
        };
        d.text(tp, &f.text, 0.85, false, ink, align);
    }
}

fn draw_label(d: &mut dyn SchDraw, f: &Flag) {
    let ink = rgb(0x1F5FB8);
    let (x, y) = f.at;
    let align = match f.dir {
        PinDir::Up => Align2::CENTER_BOTTOM,
        PinDir::Down => Align2::CENTER_TOP,
        PinDir::Left => Align2::RIGHT_CENTER,
        PinDir::Right => Align2::LEFT_CENTER,
    };
    d.circle((x, y), 0.12, Some(ink), None);
    d.text(
        (x + dir_vec(f.dir).0 * 0.3, y + dir_vec(f.dir).1 * 0.3),
        &f.text,
        0.8,
        true,
        ink,
        align,
    );
}

/// Live values shown on the sheet: probe nets and MCU analog inputs.
pub fn net_values(
    project: &Project,
    conn: &Connectivity,
    tel: Option<&Telemetry>,
) -> Vec<(String, String)> {
    let Some(t) = tel else { return Vec::new() };
    let mut out = Vec::new();
    for pr in &project.probes {
        let Some(r) = t.probes.iter().find(|x| x.id == pr.id) else {
            continue;
        };
        let net = match &pr.target {
            bb_circuit::project::Endpoint::Pin { part, pin } => {
                conn.net_of(part, pin).map(|n| conn.net(n).name.clone())
            }
            _ => None,
        };
        let Some(net) = net else { continue };
        let name = crate::canvas::bench::net_display(&net);
        let text = match r.quantity {
            bb_circuit::project::Quantity::Duty => format!("{name} PWM {:.0}%", r.value),
            bb_circuit::project::Quantity::Logic => {
                format!("{name} {}", if r.value > 0.5 { "HIGH" } else { "LOW" })
            }
            _ => format!("{name} {:.2} V", r.value),
        };
        out.push((net, text));
    }
    out
}

/// Draws the whole sheet on `d` (paper, symbols, wires, flags, values, ERC, title block).
#[allow(clippy::too_many_arguments)]
pub fn draw_sheet(
    d: &mut dyn SchDraw,
    project: &Project,
    lib: &Library,
    sheet: &Sheet,
    values: &[(String, String)],
    erc: Option<&str>,
    selected: &HashSet<String>,
    hover_net: Option<&str>,
    paper: WRect,
) {
    let ink = rgb(INK);
    d.rect(paper, 0.0, Some(rgb(PAPER)), None);
    // wires
    for s in &sheet.segments {
        let hot = hover_net.is_some() && s.net.as_deref() == hover_net;
        let c = if hot { rgb(0x1F7FB8) } else { ink };
        d.polyline(&s.pts, if hot { 0.24 } else { 0.16 }, c);
    }
    for j in &sheet.junctions {
        d.circle(*j, 0.32, Some(ink), None);
    }
    for f in &sheet.flags {
        draw_flag(d, f);
    }
    for f in &sheet.labels {
        draw_label(d, f);
    }
    for l in &project.schematic.labels {
        d.circle((l.x, l.y), 0.12, Some(rgb(0x1F5FB8)), None);
        d.text(
            (l.x + 0.3, l.y - 0.2),
            &l.name,
            0.8,
            true,
            rgb(0x1F5FB8),
            Align2::LEFT_BOTTOM,
        );
    }
    // symbols
    for p in &project.parts {
        if let Some(def) = lib.get(&p.part)
            && def.schematic.symbol != "none"
            && p.schematic.is_some()
        {
            draw_symbol(d, def, p, project.schematic.iec, selected.contains(&p.id));
        }
    }
    // live values: a chip at the first anchor of each probed net
    let mut done = HashSet::new();
    for (net, text) in values {
        if !done.insert(net.clone()) {
            continue;
        }
        if let Some(a) = sheet.anchors.iter().find(|a| a.5.as_deref() == Some(net)) {
            let (dx, dy) = dir_vec(a.4);
            let at = (a.2 + dx * 1.2, a.3 + dy * 1.2 - 1.6);
            let w = text.chars().count() as f64 * 0.62 + 1.0;
            let x0 = if dx < 0.0 { at.0 - w } else { at.0 };
            d.rect(
                WRect::new(x0, at.1 - 0.75, x0 + w, at.1 + 0.75),
                0.25,
                Some(rgb(0xDCE8FA)),
                None,
            );
            d.text(
                (x0 + w / 2.0, at.1),
                text,
                0.82,
                true,
                rgb(0x1F5FB8),
                Align2::CENTER_CENTER,
            );
        }
    }
    // ERC + title block, bottom right of the paper
    let (bx, by) = (paper.x1 - 25.0, paper.y1 - 5.0);
    if let Some(e) = erc {
        let y = by - 2.0;
        d.polygon(
            &[
                (bx - 22.0, y + 0.55),
                (bx - 21.3, y - 0.65),
                (bx - 20.6, y + 0.55),
            ],
            Some(rgb(0xF2A33A)),
            None,
        );
        d.text(
            (bx - 20.1, y),
            &format!("ERC: {e}"),
            0.75,
            false,
            rgb(0x8A5A12),
            Align2::LEFT_CENTER,
        );
    }
    let tb = WRect::new(bx, by, bx + 24.0, by + 4.0);
    d.rect(tb, 0.0, Some(rgb(PAPER)), Some((0.12, ink)));
    d.line((bx + 15.0, by), (bx + 15.0, by + 4.0), 0.08, ink);
    d.text(
        (bx + 1.0, by + 1.3),
        &format!("{}.sch", project.name),
        0.8,
        true,
        ink,
        Align2::LEFT_CENTER,
    );
    d.text(
        (bx + 1.0, by + 2.9),
        "Breadbox · sheet 1/1",
        0.75,
        false,
        rgb(GREY),
        Align2::LEFT_CENTER,
    );
    d.text(
        (bx + 16.0, by + 1.3),
        "Rev A",
        0.8,
        false,
        ink,
        Align2::LEFT_CENTER,
    );
    d.text(
        (bx + 16.0, by + 2.9),
        &today(),
        0.75,
        false,
        rgb(GREY),
        Align2::LEFT_CENTER,
    );
}

fn today() -> String {
    // civil date from the system clock (no chrono dependency)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// The paper rectangle around a sheet's content.
pub fn paper_for(sheet: &Sheet) -> WRect {
    let e = sheet.extent;
    WRect::new(
        e.x0 - 6.0,
        e.y0 - 6.0,
        e.x1.max(e.x0 + 60.0) + 8.0,
        e.y1.max(e.y0 + 30.0) + 12.0,
    )
}

/// The whole sheet as an SVG document (Share → schematic SVG).
pub fn render_svg(
    project: &Project,
    lib: &Library,
    conn: &Connectivity,
    values: &[(String, String)],
    erc: Option<&str>,
) -> String {
    let sheet = Sheet::build(project, lib, conn);
    let paper = paper_for(&sheet);
    let mut s = SvgSch { out: String::new() };
    let _ = writeln!(
        s.out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{:.0} {:.0} {:.0} {:.0}" width="{:.0}" height="{:.0}">"#,
        paper.x0 * SVG_K,
        paper.y0 * SVG_K,
        paper.w() * SVG_K,
        paper.h() * SVG_K,
        paper.w() * SVG_K,
        paper.h() * SVG_K
    );
    draw_sheet(
        &mut s,
        project,
        lib,
        &sheet,
        values,
        erc,
        &HashSet::new(),
        None,
        paper,
    );
    s.out.push_str("</svg>\n");
    s.out
}

// ------------------------------------------------------------------------------------------
// the interactive canvas

#[derive(Debug, Clone, PartialEq)]
enum SDrag {
    None,
    Pan {
        origin: Pos2,
        start: Pos2,
    },
    Move {
        id: String,
        start: (f64, f64),
        delta: (f64, f64),
    },
    Wire {
        from: (f64, f64),
        dir: PinDir,
        to: (f64, f64),
    },
}

pub struct SchematicCanvas {
    pub view: View,
    fitted: Option<u64>,
    pub selected: HashSet<String>,
    pub selected_wire: Option<String>,
    drag: SDrag,
    sheet: Option<(u64, std::sync::Arc<Sheet>)>,
    refit: bool,
    pub hover_net: Option<String>,
    /// Armed net-label tool: the next click on a pin or schematic wire drops this name.
    pub label_tool: Option<String>,
    pub selected_label: Option<String>,
}

impl Default for SchematicCanvas {
    fn default() -> Self {
        SchematicCanvas {
            view: View {
                origin: pos2(0.0, 0.0),
                scale: SCH_SCALE,
            },
            fitted: None,
            selected: HashSet::new(),
            selected_wire: None,
            drag: SDrag::None,
            sheet: None,
            refit: false,
            hover_net: None,
            label_tool: None,
            selected_label: None,
        }
    }
}

impl SchematicCanvas {
    pub fn zoom_pct(&self) -> f32 {
        self.view.scale / SCH_SCALE * 100.0
    }
    pub fn zoom(&mut self, f: f32) {
        let c = self.view.to_screen(0.0, 0.0);
        let _ = c;
        let new = (self.view.scale * f).clamp(SCH_SCALE * 0.25, SCH_SCALE * 4.0);
        self.view.scale = new;
    }
    pub fn fit(&mut self) {
        self.refit = true;
    }

    fn sheet(&mut self, doc: &mut CircuitDoc) -> std::sync::Arc<Sheet> {
        let g = doc.generation();
        if let Some((sg, s)) = &self.sheet
            && *sg == g
        {
            return s.clone();
        }
        let conn = doc.connectivity();
        let s = std::sync::Arc::new(Sheet::build(doc.project(), &doc.lib, &conn));
        self.sheet = Some((g, s.clone()));
        s
    }

    /// Draws and edits the schematic in `rect`; returns a part id that was clicked (to
    /// select it on the bench too).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        doc: &mut CircuitDoc,
        tel: Option<&Telemetry>,
        project_generation: u64,
        _theme: crate::theme::ThemeChoice,
    ) -> Option<String> {
        let sheet = self.sheet(doc);
        let paper = paper_for(&sheet);
        if self.fitted != Some(project_generation) || self.refit {
            let mut v = View::fit(
                rect,
                (paper.x0, paper.y0, paper.x1, paper.y1),
                SCH_SCALE * 1.5,
            );
            v.scale = v.scale.max(SCH_SCALE * 0.25);
            self.view = v;
            self.fitted = Some(project_generation);
            self.refit = false;
        }
        let resp = ui.interact(
            rect,
            Id::unique("schematic-canvas"),
            Sense::click_and_drag(),
        );
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Schematic canvas")
        });
        ui.ctx().data_mut(|d| {
            d.insert_temp(Id::unique("schematic-view"), self.view);
            d.insert_temp(Id::unique("schematic-sheet"), sheet.clone());
        });
        let mut clicked_part = None;
        // view input
        if resp.contains_pointer() {
            let (scroll, zoom, cmd, hover) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers.command,
                    i.pointer.hover_pos(),
                )
            });
            let at = hover.unwrap_or(rect.center());
            if zoom != 1.0 {
                zoom_about(&mut self.view, at, zoom);
            } else if cmd && scroll.y != 0.0 {
                zoom_about(&mut self.view, at, (scroll.y * 0.004).exp());
            } else {
                self.view.origin += scroll;
            }
        }
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let world = pointer.map(|p| self.view.to_world(p));
        // hover net
        self.hover_net = world.and_then(|(x, y)| {
            sheet
                .anchors
                .iter()
                .find(|a| (a.2 - x).abs() < 0.6 && (a.3 - y).abs() < 0.6)
                .and_then(|a| a.5.clone())
                .or_else(|| {
                    sheet
                        .segments
                        .iter()
                        .find(|s| {
                            s.pts.windows(2).any(|w| {
                                bb_circuit::geometry::point_segment_distance((x, y), w[0], w[1])
                                    < 0.4
                            })
                        })
                        .and_then(|s| s.net.clone())
                })
        });
        let space = ui.input(|i| i.key_down(egui::Key::Space) || i.pointer.middle_down());
        let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
        if resp.drag_started()
            && let Some(p) = origin
        {
            let (x, y) = self.view.to_world(p);
            self.drag = if space {
                SDrag::Pan {
                    origin: self.view.origin,
                    start: p,
                }
            } else if let Some(a) = sheet
                .anchors
                .iter()
                .find(|a| (a.2 - x).abs() < 0.6 && (a.3 - y).abs() < 0.6)
            {
                SDrag::Wire {
                    from: (a.2, a.3),
                    dir: a.4,
                    to: (x, y),
                }
            } else if let Some(id) = self.part_at(doc, (x, y)) {
                self.selected.clear();
                self.selected.insert(id.clone());
                SDrag::Move {
                    id,
                    start: (x, y),
                    delta: (0.0, 0.0),
                }
            } else {
                SDrag::Pan {
                    origin: self.view.origin,
                    start: p,
                }
            };
        }
        if resp.dragged()
            && let (Some(p), Some((x, y))) = (pointer, world)
        {
            match &mut self.drag {
                SDrag::Pan { origin, start } => self.view.origin = *origin + (p - *start),
                SDrag::Move { start, delta, .. } => {
                    *delta = (
                        ((x - start.0) * 2.0).round() / 2.0,
                        ((y - start.1) * 2.0).round() / 2.0,
                    )
                }
                SDrag::Wire { to, .. } => *to = (x, y),
                SDrag::None => {}
            }
        }
        if resp.drag_stopped() {
            match std::mem::replace(&mut self.drag, SDrag::None) {
                SDrag::Move { id, delta, .. } if delta != (0.0, 0.0) => {
                    if let Some(at) = doc.project().part(&id).and_then(|p| p.schematic) {
                        let mut na = at;
                        na.x += delta.0;
                        na.y += delta.1;
                        let _ = doc.editor.place_in_schematic(&id, na);
                        doc.touched();
                    }
                }
                SDrag::Wire { from, dir, to } => {
                    let target = sheet
                        .anchors
                        .iter()
                        .find(|a| (a.2 - to.0).abs() < 0.6 && (a.3 - to.1).abs() < 0.6)
                        .map(|a| ((a.2, a.3), a.4))
                        .or_else(|| {
                            // end on a wire vertex / segment: snap to the 0.5 grid
                            let q = ((to.0 * 2.0).round() / 2.0, (to.1 * 2.0).round() / 2.0);
                            sheet
                                .segments
                                .iter()
                                .any(|s| s.pts.windows(2).any(|w| on_segment(q, w[0], w[1])))
                                .then_some((q, PinDir::Left))
                        });
                    if let Some((b, db)) = target
                        && !close(b, from)
                    {
                        let pts = route(from, dir, b, db);
                        doc.editor.add_schematic_wire(pts);
                        doc.touched();
                    }
                }
                _ => {}
            }
        }
        if resp.clicked()
            && let Some((x, y)) = world
            && let Some(name) = self.label_tool.clone()
        {
            // the armed label tool: drop the label where it connects, then disarm
            if let Some((lx, ly)) = label_snap(&sheet, (x, y)) {
                let id = doc.editor.add_label(&name, lx, ly);
                doc.touched();
                self.label_tool = None;
                self.selected.clear();
                self.selected_wire = None;
                self.selected_label = Some(id);
            }
        } else if resp.clicked()
            && let Some((x, y)) = world
        {
            self.selected.clear();
            self.selected_wire = None;
            self.selected_label = None;
            if let Some(id) = self.label_at(doc, (x, y)) {
                self.selected_label = Some(id);
            } else if let Some(id) = self.part_at(doc, (x, y)) {
                self.selected.insert(id.clone());
                clicked_part = Some(id);
            } else if let Some(s) = sheet.segments.iter().find(|s| {
                s.explicit
                    && s.pts.windows(2).any(|w| {
                        bb_circuit::geometry::point_segment_distance((x, y), w[0], w[1]) < 0.4
                    })
            }) {
                self.selected_wire = s.id.clone();
            }
        }
        // keys
        if resp.contains_pointer() && !ui.ctx().egui_wants_keyboard_input() {
            let (r, del, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::R),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if esc {
                self.label_tool = None;
                self.selected_label = None;
            }
            if del
                && let Some(l) = self.selected_label.take()
                && let Some(before) = doc
                    .project()
                    .schematic
                    .labels
                    .iter()
                    .find(|x| x.id == l)
                    .cloned()
            {
                let mut tx = bb_circuit::edit::Transaction::new("Delete net label");
                tx.ops.push(bb_circuit::edit::Op::Label {
                    before: Some(before),
                    after: None,
                    index: None,
                });
                doc.editor.commit(tx);
                doc.touched();
            }
            if r {
                for id in self.selected.clone() {
                    if let Some(at) = doc.project().part(&id).and_then(|p| p.schematic) {
                        let _ = doc.editor.place_in_schematic(&id, at.rotated(90.0));
                        doc.touched();
                    }
                }
            }
            if del
                && let Some(w) = self.selected_wire.take()
                && let Some(before) = doc
                    .project()
                    .schematic
                    .wires
                    .iter()
                    .find(|x| x.id == w)
                    .cloned()
            {
                let mut tx = bb_circuit::edit::Transaction::new("Delete schematic wire");
                tx.ops.push(bb_circuit::edit::Op::SchWire {
                    before: Some(before),
                    after: None,
                    index: None,
                });
                doc.editor.commit(tx);
                doc.touched();
            }
        }
        // draw
        let sheet = self.sheet(doc);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, rgb(0x2A2A28));
        let conn = doc.connectivity();
        let values = net_values(doc.project(), &conn, tel);
        let checks = doc.checks();
        let erc = checks.erc.first().map(|e| e.message.clone());
        // moving preview: draw a shifted project
        let project = match &self.drag {
            SDrag::Move { id, delta, .. } if *delta != (0.0, 0.0) => {
                let mut p = doc.project().clone();
                if let Some(pp) = p.part_mut(id)
                    && let Some(s) = pp.schematic.as_mut()
                {
                    s.x += delta.0;
                    s.y += delta.1;
                }
                std::borrow::Cow::Owned(p)
            }
            _ => std::borrow::Cow::Borrowed(doc.project()),
        };
        let moved_sheet;
        let sheet_ref: &Sheet = if matches!(project, std::borrow::Cow::Owned(_)) {
            moved_sheet = Sheet::build(
                &project,
                &doc.lib,
                &Connectivity::compute(&project, &doc.lib),
            );
            &moved_sheet
        } else {
            &sheet
        };
        // paper shadow + dot grid
        let pr = Rect::from_two_pos(
            self.view.to_screen(paper.x0, paper.y0),
            self.view.to_screen(paper.x1, paper.y1),
        );
        painter.add(
            egui::epaint::RectShape::filled(
                pr.translate(vec2(0.0, 10.0)),
                6.0,
                Color32::from_black_alpha(130),
            )
            .with_blur_width(30.0),
        );
        let mut d = EguiSch {
            painter: &painter,
            view: self.view,
        };
        draw_sheet(
            &mut d,
            &project,
            &doc.lib,
            sheet_ref,
            &values,
            erc.as_deref(),
            &self.selected,
            self.hover_net.as_deref(),
            paper,
        );
        dot_grid(&painter, pr, self.view);
        // wire preview
        if let SDrag::Wire { from, dir, to } = &self.drag {
            let pts = route(*from, *dir, *to, PinDir::Left);
            let v: Vec<Pos2> = pts.iter().map(|q| self.view.to_screen(q.0, q.1)).collect();
            painter.add(Shape::line(v, Stroke::new(2.0, rgb(0xC77A12))));
        }
        if let Some(w) = &self.selected_wire
            && let Some(sw) = doc.project().schematic.wires.iter().find(|x| &x.id == w)
        {
            let v: Vec<Pos2> = sw
                .points
                .iter()
                .map(|q| self.view.to_screen(q.0, q.1))
                .collect();
            painter.add(Shape::line(
                v,
                Stroke::new(
                    4.0,
                    egui::Color32::from_rgba_unmultiplied(199, 122, 18, 120),
                ),
            ));
        }
        let accent = egui::Color32::from_rgba_unmultiplied(199, 122, 18, 140);
        if let Some(l) = &self.selected_label
            && let Some(nl) = doc.project().schematic.labels.iter().find(|x| &x.id == l)
        {
            let b = label_box(nl);
            painter.rect_stroke(
                Rect::from_two_pos(
                    self.view.to_screen(b.x0, b.y0),
                    self.view.to_screen(b.x1, b.y1),
                ),
                3.0,
                Stroke::new(2.0, accent),
                egui::StrokeKind::Outside,
            );
        }
        // armed label tool: a ghost where it would land
        if let Some(name) = &self.label_tool
            && let Some(q) = world
            && resp.hovered()
        {
            let font = crate::theme::font::mono_medium(self.view.len(0.8).max(9.0));
            match label_snap(&sheet, q) {
                Some((lx, ly)) => {
                    let at = self.view.to_screen(lx, ly);
                    painter.circle_filled(at, 4.0, accent);
                    painter.text(
                        at + vec2(self.view.len(0.3), -self.view.len(0.2)),
                        Align2::LEFT_BOTTOM,
                        name,
                        font,
                        rgb(0xC77A12),
                    );
                }
                None => {
                    let at = self.view.to_screen(q.0, q.1);
                    painter.text(
                        at + vec2(12.0, -6.0),
                        Align2::LEFT_BOTTOM,
                        format!("{name} — click a pin or wire"),
                        crate::theme::font::mono(11.0),
                        egui::Color32::from_rgba_unmultiplied(199, 122, 18, 170),
                    );
                }
            }
        }
        // hover tooltip with the live net value
        if let Some(net) = &self.hover_net
            && resp.hovered()
        {
            let disp = crate::canvas::bench::net_display(net);
            let v = tel.and_then(|t| t.nets.iter().find(|(n, _)| n == &disp).map(|(_, v)| *v));
            let text = match v {
                Some(v) => format!("net {disp} · {}", bb_core::si::format(v, "V")),
                None => format!("net {disp}"),
            };
            resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(egui::RichText::new(text).font(crate::theme::font::mono(11.5)));
            });
        }
        clicked_part
    }

    /// A free net-label name (`NET1`, `NET2`, …) not used by a label or net yet.
    pub fn suggest_label(doc: &mut CircuitDoc) -> String {
        let conn = doc.connectivity();
        let taken = |n: &str| {
            doc.project()
                .schematic
                .labels
                .iter()
                .any(|l| l.name.eq_ignore_ascii_case(n))
                || conn.nets.iter().any(|x| x.name.eq_ignore_ascii_case(n))
        };
        (1..)
            .map(|i| format!("NET{i}"))
            .find(|n| !taken(n))
            .unwrap_or_default()
    }

    fn label_at(&self, doc: &CircuitDoc, q: (f64, f64)) -> Option<String> {
        doc.project()
            .schematic
            .labels
            .iter()
            .rev()
            .find(|l| label_box(l).contains(q.0, q.1))
            .map(|l| l.id.clone())
    }

    fn part_at(&self, doc: &CircuitDoc, q: (f64, f64)) -> Option<String> {
        for p in doc.project().parts.iter().rev() {
            let (Some(def), Some(at)) = (doc.lib.get(&p.part), p.schematic.as_ref()) else {
                continue;
            };
            if def.schematic.symbol == "none" {
                continue;
            }
            let (w, h) = def.schematic.size;
            let (lx, ly) = at.inverse(q.0, q.1);
            if lx.abs() <= w / 2.0 + 0.6 && ly.abs() <= h / 2.0 + 0.8 {
                return Some(p.id.clone());
            }
        }
        None
    }
}

/// World box of a placed net label (its name sits up-right of the connection point).
fn label_box(l: &bb_circuit::project::NetLabel) -> WRect {
    let w = 0.5 * l.name.chars().count() as f64;
    WRect::new(l.x - 0.35, l.y - 1.2, l.x + 0.45 + w, l.y + 0.35)
}

/// Where a net label dropped at `q` attaches: a pin anchor, or a point on a schematic wire
/// (the two places connectivity reads labels from).
pub fn label_snap(sheet: &Sheet, q: (f64, f64)) -> Option<(f64, f64)> {
    if let Some(a) = sheet
        .anchors
        .iter()
        .filter(|a| (a.2 - q.0).abs() < 0.7 && (a.3 - q.1).abs() < 0.7)
        .min_by(|a, b| {
            let da = (a.2 - q.0).hypot(a.3 - q.1);
            let db = (b.2 - q.0).hypot(b.3 - q.1);
            da.total_cmp(&db)
        })
    {
        return Some((a.2, a.3));
    }
    for s in sheet.segments.iter().filter(|s| s.explicit) {
        for w in s.pts.windows(2) {
            if bb_circuit::geometry::point_segment_distance(q, w[0], w[1]) < 0.5 {
                // project onto the (orthogonal) segment, on the 0.5 grid when it stays on it
                let (a, b) = (w[0], w[1]);
                let p = if (a.1 - b.1).abs() < 1e-6 {
                    (q.0.clamp(a.0.min(b.0), a.0.max(b.0)), a.1)
                } else {
                    (a.0, q.1.clamp(a.1.min(b.1), a.1.max(b.1)))
                };
                let g = ((p.0 * 2.0).round() / 2.0, (p.1 * 2.0).round() / 2.0);
                return Some(if on_segment(g, a, b) { g } else { p });
            }
        }
    }
    None
}

fn zoom_about(v: &mut View, at: Pos2, factor: f32) {
    let new = (v.scale * factor).clamp(SCH_SCALE * 0.25, SCH_SCALE * 4.0);
    let k = new / v.scale;
    v.origin = at + (v.origin - at) * k;
    v.scale = new;
}

fn dot_grid(painter: &Painter, r: Rect, view: View) {
    let step = view.scale * 2.0;
    if step < 8.0 {
        return;
    }
    let c = rgb(DOT);
    let (x0, y0) = view.to_world(r.min);
    let sx = (x0 / 2.0).ceil() * 2.0;
    let sy = (y0 / 2.0).ceil() * 2.0;
    let mut mesh = egui::Mesh::default();
    let mut y = sy;
    loop {
        let py = view.to_screen(0.0, y).y;
        if py > r.bottom() {
            break;
        }
        let mut x = sx;
        loop {
            let px = view.to_screen(x, 0.0).x;
            if px > r.right() {
                break;
            }
            let q = Rect::from_center_size(pos2(px, py), vec2(1.4, 1.4));
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(q.left_top(), c);
            mesh.colored_vertex(q.right_top(), c);
            mesh.colored_vertex(q.right_bottom(), c);
            mesh.colored_vertex(q.left_bottom(), c);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
            x += 2.0;
        }
        y += 2.0;
    }
    painter.add(Shape::mesh(mesh));
}

#[allow(dead_code)]
fn unused(_: BTreeMap<u8, u8>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_orthogonal() {
        let pts = route((0.0, 0.0), PinDir::Right, (10.0, 5.0), PinDir::Left);
        for w in pts.windows(2) {
            assert!(w[0].0 == w[1].0 || w[0].1 == w[1].1, "{pts:?}");
        }
        assert_eq!(pts.first(), Some(&(0.0, 0.0)));
        assert_eq!(pts.last(), Some(&(10.0, 5.0)));
    }

    #[test]
    fn garden_node_sheet_mirrors_bench_wiring_with_power_flags() {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/garden-node");
        let doc = CircuitDoc::load(&dir, true).unwrap();
        let conn = Connectivity::compute(doc.project(), &doc.lib);
        let sheet = Sheet::build(doc.project(), &doc.lib, &conn);
        assert!(sheet.flags.iter().any(|f| f.ground), "ground symbols");
        assert!(
            sheet
                .flags
                .iter()
                .any(|f| !f.ground && f.text.contains('5')),
            "+5V flags: {:?}",
            sheet.flags
        );
        // A0 (U1) is joined to the soil sensor output in the schematic
        assert!(
            sheet
                .segments
                .iter()
                .any(|s| s.net.as_deref() == Some("A0") && !s.explicit)
        );
        let svg = render_svg(
            doc.project(),
            &doc.lib,
            &conn,
            &[("A0".into(), "A0 1.88 V".into())],
            Some("U2.RX is 3.3 V tolerant"),
        );
        assert!(svg.starts_with("<svg") && svg.contains("ATmega328P") && svg.contains("A0 1.88 V"));
        let png = crate::export::svg_to_png(&svg, 1.0).unwrap();
        assert!(png.len() > 1000);
    }
}
