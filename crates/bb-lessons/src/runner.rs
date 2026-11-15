//! A lesson on a headless bench: [`Runner`] couples a [`Session`] with a `bb_sim::Bench`
//! built from the lesson's start circuit (with the plugins' parts, e.g. the Handheld DMM),
//! applies worked-solution [`Action`]s through the public engine API (circuit edits through
//! `bb_circuit::edit::Editor`, choices, predictions, bench runs, instrument readings) and
//! evaluates the checks. `solve()` is the scripted solver that completes a lesson end to end.

use crate::check::{self, CheckResult};
use crate::format::{Action, Check};
use crate::session::{Edit, Session};
use crate::{LessonError, LoadedLesson};
use bb_circuit::PropValue;
use bb_circuit::edit::{Op, Transaction};
use bb_circuit::project::{Endpoint, PartInstance};
use bb_core::{SimTime, UserInput};
use bb_plugin::PluginHost;
use bb_sim::radio::{PhoneCommand, RadioCommand};
use bb_sim::{Bench, BenchOptions};

fn prop_value(v: &toml::Value) -> PropValue {
    match v {
        toml::Value::Integer(i) => PropValue::Num(*i as f64),
        toml::Value::Float(f) => PropValue::Num(*f),
        toml::Value::Boolean(b) => PropValue::Bool(*b),
        toml::Value::String(s) => PropValue::Text(s.clone()),
        other => PropValue::Text(other.to_string()),
    }
}

fn pin(s: &str) -> Endpoint {
    let (p, n) = s.split_once('.').unwrap_or((s, ""));
    Endpoint::pin(p, n)
}

/// A lesson running headless.
pub struct Runner {
    pub session: Session,
    /// The running bench (`None` for Pro lessons, which are checked with SPICE analyses).
    pub bench: Option<Bench>,
    opts: BenchOptions,
    /// What happened (for failure messages).
    pub log: Vec<String>,
    /// Each solved step's label and its evaluated goals.
    pub transcript: Vec<(String, Vec<CheckResult>)>,
}

impl Runner {
    /// Starts a lesson: session, bench with the plugins applied, firmware, world overrides,
    /// warm-up.
    pub fn new(l: &LoadedLesson, host: &PluginHost) -> Result<Runner, LessonError> {
        let session = Session::new(l, host.library())?;
        let project = session.project().clone();
        let mut opts = host.bench_options();
        opts.firmware = l.images(&project)?;
        opts.host_firmware = l.host_firmware(&project, &host.library())?;
        let bench = if project.mode == bb_circuit::project::Mode::Pro {
            None
        } else {
            let mut bench = Bench::new(project, host.library(), opts.clone())
                .map_err(|e| LessonError::Bench(e.to_string()))?;
            for (k, v) in &l.lesson.bench.world {
                bench.set_world(k, *v);
            }
            // MQTT checks watch the built-in broker
            let _ = bench.radio_command(bb_sim::radio::RadioCommand::ExplorerSubscribe("#".into()));
            Some(bench)
        };
        let mut r = Runner {
            session,
            bench,
            opts,
            log: Vec::new(),
            transcript: Vec::new(),
        };
        if l.lesson.bench.warmup > 0.0 {
            r.run(l.lesson.bench.warmup)?;
        }
        r.enter();
        Ok(r)
    }

    pub fn options(&self) -> &BenchOptions {
        &self.opts
    }

    fn enter(&mut self) {
        let checks: Vec<Check> = self.session.current().checks.clone();
        if let Some(b) = &mut self.bench {
            check::prepare(&checks, b);
        }
    }

    fn bench(&mut self) -> Result<&mut Bench, LessonError> {
        self.bench
            .as_mut()
            .ok_or_else(|| LessonError::Bench("this lesson has no running bench".into()))
    }

    fn sync_project(&mut self) -> Result<(), LessonError> {
        let p = self.session.project().clone();
        match &mut self.bench {
            Some(b) => b
                .update_project(p)
                .map_err(|e| LessonError::Bench(e.to_string())),
            None => Ok(()),
        }
    }

    pub fn run(&mut self, seconds: f64) -> Result<(), LessonError> {
        match &mut self.bench {
            Some(b) => b
                .run_for(SimTime::from_secs_f64(seconds.max(0.0)))
                .map_err(|e| LessonError::Bench(e.to_string())),
            None => Ok(()),
        }
    }

    /// The current step's meter reading in the step's unit (`dmm1` value × 1000 for mA).
    pub fn meter(&self) -> Option<f64> {
        let step = self.session.current();
        let (part, key, scale) = match (&step.measure, &step.predict) {
            (Some(m), _) => (m.part.clone(), m.key.clone(), m.scale),
            (None, Some(p)) => (
                p.meter.clone()?,
                "value".to_string(),
                if p.unit == "mA" { 1000.0 } else { 1.0 },
            ),
            _ => return None,
        };
        self.bench
            .as_ref()?
            .readings(&part)
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|x| x.1 * scale)
            .filter(|v| v.is_finite())
    }

    fn apply_edit(&mut self, e: Edit) -> Result<(), LessonError> {
        let Some(b) = &mut self.bench else {
            return Ok(());
        };
        match e {
            Edit::SetProp { part, prop, value } => b
                .set_part_param(&part, &prop, value)
                .map_err(|e| LessonError::Bench(e.to_string())),
            Edit::Project(p) => b
                .update_project(*p)
                .map_err(|e| LessonError::Bench(e.to_string())),
        }
    }

    /// Applies one action.
    pub fn apply(&mut self, a: &Action) -> Result<(), LessonError> {
        self.log.push(format!("{a:?}"));
        let lib = self.session.library().clone();
        let bench_err = |e: bb_sim::SimError| LessonError::Bench(e.to_string());
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
                self.session.editor.commit(tx);
                for (k, v) in props {
                    self.session
                        .editor
                        .set_prop(&lib, &id, k, prop_value(v))
                        .map_err(|e| LessonError::Bench(e.to_string()))?;
                }
                self.sync_project()
            }
            Action::RemovePart { part } => {
                self.session.editor.delete(std::slice::from_ref(part), &[]);
                self.sync_project()
            }
            Action::Wire { a, b, color } => {
                let id = self.session.editor.add_wire(&lib, pin(a), pin(b));
                if let Some(c) = color
                    && let Some(mut w) = self.session.editor.project.wire(&id).cloned()
                {
                    w.color = c.clone();
                    let _ = self.session.editor.update_wire(w);
                }
                self.sync_project()
            }
            Action::Unwire { a, b } => {
                let (ea, eb) = (pin(a), pin(b));
                let ids: Vec<String> = self
                    .session
                    .project()
                    .wires
                    .iter()
                    .filter(|w| (w.a == ea && w.b == eb) || (w.a == eb && w.b == ea))
                    .map(|w| w.id.clone())
                    .collect();
                if ids.is_empty() {
                    return Err(LessonError::Bench(format!("no wire between {a} and {b}")));
                }
                self.session.editor.delete(&[], &ids);
                self.sync_project()
            }
            Action::SetProp { part, prop, value } => {
                let v = prop_value(value);
                self.session
                    .editor
                    .set_prop(&lib, part, prop, v.clone())
                    .map_err(|e| LessonError::Bench(e.to_string()))?;
                match (v.as_f64(), &mut self.bench) {
                    (Some(x), Some(b)) if !matches!(v, PropValue::Text(_)) => {
                        b.set_part_param(part, prop, x).map_err(bench_err)
                    }
                    _ => self.sync_project(),
                }
            }
            Action::Choose { index } => {
                let e = self.session.choose(*index)?;
                self.apply_edit(e)
            }
            Action::MathMode { on } => {
                self.session.set_math(*on);
                Ok(())
            }
            Action::Predict { value } => {
                let meter = self.meter();
                let v = match value {
                    Some(v) => *v,
                    None => self
                        .session
                        .vars(meter)
                        .get("answer")
                        .ok_or_else(|| LessonError::Bench("no answer to predict".into()))?,
                };
                self.session.predict(v, meter);
                Ok(())
            }
            Action::Record => {
                let m = self
                    .meter()
                    .ok_or_else(|| LessonError::Bench("no meter reading to record".into()))?;
                self.session.record(m);
                Ok(())
            }
            Action::Quiz { answer } => {
                self.session.answer_quiz(*answer);
                Ok(())
            }
            Action::Run { seconds } => self.run(*seconds),
            Action::Press { part, on } => self
                .bench()?
                .input(part, &UserInput::Press(*on))
                .map_err(bench_err),
            Action::Position { part, value } => self
                .bench()?
                .input(part, &UserInput::Position(*value))
                .map_err(bench_err),
            Action::Control { part, key, value } => self
                .bench()?
                .input(
                    part,
                    &UserInput::Value {
                        key: key.clone(),
                        value: *value,
                    },
                )
                .map_err(bench_err),
            Action::World { key, value } => {
                self.bench()?.set_world(key, *value);
                Ok(())
            }
            Action::Firmware { mcu } => {
                let step = self.session.current().clone();
                let code = step
                    .code
                    .as_ref()
                    .ok_or_else(|| LessonError::Bench("not a code step".into()))?;
                let img = code.image.as_ref().ok_or_else(|| {
                    LessonError::Bench("the code step has no prebuilt image".into())
                })?;
                let path = self.session.loaded.resolve(img);
                let image = bb_sim::firmware::load_image(&path)
                    .map_err(|e| LessonError::Bench(format!("{}: {e}", path.display())))?;
                self.bench()?
                    .mcu(mcu)
                    .ok_or_else(|| LessonError::Bench(format!("no MCU `{mcu}`")))?
                    .flash(&image)
                    .map_err(LessonError::Bench)?;
                self.opts.firmware.insert(mcu.clone(), image);
                self.session.set_firmware_loaded(true);
                Ok(())
            }
            Action::Repair => self.bench()?.repair_all().map_err(bench_err),
            Action::PhoneConnect { name } => {
                let b = self.bench()?;
                let tel = b.telemetry();
                let id = tel
                    .radio
                    .as_ref()
                    .and_then(|r| r.phone.scan.iter().find(|s| s.name == *name).map(|s| s.id))
                    .ok_or_else(|| {
                        LessonError::Bench(format!("the phone does not see `{name}`"))
                    })?;
                b.radio_command(RadioCommand::Phone(PhoneCommand::Connect { id, pin: None }))
                    .map_err(bench_err)
            }
            Action::PhoneTerminal => self
                .bench()?
                .radio_command(RadioCommand::Phone(PhoneCommand::TerminalOpen))
                .map_err(bench_err),
            Action::PhoneSend { text } => self
                .bench()?
                .radio_command(RadioCommand::Phone(PhoneCommand::TerminalSend(
                    text.as_bytes().to_vec(),
                )))
                .map_err(bench_err),
            Action::Publish { topic, payload } => self
                .bench()?
                .radio_command(RadioCommand::ExplorerPublish {
                    topic: topic.clone(),
                    payload: payload.as_bytes().to_vec(),
                    qos: 0,
                    retain: false,
                })
                .map_err(bench_err),
        }
    }

    /// Evaluates the current step's goals.
    pub fn evaluate(&mut self) -> Vec<CheckResult> {
        let step = self.session.current().clone();
        let project = self.session.project().clone();
        let lib = self.session.library().clone();
        let state = self.session.state().clone();
        let r = check::evaluate_step(&step, &project, &lib, self.bench.as_mut(), &state);
        self.session.mark(r.iter().all(|c| c.ok));
        r
    }

    /// Replays the current step's worked solution, runs the bench for the step's `run`, and
    /// evaluates.
    pub fn solve_step(&mut self) -> Result<Vec<CheckResult>, LessonError> {
        let step = self.session.current().clone();
        for a in &step.solution {
            self.apply(a)?;
        }
        if step.run > 0.0 {
            self.run(step.run)?;
        }
        Ok(self.evaluate())
    }

    /// Goes to the next step (preparing its probes).
    pub fn next_step(&mut self) -> bool {
        let ok = self.session.next_step();
        if ok {
            self.enter();
        }
        ok
    }

    /// Solves every step in order; the error names the first step whose goals failed.
    pub fn solve(&mut self) -> Result<(), LessonError> {
        loop {
            let r = self.solve_step()?;
            self.transcript.push((self.session.label(), r.clone()));
            if let Some(bad) = r.iter().find(|c| !c.ok) {
                return Err(LessonError::Invalid {
                    lesson: self.session.lesson.id.clone(),
                    msg: format!(
                        "{}: “{}” failed ({}); all: {:?}",
                        self.session.label(),
                        bad.label,
                        bad.detail,
                        r
                    ),
                });
            }
            if !self.next_step() {
                return Ok(());
            }
        }
    }
}
