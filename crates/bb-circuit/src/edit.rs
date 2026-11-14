//! Undoable editing (WB-5): every change is a [`Transaction`] of invertible [`Op`]s
//! (before/after snapshots of the touched objects), so undo/redo is unlimited and exact.
//! Copy/paste goes through a [`Clipboard`] that remaps ids and designators.

use crate::geometry::Placement;
use crate::library::Library;
use crate::part::PropValue;
use crate::project::{
    Endpoint, InstrumentDef, NetLabel, PartInstance, ProbeDef, Project, SchWire, Wire,
};
use bb_world::WorldConfig;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum EditError {
    #[error("unknown part definition `{0}`")]
    UnknownDef(String),
    #[error("no object `{0}`")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

/// One invertible change: `before` → `after` (`None` = absent). List ops remember where a
/// removed object was (`index`) so undo puts it back in the same place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Op {
    Part {
        before: Option<PartInstance>,
        after: Option<PartInstance>,
        #[serde(default)]
        index: Option<usize>,
    },
    Wire {
        before: Option<Wire>,
        after: Option<Wire>,
        #[serde(default)]
        index: Option<usize>,
    },
    SchWire {
        before: Option<SchWire>,
        after: Option<SchWire>,
        #[serde(default)]
        index: Option<usize>,
    },
    Label {
        before: Option<NetLabel>,
        after: Option<NetLabel>,
        #[serde(default)]
        index: Option<usize>,
    },
    Probe {
        before: Option<ProbeDef>,
        after: Option<ProbeDef>,
        #[serde(default)]
        index: Option<usize>,
    },
    Instrument {
        before: Option<InstrumentDef>,
        after: Option<InstrumentDef>,
        #[serde(default)]
        index: Option<usize>,
    },
    World {
        before: Box<WorldConfig>,
        after: Box<WorldConfig>,
    },
    Meta {
        before: (String, String),
        after: (String, String),
    },
}

impl Op {
    fn inverse(&self) -> Op {
        match self.clone() {
            Op::Part {
                before,
                after,
                index,
            } => Op::Part {
                before: after,
                after: before,
                index,
            },
            Op::Wire {
                before,
                after,
                index,
            } => Op::Wire {
                before: after,
                after: before,
                index,
            },
            Op::SchWire {
                before,
                after,
                index,
            } => Op::SchWire {
                before: after,
                after: before,
                index,
            },
            Op::Label {
                before,
                after,
                index,
            } => Op::Label {
                before: after,
                after: before,
                index,
            },
            Op::Probe {
                before,
                after,
                index,
            } => Op::Probe {
                before: after,
                after: before,
                index,
            },
            Op::Instrument {
                before,
                after,
                index,
            } => Op::Instrument {
                before: after,
                after: before,
                index,
            },
            Op::World { before, after } => Op::World {
                before: after,
                after: before,
            },
            Op::Meta { before, after } => Op::Meta {
                before: after,
                after: before,
            },
        }
    }

    /// Does this op change the electrical circuit (vs. only positions / cosmetics)?
    pub fn is_electrical(&self) -> bool {
        match self {
            Op::Part { before, after, .. } => match (before, after) {
                (Some(a), Some(b)) => {
                    a.part != b.part
                        || a.props != b.props
                        || a.bench != b.bench
                        || a.schematic != b.schematic
                        || a.attach != b.attach
                        || a.firmware != b.firmware
                }
                _ => true,
            },
            Op::Wire { before, after, .. } => match (before, after) {
                (Some(a), Some(b)) => a.a != b.a || a.b != b.b,
                _ => true,
            },
            Op::SchWire { .. } | Op::Label { .. } => true,
            Op::Probe { .. } | Op::Instrument { .. } | Op::World { .. } | Op::Meta { .. } => false,
        }
    }
}

fn replace<T: Clone>(
    v: &mut Vec<T>,
    id_of: impl Fn(&T) -> &str,
    before: &Option<T>,
    after: &Option<T>,
    index: &mut Option<usize>,
) {
    let key = before
        .as_ref()
        .or(after.as_ref())
        .map(|x| id_of(x).to_string());
    let Some(key) = key else { return };
    let pos = v.iter().position(|x| id_of(x) == key);
    match (pos, after) {
        (Some(i), Some(a)) => v[i] = a.clone(),
        (Some(i), None) => {
            v.remove(i);
            *index = Some(i);
        }
        (None, Some(a)) => match *index {
            Some(i) if i <= v.len() => v.insert(i, a.clone()),
            _ => v.push(a.clone()),
        },
        (None, None) => {}
    }
}

fn apply_op(p: &mut Project, op: &mut Op) {
    match op {
        Op::Part {
            before,
            after,
            index,
        } => replace(&mut p.parts, |x| &x.id, before, after, index),
        Op::Wire {
            before,
            after,
            index,
        } => replace(&mut p.wires, |x| &x.id, before, after, index),
        Op::SchWire {
            before,
            after,
            index,
        } => replace(&mut p.schematic.wires, |x| &x.id, before, after, index),
        Op::Label {
            before,
            after,
            index,
        } => replace(&mut p.schematic.labels, |x| &x.id, before, after, index),
        Op::Probe {
            before,
            after,
            index,
        } => replace(&mut p.probes, |x| &x.id, before, after, index),
        Op::Instrument {
            before,
            after,
            index,
        } => replace(&mut p.instruments, |x| &x.id, before, after, index),
        Op::World { after, .. } => p.world = (**after).clone(),
        Op::Meta { after, .. } => {
            p.name = after.0.clone();
            p.description = after.1.clone();
        }
    }
}

/// A named group of ops (one undo step).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Transaction {
    pub label: String,
    pub ops: Vec<Op>,
}

impl Transaction {
    pub fn new(label: &str) -> Transaction {
        Transaction {
            label: label.into(),
            ops: Vec::new(),
        }
    }
    pub fn is_electrical(&self) -> bool {
        self.ops.iter().any(Op::is_electrical)
    }
}

/// Parts + wires copied from the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Clipboard {
    pub parts: Vec<PartInstance>,
    pub wires: Vec<Wire>,
}

/// A project plus its undo history.
#[derive(Debug, Clone)]
pub struct Editor {
    pub project: Project,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// Bumped on every change (autosave / rebuild triggers).
    pub generation: u64,
    /// Bumped only on electrical changes (bench rebuild trigger).
    pub electrical_generation: u64,
}

impl Editor {
    pub fn new(project: Project) -> Editor {
        Editor {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            generation: 0,
            electrical_generation: 0,
        }
    }

    /// Applies a transaction and records it for undo.
    pub fn commit(&mut self, mut tx: Transaction) {
        if tx.ops.is_empty() {
            return;
        }
        for op in tx.ops.iter_mut() {
            apply_op(&mut self.project, op);
        }
        self.bump(tx.is_electrical());
        self.undo.push(tx);
        self.redo.clear();
    }

    fn bump(&mut self, electrical: bool) {
        self.generation += 1;
        if electrical {
            self.electrical_generation += 1;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut tx) = self.undo.pop() else {
            return false;
        };
        for op in tx.ops.iter_mut().rev() {
            let mut inv = op.inverse();
            apply_op(&mut self.project, &mut inv);
            *op = inv.inverse();
        }
        self.bump(tx.is_electrical());
        self.redo.push(tx);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut tx) = self.redo.pop() else {
            return false;
        };
        for op in tx.ops.iter_mut() {
            apply_op(&mut self.project, op);
        }
        self.bump(tx.is_electrical());
        self.undo.push(tx);
        true
    }

    // ------------------------------------------------------------------ high-level edits

    /// Adds a part from the library at a bench position; returns its id.
    pub fn add_part(
        &mut self,
        lib: &Library,
        def_id: &str,
        x: f64,
        y: f64,
    ) -> Result<String, EditError> {
        let def = lib
            .get(def_id)
            .ok_or_else(|| EditError::UnknownDef(def_id.into()))?;
        let reference = self.project.next_ref(&def.ref_prefix);
        let id = self.unique_id(&reference.to_ascii_lowercase());
        let mut inst = PartInstance::new(&id, &reference, def_id);
        if !def.bench.hidden {
            inst = inst.at(x, y);
        } else {
            inst.schematic = Some(Placement::at(x, y));
        }
        let mut tx = Transaction::new(&format!("Add {}", def.name));
        tx.ops.push(Op::Part {
            before: None,
            after: Some(inst),
            index: None,
        });
        self.commit(tx);
        Ok(id)
    }

    fn unique_id(&self, base: &str) -> String {
        if self.project.part(base).is_none()
            && self.project.wire(base).is_none()
            && !base.is_empty()
        {
            base.to_string()
        } else {
            self.project.next_id(&format!("{base}_"))
        }
    }

    /// Removes parts and every wire/probe attached to them.
    pub fn delete(&mut self, part_ids: &[String], wire_ids: &[String]) {
        let parts: HashSet<&str> = part_ids.iter().map(String::as_str).collect();
        let mut tx = Transaction::new("Delete");
        for w in &self.project.wires {
            let attached = [&w.a, &w.b]
                .iter()
                .any(|e| e.part().is_some_and(|p| parts.contains(p)));
            if attached || wire_ids.contains(&w.id) {
                tx.ops.push(Op::Wire {
                    before: Some(w.clone()),
                    after: None,
                    index: None,
                });
            }
        }
        for pr in &self.project.probes {
            if pr.target.part().is_some_and(|p| parts.contains(p)) {
                tx.ops.push(Op::Probe {
                    before: Some(pr.clone()),
                    after: None,
                    index: None,
                });
            }
        }
        for p in &self.project.parts {
            if parts.contains(p.id.as_str()) {
                tx.ops.push(Op::Part {
                    before: Some(p.clone()),
                    after: None,
                    index: None,
                });
            }
        }
        self.commit(tx);
    }

    fn edit_part(
        &mut self,
        label: &str,
        id: &str,
        f: impl FnOnce(&mut PartInstance),
    ) -> Result<(), EditError> {
        let before = self
            .project
            .part(id)
            .cloned()
            .ok_or_else(|| EditError::NotFound(id.into()))?;
        let mut after = before.clone();
        f(&mut after);
        if after != before {
            let mut tx = Transaction::new(label);
            tx.ops.push(Op::Part {
                before: Some(before),
                after: Some(after),
                index: None,
            });
            self.commit(tx);
        }
        Ok(())
    }

    /// Moves parts on the bench by (dx, dy) (snaps to the 0.1" grid when `snap`).
    pub fn move_parts(&mut self, ids: &[String], dx: f64, dy: f64, snap: bool) {
        let mut tx = Transaction::new("Move");
        for id in ids {
            let Some(before) = self.project.part(id).cloned() else {
                continue;
            };
            let mut after = before.clone();
            if let Some(b) = after.bench.as_mut() {
                b.at.x += dx;
                b.at.y += dy;
                if snap {
                    b.at.x = b.at.x.round();
                    b.at.y = b.at.y.round();
                }
            }
            tx.ops.push(Op::Part {
                before: Some(before),
                after: Some(after),
                index: None,
            });
        }
        self.commit(tx);
    }

    /// Moves a part in the schematic.
    pub fn place_in_schematic(&mut self, id: &str, at: Placement) -> Result<(), EditError> {
        self.edit_part("Move in schematic", id, |p| p.schematic = Some(at))
    }

    /// Rotates by 90° (R).
    pub fn rotate(&mut self, id: &str) -> Result<(), EditError> {
        self.edit_part("Rotate", id, |p| {
            if let Some(b) = p.bench.as_mut() {
                b.at = b.at.rotated(90.0);
            }
        })
    }

    /// Mirrors (F).
    pub fn flip(&mut self, id: &str) -> Result<(), EditError> {
        self.edit_part("Flip", id, |p| {
            if let Some(b) = p.bench.as_mut() {
                b.at.flip = !b.at.flip;
            }
        })
    }

    /// Sets a property (validated against the definition).
    pub fn set_prop(
        &mut self,
        lib: &Library,
        id: &str,
        key: &str,
        value: PropValue,
    ) -> Result<(), EditError> {
        let part = self
            .project
            .part(id)
            .ok_or_else(|| EditError::NotFound(id.into()))?;
        let def = lib
            .get(&part.part)
            .ok_or_else(|| EditError::UnknownDef(part.part.clone()))?;
        let pd = def
            .prop(key)
            .ok_or_else(|| EditError::Invalid(format!("{} has no property `{key}`", def.name)))?;
        let v = pd.coerce(&value).map_err(EditError::Invalid)?;
        self.edit_part(&format!("Change {}", pd.label), id, |p| {
            p.props.insert(key.into(), v);
        })
    }

    /// Renames a designator.
    pub fn set_reference(&mut self, id: &str, reference: &str) -> Result<(), EditError> {
        if self
            .project
            .part_by_ref(reference)
            .is_some_and(|p| p.id != id)
        {
            return Err(EditError::Invalid(format!("{reference} is already used")));
        }
        self.edit_part("Rename", id, |p| p.reference = reference.into())
    }

    /// Adds a jumper wire; colour defaults to red for +, black for GND, else `fallback`.
    pub fn add_wire(&mut self, lib: &Library, a: Endpoint, b: Endpoint) -> String {
        let color = auto_color(&self.project, lib, &a, &b);
        let id = self.project.next_id("w");
        let mut tx = Transaction::new("Add wire");
        tx.ops.push(Op::Wire {
            before: None,
            after: Some(Wire {
                id: id.clone(),
                a,
                b,
                color,
                bends: Vec::new(),
            }),
            index: None,
        });
        self.commit(tx);
        id
    }

    /// Replaces a wire (colour, bend points, re-attached ends).
    pub fn update_wire(&mut self, wire: Wire) -> Result<(), EditError> {
        let before = self
            .project
            .wire(&wire.id)
            .cloned()
            .ok_or_else(|| EditError::NotFound(wire.id.clone()))?;
        let mut tx = Transaction::new("Edit wire");
        tx.ops.push(Op::Wire {
            before: Some(before),
            after: Some(wire),
            index: None,
        });
        self.commit(tx);
        Ok(())
    }

    pub fn add_schematic_wire(&mut self, points: Vec<(f64, f64)>) -> String {
        let id = self.project.next_id("sw");
        let mut tx = Transaction::new("Draw wire");
        tx.ops.push(Op::SchWire {
            before: None,
            after: Some(SchWire {
                id: id.clone(),
                points,
            }),
            index: None,
        });
        self.commit(tx);
        id
    }

    pub fn add_label(&mut self, name: &str, x: f64, y: f64) -> String {
        let id = self.project.next_id("nl");
        let mut tx = Transaction::new("Add net label");
        tx.ops.push(Op::Label {
            before: None,
            after: Some(NetLabel {
                id: id.clone(),
                name: name.into(),
                x,
                y,
                rot: 0.0,
            }),
            index: None,
        });
        self.commit(tx);
        id
    }

    pub fn add_probe(
        &mut self,
        label: &str,
        target: Endpoint,
        q: crate::project::Quantity,
    ) -> String {
        let id = self.project.next_id("probe");
        let mut tx = Transaction::new("Add probe");
        tx.ops.push(Op::Probe {
            before: None,
            after: Some(ProbeDef {
                id: id.clone(),
                label: label.into(),
                target,
                quantity: q,
                pinned: None,
            }),
            index: None,
        });
        self.commit(tx);
        id
    }

    pub fn remove_probe(&mut self, id: &str) {
        if let Some(p) = self.project.probes.iter().find(|p| p.id == id).cloned() {
            let mut tx = Transaction::new("Remove probe");
            tx.ops.push(Op::Probe {
                before: Some(p),
                after: None,
                index: None,
            });
            self.commit(tx);
        }
    }

    /// Records the current world settings (sliders) as an undoable change.
    pub fn set_world(&mut self, world: WorldConfig) {
        if world == self.project.world {
            return;
        }
        let mut tx = Transaction::new("World");
        tx.ops.push(Op::World {
            before: Box::new(self.project.world.clone()),
            after: Box::new(world),
        });
        self.commit(tx);
    }

    // ------------------------------------------------------------------ clipboard

    /// Copies parts and the wires running between them.
    pub fn copy(&self, part_ids: &[String]) -> Clipboard {
        let set: HashSet<&str> = part_ids.iter().map(String::as_str).collect();
        Clipboard {
            parts: self
                .project
                .parts
                .iter()
                .filter(|p| set.contains(p.id.as_str()))
                .cloned()
                .collect(),
            wires: self
                .project
                .wires
                .iter()
                .filter(|w| {
                    [&w.a, &w.b]
                        .iter()
                        .all(|e| e.part().is_some_and(|p| set.contains(p)))
                })
                .cloned()
                .collect(),
        }
    }

    /// Pastes with fresh ids and designators, offset by (dx, dy); returns the new part ids.
    pub fn paste(&mut self, lib: &Library, clip: &Clipboard, dx: f64, dy: f64) -> Vec<String> {
        let mut tx = Transaction::new("Paste");
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        let mut scratch = self.project.clone();
        let mut new_ids = Vec::new();
        for p in &clip.parts {
            let prefix = lib
                .get(&p.part)
                .map(|d| d.ref_prefix.clone())
                .unwrap_or_else(|| "X".into());
            let reference = scratch.next_ref(&prefix);
            let mut id = reference.to_ascii_lowercase();
            if scratch.part(&id).is_some() {
                id = scratch.next_id(&format!("{id}_"));
            }
            let mut np = p.clone();
            np.id = id.clone();
            np.reference = reference;
            if let Some(b) = np.bench.as_mut() {
                b.at.x += dx;
                b.at.y += dy;
            }
            if let Some(s) = np.schematic.as_mut() {
                s.x += dx;
                s.y += dy;
            }
            map.insert(p.id.clone(), id.clone());
            scratch.parts.push(np.clone());
            new_ids.push(id);
            tx.ops.push(Op::Part {
                before: None,
                after: Some(np),
                index: None,
            });
        }
        for w in &clip.wires {
            let remap = |e: &Endpoint| match e {
                Endpoint::Pin { part, pin } => Endpoint::Pin {
                    part: map.get(part).cloned().unwrap_or_else(|| part.clone()),
                    pin: pin.clone(),
                },
                f => f.clone(),
            };
            let id = scratch.next_id("w");
            let nw = Wire {
                id,
                a: remap(&w.a),
                b: remap(&w.b),
                color: w.color.clone(),
                bends: w.bends.iter().map(|(x, y)| (x + dx, y + dy)).collect(),
            };
            scratch.wires.push(nw.clone());
            tx.ops.push(Op::Wire {
                before: None,
                after: Some(nw),
                index: None,
            });
        }
        self.commit(tx);
        new_ids
    }

    /// ⌘D: copy + paste one pitch down-right.
    pub fn duplicate(&mut self, lib: &Library, part_ids: &[String]) -> Vec<String> {
        let clip = self.copy(part_ids);
        self.paste(lib, &clip, 2.0, 2.0)
    }
}

/// Red for wires touching a + supply, black for ground, orange otherwise (WB-4).
pub fn auto_color(project: &Project, lib: &Library, a: &Endpoint, b: &Endpoint) -> String {
    use crate::part::PinRole;
    let mut color = "orange";
    for e in [a, b] {
        let Endpoint::Pin { part, pin } = e else {
            continue;
        };
        let Some(p) = project.part(part) else {
            continue;
        };
        let Some(def) = lib.get(&p.part) else {
            continue;
        };
        if def.is_breadboard() {
            if pin.starts_with("t+") || pin.starts_with("b+") {
                color = "red";
            } else if pin.starts_with("t-") || pin.starts_with("b-") {
                color = "black";
            }
            continue;
        }
        if let Some(pd) = def.pin(pin) {
            match pd.role {
                PinRole::Ground => color = "black",
                PinRole::PowerOut | PinRole::PowerIn => color = "red",
                _ => {}
            }
        }
    }
    color.into()
}
