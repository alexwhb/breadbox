//! The diagnosis engine ("Test circuit") on real lesson benches: the user's exact attempt at
//! lesson 1 step 2 (USB +5 V → top + rail → hole d11, while R1 sits in column 5), and each
//! common mistake — not wired, one column off, the other half of the board, only as far as a
//! rail, the wrong column's parts, a lead over the centre gap, a part shorted by its strip,
//! a bypassed resistor, an LED in backwards, a shorted supply, a burnt-out LED, too little
//! current, a paused bench, a jumper that must go, the meter on the wrong dial setting or in
//! parallel, a value out of range, a sketch that isn't loaded / doesn't compile / blinks at the
//! wrong rate — plus friendly goal labels, values without numerical dust, the success payoff
//! and "Show me" for every worked-solution action of every lesson.

use bb_lessons::diagnose::{self, Diagnosis, Fix, Mistake, Spot, Tone};
use bb_lessons::format::{Action, Check};
use bb_lessons::{Catalog, LoadedLesson, Runner};
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

fn lesson(id: &str) -> LoadedLesson {
    Catalog::builtin().lesson(id).unwrap().clone()
}

/// A runner on `step` (0-based) of a lesson, the steps before it solved.
fn at_step(id: &str, step: usize) -> Runner {
    let mut r = Runner::new(&lesson(id), &host()).unwrap();
    while r.session.step < step {
        let res = r.solve_step().unwrap();
        assert!(res.iter().all(|c| c.ok), "{id}: {res:?}");
        r.next_step();
    }
    r
}

fn wire(r: &mut Runner, a: &str, b: &str, color: &str) {
    r.apply(&Action::Wire {
        a: a.into(),
        b: b.into(),
        color: Some(color.into()),
    })
    .unwrap();
}

fn unwire(r: &mut Runner, a: &str, b: &str) {
    r.apply(&Action::Unwire {
        a: a.into(),
        b: b.into(),
    })
    .unwrap();
}

/// Moves a part's leads (relative to its origin) and pushes the change to the bench.
fn leads(r: &mut Runner, part: &str, leads: &[(&str, (f64, f64))]) {
    let mut p = r.session.project().clone();
    let b = p.part_mut(part).unwrap().bench.as_mut().unwrap();
    for (pin, xy) in leads {
        b.leads.insert(pin.to_string(), *xy);
    }
    r.session.editor.replace_circuit("move", &p);
    r.bench.as_mut().unwrap().update_project(p).unwrap();
}

fn kinds(d: &Diagnosis) -> Vec<Mistake> {
    d.findings().map(|f| f.kind).collect()
}

fn pin_marks(d: &Diagnosis, tone: Tone) -> Vec<String> {
    let mut v: Vec<String> = d
        .marks()
        .into_iter()
        .filter(|m| m.tone == tone)
        .filter_map(|m| match m.spot {
            Spot::Pin { part, pin } => Some(format!("{part}.{pin}")),
            _ => None,
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn the_users_attempt_rail_then_d11_points_at_column_5() {
    let mut r = at_step("what-is-a-circuit", 1);
    // what the user did: VBUS → the top + rail, then a jumper from the rail down to d11
    wire(&mut r, "ps1.VBUS", "bb1.t+1", "red");
    wire(&mut r, "bb1.t+10", "bb1.d11", "red");
    r.run(0.3).unwrap();
    let d = r.diagnose();
    assert!(!d.passed);
    // friendly goals, no `0 aA`
    assert_eq!(d.goals[0].label, "USB +5 V reaches R1's lead in column 5");
    assert_eq!(d.goals[1].label, "D1 (red LED) lights up");
    assert_eq!(d.goals[1].detail, "off");
    let f = d.goals[0]
        .finding
        .as_ref()
        .expect("a finding for the connection");
    assert_eq!(f.kind, Mistake::WrongColumn);
    assert_eq!(
        f.what,
        "Your red jumper reaches column 11, but R1's lead is in column 5."
    );
    assert_eq!(f.fix, "Move it to any hole a5–e5.");
    // where: the misplaced end in red, the free holes of column 5 pulsing, the supply's
    // side glowing
    assert_eq!(pin_marks(&d, Tone::Wrong), ["bb1.d11"]);
    let targets = pin_marks(&d, Tone::Target);
    for h in ["bb1.a5", "bb1.b5", "bb1.d5", "bb1.e5"] {
        assert!(targets.contains(&h.to_string()), "{h} in {targets:?}");
    }
    assert!(
        !targets.contains(&"bb1.c5".to_string()),
        "c5 holds R1's lead"
    );
    let ok = pin_marks(&d, Tone::Ok);
    for t in ["ps1.VBUS", "bb1.t+1", "bb1.t+10", "bb1.a11"] {
        assert!(ok.contains(&t.to_string()), "{t} in {ok:?}");
    }
    assert!(
        d.marks()
            .iter()
            .any(|m| m.note == "nothing else is in column 11")
    );
    // the LED goal says where the loop is open
    let led = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(led.kind, Mistake::OpenLoop);
    assert_eq!(
        led.what,
        "The loop is open at R1's lead in column 5: nothing connects it to USB +5 V."
    );
    // the dead-end jumper is explained by the goal, not listed twice
    assert!(d.extra.is_empty(), "{:?}", d.extra);

    // moving the end into column 5 closes the loop: the payoff quotes the live current
    unwire(&mut r, "bb1.t+10", "bb1.d11");
    wire(&mut r, "bb1.t+10", "bb1.d5", "red");
    r.run(0.3).unwrap();
    let d = r.diagnose();
    assert!(d.passed, "{d:#?}");
    assert!(
        d.payoff.starts_with("Loop closed — D1 is lit at ") && d.payoff.ends_with(" mA"),
        "{}",
        d.payoff
    );
}

#[test]
fn nothing_wired_yet_and_only_as_far_as_the_rail() {
    let mut r = at_step("what-is-a-circuit", 1);
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::NotWired);
    assert_eq!(
        f.what,
        "Nothing connects USB +5 V to R1's lead in column 5 yet."
    );
    assert_eq!(f.fix, "Add a jumper from USB +5 V to any hole a5–e5.");
    assert!(pin_marks(&d, Tone::Target).contains(&"ps1.VBUS".to_string()));
    // VBUS → + rail only
    wire(&mut r, "ps1.VBUS", "bb1.t+1", "red");
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::RailOnly, "{f:?}");
    assert_eq!(
        f.what,
        "USB +5 V reaches the top + rail, but nothing connects the rail to R1's lead in column 5."
    );
    assert_eq!(f.fix, "Add a jumper from the top + rail to any hole a5–e5.");
}

#[test]
fn one_column_off_and_the_other_half() {
    let mut r = at_step("what-is-a-circuit", 1);
    wire(&mut r, "ps1.VBUS", "bb1.a6", "red");
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::OneColumnOff);
    assert_eq!(
        f.what,
        "Your red jumper is in column 6, one column off: R1's lead is in column 5."
    );
    assert_eq!(f.fix, "Move it to any hole a5–e5.");
    unwire(&mut r, "ps1.VBUS", "bb1.a6");
    wire(&mut r, "ps1.VBUS", "bb1.g5", "red");
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::OtherHalf);
    assert!(f.what.contains("across the centre gap"), "{}", f.what);
    assert_eq!(f.fix, "Move it to any hole a5–e5.");
}

#[test]
fn the_wrong_column_with_parts_in_it() {
    let mut r = at_step("what-is-a-circuit", 1);
    // column 9 has R1's other lead and D1's long leg
    wire(&mut r, "ps1.VBUS", "bb1.a9", "red");
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::WrongPlace);
    assert!(
        f.what.starts_with(
            "Your red jumper reaches column 9 (R1's lead in column 9, D1's long leg (+))"
        ),
        "{}",
        f.what
    );
    assert!(f.what.ends_with("but R1's lead is in column 5."));
}

#[test]
fn a_lead_over_the_centre_gap_and_a_part_shorted_by_its_strip() {
    let mut r = at_step("what-is-a-circuit", 1);
    wire(&mut r, "ps1.VBUS", "bb1.a5", "red");
    // R1 stood up: lead 2 lands between rows e and f
    leads(&mut r, "r1", &[("1", (0.0, 0.0)), ("2", (0.0, 3.0))]);
    let d = r.diagnose();
    let f = d
        .findings()
        .find(|f| f.kind == Mistake::NotInHole)
        .unwrap_or_else(|| panic!("{:?}", kinds(&d)));
    assert_eq!(
        f.what,
        "R1's lead 2 isn't in a hole (it sits over the centre gap)."
    );
    // both leads in column 5
    leads(&mut r, "r1", &[("1", (0.0, 0.0)), ("2", (0.0, 1.0))]);
    let d = r.diagnose();
    let f = d
        .findings()
        .find(|f| f.kind == Mistake::ShortedPart)
        .unwrap_or_else(|| panic!("{:?}", kinds(&d)));
    assert_eq!(
        f.what,
        "Both of R1's leads are in column 5, so the strip connects them and the current goes around R1."
    );
    assert_eq!(f.fix, "Move one lead to another column.");
}

#[test]
fn a_bypassed_resistor_burns_the_led() {
    let mut r = at_step("what-is-a-circuit", 1);
    wire(&mut r, "ps1.VBUS", "bb1.a5", "red");
    wire(&mut r, "bb1.b5", "bb1.b9", "green");
    r.run(0.5).unwrap();
    let d = r.diagnose();
    let f = d
        .findings()
        .find(|f| f.kind == Mistake::Bypassed)
        .unwrap_or_else(|| panic!("{:?}", kinds(&d)));
    assert_eq!(
        f.what,
        "The green jumper from column 5 to column 9 connects both ends of R1, so the current skips it."
    );
    assert!(
        f.marks
            .iter()
            .any(|m| m.tone == Tone::Remove && matches!(m.spot, Spot::Wire(_)))
    );
    // 5 V straight into the LED: it's gone, and Repair is offered
    let led = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(led.kind, Mistake::BurntOut, "{led:?}");
    assert_eq!(led.action, Some(Fix::Repair));
}

#[test]
fn an_led_in_backwards() {
    let mut r = at_step("what-is-a-circuit", 1);
    wire(&mut r, "ps1.VBUS", "bb1.a5", "red");
    // the short leg in column 9 (toward +), the long leg in column 10 (toward GND)
    leads(&mut r, "d1", &[("a", (1.0, 0.0)), ("k", (0.0, 0.0))]);
    r.run(0.3).unwrap();
    let d = r.diagnose();
    let f = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::Reversed, "{f:?}");
    assert_eq!(
        f.what,
        "D1 (red LED) is in backwards: its short leg (−) faces the + side, so no current can flow."
    );
}

#[test]
fn a_shorted_supply() {
    let mut r = at_step("what-is-a-circuit", 1);
    wire(&mut r, "ps1.VBUS", "bb1.t-3", "red");
    r.run(0.2).unwrap();
    let d = r.diagnose();
    let f = d
        .findings()
        .find(|f| f.kind == Mistake::SupplyShort)
        .unwrap_or_else(|| panic!("{:?}", kinds(&d)));
    assert!(
        f.what
            .starts_with("USB +5 V is wired straight to USB GND: a short circuit."),
        "{}",
        f.what
    );
    assert_eq!(
        f.fix,
        "Remove the red jumper from USB +5 V to the top − rail."
    );
}

#[test]
fn breaking_the_loop_names_the_jumper_to_remove() {
    let r = &mut at_step("what-is-a-circuit", 2);
    r.run(0.3).unwrap();
    let d = r.diagnose();
    assert_eq!(
        d.goals[0].label,
        "D1's short leg (−) no longer reaches USB GND"
    );
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::StillConnected);
    assert_eq!(
        f.what,
        "D1's short leg (−) still reaches USB GND through the black jumper from column 10 to the top − rail."
    );
    let lit = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(lit.kind, Mistake::StillLit);
    assert!(
        lit.what.starts_with("D1 (red LED) is still lit ("),
        "{}",
        lit.what
    );
    assert!(lit.what.contains("the black jumper from column 10"));
}

#[test]
fn too_little_current_paused_and_burnt_out() {
    // lesson 3 step 2: the meter in series reads the LED current
    let mut r = at_step("ohms-law-led", 1);
    r.apply(&Action::SetProp {
        part: "r1".into(),
        prop: "resistance".into(),
        value: toml::Value::Integer(10_000),
    })
    .unwrap();
    r.run(1.0).unwrap();
    let d = r.diagnose();
    let lit = d
        .goals
        .iter()
        .find(|g| g.label == "D1 (red LED) lights up")
        .unwrap();
    let f = lit.finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::TooDim, "{f:?}");
    assert!(
        f.what.starts_with("D1 (red LED) carries only 0.3")
            && f.what
                .ends_with(" mA, too little to see (it needs about 1 mA)."),
        "{}",
        f.what
    );
    assert!(f.fix.starts_with("R1 is 10 kΩ"), "{}", f.fix);
    // the reading goal quotes mA, not raw amps
    let meter = &d.goals[0];
    assert_eq!(meter.label, "The meter reads 11–14.5 mA");
    assert!(meter.detail.ends_with(" mA"), "{}", meter.detail);
    // a paused bench
    let step = r.session.current().clone();
    let project = r.session.project().clone();
    let lib = r.session.library().clone();
    let state = r.session.state().clone();
    let d = diagnose::diagnose(
        &step,
        &project,
        &lib,
        r.bench.as_mut(),
        &state,
        &diagnose::Options::default(),
    );
    let f = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::Paused);
    assert_eq!(f.action, Some(Fix::Run));
    // no resistor at all: the LED burns out
    r.apply(&Action::SetProp {
        part: "r1".into(),
        prop: "resistance".into(),
        value: toml::Value::Float(0.01),
    })
    .unwrap();
    r.run(0.5).unwrap();
    let d = r.diagnose();
    let f = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::BurntOut, "{f:?}");
}

#[test]
fn a_meter_in_parallel_where_it_must_be_in_series() {
    // lesson 3 step 2, with the meter moved across the LED like a voltmeter
    let mut r = at_step("ohms-law-led", 1);
    unwire(&mut r, "bb1.a10", "dmm1.MA");
    wire(&mut r, "bb1.b10", "bb1.t-14", "black");
    wire(&mut r, "dmm1.MA", "bb1.b9", "yellow");
    r.run(1.0).unwrap();
    let d = r.diagnose();
    assert!(!d.passed);
    let meter = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(meter.kind, Mistake::MeterParallel, "{meter:?}");
    assert_eq!(
        meter.what,
        "The meter is wired across D1 (red LED), in parallel, so the current goes around the meter instead of through it."
    );
    assert!(meter.fix.starts_with("Current is measured in series"));
    let led = d.goals[1].finding.as_ref().unwrap();
    assert_eq!(led.kind, Mistake::MeterParallel, "{led:?}");
}

#[test]
fn the_meter_on_the_wrong_dial_setting() {
    // lesson 2 step 2: the meter starts switched off
    let mut r = at_step("volts-amps-ohms", 1);
    r.run(1.0).unwrap();
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::MeterMode);
    assert_eq!(
        f.what,
        "The meter is switched off; this step needs DC volts."
    );
    assert_eq!(f.fix, "Click the meter's dial until it points at DC volts.");
    assert!(
        f.marks
            .iter()
            .any(|m| m.spot == Spot::Part("dmm1".into()) && m.tone == Tone::Target)
    );
}

#[test]
fn a_value_out_of_range_and_a_choice_to_make() {
    // SPICE lesson: R2 must become 10 kΩ
    let mut r = at_step("operating-point", 2);
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.kind, Mistake::WrongValue);
    assert!(
        f.what.starts_with("R2 is ") && f.what.ends_with("; this step needs 9.9–10.1 kΩ."),
        "{}",
        f.what
    );
    assert_eq!(f.fix, "Set R2's resistance to 10 kΩ with the field below.");
    assert!(
        f.marks
            .iter()
            .any(|m| m.spot == Spot::Control("prop:r2.resistance".into()))
    );
    // lesson 3 "break it on purpose": pick None
    let mut r = at_step("ohms-law-led", 3);
    r.run(0.3).unwrap();
    let d = r.diagnose();
    let f = d.goals[0].finding.as_ref().unwrap();
    assert_eq!(f.what, "D1 (red LED) is still fine.");
    assert_eq!(f.fix, "Pick None: then nothing limits the current.");
    assert!(
        f.marks
            .iter()
            .any(|m| m.spot == Spot::Control("choice:5".into()))
    );
}

#[test]
fn code_steps_not_loaded_not_compiling_wrong_rate() {
    let mut r = at_step("first-sketch-blink", 2);
    r.run(1.0).unwrap();
    let d = r.diagnose();
    let f = d.findings().next().unwrap();
    assert_eq!(f.kind, Mistake::NoSketch);
    assert_eq!(f.what, "Your sketch isn't on the UNO yet.");
    assert_eq!(f.fix, "Press Compile & upload.");
    // a compile error
    let step = r.session.current().clone();
    let project = r.session.project().clone();
    let lib = r.session.library().clone();
    let state = r.session.state().clone();
    let opts = diagnose::Options {
        running: true,
        compile_errors: vec!["line 8: expected ';' before '}' token".into()],
    };
    let d = diagnose::diagnose(&step, &project, &lib, r.bench.as_mut(), &state, &opts);
    let f = d.findings().next().unwrap();
    assert_eq!(f.kind, Mistake::CompileError);
    assert_eq!(
        f.what,
        "Your sketch doesn't compile: line 8: expected ';' before '}' token"
    );
    // loaded, but the goal asks for 1 Hz while the sketch blinks at 0.5 Hz
    r.solve_step().unwrap();
    let mut step = r.session.current().clone();
    for c in step.checks.iter_mut() {
        if let Check::PinFrequency { hz, .. } = c {
            *hz = 1.0;
        }
    }
    let project = r.session.project().clone();
    let state = r.session.state().clone();
    let d = diagnose::diagnose(
        &step,
        &project,
        &lib,
        r.bench.as_mut(),
        &state,
        &diagnose::Options::running(),
    );
    let f = d.findings().next().unwrap();
    assert_eq!(f.kind, Mistake::WrongRate, "{f:?}");
    assert!(
        f.what.starts_with(
            "UNO pin D13 switches at 0.5 Hz; the goal is 1 Hz (about 500 ms on, 500 ms off)."
        ),
        "{}",
        f.what
    );
    assert_eq!(
        f.fix,
        "Your delays add up to about 1000 ms per half; use delay(500) for each."
    );
}

#[test]
fn a_sketch_on_the_wrong_pin_never_changes_it() {
    // digital-outputs wants D8 at 5 Hz; the blink image toggles D13
    let mut r = at_step("digital-outputs", 1);
    let img =
        bb_sim::firmware::load_image(&root().join("lessons/1-electricity/firmware/blink.uno.hex"))
            .unwrap();
    r.upload("u1", &img).unwrap();
    r.run(2.0).unwrap();
    let d = r.diagnose();
    let f = d
        .findings()
        .find(|f| f.kind == Mistake::NeverChanges)
        .unwrap_or_else(|| panic!("{:?}", kinds(&d)));
    assert_eq!(f.what, "UNO pin D8 never changes, so nothing blinks.");
}

#[test]
fn every_failed_goal_of_every_lesson_gets_a_finding_with_a_next_action() {
    // the start of every hands-on step: each failing goal is explained, and says what to do
    let h = host();
    let cat = Catalog::builtin();
    let mut checked = 0;
    for l in cat.all() {
        let mut r = Runner::new(l, &h).unwrap();
        loop {
            let step = r.session.current().clone();
            if step.is_hands_on() {
                let _ = r.run(0.2);
                let d = r.diagnose();
                for g in &d.goals {
                    assert!(
                        !g.label.contains('.')
                            || g.label.contains("V(")
                            || g.label.contains("“")
                            || g.label.contains("kΩ")
                            || g.label.chars().any(|c| c.is_ascii_digit()),
                        "{}: {}: raw pin in label {:?}",
                        l.lesson.id,
                        step.title,
                        g.label
                    );
                    assert!(
                        !g.detail.contains("aA") && !g.detail.contains("fA"),
                        "{}: {}: {:?}",
                        l.lesson.id,
                        step.title,
                        g.detail
                    );
                    if let Some(f) = &g.finding {
                        assert!(!f.what.is_empty(), "{}: {}", l.lesson.id, step.title);
                        assert!(
                            !f.fix.is_empty() || f.action.is_some(),
                            "{}: {}: no next action for {:?}",
                            l.lesson.id,
                            step.title,
                            f
                        );
                        checked += 1;
                    }
                }
            }
            let res = r.solve_step().unwrap();
            assert!(res.iter().all(|c| c.ok), "{}: {res:?}", l.lesson.id);
            let d = r.diagnose();
            if !step.checks.is_empty() {
                assert!(d.passed, "{}: {}: {d:#?}", l.lesson.id, step.title);
                assert!(!d.payoff.is_empty());
            }
            if !r.next_step() {
                break;
            }
        }
    }
    assert!(checked >= 30, "{checked} findings");
}

#[test]
fn show_me_covers_every_solution_action() {
    let h = host();
    let lib = h.library();
    let cat = Catalog::builtin();
    let mut ghosts = 0;
    let mut kinds = std::collections::BTreeSet::new();
    for l in cat.all() {
        let mut r = Runner::new(l, &h).unwrap();
        loop {
            let step = r.session.current().clone();
            let s = bb_lessons::guide::show_me(&step, r.session.project(), &lib);
            assert_eq!(s.moves.len(), step.solution.len());
            for (a, m) in step.solution.iter().zip(&s.moves) {
                assert!(!m.text.is_empty(), "{a:?}");
                assert!(
                    !m.text.contains("bb1.") && !m.text.contains("ps1."),
                    "{}",
                    m.text
                );
                kinds.insert(
                    format!("{a:?}")
                        .split([' ', '{'])
                        .next()
                        .unwrap()
                        .to_string(),
                );
                if matches!(a, Action::Wire { .. }) {
                    assert!(m.ghost.is_some());
                    ghosts += 1;
                }
                if matches!(a, Action::Unwire { .. }) {
                    assert!(
                        m.marks.iter().any(|k| k.tone == Tone::Remove),
                        "{}: {}",
                        l.lesson.id,
                        m.text
                    );
                }
                if !m.wait {
                    assert!(
                        !m.marks.is_empty(),
                        "{}: {} has no marks",
                        l.lesson.id,
                        m.text
                    );
                }
            }
            if step.is_hands_on() && !step.solution.is_empty() {
                assert!(!s.why.is_empty(), "{}: {}: no why", l.lesson.id, step.title);
            }
            r.solve_step().unwrap();
            if !r.next_step() {
                break;
            }
        }
    }
    assert!(ghosts >= 8, "{ghosts}");
    // the built-in lessons exercise most action kinds
    assert!(kinds.len() >= 18, "{kinds:?}");
    // lesson 1 step 2's move reads like a person
    let r = at_step("what-is-a-circuit", 1);
    let s = bb_lessons::guide::show_me(r.session.current(), r.session.project(), &lib);
    assert_eq!(
        s.moves[0].text,
        "Add a red jumper from USB +5 V to hole a5 (column 5)"
    );
    assert_eq!(s.moves[0].ghost.as_ref().unwrap().label, "USB +5 V → a5");
}
