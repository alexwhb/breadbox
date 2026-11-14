//! The parts library: built-in definitions (JSON in `crates/bb-circuit/parts/`) plus parts
//! added at run time (plugins), with geometry derived from layout shorthands and validated.

use crate::breadboard;
use crate::part::*;
use crate::template;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LibraryError {
    #[error("part file `{source_name}`: {msg}")]
    Json { source_name: String, msg: String },
    #[error("part `{part}`: {msg}")]
    Invalid { part: String, msg: String },
}

const BUILTIN: [(&str, &str); 7] = [
    ("boards.json", include_str!("../parts/boards.json")),
    ("blocks.json", include_str!("../parts/blocks.json")),
    (
        "light_sound.json",
        include_str!("../parts/light_sound.json"),
    ),
    ("sensors.json", include_str!("../parts/sensors.json")),
    ("semis.json", include_str!("../parts/semis.json")),
    (
        "displays_motors.json",
        include_str!("../parts/displays_motors.json"),
    ),
    ("misc.json", include_str!("../parts/misc.json")),
];

/// PART-8: the Simple library in palette order.
pub const SIMPLE_ORDER: [&str; 25] = [
    "uno",
    "esp32_devkit",
    "pico",
    "breadboard_830",
    "jumper_wire",
    "resistor",
    "capacitor",
    "push_button",
    "potentiometer",
    "battery_pack",
    "usb_power",
    "led",
    "rgb_led",
    "buzzer_passive",
    "ldr",
    "capsoil",
    "tmp36",
    "dht22",
    "servo",
    "dc_motor",
    "lcd_1602",
    "esp01",
    "hm10",
    "npn",
    "diode",
];

#[derive(Debug, Clone, Default)]
pub struct Library {
    parts: Vec<Arc<PartDef>>,
    index: HashMap<String, usize>,
}

static BUILTIN_LIB: OnceLock<Arc<Library>> = OnceLock::new();

impl Library {
    /// The built-in library (parsed once).
    pub fn builtin() -> Arc<Library> {
        BUILTIN_LIB
            .get_or_init(|| {
                let mut lib = Library::default();
                for (name, text) in BUILTIN {
                    if let Err(e) = lib.add_json(text, name, "builtin") {
                        // built-in data is covered by tests; never panic in release
                        log::error!("{e}");
                    }
                }
                Arc::new(lib)
            })
            .clone()
    }

    /// A copy of the built-in library to extend with plugin parts.
    pub fn with_builtin() -> Library {
        (*Library::builtin()).clone()
    }

    /// Adds every part in a JSON array (or a single JSON object). Returns their ids.
    pub fn add_json(
        &mut self,
        json: &str,
        source_name: &str,
        source: &str,
    ) -> Result<Vec<String>, LibraryError> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| LibraryError::Json {
            source_name: source_name.into(),
            msg: e.to_string(),
        })?;
        let items = match v {
            serde_json::Value::Array(a) => a,
            o => vec![o],
        };
        let mut ids = Vec::new();
        for item in items {
            let mut def: PartDef =
                serde_json::from_value(item).map_err(|e| LibraryError::Json {
                    source_name: source_name.into(),
                    msg: e.to_string(),
                })?;
            if def.source.is_empty() {
                def.source = source.into();
            }
            ids.push(def.id.clone());
            self.add(def)?;
        }
        Ok(ids)
    }

    /// Finalizes (derives geometry), validates and adds a definition (replacing one with the
    /// same id).
    pub fn add(&mut self, mut def: PartDef) -> Result<(), LibraryError> {
        finalize(&mut def).map_err(|msg| LibraryError::Invalid {
            part: def.id.clone(),
            msg,
        })?;
        validate(&def).map_err(|msg| LibraryError::Invalid {
            part: def.id.clone(),
            msg,
        })?;
        let arc = Arc::new(def);
        if let Some(&i) = self.index.get(&arc.id) {
            self.parts[i] = arc;
        } else {
            self.index.insert(arc.id.clone(), self.parts.len());
            self.parts.push(arc);
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Arc<PartDef>> {
        self.index.get(id).map(|&i| &self.parts[i])
    }

    pub fn parts(&self) -> impl Iterator<Item = &Arc<PartDef>> {
        self.parts.iter()
    }

    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The 25-part Simple library in palette order.
    pub fn simple(&self) -> Vec<Arc<PartDef>> {
        SIMPLE_ORDER
            .iter()
            .filter_map(|id| self.get(id).cloned())
            .collect()
    }

    /// Palette sections: (category, parts) in category order; `simple` restricts to PART-8.
    pub fn categories(&self, simple: bool) -> Vec<(Category, Vec<Arc<PartDef>>)> {
        let pool: Vec<Arc<PartDef>> = if simple {
            self.simple()
        } else {
            self.parts
                .iter()
                .filter(|p| !p.has_tag("symbol"))
                .cloned()
                .collect()
        };
        Category::ALL
            .iter()
            .filter_map(|&c| {
                let v: Vec<_> = pool.iter().filter(|p| p.category == c).cloned().collect();
                (!v.is_empty()).then_some((c, v))
            })
            .collect()
    }

    /// Case-insensitive search over name, part number, id, description, tags and SPICE model
    /// names; results ranked name matches first.
    pub fn search(&self, query: &str, simple: bool) -> Vec<Arc<PartDef>> {
        let q = query.trim().to_lowercase();
        let pool: Vec<Arc<PartDef>> = if simple {
            self.simple()
        } else {
            self.parts.clone()
        };
        if q.is_empty() {
            return pool;
        }
        let mut scored: Vec<(u32, Arc<PartDef>)> = pool
            .into_iter()
            .filter_map(|p| {
                let name = p.display_name(simple).to_lowercase();
                let score = if name.starts_with(&q) {
                    0
                } else if name.contains(&q) || p.part_number.to_lowercase().contains(&q) {
                    1
                } else if p.id.contains(&q) || p.tags.iter().any(|t| t.contains(&q)) {
                    2
                } else if p.description.to_lowercase().contains(&q)
                    || p.model
                        .library
                        .iter()
                        .chain(p.model.spice.iter())
                        .any(|l| l.to_lowercase().contains(&q))
                {
                    3
                } else {
                    return None;
                };
                Some((score, p))
            })
            .collect();
        scored.sort_by_key(|(s, _)| *s);
        scored.into_iter().map(|(_, p)| p).collect()
    }

    /// All definitions as a JSON array (the schema plugin authors follow).
    pub fn to_json(&self) -> String {
        let defs: Vec<&PartDef> = self.parts.iter().map(|p| p.as_ref()).collect();
        serde_json::to_string_pretty(&defs).unwrap_or_default()
    }
}

/// Derives pins/leads/sizes/symbol anchors from shorthands.
pub fn finalize(def: &mut PartDef) -> Result<(), String> {
    if def.pins.is_empty() && !def.pin_specs.is_empty() {
        def.pins = def
            .pin_specs
            .iter()
            .map(|s| s.resolve())
            .collect::<Result<_, _>>()?;
    }
    def.pin_specs.clear();
    match def.bench.layout.clone() {
        Some(Layout::Breadboard {
            columns,
            rail_holes,
        }) => {
            if columns == 0 || rail_holes == 0 {
                return Err("breadboard needs columns and rail holes".into());
            }
            let (pins, leads) = breadboard::holes(columns, rail_holes);
            def.pins = pins;
            def.bench.leads = leads;
            def.bench.size = breadboard::size(columns);
        }
        Some(Layout::Board(id)) => {
            let (pins, leads, size, bonds) = board_layout(&id)?;
            def.pins = pins;
            def.bench.leads = leads;
            def.bench.size = size;
            def.bonds.extend(bonds);
        }
        Some(layout) if def.bench.leads.is_empty() => {
            let ids: Vec<String> = def.pins.iter().map(|p| p.id.clone()).collect();
            let (leads, size) = layout_leads(&layout, &ids)?;
            def.bench.leads = leads;
            if def.bench.size == (0.0, 0.0) {
                def.bench.size = size;
            }
        }
        _ => {}
    }
    if def.bench.size == (0.0, 0.0) && !def.bench.leads.is_empty() {
        let maxx = def.bench.leads.iter().map(|l| l.x).fold(0.0, f64::max);
        let maxy = def.bench.leads.iter().map(|l| l.y).fold(0.0, f64::max);
        def.bench.size = (maxx + 1.0, maxy + 1.0);
    }
    if let Some(m) = &def.model.mcu {
        let host = m.core == "host";
        for p in def.pins.iter_mut() {
            if host && p.mcu.is_none() && p.role == PinRole::Io {
                p.mcu = Some(p.id.clone());
            }
        }
    }
    crate::symbols::finalize(def);
    Ok(())
}

fn layout_leads(layout: &Layout, ids: &[String]) -> Result<(Vec<Lead>, (f64, f64)), String> {
    let lead = |pin: &str, x: f64, y: f64| Lead {
        pin: pin.into(),
        x,
        y,
    };
    Ok(match layout {
        Layout::Axial(span) => {
            if ids.len() < 2 {
                return Err("axial layout needs two pins".into());
            }
            (
                vec![lead(&ids[0], 0.0, 0.0), lead(&ids[1], *span, 0.0)],
                (span + 1.0, 1.0),
            )
        }
        Layout::Row => (
            ids.iter()
                .enumerate()
                .map(|(i, id)| lead(id, i as f64, 0.0))
                .collect(),
            (ids.len() as f64, 1.0),
        ),
        Layout::Dip(spacing) => {
            let n = ids.len();
            if !n.is_multiple_of(2) {
                return Err("DIP layout needs an even pin count".into());
            }
            let half = n / 2;
            let mut v = Vec::new();
            for (i, id) in ids.iter().enumerate() {
                if i < half {
                    v.push(lead(id, i as f64, *spacing));
                } else {
                    v.push(lead(id, (n - 1 - i) as f64, 0.0));
                }
            }
            (v, (half as f64, spacing + 1.0))
        }
        Layout::Rows { spacing, rows } => {
            let mut v = Vec::new();
            for (r, row) in rows.iter().enumerate() {
                for (i, id) in row.iter().enumerate() {
                    if !ids.contains(id) {
                        return Err(format!("layout row names unknown pin `{id}`"));
                    }
                    v.push(lead(id, i as f64, r as f64 * spacing));
                }
            }
            let w = rows.iter().map(|r| r.len()).max().unwrap_or(0) as f64;
            (v, (w, spacing * (rows.len().max(1) - 1) as f64 + 1.0))
        }
        Layout::Board(_) | Layout::Breadboard { .. } => (Vec::new(), (0.0, 0.0)),
    })
}

type BoardGeom = (Vec<PinDef>, Vec<Lead>, (f64, f64), Vec<Vec<String>>);

/// Header pins of an AVR board from `bb_firmware::Board` with bench coordinates.
pub fn board_layout(id: &str) -> Result<BoardGeom, String> {
    let board = bb_firmware::Board::by_id(id).ok_or_else(|| format!("unknown board `{id}`"))?;
    // header name → (x of first label, y, step, vertical pairs?)
    let place = |name: &str| -> Option<(f64, f64, f64, bool)> {
        Some(match (id, name) {
            ("uno", "DIGITAL (PWM~) 0-7") | ("mega", "PWM 0-7") => (19.0, 1.0, 1.0, false),
            ("uno", "DIGITAL (PWM~) 8-13") | ("mega", "PWM 8-13") => (8.4, 1.0, 1.0, false),
            ("uno", "POWER") | ("mega", "POWER") => (10.0, 20.0, 1.0, false),
            ("uno", "ANALOG IN") | ("mega", "ANALOG IN 0-7") => (19.0, 20.0, 1.0, false),
            ("mega", "ANALOG IN 8-15") => (28.0, 20.0, 1.0, false),
            ("mega", "COMMUNICATION") => (28.0, 1.0, 1.0, false),
            ("mega", "DIGITAL 22-53") => (37.0, 2.0, 1.0, true),
            ("nano", "J2 (left)") => (0.0, 0.0, 1.0, false),
            ("nano", "J1 (right)") => (0.0, 6.0, 1.0, false),
            _ => return None,
        })
    };
    let size = match id {
        "uno" => (27.0, 21.0),
        "mega" => (40.0, 21.0),
        _ => (15.0, 7.0),
    };
    let mut pins: Vec<PinDef> = Vec::new();
    let mut leads = Vec::new();
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for h in &board.headers {
        let Some((x0, y0, step, pairs)) = place(h.name) else {
            continue;
        };
        for (i, label) in h.labels.iter().enumerate() {
            let base = match *label {
                "3.3V" | "3V3" => "3V3",
                l => l,
            };
            let n = pins
                .iter()
                .filter(|p| p.id == base || p.id.starts_with(&format!("{base}.")))
                .count();
            let pid = if n == 0 {
                base.to_string()
            } else {
                format!("{base}.{}", n + 1)
            };
            let role = match base {
                "5V" | "3V3" | "IOREF" => PinRole::PowerOut,
                "GND" => PinRole::Ground,
                "VIN" => PinRole::PowerIn,
                "AREF" | "RESET" => PinRole::Input,
                "NC" => PinRole::Nc,
                _ => PinRole::Io,
            };
            let mut p = PinDef::new(&pid, role);
            if base != pid {
                p.name = base.into();
            }
            if base == "3V3" {
                p.name = "3.3V".into();
            }
            if let Some(bp) = board.pin_by_name(base)
                && role == PinRole::Io
                && (base.starts_with('D') || base.starts_with('A'))
            {
                p.mcu = Some(bp.name.clone());
            }
            if role == PinRole::Io && p.mcu.is_none() {
                // SDA / SCL header pins: bonded to the real MCU pin below
                if let Some(bp) = board.pin_by_name(base) {
                    groups
                        .entry(format!("bus:{base}"))
                        .or_default()
                        .extend([pid.clone(), bp.name.clone()]);
                }
            }
            groups
                .entry(base.to_string())
                .or_default()
                .push(pid.clone());
            let (x, y) = if pairs {
                (x0 + (i % 2) as f64, y0 + (i / 2) as f64)
            } else {
                (x0 + i as f64 * step, y0)
            };
            leads.push(Lead {
                pin: pid.clone(),
                x,
                y,
            });
            pins.push(p);
        }
    }
    let mut bonds: Vec<Vec<String>> = groups
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(_, mut v)| {
            v.sort();
            v.dedup();
            v
        })
        .filter(|v| v.len() > 1)
        .collect();
    bonds.sort();
    Ok((pins, leads, size, bonds))
}

/// Checks pins, leads, templates, ratings and props for consistency.
pub fn validate(def: &PartDef) -> Result<(), String> {
    if def.id.is_empty() {
        return Err("empty id".into());
    }
    let mut seen = std::collections::HashSet::new();
    for p in &def.pins {
        if !seen.insert(p.id.as_str()) {
            return Err(format!("duplicate pin `{}`", p.id));
        }
        if p.id.contains(')') || p.id.contains(':') || p.id.contains(char::is_whitespace) {
            return Err(format!(
                "pin id `{}` may not contain `)`, `:` or spaces",
                p.id
            ));
        }
    }
    for l in &def.bench.leads {
        if def.pin(&l.pin).is_none() {
            return Err(format!("lead for unknown pin `{}`", l.pin));
        }
    }
    for s in &def.schematic.pins {
        if def.pin(&s.pin).is_none() {
            return Err(format!("symbol anchor for unknown pin `{}`", s.pin));
        }
    }
    for b in &def.bonds {
        for p in b {
            if def.pin(p).is_none() {
                return Err(format!("bond names unknown pin `{p}`"));
            }
        }
    }
    let mut seen_keys = std::collections::HashSet::new();
    for p in &def.props {
        if !seen_keys.insert(p.key.as_str()) {
            return Err(format!("duplicate property `{}`", p.key));
        }
        if def.pin(&p.key).is_some() {
            return Err(format!("property `{}` has the same name as a pin", p.key));
        }
    }
    // templates: every bare placeholder must be a pin or a property
    let lines = def
        .model
        .spice
        .iter()
        .chain(def.model.pro.iter().flatten())
        .chain(def.model.damaged.iter().flatten());
    let mut elements = Vec::new();
    for line in lines {
        for ph in template::placeholders(line) {
            if ph != "ref" && ph != "gnd" && def.pin(&ph).is_none() && def.prop(&ph).is_none() {
                return Err(format!("template uses unknown `$({ph})`: {line}"));
            }
        }
        elements.extend(template::element_specs(line));
        let mut rest = line.as_str();
        while let Some(i) = rest.find("$(ext:") {
            let after = &rest[i + 6..];
            let j = after.find(')').unwrap_or(after.len());
            let name = &after[..j];
            if !def.model.ext.iter().any(|e| e.name == name) {
                return Err(format!("template uses undeclared ext `{name}`"));
            }
            rest = &after[j.min(after.len())..];
        }
    }
    for r in &def.ratings {
        if !elements.contains(&r.element) {
            return Err(format!(
                "rating names element `{}` not in the model",
                r.element
            ));
        }
        if let ValueRef::Expr(e) = &r.limit {
            let k = e.trim().trim_start_matches("$(").trim_end_matches(')');
            if def.prop(k).is_none() {
                return Err(format!("rating limit `{e}` is not a property"));
            }
        }
    }
    if let Some(d) = &def.model.device_pins {
        for p in d {
            if def.pin(p).is_none() {
                return Err(format!("device pin `{p}` is not a pin"));
            }
        }
    }
    if let Some(m) = &def.model.mcu
        && def.pin(&m.vcc_pin).is_none()
    {
        return Err(format!("MCU supply pin `{}` is not a pin", m.vcc_pin));
    }
    for p in &def.props {
        if let PropKind::Choice { default, options } = &p.kind
            && !options.iter().any(|o| &o.value == default)
        {
            return Err(format!(
                "property `{}` default `{default}` is not an option",
                p.key
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
