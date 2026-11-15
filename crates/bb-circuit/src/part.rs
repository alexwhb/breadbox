//! Declarative part definitions (`PART-9`): one schema for built-in parts (JSON files in
//! `crates/bb-circuit/parts/`) and plugin parts.
//!
//! A [`PartDef`] says what a part *is*: pins, properties, electrical model (SPICE templates
//! and/or a behavioural device id), datasheet fields for the inspector, damage ratings, and the
//! geometry the UI needs to draw and hit-test it on the breadboard, in the schematic and on the
//! PCB. See `docs/ARCHITECTURE.md` §3a for the template language.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Palette sections (WB-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Boards,
    BuildingBlocks,
    LightSound,
    Sensors,
    Power,
    Radios,
    Displays,
    MotorsActuators,
    Semiconductors,
    Ics,
    Instruments,
    Spice,
    Plugins,
}

impl Category {
    pub const ALL: [Category; 13] = [
        Category::Boards,
        Category::BuildingBlocks,
        Category::LightSound,
        Category::Sensors,
        Category::Power,
        Category::Radios,
        Category::Displays,
        Category::MotorsActuators,
        Category::Semiconductors,
        Category::Ics,
        Category::Instruments,
        Category::Spice,
        Category::Plugins,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Category::Boards => "Boards",
            Category::BuildingBlocks => "Building blocks",
            Category::LightSound => "Light & sound",
            Category::Sensors => "Sensors",
            Category::Power => "Power",
            Category::Radios => "Radios",
            Category::Displays => "Displays",
            Category::MotorsActuators => "Motors & actuators",
            Category::Semiconductors => "Semiconductors",
            Category::Ics => "ICs",
            Category::Instruments => "Instruments",
            Category::Spice => "SPICE primitives",
            Category::Plugins => "Plugins",
        }
    }
}

/// Electrical role of a pin (used by ERC and the netlister).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PinRole {
    /// Resistor/capacitor lead, switch contact.
    #[default]
    Passive,
    /// Digital/analog input of a device (expects to be driven).
    Input,
    /// Push-pull output.
    Output,
    /// Open-drain / open-collector output (needs a pull-up).
    OpenDrain,
    /// Bidirectional (MCU GPIO, I²C SDA, 1-Wire DQ).
    Io,
    /// Analog output of a sensor.
    AnalogOut,
    /// Supply input of a device (VCC, VIN).
    PowerIn,
    /// Supply output of a source (USB 5 V, battery +, regulator OUT, board 5V/3V3).
    PowerOut,
    /// Ground reference.
    Ground,
    /// Not connected internally.
    Nc,
}

impl PinRole {
    pub fn is_power_source(self) -> bool {
        matches!(self, PinRole::PowerOut)
    }
    pub fn drives(self) -> bool {
        matches!(
            self,
            PinRole::Output | PinRole::AnalogOut | PinRole::PowerOut
        )
    }
}

/// One pin of a part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinDef {
    /// Unique id within the part (`"1"`, `"a"`, `"D9"`, `"GND.2"`).
    pub id: String,
    /// Label printed next to the pin (defaults to the id without a `.n` suffix).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub role: PinRole,
    /// Absolute maximum voltage on this pin (ERC: 5 V into a 3.3 V-only input).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_voltage: Option<f64>,
    /// ERC warns when this pin is left unconnected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// For board parts: the MCU pin this header pin is (Arduino name, `"D9"`, `"A0"`, `"GPIO4"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcu: Option<String>,
}

impl PinDef {
    pub fn new(id: &str, role: PinRole) -> PinDef {
        PinDef {
            id: id.into(),
            name: String::new(),
            role,
            max_voltage: None,
            required: false,
            mcu: None,
        }
    }
    /// Display label.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            self.id.split('.').next().unwrap_or(&self.id)
        } else {
            &self.name
        }
    }
}

/// Pin written in JSON either as a full object or as `"id"` / `"id:role"` / `"id:role:Name"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PinSpec {
    Short(String),
    Full(PinDef),
}

impl PinSpec {
    pub fn resolve(&self) -> Result<PinDef, String> {
        match self {
            PinSpec::Full(p) => Ok(p.clone()),
            PinSpec::Short(s) => {
                let mut it = s.splitn(3, ':');
                let id = it.next().unwrap_or("").trim();
                if id.is_empty() {
                    return Err(format!("empty pin spec `{s}`"));
                }
                let role = match it.next().map(str::trim) {
                    None | Some("") => PinRole::Passive,
                    Some(r) => serde_json::from_value(serde_json::Value::String(r.into()))
                        .map_err(|_| format!("unknown pin role `{r}` in `{s}`"))?,
                };
                let mut p = PinDef::new(id, role);
                if let Some(n) = it.next() {
                    p.name = n.trim().into();
                }
                Ok(p)
            }
        }
    }
}

/// Standard resistor/capacitor value series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ESeries {
    E6,
    E12,
    E24,
    E96,
}

/// One option of a choice property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOpt {
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Extra numeric values implied by this choice (e.g. a battery chemistry's EMF).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sets: BTreeMap<String, f64>,
}

/// Property type, default and constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PropKind {
    Number {
        default: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        /// Snap to an E-series in the UI picker.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        series: Option<ESeries>,
    },
    Choice {
        default: String,
        options: Vec<ChoiceOpt>,
    },
    Bool {
        default: bool,
    },
    Text {
        default: String,
    },
}

/// A user-editable property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropDef {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(flatten)]
    pub kind: PropKind,
    /// Can change while the simulation runs without rebuilding the bench.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub live: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
}

/// A property value on an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    Num(f64),
    Bool(bool),
    Text(String),
}

impl PropValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PropValue::Num(v) => Some(*v),
            PropValue::Bool(b) => Some(*b as u8 as f64),
            PropValue::Text(t) => bb_core::si::parse(t).ok(),
        }
    }
    pub fn as_text(&self) -> String {
        match self {
            PropValue::Num(v) => bb_spice::expr::fmt_g(*v),
            PropValue::Bool(b) => b.to_string(),
            PropValue::Text(t) => t.clone(),
        }
    }
}

impl PropDef {
    pub fn default_value(&self) -> PropValue {
        match &self.kind {
            PropKind::Number { default, .. } => PropValue::Num(*default),
            PropKind::Choice { default, .. } => PropValue::Text(default.clone()),
            PropKind::Bool { default } => PropValue::Bool(*default),
            PropKind::Text { default } => PropValue::Text(default.clone()),
        }
    }
    /// Validates/clamps a value for this property.
    pub fn coerce(&self, v: &PropValue) -> Result<PropValue, String> {
        match &self.kind {
            PropKind::Number { min, max, .. } => {
                let x = v
                    .as_f64()
                    .ok_or_else(|| format!("{}: `{}` is not a number", self.label, v.as_text()))?;
                if !x.is_finite() {
                    return Err(format!("{}: value must be finite", self.label));
                }
                let x = min.map_or(x, |m| x.max(m));
                let x = max.map_or(x, |m| x.min(m));
                Ok(PropValue::Num(x))
            }
            PropKind::Choice { options, .. } => {
                let t = v.as_text();
                options
                    .iter()
                    .find(|o| o.value.eq_ignore_ascii_case(&t))
                    .map(|o| PropValue::Text(o.value.clone()))
                    .ok_or_else(|| format!("{}: `{t}` is not one of the options", self.label))
            }
            PropKind::Bool { .. } => match v {
                PropValue::Bool(b) => Ok(PropValue::Bool(*b)),
                PropValue::Num(n) => Ok(PropValue::Bool(*n != 0.0)),
                PropValue::Text(t) => Ok(PropValue::Bool(matches!(
                    t.to_ascii_lowercase().as_str(),
                    "true" | "1" | "yes" | "on"
                ))),
            },
            PropKind::Text { .. } => Ok(PropValue::Text(v.as_text())),
        }
    }
}

/// An analog external variable a part's SPICE template reads (`$(ext:lux)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtDef {
    pub name: String,
    /// Initial value: a number or a property reference (`"$(resistance)"`).
    pub default: ValueRef,
}

/// MCU model of a board part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McuDef {
    /// Board id: `"uno"`, `"nano"`, `"mega"` (cycle-accurate AVR) or `"esp32"`, `"pico"`
    /// (API-level host emulation).
    pub board: String,
    /// `"avr"` or `"host"`.
    pub core: String,
    /// Pin whose net is the MCU's supply (output drivers hang between it and ground).
    pub vcc_pin: String,
    /// Display string: `"ATmega328P @ 16 MHz"`.
    #[serde(default)]
    pub label: String,
}

/// Electrical model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelDef {
    /// SPICE element templates used on the bench and in Pro mode (see ARCHITECTURE §3a).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spice: Vec<String>,
    /// `.model` / `.subckt` cards this part needs (shared, de-duplicated by name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library: Vec<String>,
    /// Alternative templates for Pro/export netlists (e.g. a `CAPSOIL` subcircuit instance for a
    /// behavioural sensor). Default: `spice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pro: Option<Vec<String>>,
    /// Templates used while the part is damaged (default: the part disappears = open circuit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damaged: Option<Vec<String>>,
    /// Behavioural device id implemented by bb-sim (or a plugin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behaviour: Option<String>,
    /// Pins the behavioural device drives/reads through co-simulation ports. Default: every pin
    /// not used by `spice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_pins: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcu: Option<McuDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ext: Vec<ExtDef>,
}

/// A datasheet-style inspector line: `label` + `value` template (`"τ ≈ $(tau) s"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectorField {
    pub label: String,
    pub value: String,
}

/// What a rating limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RatingKind {
    /// |current| through the element.
    Current,
    /// Power dissipated in the element (V·I).
    Power,
    /// |voltage| across the element.
    Voltage,
    /// Voltage across the element in the reverse direction (electrolytics: −V).
    ReverseVoltage,
}

/// A number or a property reference (`"$(power)"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueRef {
    Num(f64),
    Expr(String),
}

/// One damage rating (WB-13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rating {
    /// Local element name in the SPICE template (`"D"`, `"R"`, `"C"`).
    pub element: String,
    pub kind: RatingKind,
    pub limit: ValueRef,
    /// Thermal time constant of the overload filter (s).
    #[serde(default = "default_tau")]
    pub tau: f64,
    /// Filtered value / limit above which the part is destroyed (stressed between 1 and this).
    #[serde(default = "default_factor")]
    pub destroy_at: f64,
    /// Text for the damage event ("LED burnt out — 120 mA through D1").
    #[serde(default)]
    pub message: String,
    /// What happens: `"open"` (default), `"short"`, `"pop"` (electrolytic), `"smoke"`.
    #[serde(default)]
    pub effect: String,
}

fn default_tau() -> f64 {
    0.05
}
fn default_factor() -> f64 {
    2.0
}

/// Breadboard art + lead geometry. Units: 0.1" (one breadboard pitch), origin top-left of the
/// body, y down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BenchGeom {
    /// Art id the renderer draws (`"resistor_axial"`, `"led_5mm"`, `"uno_r3"`, …).
    #[serde(default)]
    pub art: String,
    /// Static art parameters (colours, labels).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub art_params: BTreeMap<String, String>,
    /// Body size (w, h) in 0.1" units.
    #[serde(default)]
    pub size: (f64, f64),
    /// Lead positions (filled from `layout` when empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leads: Vec<Lead>,
    /// Shorthand that generates `leads`/`size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    /// Pins are female sockets other leads can plug into (Arduino UNO/Mega headers).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sockets: bool,
    /// Leads can be bent (axial parts): instances may override lead positions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flexible: bool,
    /// Not plugged into anything (pot of soil, lamp, instruments, USB supply brick).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off_board: bool,
    /// Not drawn on the bench at all (schematic-only symbols such as GND / VCC).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// One lead/pin position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lead {
    pub pin: String,
    pub x: f64,
    pub y: f64,
}

/// Geometry shorthands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Two leads `span` pitches apart (resistor, diode, axial cap).
    Axial(f64),
    /// All pins in one row at 0.1" pitch (TO-92, radial LEDs, sensor modules).
    Row,
    /// DIP package with `row spacing` (3 = 0.3"); pin 1 bottom-left, counter-clockwise.
    Dip(f64),
    /// Two rows of pins `spacing` apart: first list along y=0, second along y=spacing, both
    /// left to right (dev boards, modules).
    Rows {
        spacing: f64,
        rows: Vec<Vec<String>>,
    },
    /// Board headers generated from a board id (`"uno"`, `"mega"`).
    Board(String),
    /// A breadboard (`columns`, rail holes per rail).
    Breadboard { columns: u32, rail_holes: u32 },
}

/// Schematic symbol geometry. Units: 0.1" grid; pin anchors relative to the symbol origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SchGeom {
    /// Symbol id (`"resistor"`, `"led"`, `"npn"`, `"opamp"`, `"box"`, `"ground"`, …).
    #[serde(default)]
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<SymPin>,
    /// Body size (w, h) (box symbols are sized from their pins).
    #[serde(default)]
    pub size: (f64, f64),
}

/// Direction a wire leaves a pin anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PinDir {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymPin {
    pub pin: String,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub dir: PinDir,
}

/// PCB footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PcbGeom {
    /// KiCad-style footprint name (`"Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P10.16mm_Horizontal"`).
    #[serde(default)]
    pub footprint: String,
    /// Common SMD alternative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smd: Option<String>,
}

/// A complete part definition (built-in or plugin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartDef {
    pub id: String,
    pub name: String,
    /// Manufacturer part number shown in Advanced mode (`"GL5528"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub part_number: String,
    pub category: Category,
    /// Member of the 25-part Simple library (PART-8).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub simple: bool,
    /// Tile label in Simple mode ("Knob (pot)").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simple_name: Option<String>,
    /// One-line hover description.
    pub description: String,
    /// Palette icon id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Reference designator prefix (`"R"`, `"D"`, `"U"`, `"J"`, `"LDR"`).
    pub ref_prefix: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<PropDef>,
    #[serde(default, rename = "pins", skip_serializing)]
    pub pin_specs: Vec<PinSpec>,
    /// Resolved pins (from `pins` in JSON).
    #[serde(default, rename = "pin_defs", skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<PinDef>,
    /// Groups of pins connected inside the part (all GND pins of a board).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bonds: Vec<Vec<String>>,
    #[serde(default)]
    pub model: ModelDef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inspector: Vec<InspectorField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ratings: Vec<Rating>,
    #[serde(default)]
    pub bench: BenchGeom,
    #[serde(default)]
    pub schematic: SchGeom,
    #[serde(default)]
    pub pcb: PcbGeom,
    /// Filter tags: `"spice"`, `"firmware"`, `"3d"`, `"world"`, `"tool"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Where the definition came from (`"builtin"`, or a plugin id).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

impl PartDef {
    pub fn pin(&self, id: &str) -> Option<&PinDef> {
        self.pins.iter().find(|p| p.id == id)
    }
    pub fn pin_index(&self, id: &str) -> Option<usize> {
        self.pins.iter().position(|p| p.id == id)
    }
    pub fn prop(&self, key: &str) -> Option<&PropDef> {
        self.props.iter().find(|p| p.key == key)
    }
    pub fn is_mcu(&self) -> bool {
        self.model.mcu.is_some()
    }
    pub fn is_breadboard(&self) -> bool {
        matches!(self.bench.layout, Some(Layout::Breadboard { .. }))
    }
    pub fn has_tag(&self, t: &str) -> bool {
        self.tags.iter().any(|x| x == t)
    }
    pub fn display_name(&self, simple: bool) -> &str {
        if simple {
            self.simple_name.as_deref().unwrap_or(&self.name)
        } else {
            &self.name
        }
    }
    /// Pins referenced by SPICE templates (bench model).
    pub fn spice_pins(&self) -> Vec<String> {
        let mut out = Vec::new();
        for line in &self.model.spice {
            for tok in crate::template::placeholders(line) {
                if self.pin(&tok).is_some() && !out.contains(&tok) {
                    out.push(tok);
                }
            }
        }
        out
    }
    /// Pins the behavioural device owns through ports.
    pub fn device_pins(&self) -> Vec<String> {
        if self.model.behaviour.is_none() {
            return Vec::new();
        }
        if let Some(d) = &self.model.device_pins {
            return d.clone();
        }
        let sp = self.spice_pins();
        self.pins
            .iter()
            .filter(|p| !sp.contains(&p.id) && p.role != PinRole::Nc)
            .map(|p| p.id.clone())
            .collect()
    }
}
