//! Programmatic bench construction: place parts by breadboard hole names and wire pins
//! together. Used to author the example projects, lessons and tests.
//!
//! ```
//! use bb_circuit::builder::BenchBuilder;
//! use bb_circuit::{Library, project::Mode};
//! let lib = Library::builtin();
//! let mut b = BenchBuilder::new(&lib, "blink", Mode::Playground);
//! let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
//! let bb = b.breadboard("breadboard_400", 30.0, 2.0);
//! let r1 = b.two_lead("resistor", "R1", &bb, "c5", "c9", &[("resistance", 220.0.into())]);
//! let d1 = b.two_lead("led", "D1", &bb, "e9", "e10", &[]);
//! b.wire((&u1, "D13"), (&bb, "a5"), "orange");
//! b.wire((&bb, "a10"), (&u1, "GND"), "black");
//! let conn = bb_circuit::Connectivity::compute(&b.project, &lib);
//! assert!(conn.connected((&r1, "2"), (&d1, "a")));
//! assert!(conn.connected((&u1, "D13"), (&r1, "1")));
//! ```

use crate::breadboard;
use crate::geometry::Placement;
use crate::library::Library;
use crate::part::PropValue;
use crate::project::{
    BenchPlacement, Endpoint, InstrumentDef, Mode, PartInstance, ProbeDef, Project, Quantity, Wire,
};
use std::collections::BTreeMap;

pub struct BenchBuilder<'a> {
    pub project: Project,
    lib: &'a Library,
}

impl<'a> BenchBuilder<'a> {
    pub fn new(lib: &'a Library, name: &str, mode: Mode) -> BenchBuilder<'a> {
        BenchBuilder {
            project: Project::new(name, mode),
            lib,
        }
    }

    fn id_for(&self, reference: &str) -> String {
        let base = reference.to_ascii_lowercase();
        if self.project.part(&base).is_none() {
            base
        } else {
            self.project.next_id(&format!("{base}_"))
        }
    }

    fn push(&mut self, mut p: PartInstance, props: &[(&str, PropValue)]) -> String {
        for (k, v) in props {
            p.props.insert((*k).into(), v.clone());
        }
        let id = p.id.clone();
        self.project.parts.push(p);
        id
    }

    /// Adds a breadboard; its reference is `BB<n>`.
    pub fn breadboard(&mut self, def: &str, x: f64, y: f64) -> String {
        let r = self.project.next_ref("BB");
        let id = self.id_for(&r);
        self.push(PartInstance::new(&id, &r, def).at(x, y), &[])
    }

    /// Bench position of a breadboard hole.
    pub fn hole(&self, bb: &str, hole: &str) -> (f64, f64) {
        let p = self.project.part(bb).expect("breadboard id");
        let def = self.lib.get(&p.part).expect("breadboard def");
        let lead = def
            .bench
            .leads
            .iter()
            .find(|l| l.pin == hole)
            .unwrap_or_else(|| panic!("no hole {hole}"));
        p.bench.as_ref().expect("placed").at.apply(lead.x, lead.y)
    }

    /// Places a part anywhere on the bench (boards, off-board items).
    pub fn offboard(
        &mut self,
        def: &str,
        reference: &str,
        x: f64,
        y: f64,
        props: &[(&str, PropValue)],
    ) -> String {
        let id = self.id_for(reference);
        self.push(PartInstance::new(&id, reference, def).at(x, y), props)
    }

    /// Places a two-lead part with its leads bent to land in `hole_a` and `hole_b`.
    pub fn two_lead(
        &mut self,
        def: &str,
        reference: &str,
        bb: &str,
        hole_a: &str,
        hole_b: &str,
        props: &[(&str, PropValue)],
    ) -> String {
        let d = self
            .lib
            .get(def)
            .unwrap_or_else(|| panic!("no part {def}"))
            .clone();
        let (ax, ay) = self.hole(bb, hole_a);
        let (bx, by) = self.hole(bb, hole_b);
        let mut leads = BTreeMap::new();
        leads.insert(d.pins[0].id.clone(), (0.0, 0.0));
        leads.insert(d.pins[1].id.clone(), (bx - ax, by - ay));
        let id = self.id_for(reference);
        let mut p = PartInstance::new(&id, reference, def);
        p.bench = Some(BenchPlacement {
            at: Placement::at(ax, ay),
            leads,
        });
        self.push(p, props)
    }

    /// Places a rigid part so that `pin` lands in `hole`, rotated by `rot` degrees.
    #[allow(clippy::too_many_arguments)]
    pub fn rigid(
        &mut self,
        def: &str,
        reference: &str,
        bb: &str,
        pin: &str,
        hole: &str,
        rot: f64,
        props: &[(&str, PropValue)],
    ) -> String {
        let d = self
            .lib
            .get(def)
            .unwrap_or_else(|| panic!("no part {def}"))
            .clone();
        let lead = d
            .bench
            .leads
            .iter()
            .find(|l| l.pin == pin)
            .unwrap_or_else(|| panic!("{def} has no lead {pin}"));
        let (hx, hy) = self.hole(bb, hole);
        let mut at = Placement::at(0.0, 0.0);
        at.rot = rot;
        let (ox, oy) = at.apply(lead.x, lead.y);
        at.x = hx - ox;
        at.y = hy - oy;
        let id = self.id_for(reference);
        let mut p = PartInstance::new(&id, reference, def);
        p.bench = Some(BenchPlacement {
            at,
            leads: BTreeMap::new(),
        });
        self.push(p, props)
    }

    /// A part that only lives in the schematic (Pro circuits).
    pub fn schematic_only(
        &mut self,
        def: &str,
        reference: &str,
        x: f64,
        y: f64,
        rot: f64,
        props: &[(&str, PropValue)],
    ) -> String {
        let id = self.id_for(reference);
        let p = PartInstance::new(&id, reference, def).sch(x, y, rot);
        self.push(p, props)
    }

    /// A jumper wire between two pins (holes are breadboard pins).
    pub fn wire(&mut self, a: (&str, &str), b: (&str, &str), color: &str) -> String {
        let id = self.project.next_id("w");
        self.project.wires.push(Wire {
            id: id.clone(),
            a: Endpoint::pin(a.0, a.1),
            b: Endpoint::pin(b.0, b.1),
            color: color.into(),
            bends: Vec::new(),
        });
        id
    }

    /// A pinned live label.
    pub fn probe(
        &mut self,
        label: &str,
        target: (&str, &str),
        q: Quantity,
        at: Option<(f64, f64)>,
    ) -> String {
        let id = self.project.next_id("probe");
        self.project.probes.push(ProbeDef {
            id: id.clone(),
            label: label.into(),
            target: Endpoint::pin(target.0, target.1),
            quantity: q,
            pinned: at,
        });
        id
    }

    /// A bench instrument with probe leads.
    pub fn instrument(
        &mut self,
        kind: &str,
        part: Option<&str>,
        probes: &[(&str, (&str, &str))],
        settings: serde_json::Value,
    ) -> String {
        let id = self.project.next_id(kind);
        self.project.instruments.push(InstrumentDef {
            id: id.clone(),
            kind: kind.into(),
            part: part.map(String::from),
            probes: probes
                .iter()
                .map(|(k, (p, pin))| ((*k).to_string(), Endpoint::pin(p, pin)))
                .collect(),
            settings,
        });
        id
    }

    /// Schematic wire between two absolute points (orthogonal: horizontal then vertical).
    pub fn sch_wire(&mut self, points: Vec<(f64, f64)>) -> String {
        let id = self.project.next_id("sw");
        self.project.schematic.wires.push(crate::project::SchWire {
            id: id.clone(),
            points,
        });
        id
    }

    /// Absolute schematic anchor of a part pin.
    pub fn anchor(&self, part: &str, pin: &str) -> (f64, f64) {
        let p = self.project.part(part).expect("part");
        let d = self.lib.get(&p.part).expect("def");
        Project::schematic_pins(d, p)
            .and_then(|v| {
                v.into_iter()
                    .find(|(id, _, _)| id == pin)
                    .map(|(_, x, y)| (x, y))
            })
            .unwrap_or_else(|| panic!("{part}.{pin} not placed in schematic"))
    }

    pub fn finish(mut self) -> Project {
        self.project.auto_place_schematic(self.lib);
        self.project
    }
}

/// Holes of a breadboard column strip (for tests): `strip_holes(5, true)` → a5…e5.
pub fn strip_holes(column: u32, top: bool) -> Vec<String> {
    let rows = if top {
        &breadboard::ROWS[..5]
    } else {
        &breadboard::ROWS[5..]
    };
    rows.iter().map(|r| format!("{r}{column}")).collect()
}

impl Project {
    /// Convenience: is a `Quantity` probe defined for this pin?
    pub fn has_probe(&self, part: &str, pin: &str) -> bool {
        self.probes
            .iter()
            .any(|p| p.target == Endpoint::pin(part, pin))
    }
}
