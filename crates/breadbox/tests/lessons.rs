//! Lessons GUI end-to-end (`TEST-5`, `LES-2`, `LES-5`, `LES-6`): Track 1 lesson 3 on its live
//! bench — the design's step 3 with "Do the math myself" (a wrong value gets a hint, "Show
//! me the working", the right value passes and the measurements table fills from the meter),
//! Next → break it on purpose (the LED pops, Repair), Open in free play; and "Make your own
//! lesson" (two steps from the current bench → saved pack → shows up in Lessons). Saves
//! `docs/screenshots/lessons.png`.

mod common;

use breadbox::BreadboxApp;
use breadbox::router::{Route, ScreenSlot};
use breadbox::screens::lessons::LessonsScreen;
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn go(h: &mut H, r: Route) {
    h.state_mut().ctx.navigate(r);
    h.run_steps(3);
}

fn text_of(h: &H, prefix: &str) -> Option<String> {
    h.query_all_by_label_contains(prefix).next().and_then(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    })
}

fn enabled(h: &H, label: &str) -> bool {
    h.query_all_by_label(label)
        .next()
        .is_some_and(|n| !n.accesskit_node().is_disabled())
}

/// Waits for the Next button and clicks it.
fn next(h: &mut H, label: &str) {
    assert!(
        wait_for(h, 90.0, |h| enabled(h, label)),
        "{label} never enabled"
    );
    h.get_by_label(label).click();
    h.run_steps(4);
}

fn type_prediction(h: &mut H, v: &str) {
    h.get_by_label("Prediction (mA)").click();
    h.run_steps(1);
    // clear what is there
    for _ in 0..8 {
        h.key_press(egui::Key::Backspace);
    }
    h.get_by_label("Prediction (mA)").type_text(v);
    h.run_steps(1);
    h.get_by_label("Check").click();
    h.run_steps(3);
    // the check waits for the meter to settle after the last change
    assert!(
        wait_for(h, 30.0, |h| !has_label(h, "Checking against the meter…")),
        "the meter never settled"
    );
    h.run_steps(2);
}

/// "220Ω · 13.6 mA · 13.4 mA" → the meter column in mA.
fn row_meter(h: &H, option: &str) -> f64 {
    let l = h
        .query_all_by_role(egui::accesskit::Role::Row)
        .filter_map(|n| {
            let a = n.accesskit_node();
            a.label().or_else(|| a.value())
        })
        .find(|l| l.starts_with(&format!("{option} · ")))
        .unwrap_or_else(|| panic!("no row for {option}"));
    l.rsplit(" · ")
        .next()
        .unwrap()
        .trim_end_matches(" mA")
        .parse()
        .unwrap_or_else(|_| panic!("{l}"))
}

fn rows(h: &H) -> usize {
    h.query_all_by_role(egui::accesskit::Role::Row)
        .filter(|n| {
            n.accesskit_node()
                .label()
                .or_else(|| n.accesskit_node().value())
                .is_some_and(|l| l.ends_with(" mA") && !l.starts_with("R1"))
        })
        .count()
}

#[test]
fn ohms_law_step_3_math_check_break_it_and_free_play() {
    let projects = breadbox::plugins::temp_dir("lesson-projects");
    // SAFETY: tests in this binary only read this variable
    unsafe { std::env::set_var("BREADBOX_PROJECTS_DIR", &projects) };
    let mut h = harness();
    go(&mut h, Route::Lessons);
    assert!(has_label(&h, "Track 1 · Electricity you can see"));
    assert!(has_label_containing(
        &h,
        "Lesson 8: Your first sketch: blink"
    ));
    assert!(has_label(&h, "Make your own lesson"));
    h.get_by_label("Lesson 3: Ohm's law with an LED").click();
    h.run_steps(4);
    assert!(has_label(&h, "Lesson 3 of 8"));
    assert!(has_label(&h, "Meet the circuit"));
    next(&mut h, "Next: read the current");
    // step 2: the meter in series reads the LED current
    assert!(has_label(&h, "Read the current"));
    next(&mut h, "Next: change the resistor, watch the current");
    // step 3 (the design)
    assert!(has_label(&h, "Change the resistor, watch the current"));
    assert!(has_label(&h, "Step 3 of 5"));
    assert!(has_label_containing(&h, "I = (5.0 V − 2.0 V) ÷ R"));
    assert!(has_label_containing(&h, "Meter is wired in series"));
    h.get_by_label("220Ω").click();
    h.run_steps(2);
    // "Do the math myself" is on: the meter waits for the prediction
    if !has_label_containing(&h, "What current do you expect") {
        h.get_by_label("Do the math myself").click();
        h.run_steps(2);
    }
    assert!(has_label_containing(
        &h,
        "What current do you expect with 220Ω?"
    ));
    assert!(
        wait_for(&mut h, 30.0, |h| text_of(h, "DMM1 display:")
            .is_some_and(|t| t.contains("----"))),
        "meter should wait: {:?}",
        text_of(&h, "DMM1 display:")
    );
    // a wrong value → hint
    type_prediction(&mut h, "40");
    assert!(has_label_containing(&h, "Not quite. Take 5.0 − 2.0 first"));
    assert!(text_of(&h, "DMM1 display:").unwrap().contains("----"));
    // full 5 V hint
    type_prediction(&mut h, "22.7");
    assert!(has_label_containing(&h, "Looks like you used the full 5 V"));
    // "Show me the working"
    h.get_by_label("Show me the working").click();
    h.run_steps(3);
    assert!(has_label(&h, "V(R1) = 5.0 − 2.0 = 3.0 V"));
    assert!(has_label(&h, "I = 0.01364 A = 13.6 mA"));
    h.get_by_label("Hide the working").click();
    h.run_steps(2);
    // the right value: the meter (≈ 13.4 mA through its burden) agrees
    type_prediction(&mut h, "13.6");
    let verdict = text_of(&h, "Nailed it. The meter agrees:").expect("verdict");
    let ma: f64 = verdict
        .trim_start_matches("Nailed it. The meter agrees: ")
        .trim_end_matches(" mA.")
        .parse()
        .unwrap_or_else(|_| panic!("{verdict}"));
    assert!((12.8..14.2).contains(&ma), "{verdict}");
    assert!(rows(&h) >= 1, "a row for 220Ω");
    assert!(has_label_containing(&h, "220Ω · 13.6 mA · "));
    assert!(has_label(&h, "Try 2 more values"));
    // two more values
    for (opt, guess) in [("1kΩ", "3.0"), ("100Ω", "30")] {
        h.get_by_label(opt).click();
        h.run_steps(4);
        type_prediction(&mut h, guess);
        assert!(
            has_label_containing(&h, "Nailed it. The meter agrees:"),
            "{opt}: {:?}",
            text_of(&h, "Not quite")
        );
    }
    assert!(has_label(&h, "Checkpoint passed"));
    // every row quotes what the meter showed for that resistor (not a stale reading)
    let (r220, r1k, r100) = (
        row_meter(&h, "220Ω"),
        row_meter(&h, "1kΩ"),
        row_meter(&h, "100Ω"),
    );
    assert!((12.5..14.5).contains(&r220), "220Ω: {r220}");
    assert!((2.8..3.5).contains(&r1k), "1kΩ: {r1k}");
    assert!((24.0..31.0).contains(&r100), "100Ω: {r100}");
    let verdict = text_of(&h, "Nailed it. The meter agrees:").unwrap();
    assert!(
        verdict.contains(&format!("{r100:.1} mA")),
        "{verdict} vs {r100}"
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "lessons-checkpoint");
    next(&mut h, "Next: break it on purpose");
    // step 4: None pops the LED
    assert!(has_label(&h, "Break it on purpose"));
    h.get_by_label("None").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 60.0, |h| has_label(h, "Pop. That LED is gone.")),
        "the LED should burn"
    );
    h.get_by_label("Repair LED & pick a resistor").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 60.0, |h| !has_label(h, "Pop. That LED is gone.")),
        "repair"
    );
    // Open in free play → a Playground project with the lesson bench
    h.get_by_label("Open in free play").click();
    h.run_steps(5);
    assert_eq!(h.state().ctx.route, Route::Workbench);
    let p = h.state().ctx.project.as_ref().expect("free play project");
    assert!(p.name.contains("Ohm's law with an LED"), "{}", p.name);
    assert!(p.dir.starts_with(&projects));
    let doc = h.state().ctx.circuit.as_ref().unwrap();
    assert!(doc.project().part("dmm1").is_some() && doc.project().part("d1").is_some());
}

#[test]
fn lesson_screenshot_like_the_design() {
    let mut h = harness();
    go(&mut h, Route::Lessons);
    // lessons 1 and 2 done, lesson 3 at step 3 (the design)
    let path = h.state().ctx.progress_path.clone();
    let mut prog = bb_lessons::Progress::load(&path).unwrap();
    prog.complete("what-is-a-circuit");
    prog.complete("volts-amps-ohms");
    prog.set_current("ohms-law-led", 2);
    prog.save().unwrap();
    // a fresh app reads the progress
    drop(h);
    let mut opts = breadbox::AppOptions::test();
    opts.progress_path = Some(path);
    let mut h = app_harness(opts);
    go(&mut h, Route::Lessons);
    h.get_by_label("Lesson 3: Ohm's law with an LED").click();
    h.run_steps(4);
    assert!(has_label(&h, "Change the resistor, watch the current"));
    assert!(has_label_containing(&h, "(done)"));
    assert!(wait_for(&mut h, 30.0, |h| text_of(h, "DMM1 display:")
        .is_some_and(|t| t.contains("----"))));
    h.run_steps(10);
    assert!(
        !enabled(&h, "Next: break it on purpose"),
        "Next waits for the checkpoint"
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "lessons");
}

#[test]
fn make_your_own_lesson_from_the_bench_appears_in_lessons() {
    let mut h = harness();
    h.get_by_label("Open example blink").click();
    h.run_steps(4);
    go(&mut h, Route::Lessons);
    h.get_by_label("Make your own lesson").click();
    h.run_steps(3);
    // title
    h.get_by_label("Lesson title").click();
    h.run_steps(1);
    for _ in 0..20 {
        h.key_press(egui::Key::Backspace);
    }
    h.get_by_label("Lesson title").type_text("Blink check");
    h.run_steps(2);
    h.get_by_label("Capture the current bench").click();
    h.run_steps(2);
    assert!(has_label_containing(&h, "from blink"));
    // two steps
    h.get_by_label("Add step").click();
    h.run_steps(2);
    h.get_by_label("explain").click();
    h.run_steps(2);
    h.get_by_label("Add step").click();
    h.run_steps(2);
    h.get_by_label("build").click();
    h.run_steps(2);
    assert!(has_label(&h, "Step 2: Build it"));
    // a goal from the bench: pick the LED
    let led = h
        .query_all_by_label_contains("· led")
        .next()
        .expect("LED chip")
        .accesskit_node()
        .label()
        .unwrap();
    h.get_by_label(&led).click();
    h.run_steps(3);
    let add = h
        .query_all_by_label_contains("Add goal:")
        .next()
        .expect("a suggestion")
        .accesskit_node()
        .label()
        .unwrap();
    h.get_by_label(&add).click();
    h.run_steps(2);
    h.get_by_label("Validate").click();
    h.run_steps(2);
    assert!(
        has_label_containing(&h, "Ready: the lesson loads"),
        "validation problems: {:?}",
        h.query_all_by_label_contains("Problem:")
            .map(|n| n
                .accesskit_node()
                .label()
                .or_else(|| n.accesskit_node().value()))
            .collect::<Vec<_>>()
    );
    h.get_by_label("Save lesson pack").click();
    h.run_steps(6);
    // the pack is a plugin; Lessons shows its track and opens the lesson
    assert!(
        h.state()
            .ctx
            .plugins
            .registry
            .get("me.my-lessons")
            .is_some_and(|p| p.active())
    );
    assert!(has_label(&h, "Your track · My lessons"));
    assert!(has_label_containing(&h, "Lesson 1: Blink check"));
    assert!(has_label(&h, "Look at the circuit"));
}

/// The open lesson's bench: (running, sim seconds).
fn lesson_bench(h: &mut H) -> Option<(bool, f64)> {
    let s = h
        .state_mut()
        .screens
        .get_mut(ScreenSlot::Lessons)?
        .as_any_mut()?
        .downcast_mut::<LessonsScreen>()?;
    let run = s.run.as_ref()?;
    let t = run.telemetry().map(|t| t.time.as_secs_f64()).unwrap_or(0.0);
    Some((run.is_running(), t))
}

#[test]
fn lesson_bench_pauses_behind_other_screens_and_resumes() {
    let mut h = harness();
    go(&mut h, Route::Lessons);
    assert!(
        wait_for(&mut h, 30.0, |h| lesson_bench(h)
            .is_some_and(|(on, t)| on && t > 0.05)),
        "the lesson bench never ran: {:?}",
        lesson_bench(&mut h)
    );
    // another screen: the lesson bench pauses and its clock stops
    go(&mut h, Route::Plugins);
    let (on, _) = lesson_bench(&mut h).expect("lesson stays open");
    assert!(!on, "the lesson bench kept running behind Plugins");
    std::thread::sleep(std::time::Duration::from_millis(200));
    h.run_steps(2);
    let (_, t1) = lesson_bench(&mut h).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));
    h.run_steps(2);
    let (_, t2) = lesson_bench(&mut h).unwrap();
    assert_eq!(t1, t2, "sim time advanced while paused");
    // back on Lessons: it runs again
    go(&mut h, Route::Lessons);
    assert!(
        wait_for(&mut h, 30.0, |h| lesson_bench(h)
            .is_some_and(|(on, t)| on && t > t2)),
        "the lesson bench did not resume"
    );
}
