//! "Show me": the step's worked solution as moves a person can follow — what to do in words
//! (`Jumper from USB +5 V to hole a5`), where on the bench or in the step panel (marks the app
//! draws: a ghost jumper with pulsing target holes, the wire to remove, the control to use),
//! and one line on *why* it works.
//!
//! Every [`Action`] kind has a move, so every step with a worked solution can be shown.

use crate::diagnose::{Mark, Spot, Tone};
use crate::format::{Action, Step};
use crate::names::{Namer, split};
use bb_circuit::connectivity::{Connectivity, TermRef};
use bb_circuit::project::Endpoint;
use bb_circuit::{Library, Project};
use serde::{Deserialize, Serialize};

/// Step-panel controls the guide can point at ([`Spot::Control`] ids).
pub mod control {
    /// Option `i` of the step's choice.
    pub fn choice(i: usize) -> String {
        format!("choice:{i}")
    }
    /// Answer `i` of the quiz.
    pub fn quiz(i: usize) -> String {
        format!("quiz:{i}")
    }
    pub const MATH: &str = "math";
    pub const PREDICT: &str = "predict";
    pub const RECORD: &str = "record";
    pub const COMPILE: &str = "compile";
    pub const REPAIR: &str = "repair";
    pub const PHONE_CONNECT: &str = "phone:connect";
    pub const PHONE_TERMINAL: &str = "phone:terminal";
    pub const PHONE_SEND: &str = "phone:send";
    /// A world variable's control (`world:light.ambient_lux`).
    pub fn world(key: &str) -> String {
        format!("world:{key}")
    }
    /// A part property's field (`prop:r2.resistance`).
    pub fn prop(part: &str, prop: &str) -> String {
        format!("prop:{part}.{prop}")
    }
    /// "Add R3" (`add:r3`).
    pub fn add(part: &str) -> String {
        format!("add:{}", part.to_ascii_lowercase())
    }
    /// The MQTT publish field (`publish:weather/cmd`).
    pub fn publish(topic: &str) -> String {
        format!("publish:{topic}")
    }
}

/// A jumper to draw as a ghost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ghost {
    pub a: TermRef,
    pub b: TermRef,
    pub color: String,
    /// Short label next to it (`USB +5 V → a5`).
    pub label: String,
}

/// One move of a worked solution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Move {
    /// What to do, one short sentence (`Jumper from USB +5 V to hole a5`).
    pub text: String,
    /// Where (holes to pulse, the wire to remove, the control to use).
    pub marks: Vec<Mark>,
    /// A jumper to draw as a ghost.
    pub ghost: Option<Ghost>,
    /// A circuit edit (wire, unwire, parts, properties).
    pub circuit: bool,
    /// Only waits for the bench (`run`).
    pub wait: bool,
}

/// "Show me" for a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ShowMe {
    pub moves: Vec<Move>,
    /// One line on why it works (the step's `why`, else its last hint).
    pub why: String,
}

impl ShowMe {
    /// Every mark of every move.
    pub fn marks(&self) -> impl Iterator<Item = &Mark> {
        self.moves.iter().flat_map(|m| m.marks.iter())
    }
    /// The moves worth listing (waits for the bench are left out).
    pub fn visible(&self) -> impl Iterator<Item = &Move> {
        self.moves.iter().filter(|m| !m.wait)
    }
}

/// DMM dial positions in words (`dmm1` `dial`).
pub const DIAL: [&str; 10] = [
    "off",
    "AC volts",
    "DC volts",
    "millivolts",
    "ohms (Ω)",
    "continuity",
    "capacitance",
    "amps (10 A)",
    "mA",
    "Hz",
];

fn pin_mark(t: &TermRef, tone: Tone, note: &str) -> Mark {
    Mark {
        spot: Spot::Pin {
            part: t.part.clone(),
            pin: t.pin.clone(),
        },
        tone,
        note: note.into(),
    }
}

fn control(id: String, note: &str) -> Mark {
    Mark {
        spot: Spot::Control(id),
        tone: Tone::Target,
        note: note.into(),
    }
}

fn smart_num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        crate::text::smart(v)
    }
}

/// A hole or pin for a ghost label: `a5`, `USB +5 V`, `the meter's mA jack`.
fn short_end(n: &Namer, t: &TermRef) -> String {
    if n.is_breadboard(&t.part) {
        match bb_circuit::breadboard::parse_hole(&t.pin) {
            Some(bb_circuit::breadboard::Hole::Rail { rail, .. }) => crate::names::rail_name(rail)
                .trim_start_matches("the ")
                .into(),
            _ => t.pin.clone(),
        }
    } else {
        n.pin(&t.part, &t.pin)
    }
}

/// Describes one action of a step's worked solution.
pub fn describe(a: &Action, step: &Step, n: &Namer) -> Move {
    let mv = |text: String, marks: Vec<Mark>| Move {
        text,
        marks,
        ghost: None,
        circuit: false,
        wait: false,
    };
    match a {
        Action::Wire { a, b, color } => {
            let (ta, tb) = (term(a), term(b));
            let name = |t: &TermRef| {
                if n.is_breadboard(&t.part) {
                    let place = n.place(t);
                    if place.starts_with("column") {
                        format!("hole {} ({place})", t.pin)
                    } else {
                        place
                    }
                } else {
                    n.pin(&t.part, &t.pin)
                }
            };
            let color = color.clone().unwrap_or_else(|| "orange".into());
            let mut marks = vec![
                pin_mark(&ta, Tone::Target, &format!("from {}", name(&ta))),
                pin_mark(&tb, Tone::Target, &format!("to {}", name(&tb))),
            ];
            // the other free holes of a target strip work just as well
            for t in [&ta, &tb] {
                if n.is_breadboard(&t.part) {
                    for h in n.free_holes(t) {
                        if h != *t {
                            marks.push(pin_mark(&h, Tone::Ok, "same strip"));
                        }
                    }
                }
            }
            Move {
                text: format!("Add a {color} jumper from {} to {}", name(&ta), name(&tb)),
                marks,
                ghost: Some(Ghost {
                    label: format!("{} → {}", short_end(n, &ta), short_end(n, &tb)),
                    a: ta,
                    b: tb,
                    color,
                }),
                circuit: true,
                wait: false,
            }
        }
        Action::Unwire { a, b } => {
            let (ea, eb) = (endpoint(a), endpoint(b));
            let wires: Vec<String> = n
                .project
                .wires
                .iter()
                .filter(|w| (w.a == ea && w.b == eb) || (w.a == eb && w.b == ea))
                .map(|w| w.id.clone())
                .collect();
            let text = match wires.first() {
                Some(w) => format!("Remove {}", n.wire(w)),
                None => format!(
                    "Remove the jumper between {} and {}",
                    n.pin_str(a),
                    n.pin_str(b)
                ),
            };
            let marks = wires
                .iter()
                .map(|w| Mark {
                    spot: Spot::Wire(w.clone()),
                    tone: Tone::Remove,
                    note: "remove this jumper".into(),
                })
                .collect();
            Move {
                circuit: true,
                ..mv(text, marks)
            }
        }
        Action::AddPart {
            def,
            reference,
            props,
            ..
        } => {
            let d = n.lib.get(def);
            let name = d
                .as_ref()
                .map(|d| d.name.to_lowercase())
                .unwrap_or(def.clone());
            let vals: Vec<String> = props
                .iter()
                .filter_map(|(k, v)| {
                    let x = match v {
                        toml::Value::Integer(i) => *i as f64,
                        toml::Value::Float(f) => *f,
                        _ => return None,
                    };
                    let unit = d
                        .as_ref()
                        .and_then(|d| d.prop(k).map(|p| p.unit.clone()))
                        .unwrap_or_default();
                    Some(bb_core::si::format(x, &unit))
                })
                .collect();
            let what = if vals.is_empty() {
                name
            } else {
                format!("{} {name}", vals.join(" "))
            };
            Move {
                circuit: true,
                ..mv(
                    format!("Add {reference}, a {what}"),
                    vec![control(control::add(reference), "add it here")],
                )
            }
        }
        Action::RemovePart { part } => Move {
            circuit: true,
            ..mv(
                format!("Remove {}", n.part(part)),
                vec![Mark {
                    spot: Spot::Part(part.clone()),
                    tone: Tone::Remove,
                    note: "remove this part".into(),
                }],
            )
        },
        Action::SetProp { part, prop, value } => {
            let unit = n
                .def(part)
                .and_then(|d| d.prop(prop).map(|p| p.unit.clone()))
                .unwrap_or_default();
            let v = match value {
                toml::Value::Integer(i) => bb_core::si::format(*i as f64, &unit),
                toml::Value::Float(f) => bb_core::si::format(*f, &unit),
                other => other.to_string().trim_matches('"').to_string(),
            };
            Move {
                circuit: true,
                ..mv(
                    format!("Set {}'s {prop} to {v}", n.reference(part)),
                    vec![
                        control(control::prop(part, prop), &format!("{prop} → {v}")),
                        Mark {
                            spot: Spot::Part(part.clone()),
                            tone: Tone::Focus,
                            note: format!("{prop} → {v}"),
                        },
                    ],
                )
            }
        }
        Action::Choose { index } => {
            let label = step
                .choice
                .as_ref()
                .and_then(|c| c.options.get(*index))
                .map(|o| o.label.clone())
                .unwrap_or_else(|| format!("option {}", index + 1));
            mv(
                format!("Pick {label}"),
                vec![control(control::choice(*index), &format!("pick {label}"))],
            )
        }
        Action::MathMode { on } => {
            let l = step
                .predict
                .as_ref()
                .map(|p| p.toggle_label.clone())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "Do the math myself".into());
            mv(
                format!("Turn {} “{l}”", if *on { "on" } else { "off" }),
                vec![control(control::MATH.into(), "")],
            )
        }
        Action::Predict { value } => {
            let unit = step
                .predict
                .as_ref()
                .map(|p| p.unit.clone())
                .unwrap_or_default();
            let text = match value {
                Some(v) => format!("Type {} {unit} and press Check", smart_num(*v)),
                None => "Work out the answer, type it in and press Check".into(),
            };
            mv(
                text.replace("  ", " "),
                vec![control(control::PREDICT.into(), "your prediction")],
            )
        }
        Action::Record => mv(
            "Record the meter's reading".into(),
            vec![control(control::RECORD.into(), "")],
        ),
        Action::Quiz { answer } => {
            let o = step
                .quiz
                .as_ref()
                .and_then(|q| q.options.get(*answer))
                .cloned()
                .unwrap_or_default();
            mv(
                format!("Pick “{o}”"),
                vec![control(control::quiz(*answer), "")],
            )
        }
        Action::Run { seconds } => Move {
            wait: true,
            ..mv(
                format!("Let the bench run for {} s", smart_num(*seconds)),
                Vec::new(),
            )
        },
        Action::Press { part, on } => mv(
            if *on {
                format!("Press and hold {}", n.reference(part))
            } else {
                format!("Let go of {}", n.reference(part))
            },
            vec![Mark {
                spot: Spot::Part(part.clone()),
                tone: Tone::Target,
                note: if *on { "press" } else { "release" }.into(),
            }],
        ),
        Action::Position { part, value } => mv(
            format!(
                "Turn {} to {:.0} %",
                n.reference(part),
                value.clamp(0.0, 1.0) * 100.0
            ),
            vec![Mark {
                spot: Spot::Part(part.clone()),
                tone: Tone::Target,
                note: format!("drag the knob to {:.0} %", value * 100.0),
            }],
        ),
        Action::Control { part, key, value } => {
            let (text, note) = if key == "dial" {
                let m = DIAL.get(value.round() as usize).copied().unwrap_or("?");
                (
                    format!("Turn {}'s dial to {m}", n.part_short(part)),
                    format!("dial → {m}"),
                )
            } else {
                (
                    format!("Set {} {key} to {}", n.part_short(part), smart_num(*value)),
                    format!("{key} → {}", smart_num(*value)),
                )
            };
            mv(
                crate::check::sentence(&text),
                vec![Mark {
                    spot: Spot::Part(part.clone()),
                    tone: Tone::Target,
                    note,
                }],
            )
        }
        Action::World { key, value } => {
            let (name, unit) = crate::check::world_name(key);
            let v = format!("{} {unit}", smart_num(*value)).trim().to_string();
            mv(
                crate::check::sentence(&format!("Set the {name} to {v}")),
                vec![control(control::world(key), &v)],
            )
        }
        Action::Firmware { mcu } => mv(
            format!(
                "Press Compile & upload to run the sketch on {}",
                n.part_short(mcu)
            ),
            vec![control(control::COMPILE.into(), "")],
        ),
        Action::Repair => mv(
            "Press Repair".into(),
            vec![control(control::REPAIR.into(), "")],
        ),
        Action::PhoneConnect { name } => mv(
            format!("Connect the phone to {name}"),
            vec![control(control::PHONE_CONNECT.into(), "")],
        ),
        Action::PhoneTerminal => mv(
            "Open the phone's terminal".into(),
            vec![control(control::PHONE_TERMINAL.into(), "")],
        ),
        Action::PhoneSend { text } => mv(
            format!("Send “{}” from the phone", text.trim()),
            vec![control(control::PHONE_SEND.into(), text.trim())],
        ),
        Action::Publish { topic, payload } => mv(
            format!("Publish “{payload}” to {topic}"),
            vec![control(control::publish(topic), payload)],
        ),
    }
}

fn term(s: &str) -> TermRef {
    let (p, n) = split(s);
    TermRef::new(p, n)
}

fn endpoint(s: &str) -> Endpoint {
    let (p, n) = split(s);
    Endpoint::pin(p, n)
}

/// "Show me" for a step on the current circuit.
pub fn show_me(step: &Step, project: &Project, lib: &Library) -> ShowMe {
    let conn = Connectivity::compute(project, lib);
    let n = Namer::new(project, lib, &conn);
    let moves: Vec<Move> = step
        .solution
        .iter()
        .map(|a| describe(a, step, &n))
        .collect();
    let why = if !step.why.trim().is_empty() {
        step.why.trim().to_string()
    } else {
        step.hints.last().cloned().unwrap_or_default()
    };
    ShowMe { moves, why }
}

/// The first thing to do in the worked solution, in words (`Set the room light to 5 lux`) —
/// the fallback next action of a diagnosis.
pub fn first_move(step: &Step, n: &Namer) -> Option<Move> {
    step.solution
        .iter()
        .map(|a| describe(a, step, n))
        .find(|m| !m.wait)
}
