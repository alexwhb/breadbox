//! A running lesson: the `bb_lessons::Session` (step state machine), the lesson circuit as a
//! `CircuitDoc` shown by an embedded `BenchCanvas`, its own live `SimBench` (with the active
//! plugins, the lesson's firmware and world), goal evaluation on the engine thread, the
//! meter reading of the step, auto-recorded measurements, damage + repair, code steps and
//! progress.

use crate::bench::sim::{OptionsHook, SimBench};
use crate::bench::{BenchSpec, Repaint, SimSpeed};
use crate::canvas::bench::BenchCanvas;
use crate::canvas::schematic::SchematicCanvas;
use crate::circuit::CircuitDoc;
use crate::jobs::Job;
use crate::widgets::{CodeDoc, EditorView};
use bb_circuit::{Library, PropValue};
use bb_lessons::check::CheckResult;
use bb_lessons::diagnose::{Diagnosis, Mark, Spot};
use bb_lessons::format::Action;
use bb_lessons::guide::ShowMe;
use bb_lessons::session::{Edit, Session};
use bb_lessons::{LessonError, LoadedLesson, Progress};
use bb_sim::{Command, Telemetry};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// "Do it for me" in progress: the step's worked solution, replayed over time on the live bench
/// (circuit edits as one undoable transaction, `run` = wait for the bench).
#[derive(Debug, Clone)]
pub struct Player {
    queue: VecDeque<Action>,
    /// Waiting until the bench reaches this sim time.
    wait_until: Option<f64>,
    /// A failing action is retried for a few seconds (the phone still scanning …).
    retry_since: Option<Instant>,
    pub started: Instant,
}

/// Result of compiling a code step's sketch.
pub type BuildResult = Result<bb_lessons::code::Build, String>;

pub struct LessonRun {
    pub loaded: LoadedLesson,
    pub session: Session,
    pub doc: CircuitDoc,
    pub canvas: BenchCanvas,
    pub schematic: SchematicCanvas,
    /// `None` for Pro lessons (checked with SPICE analyses on the schematic).
    pub bench: Option<SimBench>,
    pub lib: Arc<Library>,
    /// Bumped per lesson (the canvas fits its view once).
    pub generation: u64,
    bench_gen: u64,
    /// Latest evaluated goals of the current step.
    pub goals: Vec<CheckResult>,
    goals_at: Option<Instant>,
    goals_step: usize,
    /// The goals were evaluated for the current step.
    goals_ready: bool,
    prepared: Option<usize>,
    /// The prediction text field.
    pub guess: String,
    /// (sim seconds, reading) since the last change (choice, repair, step).
    meter_hist: Vec<(f64, f64)>,
    /// A Check waiting for the meter to settle after the last change: (guess, since).
    pub pending: Option<(f64, Instant)>,
    /// (choice index, value) last recorded into the table.
    recorded: Option<(usize, f64)>,
    pub code: Option<CodeDoc>,
    pub code_view: EditorView,
    pub build: Option<Job<BuildResult>>,
    pub build_log: Vec<String>,
    pub show_hint: bool,
    /// The lesson bench was zoomed to fill the canvas.
    pub zoomed: bool,
    /// The bench folder (firmware paths of the lesson circuit are relative to it).
    pub dir: PathBuf,
    /// "Test circuit": the diagnosis of the current circuit (cleared when it changes).
    pub diagnosis: Option<Diagnosis>,
    /// Hints shown so far (they escalate: hint 1, hint 2 …, then Show me, then Do it for me).
    pub hints_shown: usize,
    /// "Show me" is open.
    pub show_me: Option<ShowMe>,
    /// Show me was opened on this step (Do it for me is the next suggestion).
    pub show_me_used: bool,
    show_me_at: Option<Instant>,
    /// "Do it for me" in progress.
    pub player: Option<Player>,
    /// Do it for me finished on this step (the success card says why it works).
    pub did_it: bool,
    /// Do it for me failed (what went wrong).
    pub player_error: Option<String>,
    /// When the goals first all passed on this step (success animation).
    pub passed_at: Option<Instant>,
    /// `Loop closed — D1 is lit at 13.6 mA`.
    pub payoff: String,
    /// Per goal: when it turned green (tick animation).
    pub goal_ok_at: Vec<Option<Instant>>,
    /// The success chime is due (the screen plays it when sounds are on).
    pub chime_due: bool,
    /// Text of the panel's controls (property fields, phone message, MQTT payload).
    pub fields: HashMap<String, String>,
    /// Circuit edits made through [`bb_lessons::apply`] wait for one commit.
    circuit_dirty: bool,
    /// The last lines the step's MCU printed (for steps that check serial output).
    pub serial_tail: Vec<String>,
}

/// Where a lesson's circuit files live (bench file folder or the example).
fn bench_dir(l: &LoadedLesson) -> PathBuf {
    match (&l.lesson.bench.project, &l.lesson.bench.example) {
        (None, Some(ex)) => bb_lessons::examples_dir().join(ex),
        _ => l.dir.clone(),
    }
}

impl LessonRun {
    /// Opens a lesson at `resume` (step index) and starts its bench.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        loaded: &LoadedLesson,
        lib: Arc<Library>,
        hook: OptionsHook,
        fixtures: Option<PathBuf>,
        repaint: Repaint,
        speed: SimSpeed,
        resume: usize,
        generation: u64,
    ) -> Result<LessonRun, String> {
        let mut session = Session::new(loaded, lib.clone()).map_err(|e| e.to_string())?;
        while session.step < resume && session.next_step() {}
        let project = session.project().clone();
        let dir = bench_dir(loaded);
        let mut doc = CircuitDoc::new(project.clone(), &dir, true);
        doc.set_library(lib.clone());
        let pro = project.mode == bb_circuit::project::Mode::Pro;
        let bench = if pro {
            None
        } else {
            let images = loaded.images(&project).unwrap_or_default();
            let hook2: OptionsHook = Arc::new(move |o| {
                hook(o);
                for (k, v) in &images {
                    o.firmware.insert(k.clone(), v.clone());
                }
            });
            let spec = BenchSpec {
                project_dir: Some(dir.clone()),
                targets: Vec::new(),
                radios: Vec::new(),
                repaint,
                speed,
                circuit: Some(project.clone()),
                fixtures,
                audio: false,
            };
            let b = SimBench::with(&spec, lib.clone(), Some(hook2));
            for (k, v) in &loaded.lesson.bench.world {
                b.send(Command::SetWorld {
                    key: k.clone(),
                    value: *v,
                });
            }
            // MQTT goals watch the built-in broker
            b.send(Command::Radio(
                bb_sim::radio::RadioCommand::ExplorerSubscribe("#".into()),
            ));
            Some(b)
        };
        let mut run = LessonRun {
            loaded: loaded.clone(),
            session,
            bench_gen: doc.generation(),
            doc,
            canvas: BenchCanvas::default(),
            schematic: SchematicCanvas::default(),
            bench,
            lib,
            generation,
            goals: Vec::new(),
            goals_at: None,
            goals_step: usize::MAX,
            goals_ready: false,
            prepared: None,
            guess: String::new(),
            meter_hist: Vec::new(),
            pending: None,
            recorded: None,
            code: None,
            code_view: EditorView::default(),
            build: None,
            build_log: Vec::new(),
            show_hint: false,
            zoomed: false,
            dir,
            diagnosis: None,
            hints_shown: 0,
            show_me: None,
            show_me_used: false,
            show_me_at: None,
            player: None,
            did_it: false,
            player_error: None,
            passed_at: None,
            payoff: String::new(),
            goal_ok_at: Vec::new(),
            chime_due: false,
            fields: HashMap::new(),
            circuit_dirty: false,
            serial_tail: Vec::new(),
        };
        run.set_running(true);
        run.enter_step();
        Ok(run)
    }

    pub fn set_running(&mut self, on: bool) {
        if let Some(b) = self.bench.as_mut() {
            use crate::bench::BenchLink;
            b.set_running(on);
        }
    }

    pub fn is_running(&self) -> bool {
        use crate::bench::BenchLink;
        self.bench.as_ref().is_some_and(|b| b.is_running())
    }

    pub fn telemetry(&self) -> Option<Arc<Telemetry>> {
        self.bench.as_ref().and_then(|b| b.telemetry())
    }

    /// State reset when a step is entered.
    pub fn enter_step(&mut self) {
        self.goals.clear();
        self.goals_at = None;
        self.goals_ready = false;
        self.goals_step = self.session.step;
        self.guess.clear();
        self.meter_hist.clear();
        self.pending = None;
        self.recorded = None;
        self.show_hint = false;
        self.diagnosis = None;
        self.hints_shown = 0;
        self.show_me = None;
        self.show_me_used = false;
        self.player = None;
        self.did_it = false;
        self.player_error = None;
        self.passed_at = None;
        self.payoff.clear();
        self.goal_ok_at.clear();
        self.chime_due = false;
        self.fields.clear();
        self.serial_tail.clear();
        self.code = self.session.current().code.as_ref().map(|c| {
            let mut d = CodeDoc::new("sketch.ino", c.starter.clone());
            d.read_only = false;
            d
        });
        self.build = None;
        self.build_log.clear();
    }

    /// The circuit edits the student made reach the bench and the session.
    pub fn sync(&mut self) {
        let g = self.doc.generation();
        if g != self.bench_gen {
            self.bench_gen = g;
            // a diagnosis describes the circuit it was made for
            self.diagnosis = None;
            self.session.editor.project = self.doc.project().clone();
            if let Some(b) = self.bench.as_mut() {
                b.update_project(self.doc.project().clone());
            }
        }
        // checks that need probes (frequency, duty) are set up once per step
        if self.prepared != Some(self.session.step)
            && let Some(b) = &self.bench
        {
            let checks = self.session.current().checks.clone();
            if b.with_bench(move |bench| bb_lessons::check::prepare(&checks, bench))
                .is_some()
            {
                self.prepared = Some(self.session.step);
            }
        }
    }

    /// The step's instrument reading in the step's unit (`dmm1` value × 1000 for mA), `None`
    /// while it settles / ranges.
    pub fn meter(&self, tel: Option<&Telemetry>) -> Option<f64> {
        let tel = tel?;
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
        let pt = tel.parts.iter().find(|p| p.part == part)?;
        let get = |k: &str| pt.readings.iter().find(|(x, _)| x == k).map(|x| x.1);
        if get("settling").is_some_and(|s| s > 0.5) {
            return None;
        }
        get(&key).map(|v| v * scale).filter(|v| v.is_finite())
    }

    /// The meter's part id (masked "----" while it waits for the prediction).
    pub fn meter_part(&self) -> Option<String> {
        let step = self.session.current();
        step.measure
            .as_ref()
            .map(|m| m.part.clone())
            .or_else(|| step.predict.as_ref().and_then(|p| p.meter.clone()))
    }

    /// Feeds the meter history with this frame's reading (sim-time stamped; call every frame).
    pub fn track_meter(&mut self, tel: Option<&Telemetry>) {
        let Some(t) = tel else { return };
        let now = t.time.as_secs_f64();
        match self.meter(Some(t)) {
            Some(v) => {
                if self.meter_hist.last().is_none_or(|x| now > x.0) {
                    self.meter_hist.push((now, v));
                }
            }
            // ranging / settling: start over
            None => self.meter_hist.clear(),
        }
        self.meter_hist.retain(|x| now - x.0 < 2.0);
    }

    /// A reading that held still for 0.5 s of simulated time since the last change (the
    /// meter's display refresh + autorange have caught up), for checks and the table.
    pub fn settled_meter(&self) -> Option<f64> {
        let (t_last, v) = *self.meter_hist.last()?;
        let first = self.meter_hist.first()?.0;
        if t_last - first < 0.5 {
            return None;
        }
        let tol = (v.abs() * 0.01).max(0.02);
        self.meter_hist
            .iter()
            .filter(|x| t_last - x.0 <= 0.5)
            .all(|x| (x.1 - v).abs() <= tol)
            .then_some(v)
    }

    /// Records a settled reading for the current choice into "Your measurements".
    pub fn auto_record(&mut self, tel: Option<&Telemetry>) {
        let step = self.session.current().clone();
        if step.table.is_none() || self.session.bypassed() || self.session.meter_hidden() {
            return;
        }
        let Some(choice) = self.session.state().choice else {
            return;
        };
        let damaged = self.damaged(tel).is_some();
        let Some(m) = self.settled_meter() else {
            return;
        };
        if damaged {
            return;
        }
        let known = self
            .recorded
            .is_some_and(|(c, v)| c == choice && (v - m).abs() <= (m.abs() * 0.02).max(0.05));
        let has_row = self.session.state().rows.iter().any(|r| {
            step.choice
                .as_ref()
                .and_then(|c| c.options.get(choice))
                .is_some_and(|o| o.value == r.value)
        });
        if known && has_row {
            return;
        }
        if self.session.record(m) {
            self.recorded = Some((choice, m));
        }
    }

    /// A damaged part on the bench (the break-it card).
    pub fn damaged(&self, tel: Option<&Telemetry>) -> Option<String> {
        tel?.parts
            .iter()
            .find(|p| p.visual.health.is_damaged())
            .map(|p| p.reference.clone())
    }

    /// Picks option `i` of the step's choice: the circuit, the canvas and the bench follow.
    pub fn choose(&mut self, i: usize) -> Result<(), String> {
        self.session.editor.project = self.doc.project().clone();
        let edit = self.session.choose(i).map_err(|e| e.to_string())?;
        self.guess.clear();
        self.meter_hist.clear();
        self.pending = None;
        self.apply_edit(edit);
        Ok(())
    }

    fn apply_edit(&mut self, edit: Edit) {
        match edit {
            Edit::SetProp { part, prop, value } => {
                let lib = self.lib.clone();
                if self
                    .doc
                    .editor
                    .set_prop(&lib, &part, &prop, PropValue::Num(value))
                    .is_ok()
                {
                    self.doc.touched();
                }
                if let Some(b) = self.bench.as_mut() {
                    b.send(Command::SetPartParam {
                        part,
                        key: prop,
                        value,
                    });
                    b.adopt_project(self.doc.project().clone());
                }
                self.bench_gen = self.doc.generation();
            }
            Edit::Project(p) => {
                self.doc.editor.project = *p;
                self.doc.editor.generation += 1;
                self.doc.editor.electrical_generation += 1;
                self.doc.touched();
            }
        }
    }

    /// "Repair LED & pick a resistor": repairs the bench and re-picks the default option.
    pub fn repair(&mut self) {
        if let Some(b) = &self.bench {
            b.send(Command::RepairAll);
        }
        let default = self.session.current().choice.as_ref().map(|c| c.default);
        if let Some(d) = default {
            let _ = self.choose(d);
        }
    }

    /// Re-evaluates the step's goals on the engine thread (every 0.5 s).
    pub fn evaluate(&mut self, force: bool) {
        let due = force
            || self.goals_step != self.session.step
            || self
                .goals_at
                .is_none_or(|t| t.elapsed() > Duration::from_millis(500));
        if !due {
            return;
        }
        self.goals_at = Some(Instant::now());
        self.goals_step = self.session.step;
        let step = self.session.current().clone();
        let project = self.doc.project().clone();
        let lib = self.lib.clone();
        let state = self.session.state().clone();
        let goals = match &self.bench {
            Some(b) => b.with_bench(move |bench| {
                bb_lessons::check::evaluate_step(&step, &project, &lib, Some(bench), &state)
            }),
            None => Some(bb_lessons::check::evaluate_step(
                &step, &project, &lib, None, &state,
            )),
        };
        if let Some(g) = goals {
            self.goals = g;
            self.goals_ready = true;
            let passed = self.step_passed();
            self.session.mark(passed);
        }
        // what the MCU printed, for steps that check it
        let mcu = self.session.current().checks.iter().find_map(|c| match c {
            bb_lessons::format::Check::SerialContains { mcu, .. } => Some(mcu.clone()),
            _ => None,
        });
        if let (Some(mcu), Some(b)) = (mcu, &self.bench)
            && let Some(lines) = b.with_bench(move |bench| {
                bench
                    .serial(&mcu)
                    .map(|s| {
                        let mut v: Vec<String> = s
                            .data_lines()
                            .map(|l| l.text.trim_end().to_string())
                            .filter(|l| !l.is_empty())
                            .collect();
                        let n = v.len().saturating_sub(4);
                        v.drain(..n);
                        v
                    })
                    .unwrap_or_default()
            })
        {
            self.serial_tail = lines;
        }
    }

    /// Every goal of the step holds.
    pub fn step_passed(&self) -> bool {
        self.goals_ready && self.goals_step == self.session.step && self.goals.iter().all(|g| g.ok)
    }

    /// The prediction field → `Session::predict`. With an instrument on the step the check
    /// waits for its reading to settle after the last change, so the verdict ("the meter
    /// agrees: …") and the table quote what the meter really shows.
    pub fn check_prediction(&mut self, tel: Option<&Telemetry>) -> Option<bool> {
        let g = parse_guess(&self.guess)?;
        if self.bench.is_some() && self.meter_part().is_some() {
            self.pending = Some((g, Instant::now()));
            self.resolve_pending(tel);
            return Some(false);
        }
        Some(self.finish_check(g, None))
    }

    /// Completes a pending Check once the meter settled (or after 5 s, with what it shows).
    pub fn resolve_pending(&mut self, tel: Option<&Telemetry>) {
        let Some((g, since)) = self.pending else {
            return;
        };
        let m = match self.settled_meter() {
            Some(m) => Some(m),
            None if since.elapsed() > Duration::from_secs(5) => self.meter(tel),
            None => return,
        };
        self.pending = None;
        self.finish_check(g, m);
    }

    fn finish_check(&mut self, g: f64, meter: Option<f64>) -> bool {
        let v = self.session.predict(g, meter);
        if v.correct
            && let Some(m) = meter
        {
            self.session.record(m);
            if let Some(c) = self.session.state().choice {
                self.recorded = Some((c, m));
            }
        }
        self.evaluate(true);
        v.correct
    }

    /// Starts compiling the code step's sketch.
    pub fn compile(&mut self, repaint: Repaint) {
        let (Some(code), Some(step_code)) = (&self.code, self.session.current().code.clone())
        else {
            return;
        };
        let Some(fqbn) = bb_lessons::code::fqbn_for(self.doc.project(), &self.lib, &step_code.mcu)
        else {
            self.build_log = vec![format!("No board `{}` on this bench", step_code.mcu)];
            return;
        };
        let src = code.text.clone();
        self.build_log = vec![format!("Compiling sketch for {fqbn}…")];
        self.build = Some(Job::spawn("lesson-build", repaint, move |_p| {
            bb_lessons::code::compile(&src, &fqbn).map_err(|e| e.to_string())
        }));
    }

    /// Flashes a finished build (or reports its errors).
    pub fn poll_build(&mut self) {
        let Some(job) = self.build.as_mut() else {
            return;
        };
        let Some(r) = job.poll() else { return };
        self.build = None;
        let mcu = self
            .session
            .current()
            .code
            .as_ref()
            .map(|c| c.mcu.clone())
            .unwrap_or_default();
        match r {
            Ok(b) => {
                self.build_log.extend(
                    b.log
                        .lines()
                        .map(str::to_string)
                        .filter(|l| !l.trim().is_empty()),
                );
                if let Some(code) = self.code.as_mut() {
                    code.diagnostics = b
                        .diagnostics
                        .iter()
                        .filter(|d| d.line.is_some())
                        .map(|d| crate::widgets::EditorDiagnostic {
                            line: d.line.unwrap_or(1),
                            col: d.column,
                            severity: if d.severity == bb_firmware::Severity::Error {
                                crate::widgets::DiagSeverity::Error
                            } else {
                                crate::widgets::DiagSeverity::Warning
                            },
                            message: d.message.clone(),
                        })
                        .collect();
                }
                match (b.success, b.image) {
                    (true, Some(img)) => self.flash(&mcu, img),
                    _ => self
                        .build_log
                        .push("Compile failed · see the errors above".into()),
                }
            }
            Err(e) => {
                self.build_log.push(e);
                if self.reference_image().is_some() {
                    self.build_log
                        .push("No toolchain · load the reference build instead".into());
                }
            }
        }
    }

    /// The code step's prebuilt solution image (no toolchain).
    pub fn reference_image(&self) -> Option<PathBuf> {
        let c = self.session.current().code.as_ref()?;
        let rel = c.image.as_ref()?;
        let p = self.loaded.resolve(rel);
        p.exists().then_some(p)
    }

    pub fn flash_reference(&mut self) {
        let mcu = self
            .session
            .current()
            .code
            .as_ref()
            .map(|c| c.mcu.clone())
            .unwrap_or_default();
        if let Some(p) = self.reference_image() {
            match bb_sim::firmware::load_image(&p) {
                Ok(img) => self.flash(&mcu, img),
                Err(e) => self.build_log.push(format!("{}: {e}", p.display())),
            }
        }
    }

    fn flash(&mut self, mcu: &str, img: bb_avr::FirmwareImage) {
        if let Some(b) = &self.bench {
            b.send(Command::Flash {
                part: mcu.to_string(),
                image: Box::new(img),
            });
            let reference = self
                .doc
                .project()
                .part(mcu)
                .map(|p| p.reference.clone())
                .unwrap_or_else(|| mcu.to_string());
            self.build_log
                .push(format!("Flashed virtual {reference} · bench state kept"));
            self.session.set_firmware_loaded(true);
            self.evaluate(true);
        }
    }

    /// Next step (marks progress); `false` at the end of the lesson.
    pub fn next(&mut self, progress: &mut Progress) -> bool {
        let id = self.session.lesson.id.clone();
        progress.step_done(&id, self.session.step);
        progress.record(&id, self.session.step, self.session.state().rows.clone());
        let moved = self.session.next_step();
        if moved {
            progress.set_current(&id, self.session.step);
            self.enter_step();
        } else {
            progress.complete(&id);
        }
        let _ = progress.save();
        moved
    }

    pub fn back(&mut self, progress: &mut Progress) {
        if self.session.prev_step() {
            progress.set_current(&self.session.lesson.id, self.session.step);
            let _ = progress.save();
            self.enter_step();
        }
    }

    // ------------------------------------------------------------------ feedback

    /// "Test circuit": diagnoses the step now (on the engine thread, like the goals).
    pub fn test_circuit(&mut self) {
        let step = self.session.current().clone();
        let project = self.doc.project().clone();
        let lib = self.lib.clone();
        let state = self.session.state().clone();
        let opts = bb_lessons::diagnose::Options {
            running: self.bench.is_none() || self.is_running(),
            compile_errors: self.compile_errors(),
        };
        let d = match &self.bench {
            Some(b) => b.with_bench(move |bench| {
                bb_lessons::diagnose::diagnose(&step, &project, &lib, Some(bench), &state, &opts)
            }),
            None => Some(bb_lessons::diagnose::diagnose(
                &step, &project, &lib, None, &state, &opts,
            )),
        };
        if let Some(d) = d {
            self.diagnosis = Some(d);
        }
        self.evaluate(true);
    }

    /// Errors of the last compile of the student's sketch (`line 5: expected ';'`).
    pub fn compile_errors(&self) -> Vec<String> {
        self.code
            .as_ref()
            .map(|c| {
                c.diagnostics
                    .iter()
                    .filter(|d| d.severity == crate::widgets::DiagSeverity::Error)
                    .map(|d| format!("line {}: {}", d.line, d.message))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The next hint (hint 1, hint 2 …).
    pub fn next_hint(&mut self) {
        let n = self.session.current().hints.len();
        self.hints_shown = (self.hints_shown + 1).min(n);
    }

    /// Opens / closes "Show me".
    pub fn toggle_show_me(&mut self) {
        if self.show_me.is_some() {
            self.show_me = None;
        } else {
            self.show_me = Some(bb_lessons::guide::show_me(
                self.session.current(),
                self.doc.project(),
                &self.lib,
            ));
            self.show_me_used = true;
            self.show_me_at = Some(Instant::now());
        }
    }

    /// "Do it for me": replays the step's worked solution on the live bench.
    pub fn do_it(&mut self) {
        let queue: VecDeque<Action> = self.session.current().solution.iter().cloned().collect();
        self.show_me = None;
        self.player_error = None;
        self.diagnosis = None;
        self.player = Some(Player {
            queue,
            wait_until: None,
            retry_since: None,
            started: Instant::now(),
        });
        self.tick_player();
    }

    fn sim_time(&self) -> f64 {
        self.telemetry()
            .map(|t| t.time.as_secs_f64())
            .unwrap_or(0.0)
    }

    /// Advances "Do it for me" (call every frame).
    pub fn tick_player(&mut self) {
        let Some(mut pl) = self.player.take() else {
            return;
        };
        let now = self.sim_time();
        loop {
            if let Some(until) = pl.wait_until {
                if self.bench.is_some()
                    && now < until
                    && pl.started.elapsed() < Duration::from_secs(30)
                {
                    self.player = Some(pl);
                    return;
                }
                pl.wait_until = None;
            }
            let Some(a) = pl.queue.pop_front() else {
                break;
            };
            if let Action::Run { seconds } = a {
                pl.wait_until = Some(now + seconds.max(0.0));
                continue;
            }
            self.session.editor.project = self.doc.project().clone();
            if bb_lessons::apply::is_circuit(&a) {
                // consecutive circuit edits become one undoable step
                let mut group = vec![a];
                while pl.queue.front().is_some_and(bb_lessons::apply::is_circuit) {
                    group.extend(pl.queue.pop_front());
                }
                let mut err = None;
                for g in &group {
                    if let Err(e) = bb_lessons::apply::apply(self, g) {
                        err = Some(e);
                        break;
                    }
                }
                self.commit_circuit();
                if let Some(e) = err {
                    self.player_error = Some(e.to_string());
                    break;
                }
                continue;
            }
            match bb_lessons::apply::apply(self, &a) {
                Ok(()) => pl.retry_since = None,
                Err(e) => {
                    // the bench may not have caught up yet (the phone's scan, a meter settling)
                    let since = *pl.retry_since.get_or_insert_with(Instant::now);
                    if since.elapsed() < Duration::from_secs(6) {
                        pl.queue.push_front(a);
                        pl.wait_until = Some(now + 0.05);
                        self.player = Some(pl);
                        return;
                    }
                    self.player_error = Some(e.to_string());
                    break;
                }
            }
        }
        self.did_it = self.player_error.is_none();
        self.evaluate(true);
    }

    /// The circuit edits the player made reach the document as one undo step.
    fn commit_circuit(&mut self) {
        if !self.circuit_dirty {
            return;
        }
        self.circuit_dirty = false;
        let next = self.session.editor.project.clone();
        if self.doc.editor.replace_circuit("Do it for me", &next) {
            self.doc.touched();
        }
        self.sync();
    }

    /// Success tracking (call every frame after `evaluate`): goal ticks, the payoff, the chime.
    pub fn track_success(&mut self) {
        if self.goal_ok_at.len() != self.goals.len() {
            self.goal_ok_at = vec![None; self.goals.len()];
        }
        for (i, g) in self.goals.iter().enumerate() {
            if !g.ok {
                self.goal_ok_at[i] = None;
            } else if self.goal_ok_at[i].is_none() {
                self.goal_ok_at[i] = Some(Instant::now());
            }
        }
        let passed = self.step_passed() && !self.goals.is_empty();
        if passed && self.passed_at.is_none() {
            self.passed_at = Some(Instant::now());
            let conn = self.doc.connectivity();
            let n = bb_lessons::names::Namer::new(self.doc.project(), &self.lib, &conn);
            self.payoff = bb_lessons::diagnose::payoff(self.session.current(), &n, &self.goals);
            self.chime_due = self.session.current().is_hands_on()
                || self.session.current().quiz.is_some()
                || self.session.current().predict.is_some();
            self.show_me = None;
            self.diagnosis = None;
        } else if !passed
            && self
                .passed_at
                .is_some_and(|t| t.elapsed() > Duration::from_secs(1))
        {
            // the circuit changed back: the success state goes (it plays again next time)
            self.passed_at = None;
        }
    }

    /// Parts the step is about (they glow once when the goals pass).
    pub fn celebrated_parts(&self) -> Vec<String> {
        use bb_lessons::format::Check;
        let mut out = Vec::new();
        for c in &self.session.current().checks {
            let p = match c {
                Check::LedLit { part, .. } | Check::LedOff { part } | Check::Damaged { part } => {
                    part.clone()
                }
                Check::Reading { part, .. } => part.clone(),
                Check::PinFrequency { mcu, .. } | Check::SerialContains { mcu, .. } => mcu.clone(),
                Check::Voltage { a, .. } => bb_lessons::names::split(a).0.to_string(),
                _ => continue,
            };
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// What the bench canvas draws for the lesson.
    pub fn overlay(&self) -> crate::canvas::lesson_overlay::Overlay {
        let mut o = crate::canvas::lesson_overlay::Overlay {
            net_hints: true,
            ..Default::default()
        };
        if let Some(d) = &self.diagnosis {
            o.marks.extend(d.marks());
        }
        if let Some(s) = &self.show_me {
            o.marks.extend(s.marks().cloned());
            o.ghosts = s.moves.iter().filter_map(|m| m.ghost.clone()).collect();
            o.ghost_age = self
                .show_me_at
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(9.0);
        }
        if let Some(t) = self.passed_at {
            o.celebrate = self.celebrated_parts();
            o.celebrate_age = t.elapsed().as_secs_f64();
        }
        o.dedup();
        o
    }

    /// Marks pointing at a step-panel control (`choice:3`, `compile`, `world:…`).
    pub fn control_marks(&self) -> Vec<Mark> {
        let mut v: Vec<Mark> = Vec::new();
        if let Some(d) = &self.diagnosis {
            v.extend(d.marks());
        }
        if let Some(s) = &self.show_me {
            v.extend(s.marks().cloned());
        }
        v.retain(|m| matches!(m.spot, Spot::Control(_)));
        v
    }

    /// A step-panel control is pointed at.
    pub fn is_marked(&self, id: &str) -> bool {
        self.control_marks()
            .iter()
            .any(|m| m.spot == Spot::Control(id.to_string()))
    }

    /// Sets a world variable on the bench (the panel's controls).
    pub fn set_world(&mut self, key: &str, value: f64) {
        if let Some(b) = &self.bench {
            b.send(Command::SetWorld {
                key: key.to_string(),
                value,
            });
        }
    }

    /// A world variable from telemetry (the known ones; else the start value).
    pub fn world_value(&self, tel: Option<&Telemetry>, key: &str) -> Option<f64> {
        let w = &tel?.world;
        let k = key.to_ascii_lowercase();
        if k.ends_with("ambient_lux") {
            Some(w.ambient_lux)
        } else if k.ends_with("temp_c") {
            Some(w.temp_c)
        } else if k.ends_with(".rh") {
            Some(w.rh)
        } else if let Some(pot) = k.strip_suffix(".vwc") {
            w.pots.iter().find(|p| p.id == pot).map(|p| p.vwc)
        } else {
            None
        }
    }

    /// Sets a part property from the panel (an undoable edit; live on the bench).
    pub fn set_prop_text(&mut self, part: &str, prop: &str, text: &str) -> Result<(), String> {
        let v = bb_core::si::parse(text).map_err(|e| e.to_string())?;
        let lib = self.lib.clone();
        self.doc
            .editor
            .set_prop(&lib, part, prop, bb_circuit::PropValue::Num(v))
            .map_err(|e| e.to_string())?;
        self.doc.touched();
        if let Some(b) = self.bench.as_mut() {
            b.send(Command::SetPartParam {
                part: part.into(),
                key: prop.into(),
                value: v,
            });
        }
        Ok(())
    }

    /// Adds a part the step's worked solution adds ("Add R3"), where the solution puts it.
    pub fn add_solution_part(&mut self, reference: &str) {
        let Some(a) = self
            .session
            .current()
            .solution
            .iter()
            .find(|a| matches!(a, Action::AddPart { reference: r, .. } if r.eq_ignore_ascii_case(reference)))
            .cloned()
        else {
            return;
        };
        self.session.editor.project = self.doc.project().clone();
        if bb_lessons::apply::apply(self, &a).is_ok() {
            self.circuit_dirty = true;
            self.commit_circuit();
        }
    }

    /// Sends a radio command (the panel's phone / MQTT controls).
    pub fn radio(&mut self, cmd: bb_sim::radio::RadioCommand) {
        if let Some(b) = &self.bench {
            b.send(Command::Radio(cmd));
        }
    }

    /// Connects the virtual phone to the device advertising `name`.
    pub fn phone_connect(&mut self, name: &str) -> Result<(), String> {
        bb_lessons::apply::Target::phone_connect(self, name).map_err(|e| e.to_string())
    }

    /// "Open in free play" (LES-5): the lesson bench as a Playground project folder.
    pub fn free_play(&mut self, parent: &std::path::Path) -> Result<PathBuf, String> {
        self.session.editor.project = self.doc.project().clone();
        let p = self.session.free_play();
        let base: String = p
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut dir = parent.join(format!("{base}.breadbox"));
        let mut n = 2;
        while dir.exists() {
            dir = parent.join(format!("{base} {n}.breadbox"));
            n += 1;
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // sketches + images the circuit refers to come along
        for part in &p.parts {
            if let Some(fw) = &part.firmware {
                for rel in [Some(fw.sketch.clone()), fw.image.clone()]
                    .into_iter()
                    .flatten()
                {
                    if rel.is_empty() {
                        continue;
                    }
                    let src = self.dir.join(&rel);
                    let dst = dir.join(&rel);
                    if src.is_dir() {
                        let _ = copy_tree(&src, &dst);
                    } else if src.is_file() {
                        if let Some(d) = dst.parent() {
                            let _ = std::fs::create_dir_all(d);
                        }
                        let _ = std::fs::copy(&src, &dst);
                    }
                }
            }
        }
        p.save(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

impl bb_lessons::apply::Target for LessonRun {
    fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    fn circuit_changed(&mut self, _live: Option<(&str, &str, f64)>) -> Result<(), LessonError> {
        // committed to the document (one undo step) once the group of edits is done
        self.circuit_dirty = true;
        Ok(())
    }

    fn edit(&mut self, e: Edit) -> Result<(), LessonError> {
        self.apply_edit(e);
        Ok(())
    }

    fn meter(&self) -> Option<f64> {
        let tel = self.telemetry();
        self.settled_meter()
            .or_else(|| LessonRun::meter(self, tel.as_deref()))
    }

    fn run(&mut self, _seconds: f64) -> Result<(), LessonError> {
        // the live bench runs by itself; the player waits for `run`
        Ok(())
    }

    fn input(&mut self, part: &str, input: bb_core::UserInput) -> Result<(), LessonError> {
        match &self.bench {
            Some(b) => {
                b.send(Command::Input {
                    part: part.into(),
                    input,
                });
                Ok(())
            }
            None => Err(LessonError::Bench(
                "this lesson has no running bench".into(),
            )),
        }
    }

    fn set_world(&mut self, key: &str, value: f64) -> Result<(), LessonError> {
        LessonRun::set_world(self, key, value);
        Ok(())
    }

    fn flash(&mut self, mcu: &str, image: bb_avr::FirmwareImage) -> Result<(), LessonError> {
        LessonRun::flash(self, mcu, image);
        Ok(())
    }

    fn repair(&mut self) -> Result<(), LessonError> {
        if let Some(b) = &self.bench {
            b.send(Command::RepairAll);
        }
        Ok(())
    }

    fn phone_connect(&mut self, name: &str) -> Result<(), LessonError> {
        let id = self
            .telemetry()
            .and_then(|t| {
                t.radio
                    .as_ref()
                    .and_then(|r| r.phone.scan.iter().find(|s| s.name == *name).map(|s| s.id))
            })
            .ok_or_else(|| LessonError::Bench(format!("the phone does not see `{name}`")))?;
        self.radio(bb_sim::radio::RadioCommand::Phone(
            bb_sim::radio::PhoneCommand::Connect { id, pin: None },
        ));
        Ok(())
    }

    fn radio(&mut self, cmd: bb_sim::radio::RadioCommand) -> Result<(), LessonError> {
        LessonRun::radio(self, cmd);
        Ok(())
    }
}

/// `13.6`, `13,6`, `13.6 mA`, `.5` → number.
pub fn parse_guess(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .replace(',', ".")
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == 'e' || *c == 'E')
        .collect();
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_parse_like_people_type_them() {
        assert_eq!(parse_guess("13.6"), Some(13.6));
        assert_eq!(parse_guess(" 13,6 mA"), Some(13.6));
        assert_eq!(parse_guess(".5"), Some(0.5));
        assert_eq!(parse_guess("abc"), None);
    }
}
