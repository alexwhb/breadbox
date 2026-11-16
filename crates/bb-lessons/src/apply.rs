//! Applying worked-solution [`Action`]s — one code path for the scripted solver
//! ([`crate::runner::Runner`], a headless bench) and the app's "Do it for me" (the lesson
//! screen's live bench). Circuit edits go through the session's `bb_circuit::edit::Editor`;
//! what the bench must do (run, inputs, world, firmware, radios) goes through [`Target`].

use crate::LessonError;
use crate::format::Action;
use crate::session::{Edit, Session};
use bb_circuit::PropValue;
use bb_circuit::edit::{Op, Transaction};
use bb_circuit::project::{Endpoint, PartInstance};
use bb_core::UserInput;
use bb_sim::radio::{PhoneCommand, RadioCommand};

/// Whoever runs a lesson: the session plus a bench.
pub trait Target {
    fn session(&mut self) -> &mut Session;
    /// The session's circuit changed. `live` = a single numeric property change the bench can
    /// take without a rebuild (`Command::SetPartParam`).
    fn circuit_changed(&mut self, live: Option<(&str, &str, f64)>) -> Result<(), LessonError>;
    /// A choice's edit.
    fn edit(&mut self, e: Edit) -> Result<(), LessonError>;
    /// The current step's meter reading in the step's unit (`dmm1` × 1000 for mA).
    fn meter(&self) -> Option<f64>;
    fn run(&mut self, seconds: f64) -> Result<(), LessonError>;
    fn input(&mut self, part: &str, input: UserInput) -> Result<(), LessonError>;
    fn set_world(&mut self, key: &str, value: f64) -> Result<(), LessonError>;
    fn flash(&mut self, mcu: &str, image: bb_avr::FirmwareImage) -> Result<(), LessonError>;
    fn repair(&mut self) -> Result<(), LessonError>;
    /// The virtual phone connects to the device advertising `name`.
    fn phone_connect(&mut self, name: &str) -> Result<(), LessonError>;
    fn radio(&mut self, cmd: RadioCommand) -> Result<(), LessonError>;
}

pub fn prop_value(v: &toml::Value) -> PropValue {
    match v {
        toml::Value::Integer(i) => PropValue::Num(*i as f64),
        toml::Value::Float(f) => PropValue::Num(*f),
        toml::Value::Boolean(b) => PropValue::Bool(*b),
        toml::Value::String(s) => PropValue::Text(s.clone()),
        other => PropValue::Text(other.to_string()),
    }
}

pub fn pin(s: &str) -> Endpoint {
    let (p, n) = s.split_once('.').unwrap_or((s, ""));
    Endpoint::pin(p, n)
}

/// Whether an action edits the circuit (parts, wires, properties).
pub fn is_circuit(a: &Action) -> bool {
    matches!(
        a,
        Action::AddPart { .. }
            | Action::RemovePart { .. }
            | Action::Wire { .. }
            | Action::Unwire { .. }
            | Action::SetProp { .. }
    )
}

/// The code step's prebuilt solution image.
pub fn solution_image(session: &Session) -> Result<bb_avr::FirmwareImage, LessonError> {
    let step = session.current();
    let code = step
        .code
        .as_ref()
        .ok_or_else(|| LessonError::Bench("not a code step".into()))?;
    let img = code
        .image
        .as_ref()
        .ok_or_else(|| LessonError::Bench("the code step has no prebuilt image".into()))?;
    let path = session.loaded.resolve(img);
    bb_sim::firmware::load_image(&path)
        .map_err(|e| LessonError::Bench(format!("{}: {e}", path.display())))
}

/// Applies one action.
pub fn apply(t: &mut impl Target, a: &Action) -> Result<(), LessonError> {
    let lib = t.session().library().clone();
    match a {
        Action::AddPart {
            def,
            reference,
            props,
            at,
        } => {
            let d = lib
                .get(def)
                .ok_or_else(|| LessonError::Bench(format!("no part `{def}`")))?;
            let id = reference.to_ascii_lowercase();
            let mut inst = PartInstance::new(&id, reference, def);
            if !d.bench.hidden {
                inst = inst.at(at[0], at[1]);
            }
            let mut tx = Transaction::new(&format!("Add {}", d.name));
            tx.ops.push(Op::Part {
                before: None,
                after: Some(inst),
                index: None,
            });
            let s = t.session();
            s.editor.commit(tx);
            for (k, v) in props {
                s.editor
                    .set_prop(&lib, &id, k, prop_value(v))
                    .map_err(|e| LessonError::Bench(e.to_string()))?;
            }
            t.circuit_changed(None)
        }
        Action::RemovePart { part } => {
            t.session().editor.delete(std::slice::from_ref(part), &[]);
            t.circuit_changed(None)
        }
        Action::Wire { a, b, color } => {
            let s = t.session();
            let id = s.editor.add_wire(&lib, pin(a), pin(b));
            if let Some(c) = color
                && let Some(mut w) = s.editor.project.wire(&id).cloned()
            {
                w.color = c.clone();
                let _ = s.editor.update_wire(w);
            }
            t.circuit_changed(None)
        }
        Action::Unwire { a, b } => {
            let (ea, eb) = (pin(a), pin(b));
            let s = t.session();
            let ids: Vec<String> = s
                .project()
                .wires
                .iter()
                .filter(|w| (w.a == ea && w.b == eb) || (w.a == eb && w.b == ea))
                .map(|w| w.id.clone())
                .collect();
            if ids.is_empty() {
                return Err(LessonError::Bench(format!("no wire between {a} and {b}")));
            }
            s.editor.delete(&[], &ids);
            t.circuit_changed(None)
        }
        Action::SetProp { part, prop, value } => {
            let v = prop_value(value);
            t.session()
                .editor
                .set_prop(&lib, part, prop, v.clone())
                .map_err(|e| LessonError::Bench(e.to_string()))?;
            match v.as_f64() {
                Some(x) if !matches!(v, PropValue::Text(_)) => {
                    t.circuit_changed(Some((part, prop, x)))
                }
                _ => t.circuit_changed(None),
            }
        }
        Action::Choose { index } => {
            let e = t.session().choose(*index)?;
            t.edit(e)
        }
        Action::MathMode { on } => {
            t.session().set_math(*on);
            Ok(())
        }
        Action::Predict { value } => {
            let meter = t.meter();
            let v = match value {
                Some(v) => *v,
                None => t
                    .session()
                    .vars(meter)
                    .get("answer")
                    .ok_or_else(|| LessonError::Bench("no answer to predict".into()))?,
            };
            t.session().predict(v, meter);
            Ok(())
        }
        Action::Record => {
            let m = t
                .meter()
                .ok_or_else(|| LessonError::Bench("no meter reading to record".into()))?;
            t.session().record(m);
            Ok(())
        }
        Action::Quiz { answer } => {
            t.session().answer_quiz(*answer);
            Ok(())
        }
        Action::Run { seconds } => t.run(*seconds),
        Action::Press { part, on } => t.input(part, UserInput::Press(*on)),
        Action::Position { part, value } => t.input(part, UserInput::Position(*value)),
        Action::Control { part, key, value } => t.input(
            part,
            UserInput::Value {
                key: key.clone(),
                value: *value,
            },
        ),
        Action::World { key, value } => t.set_world(key, *value),
        Action::Firmware { mcu } => {
            let image = solution_image(t.session())?;
            t.flash(mcu, image)?;
            t.session().set_firmware_loaded(true);
            Ok(())
        }
        Action::Repair => t.repair(),
        Action::PhoneConnect { name } => t.phone_connect(name),
        Action::PhoneTerminal => t.radio(RadioCommand::Phone(PhoneCommand::TerminalOpen)),
        Action::PhoneSend { text } => t.radio(RadioCommand::Phone(PhoneCommand::TerminalSend(
            text.as_bytes().to_vec(),
        ))),
        Action::Publish { topic, payload } => t.radio(RadioCommand::ExplorerPublish {
            topic: topic.clone(),
            payload: payload.as_bytes().to_vec(),
            qos: 0,
            retain: false,
        }),
    }
}
