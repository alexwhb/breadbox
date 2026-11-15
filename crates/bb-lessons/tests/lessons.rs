//! Lesson content and engine model: every built-in lesson loads and validates, Track 1 is the
//! design's eight lessons, lesson 3's step 3 is the design's step verbatim (texts, hints,
//! working, measurements table, checkpoint, damage), progress persists, lesson files round-trip
//! through the editor model, and a lesson made with the editor installs as a lesson-pack plugin
//! and is solvable.

use bb_lessons::editor::{Draft, PackInfo};
use bb_lessons::format::{Action, Check};
use bb_lessons::{Catalog, LoadedLesson, Progress, Runner, Session};
use bb_plugin::PluginHost;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn host() -> PluginHost {
    let h = PluginHost::from_dirs(
        &[
            root().join("plugins/dmm-handheld"),
            root().join("plugins/lora-gateway"),
        ],
        None,
    );
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    h
}

struct Tmp(PathBuf);
impl Tmp {
    fn new(name: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "bb-lessons-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lesson(id: &str) -> LoadedLesson {
    Catalog::builtin().lesson(id).unwrap().clone()
}

#[test]
fn every_lesson_loads_and_validates() {
    let cat = Catalog::builtin();
    assert!(cat.errors.is_empty(), "{:#?}", cat.errors);
    let ids: Vec<&str> = cat.tracks.iter().map(|t| t.def.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "electricity",
            "sensors",
            "microcontrollers",
            "radios",
            "spice"
        ]
    );
    let lib = host().library();
    let mut n = 0;
    let mut problems = Vec::new();
    for t in &cat.tracks {
        assert!(
            t.lessons.len() >= 4,
            "{}: {} lessons",
            t.def.id,
            t.lessons.len()
        );
        for (i, l) in t.lessons.iter().enumerate() {
            assert_eq!(l.lesson.track, t.def.id, "{}", l.lesson.id);
            assert_eq!(l.lesson.order as usize, i + 1, "{} order", l.lesson.id);
            for issue in bb_lessons::validate(l, &lib) {
                problems.push(format!("{}: {issue}", l.lesson.id));
            }
            // the start circuit is a real bb-circuit project that passes its own checks
            let p = l.start_project().unwrap();
            let check = p.check(&lib);
            assert!(check.is_empty(), "{}: {check:?}", l.lesson.id);
            n += 1;
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(n, 24);
    // lesson ids are unique
    let mut all: Vec<&str> = cat.all().map(|l| l.lesson.id.as_str()).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n);
}

#[test]
fn track_one_is_the_designs_eight_lessons() {
    let cat = Catalog::builtin();
    let t = cat.track("electricity").unwrap();
    assert_eq!(t.def.label, "Track 1 · Electricity you can see");
    let titles: Vec<&str> = t.lessons.iter().map(|l| l.lesson.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "What is a circuit?",
            "Volts, amps, ohms",
            "Ohm's law with an LED",
            "Series & parallel",
            "Voltage dividers",
            "Capacitors & time",
            "Buttons & pull-ups",
            "Your first sketch: blink",
        ]
    );
    let more: Vec<&str> = cat.tracks[1..]
        .iter()
        .map(|t| t.def.title.as_str())
        .collect();
    assert_eq!(
        more,
        [
            "Sensors & the real world",
            "Microcontrollers from zero",
            "Radios & IoT",
            "Pro: SPICE analysis",
        ]
    );
}

#[test]
fn lesson_three_step_three_is_the_design_verbatim() {
    let l = lesson("ohms-law-led");
    let h = host();
    let mut s = Session::new(&l, h.library()).unwrap();
    assert_eq!(s.lesson_label(8), "Lesson 3 of 8");
    s.next_step();
    s.next_step();
    assert_eq!(s.label(), "Step 3 of 5");
    let st = s.current().clone();
    assert_eq!(st.title, "Change the resistor, watch the current");
    assert_eq!(
        st.body,
        "A red LED uses up about 2.0 V. Whatever is left of the 5 V lands on the resistor, and the resistor decides how much current flows."
    );
    assert_eq!(st.formula.as_deref(), Some("I = (5.0 V − 2.0 V) ÷ R"));
    assert_eq!(st.callouts[0].title, "Meter is wired in series");
    assert_eq!(
        st.callouts[0].text,
        "current must flow through it to be measured"
    );
    let c = st.choice.as_ref().unwrap();
    assert_eq!(c.label, "Pick R1");
    let labels: Vec<&str> = c.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["100Ω", "220Ω", "470Ω", "1kΩ", "10kΩ", "None"]);
    let p = st.predict.as_ref().unwrap();
    assert_eq!(p.toggle_label, "Do the math myself");
    assert_eq!(
        p.toggle_note,
        "Optional. The meter waits for your prediction."
    );
    assert_eq!(p.unit, "mA");
    assert_eq!(s.next_label(), "Next: break it on purpose");
    assert_eq!(st.damage_title, "Pop. That LED is gone.");
    assert_eq!(
        st.damage_text,
        "With no resistor, only the LED's own ~6 Ω limits the current: about 3.0 V ÷ 6 Ω ≈ 500 mA, 25× its 20 mA rating. This is exactly why every LED gets a resistor. Good thing it's a sim."
    );
    assert_eq!(st.repair_label, "Repair LED & pick a resistor");
    let t = st.table.as_ref().unwrap();
    assert_eq!(t.title, "Your measurements");
    assert_eq!(t.columns, ["R1", "you said", "meter"]);

    // the design's state: 220 Ω picked, "Do the math myself" on, the meter waiting
    assert_eq!(s.state().choice, Some(1));
    assert!(s.state().math);
    assert!(s.meter_hidden());
    assert_eq!(
        s.render(&p.prompt, None),
        "What current do you expect with 220Ω?"
    );
    assert_eq!(s.checkpoint(), (false, "Try 3 more values".to_string()));

    // the four hints, in the design's order
    let hint = |s: &mut Session, g: f64| s.predict(g, Some(13.4)).text;
    assert_eq!(
        hint(&mut s, 5000.0 / 220.0),
        "Looks like you used the full 5 V. The LED takes ~2.0 V first, so only 3.0 V is left across R1."
    );
    assert_eq!(
        hint(&mut s, 0.0136),
        "Right number, wrong unit: that’s amps. Multiply by 1000 to get mA."
    );
    assert_eq!(
        hint(&mut s, 13636.0),
        "Off by 1000. Volts ÷ ohms gives amps; you’re one step from mA."
    );
    assert_eq!(
        hint(&mut s, 40.0),
        "Not quite. Take 5.0 − 2.0 first, divide by R1, then × 1000 for mA."
    );
    assert!(s.meter_hidden(), "a wrong guess keeps the meter waiting");
    assert!(!s.record(13.4), "no row before a right prediction");

    // "Show me the working"
    assert_eq!(
        s.working(),
        [
            "V(R1) = 5.0 − 2.0 = 3.0 V",
            "I = 3.0 V ÷ 220 Ω",
            "I = 0.01364 A = 13.6 mA"
        ]
    );
    assert!(s.toggle_working());
    assert!(!s.meter_hidden(), "the working reveals the meter");
    s.toggle_working();

    // right answer → the design's message, a row, the checkpoint counts down
    let v = s.predict(13.6, Some(13.4));
    assert!(v.correct);
    assert_eq!(v.text, "Nailed it. The meter agrees: 13.6 mA.");
    assert!(!s.meter_hidden());
    assert!(s.record(13.4));
    let row = &s.state().rows[0];
    assert_eq!(
        (
            row.label.as_str(),
            row.predicted.as_str(),
            row.meter.as_str()
        ),
        ("220Ω", "13.6 mA", "13.4 mA")
    );
    assert_eq!(s.checkpoint(), (false, "Try 2 more values".to_string()));
    s.choose(3).unwrap();
    assert_eq!(s.working()[1], "I = 3.0 V ÷ 1000 Ω");
    assert_eq!(s.working()[2], "I = 0.003 A = 3.00 mA");
    s.predict(3.0, Some(2.9));
    s.record(2.9);
    assert_eq!(s.checkpoint(), (false, "Try 1 more value".to_string()));
    // with the math off, a pick records straight away with "—"
    s.set_math(false);
    s.choose(0).unwrap();
    assert!(s.record(28.1));
    assert_eq!(s.state().rows.last().unwrap().predicted, "—");
    assert_eq!(s.checkpoint(), (true, "Checkpoint passed".to_string()));
    // "None" bypasses the resistor
    let e = s.choose(5).unwrap();
    assert!(s.bypassed());
    assert!(matches!(e, bb_lessons::session::Edit::SetProp { value, .. } if value < 0.1));
}

#[test]
fn none_pops_the_led_on_the_bench_and_repair_brings_it_back() {
    let l = lesson("ohms-law-led");
    let h = host();
    let mut r = Runner::new(&l, &h).unwrap();
    r.next_step();
    r.next_step();
    r.apply(&Action::Run { seconds: 1.0 }).unwrap();
    let m = r.meter().unwrap();
    assert!((12.5..14.5).contains(&m), "220 Ω: {m} mA");
    r.apply(&Action::Choose { index: 5 }).unwrap();
    r.apply(&Action::Run { seconds: 1.0 }).unwrap();
    let b = r.bench.as_mut().unwrap();
    assert!(b.health("d1").is_damaged());
    assert!(
        r.meter().unwrap().abs() < 0.05,
        "a burnt LED is open: {:?}",
        r.meter()
    );
    // "Repair LED & pick a resistor"
    r.apply(&Action::Choose { index: 1 }).unwrap();
    r.apply(&Action::Repair).unwrap();
    r.apply(&Action::Run { seconds: 1.0 }).unwrap();
    assert!(!r.bench.as_mut().unwrap().health("d1").is_damaged());
    assert!((12.5..14.5).contains(&r.meter().unwrap()));
}

#[test]
fn free_play_and_navigation() {
    let l = lesson("voltage-dividers");
    let h = host();
    let mut s = Session::new(&l, h.library()).unwrap();
    assert!(!s.prev_step());
    assert!(s.next_step());
    assert_eq!(s.next_label(), "Next: load it down");
    while s.next_step() {}
    assert!(s.is_last());
    assert_eq!(s.next_label(), "Finish lesson");
    let p = s.free_play();
    assert_eq!(p.mode, bb_circuit::project::Mode::Playground);
    assert_eq!(p.name, "Voltage dividers (free play)");
    assert!(p.part("r1").is_some() && p.part("dmm1").is_some());
}

#[test]
fn progress_persists_and_resumes() {
    let tmp = Tmp::new("progress");
    let path = tmp.0.join("lessons/progress.json");
    let mut p = Progress::load(&path).unwrap();
    assert_eq!(p.resume("ohms-law-led"), 0);
    p.step_done("ohms-law-led", 0);
    p.step_done("ohms-law-led", 1);
    p.record(
        "ohms-law-led",
        2,
        vec![bb_lessons::session::Row {
            label: "220Ω".into(),
            value: 220.0,
            predicted: "13.6 mA".into(),
            meter: "13.4 mA".into(),
        }],
    );
    p.complete("what-is-a-circuit");
    p.complete("volts-amps-ohms");
    p.save().unwrap();
    let q = Progress::load(&path).unwrap();
    assert_eq!(q, p);
    assert_eq!(q.resume("ohms-law-led"), 2);
    assert!(q.is_done("volts-amps-ohms") && !q.is_done("ohms-law-led"));
    let cat = Catalog::builtin();
    let ids = cat
        .track("electricity")
        .unwrap()
        .lessons
        .iter()
        .map(|l| l.lesson.id.as_str());
    assert_eq!(q.done_count(ids), 2);
    assert_eq!(
        q.lesson("ohms-law-led").unwrap().measurements[&2][0].meter,
        "13.4 mA"
    );
}

#[test]
fn every_lesson_round_trips_through_the_editor_model() {
    let cat = Catalog::builtin();
    for t in &cat.tracks {
        for l in &t.lessons {
            let d = Draft::open(l, t.def.clone()).unwrap();
            let text = d.to_toml().unwrap();
            let back = bb_lessons::parse(&text, &l.lesson.id).unwrap();
            assert_eq!(back.steps, l.lesson.steps, "{}", l.lesson.id);
            assert_eq!(back.title, l.lesson.title);
        }
    }
}

#[test]
fn make_your_own_lesson_saves_a_pack_that_installs_and_solves() {
    let h = host();
    let lib = h.library();
    // the author's bench: the voltage-divider start circuit
    let src = lesson("voltage-dividers");
    let bench_project = src.start_project().unwrap();
    let track = bb_lessons::TrackDef {
        id: "my-lessons".into(),
        title: "My lessons".into(),
        label: "My lessons".into(),
        order: 10,
        description: String::new(),
    };
    let mut d = Draft::new("Half and half", track);
    d.capture_bench(&bench_project);
    assert_eq!(d.lesson.id, "half-and-half");
    assert!(!d.validate(&lib).is_empty(), "no steps yet");

    let i = d.add_step("explain", "Two equal resistors");
    d.step_mut(i).unwrap().body = "Equal halves split 5 V in two.".into();
    // pick the meter on a running bench: its live reading becomes a ±5 % check
    let mut b = bb_sim::Bench::new(d.project.clone(), lib.clone(), h.bench_options()).unwrap();
    b.run_for(bb_core::SimTime::from_secs(1)).unwrap();
    let readings = b.readings("dmm1");
    let sugg = d.suggest_for_part(&lib, "dmm1", &readings);
    let reading = sugg
        .iter()
        .find(|s| matches!(&s.check, Check::Reading { key, .. } if key == "value"))
        .expect("a reading suggestion");
    let (min, max) = match &reading.check {
        Check::Reading { min, max, .. } => (*min, *max),
        _ => unreachable!(),
    };
    assert!(min < 2.5 && max > 2.5, "{min}…{max}");
    let m = d.add_step("measure", "Read the middle");
    d.add_check(m, reading.check.clone());
    d.step_mut(m).unwrap().solution = vec![Action::Run { seconds: 1.0 }];
    // pick R1: its value, then two pins
    let r1 = d.suggest_for_part(&lib, "r1", &[]);
    assert!(
        r1.iter().any(|s| s.label == "R1 resistance is 10 kΩ"),
        "{:?}",
        r1.iter().map(|s| &s.label).collect::<Vec<_>>()
    );
    let pins = d.suggest_for_pins(&lib, "r1.2", "r2.1", Some(2.5));
    assert!(matches!(pins[0].check, Check::Connected { .. }));
    let q = d.add_step("build", "Keep them equal");
    d.add_check(q, r1[1].check.clone());
    d.add_check(q, pins[0].check.clone());
    d.step_mut(q).unwrap().solution = vec![Action::Run { seconds: 0.1 }];
    d.move_step(2, 1);
    assert_eq!(d.lesson.steps[1].title, "Keep them equal");
    assert!(d.validate(&lib).is_empty(), "{:?}", d.validate(&lib));

    // save as a lesson-pack plugin and install it
    let tmp = Tmp::new("pack");
    let dir = tmp.0.join("me.half-and-half");
    let pack = PackInfo {
        id: "me.half-and-half".into(),
        name: "Half and half".into(),
        version: "1.0.0".into(),
        author: "Me".into(),
        summary: "One lesson about equal dividers".into(),
    };
    let file = d.save_pack(&dir, &pack).unwrap();
    assert!(file.ends_with("lessons/my-lessons/01-half-and-half.toml"));
    let bytes = d.package(&dir, &pack).unwrap();
    assert_eq!(
        bb_plugin::package::inspect(&bytes).unwrap().id,
        "me.half-and-half"
    );
    let ph = PluginHost::from_dirs(&[root().join("plugins/dmm-handheld"), dir.clone()], None);
    assert!(ph.errors.is_empty(), "{:?}", ph.errors);
    let mut cat = Catalog::builtin();
    cat.add_packs(&ph.lesson_dirs());
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    let l = cat
        .lesson("half-and-half")
        .expect("installed lesson")
        .clone();
    assert_eq!(l.pack.as_deref(), Some("me.half-and-half"));
    assert_eq!(cat.track("my-lessons").unwrap().lessons.len(), 1);
    assert!(bb_lessons::validate(&l, &ph.library()).is_empty());
    let mut r = Runner::new(&l, &ph).unwrap();
    r.solve().unwrap();
}

/// docs/LESSON-FORMAT.md documents every check type and action.
#[test]
fn lesson_format_doc_lists_every_check_and_action() {
    let src = std::fs::read_to_string(root().join("crates/bb-lessons/src/format.rs")).unwrap();
    let doc = std::fs::read_to_string(root().join("docs/LESSON-FORMAT.md")).unwrap();
    let variants = |en: &str| -> Vec<String> {
        let start = src.find(&format!("pub enum {en} {{")).unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        body.lines()
            .filter_map(|l| {
                let t = l.trim_start();
                // variant lines are indented by exactly 4 spaces and start upper-case
                (l.starts_with("    ") && !l.starts_with("     "))
                    .then_some(t)
                    .filter(|t| t.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
                    .map(|t| {
                        let name: String = t.chars().take_while(|c| c.is_alphanumeric()).collect();
                        let mut snake = String::new();
                        for (i, c) in name.chars().enumerate() {
                            if c.is_ascii_uppercase() && i > 0 {
                                snake.push('_');
                            }
                            snake.push(c.to_ascii_lowercase());
                        }
                        snake
                    })
            })
            .collect()
    };
    let checks = variants("Check");
    let actions = variants("Action");
    assert!(
        checks.len() >= 20 && actions.len() >= 20,
        "{checks:?} {actions:?}"
    );
    for v in checks.iter().chain(actions.iter()) {
        assert!(
            doc.contains(&format!("`{v}`")),
            "LESSON-FORMAT.md lacks `{v}`"
        );
    }
}
