//! PCB (basic, `PRO-7`): footprints for every part with a PCB footprint, a rectangular board
//! outline, placement (`PartInstance::pcb`, mm, rotations in 90° steps, `flip` = bottom side
//! for SMD-less parts is not used), the ratsnest from the circuit's connectivity, manual
//! two-layer routing (traces with widths on the top / bottom copper, vias), the design-rule
//! check (clearance, trace width, drill, board edge, shorts, unrouted nets) and Gerber
//! RS-274X + Excellon export ([`gerber`]).
//!
//! **Data model** (persisted in `project.json`): `PartInstance::pcb` holds each footprint's
//! placement in millimetres; `Project::pcb` (`PcbLayout`) holds the board `outline`
//! `(x0, y0, x1, y1)` mm and the routed copper as JSON values — [`Trace`] `{ id, layer, width,
//! points }` and [`Via`] `{ id, x, y, drill, diameter }`. [`Board::load`] / [`Board::store`]
//! convert. Coordinates are mm with y pointing down (Gerber export flips them).
//!
//! **Copper connectivity**: pads, trace segments and vias are capsules (segment + radius);
//! two items on a shared layer that touch are connected (union–find). A net is routed when all
//! of its pads end up in one copper group (pins bonded inside a part, like an UNO's GND pins,
//! count as joined).

pub mod drc;
pub mod footprint;
pub mod gerber;

use crate::connectivity::Connectivity;
use crate::geometry::Placement;
use crate::library::Library;
use crate::part::{Category, PartDef};
use crate::project::Project;
use footprint::{Footprint, Pad, PadShape};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// A copper layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    #[default]
    Top,
    Bottom,
}

impl Layer {
    pub fn index(self) -> usize {
        match self {
            Layer::Top => 0,
            Layer::Bottom => 1,
        }
    }
    pub fn other(self) -> Layer {
        match self {
            Layer::Top => Layer::Bottom,
            Layer::Bottom => Layer::Top,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Layer::Top => "Top",
            Layer::Bottom => "Bottom",
        }
    }
}

/// A routed track (polyline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub id: String,
    pub layer: Layer,
    /// mm.
    pub width: f64,
    pub points: Vec<(f64, f64)>,
}

/// A plated via between the two layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Via {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub drill: f64,
    pub diameter: f64,
}

/// Design rules (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    pub clearance: f64,
    pub min_width: f64,
    pub min_drill: f64,
    pub edge_clearance: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            clearance: 0.2,
            min_width: 0.2,
            min_drill: 0.3,
            edge_clearance: 0.3,
        }
    }
}

/// The board: outline + routed copper.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub outline: Option<(f64, f64, f64, f64)>,
    pub traces: Vec<Trace>,
    pub vias: Vec<Via>,
    pub rules: Rules,
}

impl Board {
    /// The board stored in a project.
    pub fn load(p: &Project) -> Board {
        Board {
            outline: p.pcb.outline,
            traces: p
                .pcb
                .traces
                .iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect(),
            vias: p
                .pcb
                .vias
                .iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect(),
            rules: Rules::default(),
        }
    }

    /// Writes the board into the project.
    pub fn store(&self, p: &mut Project) {
        p.pcb.outline = self.outline;
        p.pcb.traces = self
            .traces
            .iter()
            .filter_map(|t| serde_json::to_value(t).ok())
            .collect();
        p.pcb.vias = self
            .vias
            .iter()
            .filter_map(|v| serde_json::to_value(v).ok())
            .collect();
    }

    /// A fresh id (`t7`, `v3`).
    pub fn next_id(&self, prefix: &str) -> String {
        let mut n = 1;
        loop {
            let id = format!("{prefix}{n}");
            if !self.traces.iter().any(|t| t.id == id) && !self.vias.iter().any(|v| v.id == id) {
                return id;
            }
            n += 1;
        }
    }
}

/// Parts that go on the PCB: everything with a footprint except breadboards, instruments,
/// world objects and hidden parts.
pub fn on_pcb(def: &PartDef) -> bool {
    !def.pcb.footprint.is_empty()
        && !def.is_breadboard()
        && def.category != Category::Instruments
        && !def.bench.hidden
}

/// A footprint on the board.
#[derive(Debug, Clone)]
pub struct Placed {
    pub part: String,
    pub reference: String,
    pub def: Arc<PartDef>,
    pub footprint: Footprint,
    pub at: Placement,
}

/// A pad in board coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldPad {
    pub part: String,
    pub reference: String,
    pub pin: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub shape: PadShape,
    pub drill: Option<f64>,
}

impl WorldPad {
    pub fn layers(&self) -> [bool; 2] {
        if self.drill.is_some() {
            [true, true]
        } else {
            [true, false]
        }
    }
    pub fn capsule(&self) -> Capsule {
        capsule_of_rect(self.x, self.y, self.w, self.h, self.shape)
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        (x - self.x).abs() <= self.w / 2.0 && (y - self.y).abs() <= self.h / 2.0
    }
}

/// Rotates a footprint-local point by the placement (90° steps exact).
pub fn place_point(at: &Placement, x: f64, y: f64) -> (f64, f64) {
    at.apply(x, y)
}

impl Placed {
    pub fn pads(&self) -> Vec<WorldPad> {
        let quarter = ((self.at.rot / 90.0).round() as i64).rem_euclid(2) == 1;
        self.footprint
            .pads
            .iter()
            .map(|p: &Pad| {
                let (x, y) = place_point(&self.at, p.x, p.y);
                let (w, h) = if quarter { (p.h, p.w) } else { (p.w, p.h) };
                WorldPad {
                    part: self.part.clone(),
                    reference: self.reference.clone(),
                    pin: p.pin.clone(),
                    x,
                    y,
                    w,
                    h,
                    shape: p.shape,
                    drill: p.drill,
                }
            })
            .collect()
    }

    /// Silkscreen lines in board coordinates.
    pub fn silk(&self) -> Vec<((f64, f64), (f64, f64))> {
        self.footprint
            .silk
            .iter()
            .map(|(a, b)| {
                (
                    place_point(&self.at, a.0, a.1),
                    place_point(&self.at, b.0, b.1),
                )
            })
            .collect()
    }

    /// Courtyard in board coordinates.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let (x0, y0, x1, y1) = self.footprint.bounds;
        let a = place_point(&self.at, x0, y0);
        let b = place_point(&self.at, x1, y1);
        (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
    }
}

/// Footprints of the project's PCB parts (placed or not: unplaced ones sit at the origin).
pub fn placed_parts(p: &Project, lib: &Library, smd: bool) -> Vec<Placed> {
    p.parts
        .iter()
        .filter_map(|inst| {
            let def = lib.get(&inst.part)?;
            if !on_pcb(def) {
                return None;
            }
            Some(Placed {
                part: inst.id.clone(),
                reference: inst.reference.clone(),
                footprint: footprint::footprint(def, smd),
                def: def.clone(),
                at: inst.pcb.unwrap_or_default(),
            })
        })
        .collect()
}

/// Places every PCB part without a placement (`all` = re-place everything) in rows, biggest
/// first, and sizes the board outline around them (3 mm margin). Returns how many moved.
pub fn auto_place(p: &mut Project, lib: &Library, all: bool) -> usize {
    let mut items: Vec<(String, (f64, f64, f64, f64))> = placed_parts(p, lib, false)
        .into_iter()
        .filter(|pl| all || p.part(&pl.part).is_some_and(|i| i.pcb.is_none()))
        .map(|pl| (pl.part.clone(), pl.footprint.bounds))
        .collect();
    if items.is_empty() {
        return 0;
    }
    // keep already placed parts where they are: start below them
    let fixed: Vec<(f64, f64, f64, f64)> = placed_parts(p, lib, false)
        .into_iter()
        .filter(|pl| !items.iter().any(|(id, _)| id == &pl.part))
        .map(|pl| pl.bounds())
        .collect();
    let gap = 3.0;
    let origin = (5.0, 5.0);
    let mut y = fixed.iter().map(|b| b.3 + gap).fold(origin.1, f64::max);
    items.sort_by(|a, b| {
        let area = |r: &(f64, f64, f64, f64)| (r.2 - r.0) * (r.3 - r.1);
        area(&b.1).total_cmp(&area(&a.1))
    });
    let max_w = items.iter().map(|(_, b)| b.2 - b.0).fold(60.0f64, f64::max);
    let mut x = origin.0;
    let mut row_h = 0.0f64;
    let n = items.len();
    for (id, b) in items {
        let (w, h) = (b.2 - b.0, b.3 - b.1);
        if x > origin.0 && x + w > origin.0 + max_w + 40.0 {
            x = origin.0;
            y += row_h + gap;
            row_h = 0.0;
        }
        if let Some(inst) = p.part_mut(&id) {
            // the courtyard's top-left lands at (x, y)
            inst.pcb = Some(Placement::at(
                (x - b.0).round_to(0.127),
                (y - b.1).round_to(0.127),
            ));
        }
        x += w + gap;
        row_h = row_h.max(h);
    }
    fit_outline(p, lib);
    n
}

trait RoundTo {
    fn round_to(self, step: f64) -> f64;
}
impl RoundTo for f64 {
    fn round_to(self, step: f64) -> f64 {
        (self / step).round() * step
    }
}

/// Sizes the outline around every footprint, trace and via (3 mm margin).
pub fn fit_outline(p: &mut Project, lib: &Library) {
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for pl in placed_parts(p, lib, false) {
        let r = pl.bounds();
        b = (b.0.min(r.0), b.1.min(r.1), b.2.max(r.2), b.3.max(r.3));
    }
    let board = Board::load(p);
    for t in &board.traces {
        for q in &t.points {
            b = (b.0.min(q.0), b.1.min(q.1), b.2.max(q.0), b.3.max(q.1));
        }
    }
    if b.0.is_finite() {
        let m = 3.0;
        p.pcb.outline = Some((
            (b.0 - m).floor(),
            (b.1 - m).floor(),
            (b.2 + m).ceil(),
            (b.3 + m).ceil(),
        ));
    }
}

// ------------------------------------------------------------------ geometry

/// A segment with a radius (round pads: a = b).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub r: f64,
}

fn capsule_of_rect(x: f64, y: f64, w: f64, h: f64, shape: PadShape) -> Capsule {
    let _ = shape;
    if (w - h).abs() < 1e-9 {
        return Capsule {
            a: (x, y),
            b: (x, y),
            r: w / 2.0,
        };
    }
    if w > h {
        let d = (w - h) / 2.0;
        Capsule {
            a: (x - d, y),
            b: (x + d, y),
            r: h / 2.0,
        }
    } else {
        let d = (h - w) / 2.0;
        Capsule {
            a: (x, y - d),
            b: (x, y + d),
            r: w / 2.0,
        }
    }
}

fn dot(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1
}
fn sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}

/// Distance from a point to a segment.
pub fn point_seg(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 <= 1e-18 {
        0.0
    } else {
        (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0)
    };
    let q = (a.0 + ab.0 * t, a.1 + ab.1 * t);
    let d = sub(p, q);
    dot(d, d).sqrt()
}

fn cross(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

/// Distance between two segments (0 when they cross).
pub fn seg_seg(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> f64 {
    let r = sub(b, a);
    let s = sub(d, c);
    let den = cross(r, s);
    if den.abs() > 1e-12 {
        let t = cross(sub(c, a), s) / den;
        let u = cross(sub(c, a), r) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return 0.0;
        }
    }
    point_seg(a, c, d)
        .min(point_seg(b, c, d))
        .min(point_seg(c, a, b))
        .min(point_seg(d, a, b))
}

impl Capsule {
    /// Gap between the two outlines (negative when they overlap).
    pub fn gap(&self, o: &Capsule) -> f64 {
        seg_seg(self.a, self.b, o.a, o.b) - self.r - o.r
    }
}

// ------------------------------------------------------------------ copper

/// What a copper item is.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// Index into the pad list.
    Pad(usize),
    /// Trace id + segment index.
    Trace(String, usize),
    Via(String),
}

/// One piece of copper.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: ItemKind,
    pub shape: Capsule,
    pub layers: [bool; 2],
}

/// The board's copper with its connectivity.
#[derive(Debug, Clone)]
pub struct Copper {
    pub pads: Vec<WorldPad>,
    /// Net name of each pad (`None` = not connected in the circuit).
    pub pad_net: Vec<Option<String>>,
    pub items: Vec<Item>,
    /// Copper group of each item.
    pub group: Vec<usize>,
}

struct Uf(Vec<usize>);
impl Uf {
    fn find(&mut self, x: usize) -> usize {
        let mut x = x;
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

impl Copper {
    /// Builds the copper of a project's board.
    pub fn build(p: &Project, lib: &Library, board: &Board, conn: &Connectivity) -> Copper {
        let parts = placed_parts(p, lib, false);
        let mut pads = Vec::new();
        for pl in &parts {
            pads.extend(pl.pads());
        }
        let pad_net: Vec<Option<String>> = pads
            .iter()
            .map(|pd| {
                conn.net_of(&pd.part, &pd.pin)
                    .map(|n| conn.net(n).name.clone())
            })
            .collect();
        let mut items: Vec<Item> = pads
            .iter()
            .enumerate()
            .map(|(i, pd)| Item {
                kind: ItemKind::Pad(i),
                shape: pd.capsule(),
                layers: pd.layers(),
            })
            .collect();
        for t in &board.traces {
            let mut l = [false, false];
            l[t.layer.index()] = true;
            for (k, w) in t.points.windows(2).enumerate() {
                items.push(Item {
                    kind: ItemKind::Trace(t.id.clone(), k),
                    shape: Capsule {
                        a: w[0],
                        b: w[1],
                        r: t.width / 2.0,
                    },
                    layers: l,
                });
            }
            if t.points.len() == 1 {
                items.push(Item {
                    kind: ItemKind::Trace(t.id.clone(), 0),
                    shape: Capsule {
                        a: t.points[0],
                        b: t.points[0],
                        r: t.width / 2.0,
                    },
                    layers: l,
                });
            }
        }
        for v in &board.vias {
            items.push(Item {
                kind: ItemKind::Via(v.id.clone()),
                shape: Capsule {
                    a: (v.x, v.y),
                    b: (v.x, v.y),
                    r: v.diameter / 2.0,
                },
                layers: [true, true],
            });
        }
        let mut uf = Uf((0..items.len()).collect());
        for i in 0..items.len() {
            for j in i + 1..items.len() {
                let (a, b) = (&items[i], &items[j]);
                let shared = (a.layers[0] && b.layers[0]) || (a.layers[1] && b.layers[1]);
                if shared && a.shape.gap(&b.shape) <= 1e-6 {
                    uf.union(i, j);
                }
            }
        }
        // pins bonded inside a part (an UNO's GND pins) count as one node
        for (i, a) in pads.iter().enumerate() {
            for (j, b) in pads.iter().enumerate().skip(i + 1) {
                if a.part == b.part
                    && let Some(def) =
                        lib.get(&p.part(&a.part).map(|x| x.part.clone()).unwrap_or_default())
                    && def
                        .bonds
                        .iter()
                        .any(|g| g.contains(&a.pin) && g.contains(&b.pin))
                {
                    uf.union(i, j);
                }
            }
        }
        let group = (0..items.len()).map(|i| uf.find(i)).collect();
        Copper {
            pads,
            pad_net,
            items,
            group,
        }
    }

    /// Nets and their pads (index lists), for nets with at least two pads.
    pub fn nets(&self) -> BTreeMap<String, Vec<usize>> {
        let mut m: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, n) in self.pad_net.iter().enumerate() {
            if let Some(n) = n {
                m.entry(n.clone()).or_default().push(i);
            }
        }
        m.retain(|_, v| v.len() >= 2);
        m
    }

    /// Ratsnest: for every net, the shortest lines that would join its copper groups
    /// (Prim over groups, nearest pad pairs). `(net, pad a, pad b)`.
    pub fn ratsnest(&self) -> Vec<(String, usize, usize)> {
        let mut out = Vec::new();
        for (net, pads) in self.nets() {
            // groups of this net's pads
            let mut groups: Vec<Vec<usize>> = Vec::new();
            let mut by_group: HashMap<usize, usize> = HashMap::new();
            for &pi in &pads {
                let g = self.group[pi];
                match by_group.get(&g) {
                    Some(&k) => groups[k].push(pi),
                    None => {
                        by_group.insert(g, groups.len());
                        groups.push(vec![pi]);
                    }
                }
            }
            if groups.len() < 2 {
                continue;
            }
            let mut in_tree = vec![false; groups.len()];
            in_tree[0] = true;
            for _ in 1..groups.len() {
                let mut best: Option<(f64, usize, usize, usize)> = None;
                for (gi, g) in groups.iter().enumerate() {
                    if !in_tree[gi] {
                        continue;
                    }
                    for (hj, h) in groups.iter().enumerate() {
                        if in_tree[hj] {
                            continue;
                        }
                        for &a in g {
                            for &b in h {
                                let d = (self.pads[a].x - self.pads[b].x)
                                    .hypot(self.pads[a].y - self.pads[b].y);
                                if best.is_none_or(|x| d < x.0) {
                                    best = Some((d, a, b, hj));
                                }
                            }
                        }
                    }
                }
                if let Some((_, a, b, hj)) = best {
                    in_tree[hj] = true;
                    out.push((net.clone(), a, b));
                }
            }
        }
        out
    }

    /// Nets whose pads are not all joined by copper.
    pub fn unrouted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ratsnest().into_iter().map(|r| r.0).collect();
        v.dedup();
        v
    }

    /// The net a copper group carries (from its pads; `None` = floating copper).
    pub fn group_nets(&self) -> HashMap<usize, Vec<String>> {
        let mut m: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, it) in self.items.iter().enumerate() {
            if let ItemKind::Pad(pi) = it.kind
                && let Some(n) = &self.pad_net[pi]
            {
                let e = m.entry(self.group[i]).or_default();
                if !e.contains(n) {
                    e.push(n.clone());
                }
            }
        }
        m
    }

    /// The pad under a point.
    pub fn pad_at(&self, x: f64, y: f64) -> Option<usize> {
        self.pads.iter().position(|p| p.contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn blink() -> (Project, Arc<Library>) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/blink");
        (Project::load(&dir).unwrap(), Library::builtin())
    }

    #[test]
    fn auto_place_then_route_every_net() {
        let (mut p, lib) = blink();
        let n = auto_place(&mut p, &lib, true);
        assert!(n >= 3, "UNO, R, LED: {n}");
        let (x0, y0, x1, y1) = p.pcb.outline.unwrap();
        for pl in placed_parts(&p, &lib, false) {
            let b = pl.bounds();
            assert!(
                b.0 >= x0 && b.1 >= y0 && b.2 <= x1 && b.3 <= y1,
                "{} inside",
                pl.reference
            );
        }
        // no overlapping courtyards
        let parts = placed_parts(&p, &lib, false);
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                let (a, b) = (a.bounds(), b.bounds());
                assert!(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1);
            }
        }
        let conn = Connectivity::compute(&p, &lib);
        let board = Board::load(&p);
        let cu = Copper::build(&p, &lib, &board, &conn);
        let un = cu.unrouted();
        assert!(!un.is_empty(), "nothing routed yet");
        // route each ratsnest line straight (bottom layer for every other one)
        let mut board = board;
        for (k, (_, a, b)) in cu.ratsnest().iter().enumerate() {
            let id = board.next_id("t");
            board.traces.push(Trace {
                id,
                layer: if k % 2 == 0 {
                    Layer::Top
                } else {
                    Layer::Bottom
                },
                width: 0.3,
                points: vec![
                    (cu.pads[*a].x, cu.pads[*a].y),
                    (cu.pads[*b].x, cu.pads[*b].y),
                ],
            });
        }
        board.store(&mut p);
        let back = Board::load(&p);
        assert_eq!(back, board);
        let cu = Copper::build(&p, &lib, &back, &conn);
        assert!(cu.unrouted().is_empty(), "{:?}", cu.unrouted());
    }

    #[test]
    fn capsule_gaps() {
        let a = Capsule {
            a: (0.0, 0.0),
            b: (10.0, 0.0),
            r: 0.5,
        };
        let b = Capsule {
            a: (5.0, 2.0),
            b: (5.0, 2.0),
            r: 0.5,
        };
        assert!((a.gap(&b) - 1.0).abs() < 1e-9);
        let c = Capsule {
            a: (5.0, -3.0),
            b: (5.0, 3.0),
            r: 0.1,
        };
        assert!(a.gap(&c) < 0.0, "crossing");
    }
}
