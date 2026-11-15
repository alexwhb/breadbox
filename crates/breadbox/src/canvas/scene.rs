//! The bench scene of one circuit revision: every placed part with its world-space leads and
//! body bounds in draw order, a spatial index of all pins and breadboard holes for snapping,
//! and which pins carry wires. Rebuilt only when the document changes (cached by generation).

use super::art;
use super::pen::WRect;
use bb_circuit::project::{Endpoint, Project};
use bb_circuit::{Connectivity, Library, PartDef};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One placed part.
#[derive(Debug, Clone)]
pub struct PartItem {
    pub id: String,
    pub reference: String,
    pub def: Arc<PartDef>,
    /// World positions of the leads.
    pub leads: Vec<(String, f64, f64)>,
    /// World-space body bounds (hit testing, selection outline).
    pub bounds: WRect,
    pub layer: u8,
    /// A lead sits in a breadboard hole.
    pub on_board: bool,
}

/// A pin or breadboard hole you can wire to.
#[derive(Debug, Clone)]
pub struct PinItem {
    /// Index into `Scene::parts`.
    pub part: usize,
    pub pin: String,
    pub x: f64,
    pub y: f64,
    /// A breadboard hole (vs. a component pin).
    pub hole: bool,
}

#[derive(Debug, Default)]
pub struct Scene {
    pub generation: u64,
    pub parts: Vec<PartItem>,
    pub pins: Vec<PinItem>,
    cells: HashMap<(i64, i64), Vec<usize>>,
    by_part: HashMap<String, usize>,
    /// Part id → pins with a wire, probe or instrument lead attached.
    pub wired: HashMap<String, Vec<String>>,
    /// World extent of everything (fit-to-view).
    pub extent: WRect,
}

fn cell(x: f64, y: f64) -> (i64, i64) {
    (x.floor() as i64, y.floor() as i64)
}

impl Scene {
    pub fn build(project: &Project, lib: &Library, conn: &Connectivity, generation: u64) -> Scene {
        let mut parts = Vec::new();
        // parts plugged into a breadboard hole
        let boards: HashSet<&str> = project
            .parts
            .iter()
            .filter(|p| lib.get(&p.part).is_some_and(|d| d.is_breadboard()))
            .map(|p| p.id.as_str())
            .collect();
        let on_board: HashSet<&str> = conn
            .insertions
            .iter()
            .filter(|(_, sock)| boards.contains(sock.part.as_str()))
            .map(|(lead, _)| lead.part.as_str())
            .collect();
        for p in &project.parts {
            let Some(def) = lib.get(&p.part) else {
                continue;
            };
            let Some(b) = &p.bench else { continue };
            if def.bench.hidden {
                continue;
            }
            let leads = Project::bench_leads(def, p).unwrap_or_default();
            let bounds = world_bounds(def, &b.at, &leads);
            parts.push(PartItem {
                id: p.id.clone(),
                reference: p.reference.clone(),
                def: def.clone(),
                leads,
                bounds,
                layer: art::layer(def),
                on_board: on_board.contains(p.id.as_str()),
            });
        }
        // stable draw order by layer (project order within a layer)
        parts.sort_by_key(|p| p.layer);
        let mut pins = Vec::new();
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let mut extent: Option<WRect> = None;
        let mut by_part = HashMap::new();
        for (i, it) in parts.iter().enumerate() {
            by_part.insert(it.id.clone(), i);
            extent = Some(match extent {
                Some(e) => e.union(&it.bounds),
                None => it.bounds,
            });
            let hole = it.def.is_breadboard();
            for (pin, x, y) in &it.leads {
                let k = pins.len();
                pins.push(PinItem {
                    part: i,
                    pin: pin.clone(),
                    x: *x,
                    y: *y,
                    hole,
                });
                cells.entry(cell(*x, *y)).or_default().push(k);
            }
        }
        let mut wired: HashMap<String, Vec<String>> = HashMap::new();
        let mut mark = |e: &Endpoint| {
            if let Endpoint::Pin { part, pin } = e {
                wired.entry(part.clone()).or_default().push(pin.clone());
            }
        };
        for w in &project.wires {
            mark(&w.a);
            mark(&w.b);
        }
        for pr in &project.probes {
            mark(&pr.target);
        }
        for ins in &project.instruments {
            for e in ins.probes.values() {
                mark(e);
            }
        }
        Scene {
            generation,
            parts,
            pins,
            cells,
            by_part,
            wired,
            extent: extent.unwrap_or(WRect::new(0.0, 0.0, 60.0, 30.0)),
        }
    }

    pub fn part(&self, id: &str) -> Option<&PartItem> {
        self.by_part.get(id).map(|&i| &self.parts[i])
    }

    /// World position of a part pin (or breadboard hole).
    pub fn pin_pos(&self, part: &str, pin: &str) -> Option<(f64, f64)> {
        self.part(part)?
            .leads
            .iter()
            .find(|(p, _, _)| p == pin)
            .map(|(_, x, y)| (*x, *y))
    }

    /// Where an endpoint is drawn.
    pub fn endpoint_pos(&self, e: &Endpoint) -> Option<(f64, f64)> {
        match e {
            Endpoint::Pin { part, pin } => self.pin_pos(part, pin),
            Endpoint::Free { x, y } => Some((*x, *y)),
        }
    }

    /// The nearest pin within `radius`. Component pins win over breadboard holes at the same
    /// spot (a lead plugged into a hole), and parts on top win over parts below.
    pub fn pin_at(&self, x: f64, y: f64, radius: f64) -> Option<&PinItem> {
        let (cx, cy) = cell(x, y);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(v) = self.cells.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &k in v {
                    let p = &self.pins[k];
                    let d = ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt();
                    if d > radius {
                        continue;
                    }
                    // prefer pins of higher layers / components over holes
                    let bias =
                        if p.hole { 0.05 } else { 0.0 } - self.parts[p.part].layer as f64 * 0.001;
                    let score = d + bias;
                    if best.is_none_or(|(b, _)| score < b) {
                        best = Some((score, k));
                    }
                }
            }
        }
        best.map(|(_, k)| &self.pins[k])
    }

    /// The topmost part whose body contains the point.
    pub fn part_at(&self, x: f64, y: f64) -> Option<usize> {
        self.parts
            .iter()
            .enumerate()
            .rev()
            .find(|(_, p)| p.bounds.contains(x, y))
            .map(|(i, _)| i)
    }

    /// Parts whose body intersects a world rectangle (box select).
    pub fn parts_in(&self, r: WRect) -> Vec<String> {
        self.parts
            .iter()
            .filter(|p| {
                let b = p.bounds;
                r.contains(b.x0, b.y0) && r.contains(b.x1, b.y1)
            })
            .map(|p| p.id.clone())
            .collect()
    }

    /// The breadboard hole nearest to a point, if one is within `radius`.
    pub fn hole_at(&self, x: f64, y: f64, radius: f64) -> Option<&PinItem> {
        let (cx, cy) = cell(x, y);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(v) = self.cells.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &k in v {
                    let p = &self.pins[k];
                    if !p.hole {
                        continue;
                    }
                    let d = ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt();
                    if d <= radius && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, k));
                    }
                }
            }
        }
        best.map(|(_, k)| &self.pins[k])
    }
}

/// World-space body bounds. Standing parts keep their body above the leads in screen space
/// whatever their rotation.
pub fn world_bounds(
    def: &PartDef,
    at: &bb_circuit::geometry::Placement,
    leads: &[(String, f64, f64)],
) -> WRect {
    let lb = art::local_bounds(def);
    if art::standing(&def.bench.art) && !leads.is_empty() {
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for (_, x, y) in leads {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        // local lead span (layout positions) to centre the body like the art does
        let span = def.bench.leads.iter().map(|l| l.x).fold(0.0f64, f64::max);
        let half = ((lb.x1 - lb.x0) / 2.0).max(span / 2.0 + 0.6);
        let mx = (x0 + x1) / 2.0;
        return WRect::new(mx - half, y0 + lb.y0, mx + half, y1 + lb.y1);
    }
    let a = at.apply(lb.x0, lb.y0);
    let b = at.apply(lb.x1, lb.y1);
    WRect::new(a.0, a.1, b.0, b.1)
}

/// Snaps a part's placement so its first lead lands on the 0.1" grid at the drop point.
pub fn snap_drop(def: &PartDef, x: f64, y: f64) -> (f64, f64) {
    let (lx, ly) = def
        .bench
        .leads
        .first()
        .map(|l| (l.x, l.y))
        .unwrap_or((0.0, 0.0));
    let (sx, sy) = ((x - lx).round(), (y - ly).round());
    (sx, sy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garden_node_scene_indexes_pins_and_holes() {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/garden-node");
        let p = Project::load(&dir).unwrap();
        let lib = Library::builtin();
        let conn = Connectivity::compute(&p, &lib);
        let s = Scene::build(&p, &lib, &conn, 1);
        // breadboard first, instruments last
        assert_eq!(s.parts[0].id, "bb1");
        assert!(s.parts.last().unwrap().layer == 4);
        // U1 D9 is a pin; a12 a hole
        let (x, y) = s.pin_pos("u1", "D9").unwrap();
        let hit = s.pin_at(x + 0.1, y - 0.1, 0.45).unwrap();
        assert_eq!(s.parts[hit.part].id, "u1");
        assert_eq!(hit.pin, "D9");
        let (hx, hy) = s.pin_pos("bb1", "a12").unwrap();
        let h = s.pin_at(hx, hy, 0.4).unwrap();
        assert!(h.hole && h.pin == "a12");
        // R1's lead sits in a hole: the component pin wins
        let (rx, ry) = s.pin_pos("r1", "1").unwrap();
        let r = s.pin_at(rx, ry, 0.4).unwrap();
        assert_eq!(s.parts[r.part].id, "r1");
        assert!(s.part("r1").unwrap().on_board);
        assert!(!s.part("u2").unwrap().on_board, "ESP-01 is off-board");
        // a click on the UNO body away from headers selects the UNO
        let i = s.part_at(13.0, 14.0).unwrap();
        assert_eq!(s.parts[i].id, "u1");
        assert!(s.wired["u1"].contains(&"D9".to_string()));
    }
}
