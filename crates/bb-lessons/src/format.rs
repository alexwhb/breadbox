//! The lesson file format (`lessons/<track>/<lesson>.toml`, LES-4). See
//! `docs/LESSON-FORMAT.md` for the reference with examples.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A track (`lessons/<track>/track.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackDef {
    pub id: String,
    pub title: String,
    /// `Track 1 · Electricity you can see`.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub description: String,
}

/// Where a lesson's start circuit comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BenchSpec {
    /// A bb-circuit project file next to the lesson (`03-ohms-law-led.bench.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// An example project (`examples/<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    /// Firmware for MCU parts: part id → image (`.hex`/`.elf`, lesson-relative or
    /// `examples/fixtures/…`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub firmware: BTreeMap<String, String>,
    /// World variable overrides at the start.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub world: BTreeMap<String, f64>,
    /// Bench seconds to run before the first check (settling).
    #[serde(default)]
    pub warmup: f64,
}

/// A text callout on the canvas (the design's "Meter is wired in series").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Callout {
    pub part: String,
    pub title: String,
    #[serde(default)]
    pub text: String,
}

/// One option of a choice ("Pick R1").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub label: String,
    pub value: f64,
    /// The option removes the part from the circuit (a jumper in its place: "None").
    #[serde(default)]
    pub bypass: bool,
}

/// An interactive parameter: picking an option sets a property of a part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// `Pick R1`.
    pub label: String,
    pub part: String,
    pub prop: String,
    pub options: Vec<ChoiceOption>,
    /// Option selected at the start of the step.
    #[serde(default)]
    pub default: usize,
    /// Variable name the value is known by in expressions (`R`).
    #[serde(default = "default_var")]
    pub var: String,
}

fn default_var() -> String {
    "R".into()
}

/// A conditional hint for a wrong prediction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    /// Expression over the variables and `guess` (true when non-zero).
    pub when: String,
    pub text: String,
}

/// A numeric prediction (LES-3 "predict").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predict {
    /// `What current do you expect with {R1}?`
    pub prompt: String,
    pub unit: String,
    /// The answer as an expression of the step's variables (`3.0 / R * 1000`).
    pub answer: String,
    /// Relative tolerance (0.05 = ±5 %).
    #[serde(default = "default_tol")]
    pub tolerance: f64,
    /// Optional (behind the "Do the math myself" toggle).
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub toggle_label: String,
    #[serde(default)]
    pub toggle_note: String,
    /// The instrument that "waits for your prediction" (its reading is hidden until then).
    #[serde(default)]
    pub meter: Option<String>,
    /// `Nailed it. The meter agrees: {meter} mA.`
    #[serde(default)]
    pub correct_text: String,
    #[serde(default)]
    pub hints: Vec<Hint>,
    /// Hint when no specific one applies.
    #[serde(default)]
    pub default_hint: String,
    /// "Show me the working" lines (templates).
    #[serde(default)]
    pub working: Vec<String>,
}

fn default_tol() -> f64 {
    0.05
}

/// An instrument reading recorded by the student (LES-3 "measure").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    /// Instrument part id (`dmm1`).
    pub part: String,
    /// Reading key (`value`).
    #[serde(default = "default_key")]
    pub key: String,
    /// Display scale (1000 for mA) and unit.
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub unit: String,
}

fn default_key() -> String {
    "value".into()
}
fn one() -> f64 {
    1.0
}

/// The "Your measurements" table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub title: String,
    pub columns: Vec<String>,
    /// Distinct rows needed for the checkpoint.
    #[serde(default)]
    pub min_rows: usize,
    #[serde(default)]
    pub passed: String,
    /// `Try {n} more value{s}`.
    #[serde(default)]
    pub remaining: String,
}

/// A multiple-choice question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quiz {
    pub question: String,
    pub options: Vec<String>,
    pub answer: usize,
    #[serde(default)]
    pub explain: String,
}

/// A coding step (LES-3 "code").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Code {
    /// MCU part id.
    pub mcu: String,
    /// The sketch the editor opens with (`.ino` source).
    #[serde(default)]
    pub starter: String,
    /// The reference solution (`.ino` source).
    #[serde(default)]
    pub solution: String,
    /// Prebuilt image of the solution (used when no toolchain is installed).
    #[serde(default)]
    pub image: Option<String>,
}

/// A goal checked against the circuit and the running bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Check {
    /// Two pins (`part.pin`) on the same net.
    Connected {
        a: String,
        b: String,
    },
    NotConnected {
        a: String,
        b: String,
    },
    /// A part of a definition exists (with properties within 1 %).
    Part {
        def: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        props: BTreeMap<String, f64>,
        #[serde(default = "one_usize")]
        count: usize,
    },
    /// A part property in a range.
    Prop {
        part: String,
        prop: String,
        min: f64,
        max: f64,
    },
    /// Voltage of `a` (vs `b`, default ground) in a range.
    Voltage {
        a: String,
        #[serde(default)]
        b: Option<String>,
        min: f64,
        max: f64,
    },
    /// Current through a part's SPICE element (`D`, `R`) in a range (A).
    Current {
        part: String,
        #[serde(default = "default_element")]
        element: String,
        min: f64,
        max: f64,
    },
    /// A device / instrument reading in a range (`dmm1` `value`, `scope1` `frequency`).
    Reading {
        part: String,
        key: String,
        min: f64,
        max: f64,
    },
    /// An LED is lit (≥ `min_ma`, default 1 mA) or dark.
    LedLit {
        part: String,
        #[serde(default = "default_led_ma")]
        min_ma: f64,
    },
    LedOff {
        part: String,
    },
    /// A part was destroyed / is healthy.
    Damaged {
        part: String,
    },
    Healthy {
        part: String,
    },
    /// The MCU's serial output contains `text`.
    SerialContains {
        mcu: String,
        text: String,
    },
    /// Frequency (Hz) / duty (0…1) of an MCU pin, measured over the run.
    PinFrequency {
        mcu: String,
        pin: String,
        hz: f64,
        #[serde(default = "default_tol")]
        tolerance: f64,
    },
    PinDuty {
        mcu: String,
        pin: String,
        min: f64,
        max: f64,
    },
    /// The step's sketch compiles (or its prebuilt image is loaded) and runs.
    Firmware {
        mcu: String,
    },
    /// World variable in a range.
    World {
        key: String,
        min: f64,
        max: f64,
    },
    /// An MQTT message on `topic` (optionally containing `contains`) reached the broker.
    Mqtt {
        topic: String,
        #[serde(default)]
        contains: Option<String>,
    },
    /// The virtual phone received text (Bluetooth terminal / notifications) containing `contains`.
    Phone {
        contains: String,
    },
    /// The prediction of the step was right.
    Prediction,
    /// `min` distinct measurement rows recorded.
    Measurements {
        min: usize,
    },
    /// The quiz was answered correctly.
    Quiz,
    /// ERC has no errors (warnings allowed unless `strict`).
    ErcClean {
        #[serde(default)]
        strict: bool,
    },
    /// A SPICE analysis of the circuit (Pro mode) gives a value in a range: `analysis` =
    /// `op` / `tran` / `ac` / `dc`, `vector` = `v(out)`, `at` = time / frequency / sweep value
    /// (`op`: ignored), `stat` = `value` / `max` / `min` / `mag_db` / `phase_deg`.
    Spice {
        analysis: String,
        vector: String,
        #[serde(default)]
        at: f64,
        #[serde(default = "default_stat")]
        stat: String,
        min: f64,
        max: f64,
    },
}

fn one_usize() -> usize {
    1
}
fn default_element() -> String {
    "D".into()
}
fn default_led_ma() -> f64 {
    1.0
}
fn default_stat() -> String {
    "value".into()
}

/// One step of a worked solution (the scripted solver and "Show me how" replay these).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    /// Place a part.
    AddPart {
        def: String,
        #[serde(rename = "ref")]
        reference: String,
        #[serde(default)]
        props: BTreeMap<String, toml::Value>,
        #[serde(default)]
        at: [f64; 2],
    },
    RemovePart {
        part: String,
    },
    /// A jumper wire between two pins (`part.pin`).
    Wire {
        a: String,
        b: String,
        #[serde(default)]
        color: Option<String>,
    },
    /// Remove the wire(s) directly between two pins.
    Unwire {
        a: String,
        b: String,
    },
    SetProp {
        part: String,
        prop: String,
        value: toml::Value,
    },
    /// Pick option `index` of the step's choice.
    Choose {
        index: usize,
    },
    /// Turn "Do the math myself" on/off.
    MathMode {
        on: bool,
    },
    /// Enter a prediction (`value`, or the right answer when absent).
    Predict {
        #[serde(default)]
        value: Option<f64>,
    },
    /// Record the meter reading into the measurements table.
    Record,
    Quiz {
        answer: usize,
    },
    /// Run the bench.
    Run {
        seconds: f64,
    },
    /// User input on a part (button press, knob position, panel control).
    Press {
        part: String,
        on: bool,
    },
    Position {
        part: String,
        value: f64,
    },
    Control {
        part: String,
        key: String,
        value: f64,
    },
    World {
        key: String,
        value: f64,
    },
    /// Load the code step's solution (compiled, or its prebuilt image).
    Firmware {
        mcu: String,
    },
    /// "Repair all".
    Repair,
    /// The virtual phone connects to the device advertising `name`.
    PhoneConnect {
        name: String,
    },
    /// The phone opens its Bluetooth terminal on the connected device (subscribes to its
    /// serial characteristic, so what the device sends shows up).
    PhoneTerminal,
    /// The phone's Bluetooth terminal sends `text`.
    PhoneSend {
        text: String,
    },
    /// Publish to the bench's MQTT broker (the MQTT explorer / a dashboard).
    Publish {
        topic: String,
        payload: String,
    },
}

/// A step (LES-3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// `explain`, `build`, `predict`, `measure`, `experiment`, `break`, `quiz`, `code`.
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub formula: Option<String>,
    #[serde(default)]
    pub callouts: Vec<Callout>,
    #[serde(default)]
    pub choice: Option<Choice>,
    #[serde(default)]
    pub predict: Option<Predict>,
    #[serde(default)]
    pub measure: Option<Measure>,
    #[serde(default)]
    pub table: Option<Table>,
    #[serde(default)]
    pub quiz: Option<Quiz>,
    #[serde(default)]
    pub code: Option<Code>,
    /// Goals (all must pass to finish the step).
    #[serde(default)]
    pub checks: Vec<Check>,
    /// Bench seconds to run before evaluating bench checks.
    #[serde(default)]
    pub run: f64,
    /// Hints shown on request.
    #[serde(default)]
    pub hints: Vec<String>,
    /// Shown when a part is destroyed during the step (`Pop. That LED is gone.` …).
    #[serde(default)]
    pub damage_title: String,
    #[serde(default)]
    pub damage_text: String,
    #[serde(default)]
    pub repair_label: String,
    /// Label of the Next button (`Next: break it on purpose`).
    #[serde(default)]
    pub next_label: String,
    /// The worked solution.
    #[serde(default)]
    pub solution: Vec<Action>,
    /// Variables for expressions / templates (`Vled = 2.0`).
    #[serde(default)]
    pub vars: BTreeMap<String, f64>,
}

/// A lesson.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: String,
    pub track: String,
    pub order: u32,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub minutes: u32,
    #[serde(default)]
    pub bench: BenchSpec,
    pub steps: Vec<Step>,
}

pub const STEP_KINDS: [&str; 8] = [
    "explain",
    "build",
    "predict",
    "measure",
    "experiment",
    "break",
    "quiz",
    "code",
];
