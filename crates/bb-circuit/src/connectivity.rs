//! Connectivity engine (WB-7): which pins are electrically the same net.
//!
//! Three views of the same circuit are computed:
//!
//! * **physical** (bench): breadboard strips and rails, leads plugged into holes / board header
//!   sockets, jumper wires, part-internal bonds;
//! * **schematic**: schematic wires (anchors on vertices, T-junctions), net labels and power
//!   symbols with the same name, bonds;
//! * **logical** = the union of both — what gets simulated.
//!
//! The ratsnest lists logical connections missing from a view: dashed lines on the bench for
//! connections only drawn in the schematic, and the reverse for the schematic.

use crate::breadboard;
use crate::geometry::{SNAP, cell, point_segment_distance, same_point};
use crate::library::Library;
use crate::part::{Layout, PartDef, PinRole, PropValue};
use crate::project::{Endpoint, Project};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NetId(pub usize);

/// A pin of a placed part (breadboard holes are pins of the breadboard).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TermRef {
    pub part: String,
    pub pin: String,
}

impl TermRef {
    pub fn new(part: &str, pin: &str) -> TermRef {
        TermRef {
            part: part.into(),
            pin: pin.into(),
        }
    }
}

/// One logical net.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Net {
    pub id: NetId,
    /// SPICE-safe name: `0` for ground, label names, `D9`, `VCC`, `N3`, …
    pub name: String,
    /// Component pins on the net (breadboard holes excluded).
    pub pins: Vec<TermRef>,
    /// Breadboard holes on the net.
    pub holes: Vec<TermRef>,
    pub is_ground: bool,
    /// Net label / power symbol names attached in the schematic.
    pub labels: Vec<String>,
}

/// A dashed "should be connected" line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatsLine {
    pub net: NetId,
    pub a: TermRef,
    pub b: TermRef,
    pub from: (f64, f64),
    pub to: (f64, f64),
}

/// Result of a connectivity pass.
#[derive(Debug, Clone, Default)]
pub struct Connectivity {
    pub nets: Vec<Net>,
    term_net: HashMap<TermRef, NetId>,
    phys_group: HashMap<TermRef, usize>,
    sch_group: HashMap<TermRef, usize>,
    /// Lead → socket (hole or header) insertions found on the bench.
    pub insertions: Vec<(TermRef, TermRef)>,
    pub ratsnest_bench: Vec<RatsLine>,
    pub ratsnest_schematic: Vec<RatsLine>,
}

type Cell = (i64, i64);
type Point = (f64, f64);

struct Uf {
    p: Vec<usize>,
}

impl Uf {
    fn new(n: usize) -> Uf {
        Uf {
            p: (0..n).collect(),
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.p[x] != x {
            self.p[x] = self.p[self.p[x]];
            x = self.p[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.p[ra.max(rb)] = ra.min(rb);
        }
    }
}

struct Terms<'a> {
    defs: Vec<Option<Arc<PartDef>>>,
    offset: Vec<usize>,
    project: &'a Project,
}

impl Terms<'_> {
    fn index(&self, part: &str, pin: &str) -> Option<usize> {
        let pi = self.project.parts.iter().position(|p| p.id == part)?;
        let def = self.defs[pi].as_ref()?;
        Some(self.offset[pi] + def.pin_index(pin)?)
    }
    fn tref(&self, idx: usize) -> TermRef {
        let pi = self.offset.partition_point(|&o| o <= idx) - 1;
        let def = self.defs[pi].as_ref().expect("terminal of a known part");
        TermRef::new(
            &self.project.parts[pi].id,
            &def.pins[idx - self.offset[pi]].id,
        )
    }
}

impl Connectivity {
    /// Computes nets and ratsnests for a project.
    pub fn compute(project: &Project, lib: &Library) -> Connectivity {
        let defs: Vec<Option<Arc<PartDef>>> = project
            .parts
            .iter()
            .map(|p| lib.get(&p.part).cloned())
            .collect();
        let mut offset = Vec::with_capacity(defs.len() + 1);
        let mut n = 0;
        for d in &defs {
            offset.push(n);
            n += d.as_ref().map_or(0, |d| d.pins.len());
        }
        offset.push(n);
        let terms = Terms {
            defs,
            offset,
            project,
        };
        let n_terms = n;
        // extra union-find slots: schematic wires, then names (labels / power symbols)
        let n_wires = project.schematic.wires.len();
        let mut names: BTreeMap<String, usize> = BTreeMap::new();
        let canon = |s: &str| -> String {
            let u = s.trim().to_ascii_uppercase();
            if u == "0" || u == "GND" || u == "GROUND" {
                "0".into()
            } else {
                u
            }
        };
        for l in &project.schematic.labels {
            let k = canon(&l.name);
            let next = names.len();
            names.entry(k).or_insert(next);
        }
        let mut power_syms: Vec<(usize, String)> = Vec::new();
        for (pi, p) in project.parts.iter().enumerate() {
            let Some(def) = &terms.defs[pi] else { continue };
            let name = match def.id.as_str() {
                "gnd" => Some("0".to_string()),
                "power_symbol" => Some(canon(
                    &Project::prop(def, p, "net")
                        .map(|v| v.as_text())
                        .unwrap_or_else(|| "VCC".into()),
                )),
                _ => None,
            };
            if let Some(name) = name {
                let next = names.len();
                names.entry(name.clone()).or_insert(next);
                power_syms.push((terms.offset[pi], name));
            }
        }
        let total = n_terms + n_wires + names.len();
        let mut phys = Uf::new(total);
        let mut sch = Uf::new(total);
        let mut logical = Uf::new(total);
        let mut both = |a: usize, b: usize, phys_too: bool, sch_too: bool, l: &mut Uf| {
            if phys_too {
                phys.union(a, b);
            }
            if sch_too {
                sch.union(a, b);
            }
            l.union(a, b);
        };

        // part-internal bonds + breadboard strips
        for (pi, p) in project.parts.iter().enumerate() {
            let Some(def) = &terms.defs[pi] else { continue };
            let base = terms.offset[pi];
            for bond in &def.bonds {
                let idx: Vec<usize> = bond.iter().filter_map(|b| def.pin_index(b)).collect();
                for w in idx.windows(2) {
                    both(base + w[0], base + w[1], true, true, &mut logical);
                }
            }
            if let Some(Layout::Breadboard { rail_holes, .. }) = def.bench.layout {
                let split = matches!(
                    Project::prop(def, p, "split_rails"),
                    Some(PropValue::Bool(true))
                );
                let mut first: HashMap<String, usize> = HashMap::new();
                for (i, pin) in def.pins.iter().enumerate() {
                    if let Some(h) = breadboard::parse_hole(&pin.id) {
                        let key = breadboard::strip_key(&h, rail_holes, split);
                        match first.get(&key) {
                            Some(&j) => both(base + j, base + i, true, false, &mut logical),
                            None => {
                                first.insert(key, i);
                            }
                        }
                    }
                }
            }
        }

        // leads into sockets
        let mut sockets: HashMap<Cell, Vec<(usize, Point, usize)>> = HashMap::new();
        for (pi, p) in project.parts.iter().enumerate() {
            let Some(def) = &terms.defs[pi] else { continue };
            if !def.bench.sockets {
                continue;
            }
            if let Some(leads) = Project::bench_leads(def, p) {
                for (pin, x, y) in leads {
                    if let Some(i) = def.pin_index(&pin) {
                        sockets.entry(cell((x, y))).or_default().push((
                            terms.offset[pi] + i,
                            (x, y),
                            pi,
                        ));
                    }
                }
            }
        }
        let mut insertions = Vec::new();
        for (pi, p) in project.parts.iter().enumerate() {
            let Some(def) = &terms.defs[pi] else { continue };
            if def.bench.sockets || def.bench.off_board {
                continue;
            }
            let Some(leads) = Project::bench_leads(def, p) else {
                continue;
            };
            for (pin, x, y) in leads {
                let Some(i) = def.pin_index(&pin) else {
                    continue;
                };
                let (cx, cy) = cell((x, y));
                let mut found = None;
                'search: for dx in -1..=1 {
                    for dy in -1..=1 {
                        if let Some(v) = sockets.get(&(cx + dx, cy + dy)) {
                            for &(t, pos, owner) in v {
                                if owner != pi && same_point(pos, (x, y)) {
                                    found = Some(t);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
                if let Some(t) = found {
                    let lead = terms.offset[pi] + i;
                    both(lead, t, true, false, &mut logical);
                    insertions.push((terms.tref(lead), terms.tref(t)));
                }
            }
        }

        // jumper wires
        for w in &project.wires {
            if let (Endpoint::Pin { part: pa, pin: a }, Endpoint::Pin { part: pb, pin: b }) =
                (&w.a, &w.b)
                && let (Some(ia), Some(ib)) = (terms.index(pa, a), terms.index(pb, b))
            {
                both(ia, ib, true, false, &mut logical);
            }
        }

        // schematic: anchors, wires, labels, power symbols
        let mut anchors: Vec<(usize, (f64, f64))> = Vec::new();
        for (pi, p) in project.parts.iter().enumerate() {
            let Some(def) = &terms.defs[pi] else { continue };
            if let Some(pins) = Project::schematic_pins(def, p) {
                for (pin, x, y) in pins {
                    if let Some(i) = def.pin_index(&pin) {
                        anchors.push((terms.offset[pi] + i, (x, y)));
                    }
                }
            }
        }
        // pins touching pins connect directly
        {
            let mut by_cell: HashMap<Cell, Vec<(usize, Point)>> = HashMap::new();
            for &(t, pos) in &anchors {
                by_cell.entry(cell(pos)).or_default().push((t, pos));
            }
            for &(t, pos) in &anchors {
                let (cx, cy) = cell(pos);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        if let Some(v) = by_cell.get(&(cx + dx, cy + dy)) {
                            for &(u, q) in v {
                                if u != t && same_point(pos, q) {
                                    both(t, u, false, true, &mut logical);
                                }
                            }
                        }
                    }
                }
            }
        }
        let on_poly = |pt: (f64, f64), pts: &[(f64, f64)]| -> bool {
            if pts.iter().any(|&q| same_point(q, pt)) {
                return true;
            }
            pts.windows(2)
                .any(|s| point_segment_distance(pt, s[0], s[1]) < SNAP * 0.5)
        };
        for (wi, w) in project.schematic.wires.iter().enumerate() {
            let slot = n_terms + wi;
            for &(t, pos) in &anchors {
                if on_poly(pos, &w.points) {
                    both(t, slot, false, true, &mut logical);
                }
            }
            for (wj, w2) in project.schematic.wires.iter().enumerate() {
                if wj == wi {
                    continue;
                }
                let ends = [w2.points.first(), w2.points.last()];
                if ends.iter().flatten().any(|&&e| on_poly(e, &w.points)) {
                    both(slot, n_terms + wj, false, true, &mut logical);
                }
            }
        }
        for l in &project.schematic.labels {
            let slot = n_terms + n_wires + names[&canon(&l.name)];
            for (wi, w) in project.schematic.wires.iter().enumerate() {
                if on_poly((l.x, l.y), &w.points) {
                    both(slot, n_terms + wi, false, true, &mut logical);
                }
            }
            for &(t, pos) in &anchors {
                if same_point(pos, (l.x, l.y)) {
                    both(slot, t, false, true, &mut logical);
                }
            }
        }
        for (t, name) in &power_syms {
            let slot = n_terms + n_wires + names[name];
            both(*t, slot, false, true, &mut logical);
        }

        // ---- build nets from logical roots
        let mut root_net: HashMap<usize, NetId> = HashMap::new();
        let mut nets: Vec<Net> = Vec::new();
        let mut term_net = HashMap::new();
        let mut phys_group = HashMap::new();
        let mut sch_group = HashMap::new();
        for t in 0..n_terms {
            let r = logical.find(t);
            let id = *root_net.entry(r).or_insert_with(|| {
                nets.push(Net {
                    id: NetId(nets.len()),
                    name: String::new(),
                    pins: Vec::new(),
                    holes: Vec::new(),
                    is_ground: false,
                    labels: Vec::new(),
                });
                NetId(nets.len() - 1)
            });
            let tr = terms.tref(t);
            let pi = terms.offset.partition_point(|&o| o <= t) - 1;
            let def = terms.defs[pi].as_ref().expect("known part");
            if def.is_breadboard() {
                nets[id.0].holes.push(tr.clone());
            } else {
                nets[id.0].pins.push(tr.clone());
            }
            phys_group.insert(tr.clone(), phys.find(t));
            sch_group.insert(tr.clone(), sch.find(t));
            term_net.insert(tr, id);
        }
        for (name, slot) in &names {
            let r = logical.find(n_terms + n_wires + slot);
            if let Some(&id) = root_net.get(&r) {
                nets[id.0].labels.push(name.clone());
            }
        }
        let mut c = Connectivity {
            nets,
            term_net,
            phys_group,
            sch_group,
            insertions,
            ratsnest_bench: Vec::new(),
            ratsnest_schematic: Vec::new(),
        };
        c.name_nets(project, &terms);
        c.ratsnest(project, &terms);
        c
    }

    fn name_nets(&mut self, project: &Project, terms: &Terms) {
        let def_of = |part: &str| -> Option<&Arc<PartDef>> {
            let pi = project.parts.iter().position(|p| p.id == part)?;
            terms.defs[pi].as_ref()
        };
        let reference = |part: &str| {
            project
                .part(part)
                .map(|p| p.reference.clone())
                .unwrap_or_default()
        };
        let n_mcus = project
            .parts
            .iter()
            .filter(|p| def_of(&p.id).is_some_and(|d| d.is_mcu()))
            .count();
        // ground: labels / symbols first, then source grounds
        for net in self.nets.iter_mut() {
            let has_gnd_label = net.labels.iter().any(|l| l == "0");
            let source_gnd = net.pins.iter().any(|t| {
                def_of(&t.part).is_some_and(|d| {
                    d.pin(&t.pin).is_some_and(|p| p.role == PinRole::Ground)
                        && (d.pins.iter().any(|p| p.role == PinRole::PowerOut) || d.is_mcu())
                })
            });
            net.is_ground = has_gnd_label || source_gnd;
        }
        let mut used: HashMap<String, usize> = HashMap::new();
        let mut auto = 0;
        let mut primary_ground_done = false;
        // the ground net with a label or the most pins becomes "0"
        let mut gnd_order: Vec<usize> = (0..self.nets.len())
            .filter(|&i| self.nets[i].is_ground)
            .collect();
        gnd_order.sort_by_key(|&i| {
            (
                !self.nets[i].labels.iter().any(|l| l == "0"),
                std::cmp::Reverse(self.nets[i].pins.len()),
            )
        });
        let primary = gnd_order.first().copied();
        for i in 0..self.nets.len() {
            let net = &self.nets[i];
            let mut name = None;
            if Some(i) == primary && !primary_ground_done {
                name = Some("0".to_string());
                primary_ground_done = true;
            } else if net.is_ground {
                // a second, separate reference (e.g. an isolated battery circuit)
                name = net
                    .pins
                    .first()
                    .map(|t| format!("{}_REF", reference(&t.part)));
            }
            if name.is_none()
                && let Some(l) = net.labels.iter().find(|l| *l != "0")
            {
                name = Some(l.clone());
            }
            if name.is_none() {
                // MCU supply → VCC, MCU pin → its Arduino name
                for t in &net.pins {
                    let Some(d) = def_of(&t.part) else { continue };
                    if let Some(m) = &d.model.mcu {
                        let pin = d.pin(&t.pin);
                        if m.vcc_pin == t.pin {
                            name = Some(if n_mcus > 1 {
                                format!("{}_VCC", reference(&t.part))
                            } else {
                                "VCC".into()
                            });
                            break;
                        }
                        if let Some(mcu) = pin.and_then(|p| p.mcu.clone()) {
                            name = Some(if n_mcus > 1 {
                                format!("{}_{mcu}", reference(&t.part))
                            } else {
                                mcu
                            });
                            break;
                        }
                    }
                }
            }
            if name.is_none() && n_mcus == 1 {
                // other header pins of the only board: AREF, RESET, VIN, 3V3 …
                for t in &net.pins {
                    if let Some(d) = def_of(&t.part)
                        && d.is_mcu()
                        && d.pin(&t.pin).is_some_and(|p| p.role != PinRole::Nc)
                    {
                        name = Some(t.pin.split('.').next().unwrap_or(&t.pin).to_string());
                        break;
                    }
                }
            }
            if name.is_none() {
                for t in &net.pins {
                    if let Some(d) = def_of(&t.part)
                        && let Some(p) = d.pin(&t.pin)
                        && p.role == PinRole::PowerOut
                    {
                        name = Some(if d.is_mcu() && n_mcus == 1 {
                            p.id.clone()
                        } else {
                            format!("{}_{}", reference(&t.part), p.id)
                        });
                        break;
                    }
                }
            }
            let all_nc = !net.pins.is_empty()
                && net.pins.iter().all(|t| {
                    def_of(&t.part)
                        .and_then(|d| d.pin(&t.pin).map(|p| p.role == PinRole::Nc))
                        .unwrap_or(false)
                });
            if name.is_none() && all_nc {
                name = Some(format!(
                    "{}_{}_NC",
                    reference(&net.pins[0].part),
                    net.pins[0].pin
                ));
            }
            let name = name.unwrap_or_else(|| {
                if net.pins.is_empty() {
                    // breadboard strip with nothing on it
                    format!("H{i}")
                } else {
                    auto += 1;
                    format!("N{auto}")
                }
            });
            let mut name = crate::template::sanitize(&name);
            if name != "0" {
                let k = used.entry(name.to_ascii_lowercase()).or_insert(0);
                *k += 1;
                if *k > 1 {
                    name = format!("{name}_{k}");
                }
            }
            self.nets[i].name = name;
        }
    }

    fn ratsnest(&mut self, project: &Project, terms: &Terms) {
        let def_of = |part: &str| -> Option<(Arc<PartDef>, &crate::project::PartInstance)> {
            let pi = project.parts.iter().position(|p| p.id == part)?;
            terms.defs[pi].clone().map(|d| (d, &project.parts[pi]))
        };
        let mut bench_pos: HashMap<TermRef, (f64, f64)> = HashMap::new();
        let mut sch_pos: HashMap<TermRef, (f64, f64)> = HashMap::new();
        for p in &project.parts {
            let Some((d, inst)) = def_of(&p.id) else {
                continue;
            };
            if d.is_breadboard() {
                continue;
            }
            if let Some(v) = Project::bench_leads(&d, inst) {
                for (pin, x, y) in v {
                    bench_pos.insert(TermRef::new(&p.id, &pin), (x, y));
                }
            }
            if d.schematic.symbol != "none"
                && let Some(v) = Project::schematic_pins(&d, inst)
            {
                for (pin, x, y) in v {
                    sch_pos.insert(TermRef::new(&p.id, &pin), (x, y));
                }
            }
        }
        let build = |groups: &HashMap<TermRef, usize>,
                     pos: &HashMap<TermRef, (f64, f64)>,
                     net: &Net|
         -> Vec<RatsLine> {
            let mut clusters: BTreeMap<usize, Vec<TermRef>> = BTreeMap::new();
            for t in &net.pins {
                if pos.contains_key(t) {
                    clusters.entry(groups[t]).or_default().push(t.clone());
                }
            }
            let cl: Vec<Vec<TermRef>> = clusters.into_values().collect();
            if cl.len() < 2 {
                return Vec::new();
            }
            // Prim over clusters with the closest pin pair as edge weight
            let mut in_tree = vec![false; cl.len()];
            in_tree[0] = true;
            let mut out = Vec::new();
            for _ in 1..cl.len() {
                let mut best: Option<(f64, usize, TermRef, TermRef)> = None;
                for (i, a) in cl.iter().enumerate().filter(|(i, _)| in_tree[*i]) {
                    let _ = i;
                    for (j, b) in cl.iter().enumerate().filter(|(j, _)| !in_tree[*j]) {
                        for ta in a {
                            for tb in b {
                                let (pa, pb) = (pos[ta], pos[tb]);
                                let d = (pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2);
                                if best.as_ref().is_none_or(|x| d < x.0) {
                                    best = Some((d, j, ta.clone(), tb.clone()));
                                }
                            }
                        }
                    }
                }
                let Some((_, j, a, b)) = best else { break };
                in_tree[j] = true;
                out.push(RatsLine {
                    net: net.id,
                    from: pos[&a],
                    to: pos[&b],
                    a,
                    b,
                });
            }
            out
        };
        let mut rb = Vec::new();
        let mut rs = Vec::new();
        for net in &self.nets {
            rb.extend(build(&self.phys_group, &bench_pos, net));
            rs.extend(build(&self.sch_group, &sch_pos, net));
        }
        self.ratsnest_bench = rb;
        self.ratsnest_schematic = rs;
    }

    // ------------------------------------------------------------------ queries

    pub fn net_of(&self, part: &str, pin: &str) -> Option<NetId> {
        self.term_net.get(&TermRef::new(part, pin)).copied()
    }
    pub fn net(&self, id: NetId) -> &Net {
        &self.nets[id.0]
    }
    pub fn net_by_name(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name.eq_ignore_ascii_case(name))
    }
    /// Logically connected?
    pub fn connected(&self, a: (&str, &str), b: (&str, &str)) -> bool {
        match (self.net_of(a.0, a.1), self.net_of(b.0, b.1)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }
    /// Physically connected on the bench?
    pub fn connected_on_bench(&self, a: (&str, &str), b: (&str, &str)) -> bool {
        let (ka, kb) = (TermRef::new(a.0, a.1), TermRef::new(b.0, b.1));
        matches!((self.phys_group.get(&ka), self.phys_group.get(&kb)), (Some(x), Some(y)) if x == y)
    }
    /// Connected in the schematic drawing?
    pub fn connected_in_schematic(&self, a: (&str, &str), b: (&str, &str)) -> bool {
        let (ka, kb) = (TermRef::new(a.0, a.1), TermRef::new(b.0, b.1));
        matches!((self.sch_group.get(&ka), self.sch_group.get(&kb)), (Some(x), Some(y)) if x == y)
    }
    /// Other component pins on the same net (hover highlight).
    pub fn net_mates(&self, part: &str, pin: &str) -> Vec<TermRef> {
        self.net_of(part, pin)
            .map(|n| {
                self.nets[n.0]
                    .pins
                    .iter()
                    .filter(|t| !(t.part == part && t.pin == pin))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    /// The ground net, if any.
    pub fn ground(&self) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == "0")
    }
}

#[cfg(test)]
mod tests;
