//! A lesson on a headless bench: [`Runner`] couples a [`Session`] with a `bb_sim::Bench`
//! built from the lesson's start circuit (with the plugins' parts, e.g. the Handheld DMM),
//! applies worked-solution [`Action`]s through the public engine API (circuit edits through
//! `bb_circuit::edit::Editor`, choices, predictions, bench runs, instrument readings) and
//! evaluates the checks. `solve()` is the scripted solver that completes a lesson end to end.

use crate::check::{self, CheckResult};
use crate::format::{Action, Check};
use crate::session::{Edit, Session};
use crate::{LessonError, LoadedLesson};
use bb_core::{SimTime, UserInput};
use bb_plugin::PluginHost;
use bb_sim::radio::{PhoneCommand, RadioCommand};
use bb_sim::{Bench, BenchOptions};

impl crate::apply::Target for Runner {
    fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    fn circuit_changed(&mut self, live: Option<(&str, &str, f64)>) -> Result<(), LessonError> {
        match (live, &mut self.bench) {
            (Some((part, prop, v)), Some(b)) => b
                .set_part_param(part, prop, v)
                .map_err(|e| LessonError::Bench(e.to_string())),
            _ => self.sync_project(),
        }
    }

    fn edit(&mut self, e: Edit) -> Result<(), LessonError> {
        self.apply_edit(e)
    }

    fn meter(&self) -> Option<f64> {
        Runner::meter(self)
    }

    fn run(&mut self, seconds: f64) -> Result<(), LessonError> {
        Runner::run(self, seconds)
    }

    fn input(&mut self, part: &str, input: UserInput) -> Result<(), LessonError> {
        self.bench()?
            .input(part, &input)
            .map_err(|e| LessonError::Bench(e.to_string()))
    }

    fn set_world(&mut self, key: &str, value: f64) -> Result<(), LessonError> {
        self.bench()?.set_world(key, value);
        Ok(())
    }

    fn flash(&mut self, mcu: &str, image: bb_avr::FirmwareImage) -> Result<(), LessonError> {
        self.bench()?
            .mcu(mcu)
            .ok_or_else(|| LessonError::Bench(format!("no MCU `{mcu}`")))?
            .flash(&image)
            .map_err(LessonError::Bench)?;
        self.opts.firmware.insert(mcu.to_string(), image);
        Ok(())
    }

    fn repair(&mut self) -> Result<(), LessonError> {
        self.bench()?
            .repair_all()
            .map_err(|e| LessonError::Bench(e.to_string()))
    }

    fn phone_connect(&mut self, name: &str) -> Result<(), LessonError> {
        let b = self.bench()?;
        let tel = b.telemetry();
        let id = tel
            .radio
            .as_ref()
            .and_then(|r| r.phone.scan.iter().find(|s| s.name == *name).map(|s| s.id))
            .ok_or_else(|| LessonError::Bench(format!("the phone does not see `{name}`")))?;
        b.radio_command(RadioCommand::Phone(PhoneCommand::Connect { id, pin: None }))
            .map_err(|e| LessonError::Bench(e.to_string()))
    }

    fn radio(&mut self, cmd: RadioCommand) -> Result<(), LessonError> {
        self.bench()?
            .radio_command(cmd)
            .map_err(|e| LessonError::Bench(e.to_string()))
    }
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

    /// Applies one action ([`crate::apply::apply`], the code path "Do it for me" uses too).
    pub fn apply(&mut self, a: &Action) -> Result<(), LessonError> {
        self.log.push(format!("{a:?}"));
        crate::apply::apply(self, a)
    }

    /// The step's diagnosis ("Test circuit") on the headless bench.
    pub fn diagnose(&mut self) -> crate::diagnose::Diagnosis {
        let step = self.session.current().clone();
        let project = self.session.project().clone();
        let lib = self.session.library().clone();
        let state = self.session.state().clone();
        crate::diagnose::diagnose(
            &step,
            &project,
            &lib,
            self.bench.as_mut(),
            &state,
            &crate::diagnose::Options::running(),
        )
    }

    /// Uploads a compiled image to the code step's MCU (what the UI does after a successful
    /// build of the student's sketch) and marks the sketch as loaded.
    pub fn upload(&mut self, mcu: &str, image: &bb_avr::FirmwareImage) -> Result<(), LessonError> {
        self.bench()?
            .mcu(mcu)
            .ok_or_else(|| LessonError::Bench(format!("no MCU `{mcu}`")))?
            .flash(image)
            .map_err(LessonError::Bench)?;
        self.opts.firmware.insert(mcu.to_string(), image.clone());
        self.session.set_firmware_loaded(true);
        Ok(())
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
