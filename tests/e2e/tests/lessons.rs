//! TEST-4 · lesson 3 ("Ohm's law with an LED") completed the way the Lessons screen drives
//! it: `bb_lessons::Session` for the step state (choices, "Do the math myself", predictions,
//! hints, measurements table, quiz), the threaded engine for the bench (edits as
//! `SetPartParam` / `UpdateProject`, `RepairAll`), the Handheld DMM plugin from the registry as
//! the meter, goals evaluated by the checkers on the engine thread, progress saved and reloaded.

use bb_lessons::check::{self, CheckResult};
use bb_lessons::session::{Edit, Session};
use bb_lessons::{Catalog, Progress};
use bb_sim::Command;
use breadbox_e2e::*;
use std::sync::Arc;

struct Lesson {
    session: Session,
    sim: Sim,
    lib: Arc<bb_circuit::Library>,
}

impl Lesson {
    fn apply(&mut self, e: Edit) {
        match e {
            Edit::SetProp { part, prop, value } => self.sim.send(Command::SetPartParam {
                part,
                key: prop,
                value,
            }),
            Edit::Project(p) => self.sim.send(Command::UpdateProject(p)),
        }
    }

    /// The step's meter reading in its unit (DMM value × 1000 for mA), from telemetry.
    fn meter(&self) -> f64 {
        let step = self.session.current();
        let (part, scale) = match (&step.measure, &step.predict) {
            (Some(m), _) => (m.part.clone(), m.scale),
            (None, Some(p)) => (p.meter.clone().unwrap(), 1000.0),
            _ => panic!("no meter on this step"),
        };
        self.sim.reading(&part, "value").expect("meter value") * scale
    }

    /// Enter the current step: the checkers' probes go onto the bench.
    fn enter(&mut self) {
        let checks = self.session.current().checks.clone();
        self.sim.with(move |b| check::prepare(&checks, b));
    }

    /// Evaluates the step's goals on the engine thread.
    fn evaluate(&self) -> Vec<CheckResult> {
        let step = self.session.current().clone();
        let project = self.session.project().clone();
        let state = self.session.state().clone();
        let lib = self.lib.clone();
        self.sim
            .with(move |b| check::evaluate_step(&step, &project, &lib, Some(b), &state))
    }

    fn assert_goals(&self) {
        let r = self.evaluate();
        assert!(
            r.iter().all(|c| c.ok),
            "{}: {}\n{r:#?}",
            self.session.label(),
            self.session.current().title
        );
    }

    fn next(&mut self, progress: &mut Progress) {
        let id = self.session.lesson.id.clone();
        progress.step_done(&id, self.session.step);
        assert!(self.session.next_step());
        progress.set_current(&id, self.session.step);
        self.enter();
    }
}

#[test]
fn lesson_3_ohms_law_completes_through_the_checkers() {
    let scratch = TempDir::new("lesson3");
    let mut reg = plugin_registry(&scratch.path().join("plugins"));
    let host = bb_plugin::PluginHost::new(&mut reg, None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let mut cat = Catalog::builtin();
    cat.add_packs(&host.lesson_dirs());
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    let track = cat.track("electricity").expect("Track 1");
    let loaded = track.lessons[2].clone();
    assert_eq!(loaded.lesson.id, "ohms-law-led");
    assert_eq!(loaded.lesson.title, "Ohm's law with an LED");
    let lib = host.library();
    let session = Session::new(&loaded, lib.clone()).unwrap();
    let project = session.project().clone();
    let mut opts = host.bench_options();
    opts.firmware = loaded.images(&project).unwrap();
    let sim = Sim::spawn(project, lib.clone(), opts);
    for (k, v) in &loaded.lesson.bench.world {
        sim.send(Command::SetWorld {
            key: k.clone(),
            value: *v,
        });
    }
    let mut l = Lesson { session, sim, lib };
    let progress_file = scratch.path().join("progress.json");
    let mut progress = Progress::load(&progress_file).unwrap();
    l.enter();

    // ---- step 1: explain
    assert_eq!(l.session.label(), "Step 1 of 5");
    l.sim.run_secs(0.5);
    l.assert_goals();
    l.next(&mut progress);

    // ---- step 2: measure the current with R1 = 220 Ω (the meter is in series)
    l.sim.run_secs(1.0);
    let ma = l.meter();
    assert!((ma - 13.6).abs() < 1.5, "meter reads {ma} mA");
    l.assert_goals();
    l.next(&mut progress);

    // ---- step 3: the design's step — pick R1, do the math, check against the meter
    assert_eq!(l.session.label(), "Step 3 of 5");
    let step = l.session.current().clone();
    assert_eq!(step.formula.as_deref(), Some("I = (5.0 V − 2.0 V) ÷ R"));
    l.session.set_math(true);
    for (idx, ohms) in [(0usize, 100.0f64), (1, 220.0), (3, 1000.0)] {
        let e = l.session.choose(idx).unwrap();
        assert_eq!(
            e,
            Edit::SetProp {
                part: "r1".into(),
                prop: "resistance".into(),
                value: ohms
            }
        );
        l.apply(e);
        l.sim.run_secs(1.0);
        assert!(
            l.session.meter_hidden(),
            "the meter waits for the prediction"
        );
        let meter = l.meter();
        let exact = 3.0 / ohms * 1000.0;
        // the DMM in the loop agrees with Ohm's law (the LED's Vf is not exactly 2.0 V)
        assert!(
            (meter - exact).abs() < exact * 0.15,
            "{ohms} Ω: meter {meter} mA vs {exact}"
        );
        // a classic mistake first: the full 5 V across R1
        let v = l.session.predict(5000.0 / ohms, Some(meter));
        assert!(!v.correct);
        assert!(
            v.text.starts_with("Looks like you used the full 5 V"),
            "{}",
            v.text
        );
        // amps instead of mA
        let v = l.session.predict(3.0 / ohms, Some(meter));
        assert!(!v.correct);
        assert!(v.text.starts_with("Right number, wrong unit"), "{}", v.text);
        assert!(!l.session.record(meter), "no row before a right prediction");
        let (passed, _) = l.session.checkpoint();
        if idx == 0 {
            assert!(!passed);
        }
        // the working, then the right answer
        let working = l.session.working();
        assert_eq!(working[0], "V(R1) = 5.0 − 2.0 = 3.0 V", "{working:?}");
        let v = l.session.predict(exact, Some(meter));
        assert!(v.correct, "{}", v.text);
        assert!(
            v.text.starts_with("Nailed it. The meter agrees: "),
            "{}",
            v.text
        );
        assert!(!l.session.meter_hidden());
        assert!(l.session.record(meter));
    }
    let (passed, text) = l.session.checkpoint();
    assert!(passed, "{text}");
    assert_eq!(text, "Checkpoint passed");
    let rows = l.session.state().rows.clone();
    assert_eq!(rows.len(), 3, "{rows:#?}");
    assert_eq!(rows[0].label, "100Ω");
    l.assert_goals();
    progress.record(&l.session.lesson.id, l.session.step, rows);
    l.next(&mut progress);

    // ---- step 4: break it on purpose ("None" bypasses R1)
    let e = l.session.choose(5).unwrap();
    assert!(l.session.bypassed());
    l.apply(e);
    l.sim.run_secs(0.5);
    let dmg = l.sim.damage();
    assert!(dmg.iter().any(|(s, _)| s == "D1"), "the LED pops: {dmg:?}");
    l.assert_goals();
    assert_eq!(l.session.current().damage_title, "Pop. That LED is gone.");
    // "Repair LED & pick a resistor": Repair all + back to the default choice
    assert_eq!(
        l.session.current().repair_label,
        "Repair LED & pick a resistor"
    );
    let default = l.session.current().choice.as_ref().unwrap().default;
    let e = l.session.choose(default).unwrap();
    l.apply(e);
    l.sim.send(Command::RepairAll);
    l.sim.run_secs(0.5);
    l.next(&mut progress);

    // ---- step 5: quiz (LED healthy and lit again)
    assert!(!l.session.answer_quiz(0));
    let r = l.evaluate();
    assert!(
        r.iter().any(|c| !c.ok),
        "a wrong answer does not pass: {r:#?}"
    );
    assert!(l.session.answer_quiz(2));
    l.sim.run_secs(0.5);
    l.assert_goals();
    assert!(l.session.is_last());
    let id = l.session.lesson.id.clone();
    progress.step_done(&id, l.session.step);
    progress.complete(&id);
    progress.save().unwrap();

    // progress survives a reload; "Open in free play" copies the bench into a Playground project
    let again = Progress::load(&progress_file).unwrap();
    assert!(again.is_done("ohms-law-led"));
    assert_eq!(again.lesson("ohms-law-led").unwrap().steps_done.len(), 5);
    assert_eq!(
        again.lesson("ohms-law-led").unwrap().measurements[&2].len(),
        3
    );
    let free = l.session.free_play();
    assert_eq!(free.mode, bb_circuit::project::Mode::Playground);
    assert!(free.part("dmm1").is_some() && free.part("d1").is_some());
    l.sim.shutdown();
}
