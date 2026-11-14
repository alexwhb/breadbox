//! The project document (`PRJ-1`): a folder with `project.json` + `firmware/<ref>/` sketches +
//! `analyses/`. JSON is pretty-printed with a stable key order so projects diff well and
//! autosave cheaply.

use crate::geometry::Placement;
use crate::library::Library;
use crate::part::{PartDef, PropKind, PropValue};
use bb_world::WorldConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Current file format.
pub const FORMAT: &str = "breadbox/1";

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("{path}: {msg}")]
    Io { path: PathBuf, msg: String },
    #[error("project.json: {0}")]
    Json(String),
    #[error("project.json was written by a newer Breadbox (format `{0}`)")]
    TooNew(String),
    #[error("project.json: unknown format `{0}`")]
    UnknownFormat(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Playground,
    Pro,
}

/// A part placed in the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartInstance {
    /// Stable id (never changes; wires refer to it).
    pub id: String,
    /// Reference designator shown to the user (`R1`, `U1`, `J1`).
    #[serde(rename = "ref")]
    pub reference: String,
    /// Part definition id in the library.
    pub part: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, PropValue>,
    /// Bench placement (`None`: the part only exists in the schematic, e.g. Pro imports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bench: Option<BenchPlacement>,
    /// Schematic placement (`None`: auto-placed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schematic: Option<Placement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcb: Option<Placement>,
    /// Sketch for MCU parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FirmwareRef>,
    /// World attachments (`"pot": "pot1"` for a soil sensor in a pot).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attach: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl PartInstance {
    pub fn new(id: &str, reference: &str, part: &str) -> PartInstance {
        PartInstance {
            id: id.into(),
            reference: reference.into(),
            part: part.into(),
            props: BTreeMap::new(),
            bench: None,
            schematic: None,
            pcb: None,
            firmware: None,
            attach: BTreeMap::new(),
            label: String::new(),
        }
    }
    pub fn at(mut self, x: f64, y: f64) -> Self {
        self.bench = Some(BenchPlacement {
            at: Placement::at(x, y),
            leads: BTreeMap::new(),
        });
        self
    }
    pub fn rot(mut self, deg: f64) -> Self {
        if let Some(b) = self.bench.as_mut() {
            b.at.rot = deg;
        }
        self
    }
    pub fn sch(mut self, x: f64, y: f64, rot: f64) -> Self {
        self.schematic = Some(Placement {
            x,
            y,
            rot,
            flip: false,
        });
        self
    }
    pub fn prop(mut self, k: &str, v: impl Into<PropValue>) -> Self {
        self.props.insert(k.into(), v.into());
        self
    }
}

impl From<f64> for PropValue {
    fn from(v: f64) -> Self {
        PropValue::Num(v)
    }
}
impl From<&str> for PropValue {
    fn from(v: &str) -> Self {
        PropValue::Text(v.into())
    }
}
impl From<bool> for PropValue {
    fn from(v: bool) -> Self {
        PropValue::Bool(v)
    }
}

/// Bench placement plus optional bent-lead positions (local coordinates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchPlacement {
    #[serde(flatten)]
    pub at: Placement,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub leads: BTreeMap<String, (f64, f64)>,
}

/// Firmware of an MCU part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirmwareRef {
    /// Sketch folder relative to the project folder (`firmware/U1/garden-node`).
    pub sketch: String,
    /// Override of the board's FQBN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fqbn: Option<String>,
    /// Pre-built image (`.hex`/`.elf`) relative to the project folder, used when no toolchain
    /// is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// A wire end: a part pin (breadboard holes are breadboard pins) or a free point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Endpoint {
    Pin { part: String, pin: String },
    Free { x: f64, y: f64 },
}

impl Endpoint {
    pub fn pin(part: &str, pin: &str) -> Endpoint {
        Endpoint::Pin {
            part: part.into(),
            pin: pin.into(),
        }
    }
    pub fn part(&self) -> Option<&str> {
        match self {
            Endpoint::Pin { part, .. } => Some(part),
            Endpoint::Free { .. } => None,
        }
    }
}

/// The ten standard dupont colours.
pub const WIRE_COLORS: [&str; 10] = [
    "red", "black", "orange", "yellow", "green", "blue", "purple", "grey", "white", "brown",
];

/// A jumper wire on the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    pub id: String,
    pub a: Endpoint,
    pub b: Endpoint,
    #[serde(default = "default_wire_color")]
    pub color: String,
    /// Bend points (bench coordinates).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bends: Vec<(f64, f64)>,
}

fn default_wire_color() -> String {
    "orange".into()
}

/// A schematic wire: an orthogonal polyline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchWire {
    pub id: String,
    pub points: Vec<(f64, f64)>,
}

/// A schematic net label (same name anywhere = same net).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetLabel {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rot: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Schematic {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<SchWire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<NetLabel>,
    /// IEC symbols instead of ANSI.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub iec: bool,
}

/// PCB layout (basic; phase 4 fills it in).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PcbLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<(f64, f64, f64, f64)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vias: Vec<serde_json::Value>,
}

/// What a probe measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    /// Net voltage to ground.
    #[default]
    Voltage,
    /// Current through the pin (into the part).
    Current,
    /// PWM duty cycle, %.
    Duty,
    Frequency,
    /// HIGH / LOW.
    Logic,
}

/// A live net label / probe (WB-6): `Night LED · D9 %`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeDef {
    pub id: String,
    pub label: String,
    pub target: Endpoint,
    #[serde(default)]
    pub quantity: Quantity,
    /// Where the floating label is pinned on the bench.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<(f64, f64)>,
}

/// A bench instrument (built-in mini scope, meter, or a plugin instrument).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentDef {
    pub id: String,
    /// `"mini_scope"`, `"dmm"`, or a plugin instrument id.
    pub kind: String,
    /// Bench part that draws it (its art / position), if placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// Probe leads: `"ch1"`, `"ch2"`, `"plus"`, `"com"` → endpoint.
    #[serde(default)]
    pub probes: BTreeMap<String, Endpoint>,
    /// Instrument settings (timebase, V/div, trigger, mode, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// Simulation settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub seed: u64,
    /// Co-simulation sync quantum (µs) — SIM-1.
    pub sync_quantum_us: f64,
    /// Speed multiplier (0 = Max).
    pub speed: f64,
    /// Seconds of simulated time the bench runs before pausing (0 = forever).
    pub stop_after_s: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            seed: 1,
            sync_quantum_us: 1.0,
            speed: 1.0,
            stop_after_s: 0.0,
        }
    }
}

/// Pro-mode extras: user directives (`.tran`, `.param`, `.model` …) and a detached netlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SpiceSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub directives: String,
    /// User-edited netlist that could not be mapped back to the circuit (PRO-2 "detached").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached: Option<String>,
}

/// A Breadbox project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub name: String,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub parts: Vec<PartInstance>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    #[serde(default)]
    pub schematic: Schematic,
    #[serde(default)]
    pub pcb: PcbLayout,
    #[serde(default)]
    pub world: WorldConfig,
    #[serde(default)]
    pub probes: Vec<ProbeDef>,
    #[serde(default)]
    pub instruments: Vec<InstrumentDef>,
    /// Radio settings (owned by the radio integration; kept verbatim).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub radios: serde_json::Value,
    #[serde(default)]
    pub spice: SpiceSettings,
    #[serde(default)]
    pub settings: Settings,
    /// Unknown keys from newer minor versions are preserved.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Project {
    pub fn new(name: &str, mode: Mode) -> Project {
        Project {
            format: FORMAT.into(),
            name: name.into(),
            mode,
            description: String::new(),
            parts: Vec::new(),
            wires: Vec::new(),
            schematic: Schematic::default(),
            pcb: PcbLayout::default(),
            world: WorldConfig::default(),
            probes: Vec::new(),
            instruments: Vec::new(),
            radios: serde_json::Value::Null,
            spice: SpiceSettings::default(),
            settings: Settings::default(),
            extra: BTreeMap::new(),
        }
    }

    // ------------------------------------------------------------------ (de)serialisation

    /// Parses `project.json` text, migrating older formats.
    pub fn from_json(text: &str) -> Result<Project, ProjectError> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| ProjectError::Json(e.to_string()))?;
        let v = migrate(v)?;
        serde_json::from_value(v).map_err(|e| ProjectError::Json(e.to_string()))
    }

    /// Pretty JSON with stable key order.
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    /// Loads `<dir>/project.json`.
    pub fn load(dir: impl AsRef<Path>) -> Result<Project, ProjectError> {
        let path = dir.as_ref().join("project.json");
        let text = std::fs::read_to_string(&path).map_err(|e| ProjectError::Io {
            path: path.clone(),
            msg: e.to_string(),
        })?;
        Project::from_json(&text)
    }

    /// Writes `<dir>/project.json` atomically (temp file + rename) — safe for autosave.
    pub fn save(&self, dir: impl AsRef<Path>) -> Result<(), ProjectError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| ProjectError::Io {
            path: dir.into(),
            msg: e.to_string(),
        })?;
        let path = dir.join("project.json");
        let tmp = dir.join(".project.json.tmp");
        std::fs::write(&tmp, self.to_json()).map_err(|e| ProjectError::Io {
            path: tmp.clone(),
            msg: e.to_string(),
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| ProjectError::Io {
            path,
            msg: e.to_string(),
        })
    }

    // ------------------------------------------------------------------ lookups

    pub fn part(&self, id: &str) -> Option<&PartInstance> {
        self.parts.iter().find(|p| p.id == id)
    }
    pub fn part_mut(&mut self, id: &str) -> Option<&mut PartInstance> {
        self.parts.iter_mut().find(|p| p.id == id)
    }
    pub fn part_by_ref(&self, reference: &str) -> Option<&PartInstance> {
        self.parts
            .iter()
            .find(|p| p.reference.eq_ignore_ascii_case(reference))
    }
    pub fn wire(&self, id: &str) -> Option<&Wire> {
        self.wires.iter().find(|w| w.id == id)
    }

    /// Next free designator with a prefix (`R` → `R3`).
    pub fn next_ref(&self, prefix: &str) -> String {
        let mut n = 1;
        loop {
            let r = format!("{prefix}{n}");
            if self.part_by_ref(&r).is_none() {
                return r;
            }
            n += 1;
        }
    }

    /// Next free object id with a prefix (`w` → `w7`), unique across parts, wires, schematic
    /// objects, probes and instruments.
    pub fn next_id(&self, prefix: &str) -> String {
        let used = |id: &str| {
            self.parts.iter().any(|p| p.id == id)
                || self.wires.iter().any(|w| w.id == id)
                || self.schematic.wires.iter().any(|w| w.id == id)
                || self.schematic.labels.iter().any(|w| w.id == id)
                || self.probes.iter().any(|w| w.id == id)
                || self.instruments.iter().any(|w| w.id == id)
        };
        let mut n = 1;
        loop {
            let id = format!("{prefix}{n}");
            if !used(&id) {
                return id;
            }
            n += 1;
        }
    }

    /// Definition of a placed part.
    pub fn def(&self, lib: &Library, part: &PartInstance) -> Option<Arc<PartDef>> {
        lib.get(&part.part).cloned()
    }

    /// Effective property value: the instance's value, the definition default, or a number
    /// implied by the selected choice (`battery kind=aa4` → `emf`).
    pub fn prop(def: &PartDef, part: &PartInstance, key: &str) -> Option<PropValue> {
        if let Some(v) = part.props.get(key) {
            return Some(v.clone());
        }
        if let Some(p) = def.prop(key) {
            return Some(p.default_value());
        }
        for p in &def.props {
            if let PropKind::Choice { default, options } = &p.kind {
                let sel = part
                    .props
                    .get(&p.key)
                    .map(|v| v.as_text())
                    .unwrap_or_else(|| default.clone());
                if let Some(o) = options.iter().find(|o| o.value == sel)
                    && let Some(v) = o.sets.get(key)
                {
                    return Some(PropValue::Num(*v));
                }
            }
        }
        None
    }

    /// Numeric property shorthand.
    pub fn prop_f64(def: &PartDef, part: &PartInstance, key: &str) -> Option<f64> {
        Project::prop(def, part, key).and_then(|v| v.as_f64())
    }

    /// Absolute bench positions of a part's leads (`None` if not on the bench).
    pub fn bench_leads(def: &PartDef, part: &PartInstance) -> Option<Vec<(String, f64, f64)>> {
        let b = part.bench.as_ref()?;
        if def.bench.hidden {
            return None;
        }
        Some(
            def.bench
                .leads
                .iter()
                .map(|l| {
                    let (lx, ly) = b.leads.get(&l.pin).copied().unwrap_or((l.x, l.y));
                    let (x, y) = b.at.apply(lx, ly);
                    (l.pin.clone(), x, y)
                })
                .collect(),
        )
    }

    /// Absolute schematic anchor positions of a part's pins (`None` if not placed).
    pub fn schematic_pins(def: &PartDef, part: &PartInstance) -> Option<Vec<(String, f64, f64)>> {
        let at = part.schematic.as_ref()?;
        Some(
            def.schematic
                .pins
                .iter()
                .map(|s| {
                    let (x, y) = at.apply(s.x, s.y);
                    (s.pin.clone(), x, y)
                })
                .collect(),
        )
    }

    /// Bench position of a part's origin in metres (for the world).
    pub fn world_pos(part: &PartInstance) -> Option<bb_core::Pos> {
        part.bench
            .as_ref()
            .map(|b| bb_core::Pos::new(b.at.x * 0.00254, b.at.y * 0.00254))
    }

    /// Places every part that has no schematic placement on a grid to the right of the
    /// placed ones (WB-7 auto-placement); returns how many were placed.
    pub fn auto_place_schematic(&mut self, lib: &Library) -> usize {
        let mut max_x: f64 = 0.0;
        for p in &self.parts {
            if let Some(s) = &p.schematic {
                max_x = max_x.max(s.x);
            }
        }
        let start = if max_x > 0.0 { max_x + 20.0 } else { 10.0 };
        let mut n = 0;
        let (mut col, mut row) = (0, 0);
        for p in self.parts.iter_mut() {
            if p.schematic.is_some() {
                continue;
            }
            let Some(def) = lib.get(&p.part) else {
                continue;
            };
            if def.schematic.symbol == "none" {
                continue;
            }
            let big = def.schematic.size.1 > 10.0;
            let step_y = if big { def.schematic.size.1 + 8.0 } else { 12.0 };
            p.schematic = Some(Placement::at(start + col as f64 * 16.0, 10.0 + row as f64 * step_y));
            row += 1;
            if row >= 6 || big {
                row = 0;
                col += 1;
            }
            n += 1;
        }
        n
    }

    /// Consistency problems (unknown parts, dangling references, duplicate ids).
    pub fn check(&self, lib: &Library) -> Vec<String> {
        let mut out = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for p in &self.parts {
            if !ids.insert(p.id.as_str()) {
                out.push(format!("duplicate part id `{}`", p.id));
            }
            match lib.get(&p.part) {
                None => out.push(format!("{}: unknown part `{}`", p.reference, p.part)),
                Some(def) => {
                    for (k, v) in &p.props {
                        match def.prop(k) {
                            None => out.push(format!("{}: unknown property `{k}`", p.reference)),
                            Some(pd) => {
                                if let Err(e) = pd.coerce(v) {
                                    out.push(format!("{}: {e}", p.reference));
                                }
                            }
                        }
                    }
                }
            }
        }
        let pin_ok = |e: &Endpoint| match e {
            Endpoint::Pin { part, pin } => self
                .part(part)
                .and_then(|p| lib.get(&p.part))
                .is_some_and(|d| d.pin(pin).is_some()),
            Endpoint::Free { .. } => true,
        };
        for w in &self.wires {
            for e in [&w.a, &w.b] {
                if !pin_ok(e) {
                    out.push(format!("wire {} ends on an unknown pin {:?}", w.id, e));
                }
            }
        }
        for p in &self.probes {
            if !pin_ok(&p.target) {
                out.push(format!("probe {} targets an unknown pin", p.label));
            }
        }
        out
    }
}

/// Upgrades older `project.json` layouts to [`FORMAT`] (the migration hook).
///
/// * `breadbox/0` (pre-release): parts and wires lived under `"circuit"`, wire endpoints were
///   `"part.pin"` strings and the world was `"environment"`.
pub fn migrate(mut v: serde_json::Value) -> Result<serde_json::Value, ProjectError> {
    let format = v
        .get("format")
        .and_then(|f| f.as_str())
        .unwrap_or("breadbox/0")
        .to_string();
    match format.as_str() {
        FORMAT => Ok(v),
        "breadbox/0" => {
            let obj = v
                .as_object_mut()
                .ok_or_else(|| ProjectError::Json("top level must be an object".into()))?;
            if let Some(serde_json::Value::Object(mut c)) = obj.remove("circuit") {
                for k in ["parts", "wires"] {
                    if let Some(x) = c.remove(k) {
                        obj.insert(k.into(), x);
                    }
                }
            }
            if let Some(env) = obj.remove("environment") {
                obj.insert("world".into(), env);
            }
            if let Some(serde_json::Value::Array(wires)) = obj.get_mut("wires") {
                for w in wires.iter_mut() {
                    for end in ["a", "b"] {
                        if let Some(serde_json::Value::String(s)) = w.get(end).cloned()
                            && let Some((part, pin)) = s.split_once('.')
                        {
                            w[end] = serde_json::json!({"part": part, "pin": pin});
                        }
                    }
                }
            }
            obj.insert("format".into(), FORMAT.into());
            Ok(v)
        }
        f if f.starts_with("breadbox/") => {
            let n: u32 = f["breadbox/".len()..].parse().unwrap_or(u32::MAX);
            if n > 1 {
                Err(ProjectError::TooNew(f.into()))
            } else {
                Err(ProjectError::UnknownFormat(f.into()))
            }
        }
        f => Err(ProjectError::UnknownFormat(f.into())),
    }
}
