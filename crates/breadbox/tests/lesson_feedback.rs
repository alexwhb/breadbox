//! Lesson feedback GUI end-to-end (the "I feel stuck" fixes), by AccessKit label:
//!
//! * lesson 1 step 2 with the user's own attempt (USB +5 V → top + rail → hole d11 while R1
//!   sits in column 5): **Test circuit** explains it in bench terms, **Show me** draws the
//!   ghost jumper, **Do it for me** passes the step as one undo step, the success state (green
//!   header with the live payoff, Next as the primary action), step 3, then the lesson's
//!   completion card with "Next lesson";
//! * lesson 3 with the meter wired in parallel: Test circuit says so;
//! * hints escalate to Show me; a disabled Next says what's left.
//!
//! Saves `docs/screenshots/lessons-*.png` (with `BREADBOX_UPDATE_SCREENSHOTS=1`).

mod common;

use breadbox::router::{Route, ScreenSlot};
use breadbox::screens::lessons::LessonsScreen;
use breadbox::{AppOptions, BreadboxApp};
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn go(h: &mut H, r: Route) {
    h.state_mut().ctx.navigate(r);
    h.run_steps(3);
}

fn sized(w: f32, hgt: f32) -> H {
    let mut h = Harness::builder()
        .with_size(egui::vec2(w, hgt))
        .with_pixels_per_point(1.0)
        .with_max_steps(64)
        .wgpu()
        .build_eframe(move |cc| BreadboxApp::new(cc, AppOptions::test()));
    h.run_steps(3);
    h
}

fn screen(h: &mut H) -> &mut LessonsScreen {
    h.state_mut()
        .screens
        .get_mut(ScreenSlot::Lessons)
        .and_then(|s| s.as_any_mut())
        .and_then(|a| a.downcast_mut::<LessonsScreen>())
        .expect("lessons screen")
}

fn enabled(h: &H, label: &str) -> bool {
    h.query_all_by_label(label)
        .next()
        .is_some_and(|n| !n.accesskit_node().is_disabled())
}

fn text_of(h: &H, prefix: &str) -> Option<String> {
    h.query_all_by_label_contains(prefix).next().and_then(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    })
}

fn view(h: &H) -> breadbox::canvas::pen::View {
    h.state()
        .ctx
        .egui
        .data(|d| d.get_temp::<breadbox::canvas::pen::View>(egui::Id::unique("bench-view")))
        .expect("bench view")
}

/// Screen position of a pin / hole on the lesson bench.
fn pin_screen(h: &mut H, part: &str, pin: &str) -> egui::Pos2 {
    let v = view(h);
    let run = screen(h).run.as_ref().expect("a lesson");
    let p = run.doc.project().part(part).expect("part");
    let def = run.doc.lib.get(&p.part).expect("def");
    let leads = bb_circuit::Project::bench_leads(def, p).expect("leads");
    let (_, x, y) = leads.iter().find(|(n, _, _)| n == pin).expect("pin");
    v.to_screen(*x, *y)
}

/// Drags with the primary button from `a` to `b` in a few steps.
fn drag(h: &mut H, a: egui::Pos2, b: egui::Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: a,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        h.hover_at(a + (b - a) * t);
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: b,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
}

fn wire(h: &mut H, a: (&str, &str), b: (&str, &str)) {
    let pa = pin_screen(h, a.0, a.1);
    let pb = pin_screen(h, b.0, b.1);
    drag(h, pa, pb);
}

fn next(h: &mut H, label: &str) {
    assert!(
        wait_for(h, 60.0, |h| enabled(h, label)),
        "{label} never enabled; goals: {:?}",
        h.query_all_by_role(egui::accesskit::Role::Label)
            .filter_map(|n| n.accesskit_node().label())
            .filter(|l| l.contains(" · "))
            .collect::<Vec<_>>()
    );
    h.get_by_label(label).click();
    h.run_steps(4);
}

fn shot(h: &mut H, name: &str) {
    h.state_mut().ctx.toasts.clear();
    h.run_steps(2);
    screenshot(h, name);
}

#[test]
fn lesson_1_users_attempt_test_show_do_it_success_and_completion() {
    let mut h = harness();
    go(&mut h, Route::Lessons);
    h.get_by_label("Lesson 1: What is a circuit?").click();
    h.run_steps(4);
    next(&mut h, "Next: close the loop");
    assert!(has_label(&h, "Close the loop"));
    // friendly goals, no `0 aA`
    assert!(
        wait_for(&mut h, 20.0, |h| has_label_containing(
            h,
            "USB +5 V reaches R1's lead in column 5"
        )),
        "friendly goal label"
    );
    assert!(has_label_containing(&h, "D1 (red LED) lights up · off"));
    assert!(!has_label_containing(&h, "aA"));
    // the disabled Next says what's left
    assert!(!enabled(&h, "Next: break the loop"));
    // the user's attempt: USB +5 V → the top + rail, then the rail → d11
    h.run_steps(5);
    wire(&mut h, ("ps1", "VBUS"), ("bb1", "t+1"));
    wire(&mut h, ("bb1", "t+10"), ("bb1", "d11"));
    let wires = screen(&mut h)
        .run
        .as_ref()
        .unwrap()
        .doc
        .project()
        .wires
        .len();
    assert_eq!(wires, 4, "two jumpers drawn on the canvas");
    // Test circuit
    h.get_by_label("Test circuit").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 20.0, |h| has_label_containing(
            h,
            "Problem: Your red jumper reaches column 11, but R1's lead is in column 5."
        )),
        "diagnosis: {:?}",
        text_of(&h, "Problem:")
    );
    assert!(has_label(&h, "Next: Move it to any hole a5–e5."));
    assert!(has_label_containing(
        &h,
        "Problem: The loop is open at R1's lead in column 5"
    ));
    {
        let run = screen(&mut h).run.as_ref().unwrap();
        let ov = run.overlay();
        assert!(
            ov.marks
                .iter()
                .any(|m| m.tone == bb_lessons::diagnose::Tone::Wrong),
            "a red marker on the bench"
        );
    }
    h.run_steps(6);
    shot(&mut h, "lessons-diagnosis");
    // hints escalate: hint 1, hint 2, then Show me is the suggestion
    h.get_by_label("Hint 1 of 2").click();
    h.run_steps(2);
    assert!(has_label_containing(
        &h,
        "Hint 1: Every hole in a breadboard column"
    ));
    h.get_by_label("Hint 2 of 2").click();
    h.run_steps(2);
    assert!(has_label_containing(
        &h,
        "Hint 2: Drag from the supply's VBUS pin"
    ));
    assert!(
        !has_label(&h, "Hint 2 of 2") && !has_label_containing(&h, "Hint 3"),
        "the hint button goes once every hint is out"
    );
    // Show me: the ghost jumper and why
    h.get_by_label("Show me").click();
    h.run_steps(4);
    assert!(has_label(
        &h,
        "Show me: Add a red jumper from USB +5 V to hole a5 (column 5)"
    ));
    assert!(has_label_containing(
        &h,
        "Why it works: The five holes a5–e5"
    ));
    {
        let run = screen(&mut h).run.as_ref().unwrap();
        assert_eq!(run.overlay().ghosts.len(), 1, "a ghost jumper");
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    h.run_steps(4);
    shot(&mut h, "lessons-show-me");
    // Do it for me: the worked solution as one undo step
    h.get_by_label("Do it for me").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 60.0, |h| enabled(h, "Next: break the loop")),
        "Do it for me passes the step"
    );
    assert_eq!(
        screen(&mut h).run.as_ref().unwrap().doc.editor.undo_label(),
        Some("Do it for me")
    );
    // ⌘Z takes the whole "Do it for me" back in one step, ⇧⌘Z redoes it
    let wires = |h: &mut H| screen(h).run.as_ref().unwrap().doc.project().wires.len();
    let before = wires(&mut h);
    h.state_mut().ctx.command(breadbox::router::Command::Undo);
    h.run_steps(4);
    assert_eq!(
        wires(&mut h),
        before - 1,
        "undo removes the solution's jumper"
    );
    h.state_mut().ctx.command(breadbox::router::Command::Redo);
    h.run_steps(4);
    assert_eq!(wires(&mut h), before);
    assert!(
        wait_for(&mut h, 60.0, |h| enabled(h, "Next: break the loop")),
        "redo passes the step again"
    );
    // the success state: green header with the live payoff, why, Next as the primary action
    assert!(
        wait_for(&mut h, 10.0, |h| has_label_containing(
            h,
            "Done: Loop closed — D1 is lit at"
        )),
        "payoff: {:?}",
        text_of(&h, "Done:")
    );
    assert!(has_label(&h, "Step 2 of 4 · done"));
    assert!(has_label_containing(&h, "Why it works:"));
    assert!(
        !has_label(&h, "Test circuit"),
        "no Test circuit once passed"
    );
    assert!(screen(&mut h).chimes >= 1, "the chime");
    h.run_steps(10);
    std::thread::sleep(std::time::Duration::from_millis(400));
    h.run_steps(4);
    shot(&mut h, "lessons-success");
    // step 3: break the loop (Do it for me again), step 4: the quiz
    next(&mut h, "Next: break the loop");
    assert!(has_label(&h, "Break the loop"));
    h.get_by_label("Test circuit").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 20.0, |h| has_label_containing(
            h,
            "the black jumper from column 10 to the top − rail"
        )),
        "the jumper to remove: {:?}",
        text_of(&h, "Problem:")
    );
    h.get_by_label("Do it for me").click();
    next(&mut h, "Next: what makes a circuit?");
    h.get_by_label("A closed loop from + through the LED back to −")
        .click();
    h.run_steps(3);
    next(&mut h, "Finish lesson");
    // the lesson's own celebration + Next lesson
    assert!(has_label(&h, "Lesson complete"));
    assert!(has_label(&h, "Next lesson: Volts, amps, ohms"));
    std::thread::sleep(std::time::Duration::from_millis(900));
    h.run_steps(6);
    shot(&mut h, "lessons-complete");
    h.get_by_label("Next lesson: Volts, amps, ohms").click();
    h.run_steps(5);
    assert!(has_label(&h, "Lesson 2 of 8"));
    assert!(has_label_containing(
        &h,
        "Lesson 1: What is a circuit? (done)"
    ));
}

#[test]
fn lesson_3_meter_in_parallel_is_explained() {
    let mut h = harness();
    go(&mut h, Route::Lessons);
    h.get_by_label("Lesson 3: Ohm's law with an LED").click();
    h.run_steps(4);
    next(&mut h, "Next: read the current");
    assert!(has_label(&h, "Read the current"));
    // the student moves the meter across the LED, like a voltmeter
    {
        let run = screen(&mut h).run.as_mut().unwrap();
        let lib = run.doc.lib.clone();
        let w = run
            .doc
            .project()
            .wires
            .iter()
            .find(|w| {
                w.a == bb_circuit::project::Endpoint::pin("bb1", "a10")
                    && w.b == bb_circuit::project::Endpoint::pin("dmm1", "MA")
            })
            .map(|w| w.id.clone())
            .expect("meter wire");
        run.doc.editor.delete(&[], &[w]);
        use bb_circuit::project::Endpoint;
        run.doc.editor.add_wire(
            &lib,
            Endpoint::pin("bb1", "b10"),
            Endpoint::pin("bb1", "t-14"),
        );
        run.doc.editor.add_wire(
            &lib,
            Endpoint::pin("dmm1", "MA"),
            Endpoint::pin("bb1", "b9"),
        );
        run.doc.touched();
    }
    // the success state goes once the goals fail again
    assert!(
        wait_for(&mut h, 20.0, |h| has_label(h, "Test circuit")),
        "Test circuit comes back"
    );
    h.get_by_label("Test circuit").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 20.0, |h| has_label_containing(
            h,
            "Problem: The meter is across D1 (red LED), in parallel"
        )),
        "{:?}",
        text_of(&h, "Problem:")
    );
    assert!(has_label_containing(&h, "Next: Wire it in series instead"));
    assert!(has_label(&h, "Same cause as above"));
    h.run_steps(6);
    shot(&mut h, "lessons-meter-parallel");
}

#[test]
fn diagnosis_reads_at_the_minimum_window_size() {
    let mut h = sized(1280.0, 800.0);
    go(&mut h, Route::Lessons);
    h.get_by_label("Lesson 1: What is a circuit?").click();
    h.run_steps(4);
    next(&mut h, "Next: close the loop");
    h.run_steps(5);
    wire(&mut h, ("ps1", "VBUS"), ("bb1", "t+1"));
    wire(&mut h, ("bb1", "t+10"), ("bb1", "d11"));
    h.get_by_label("Test circuit").click();
    h.run_steps(4);
    assert!(wait_for(&mut h, 20.0, |h| has_label_containing(
        h,
        "Problem: Your red jumper reaches column 11"
    )));
    h.run_steps(6);
    h.state_mut().ctx.toasts.clear();
    h.run_steps(2);
    screenshot(&mut h, "lessons-diagnosis-1280");
    // and the success state at the minimum size
    h.get_by_label("Do it for me").click();
    assert!(wait_for(&mut h, 60.0, |h| enabled(
        h,
        "Next: break the loop"
    )));
    assert!(wait_for(&mut h, 10.0, |h| has_label_containing(
        h,
        "Done: Loop closed"
    )));
    std::thread::sleep(std::time::Duration::from_millis(400));
    h.run_steps(6);
    shot(&mut h, "lessons-success-1280");
}

#[test]
fn controls_card_world_slider_part_value_and_add_part() {
    let mut h = harness();
    go(&mut h, Route::Lessons);
    // night light: the room light is a control in the step panel
    h.get_by_label("Sensors & the real world").click();
    h.run_steps(4);
    h.get_by_label("Lesson 3: A night light that thinks")
        .click();
    h.run_steps(4);
    next(&mut h, "Next: daylight");
    next(&mut h, "Next: night falls");
    assert!(has_label(&h, "Night falls"));
    assert!(
        wait_for(&mut h, 20.0, |h| has_label(h, "Room light (lux)")),
        "the room light slider"
    );
    assert!(has_label_containing(&h, "Serial: light="), "the serial box");
    h.get_by_label("Show me").click();
    h.run_steps(4);
    assert!(has_label(&h, "Show me: Set the room light to 5 lux"));
    h.run_steps(4);
    shot(&mut h, "lessons-controls");
    h.get_by_label("Do it for me").click();
    next(&mut h, "Next: where it switches");

    // voltage dividers: Do it for me through a table of predictions, then a part value + Add R3
    go(&mut h, Route::Lessons);
    h.get_by_label("Electricity you can see").click();
    h.run_steps(4);
    h.get_by_label("Lesson 5: Voltage dividers").click();
    h.run_steps(4);
    next(&mut h, "Next: change r2, predict the middle");
    h.get_by_label("Do it for me").click();
    h.run_steps(4);
    next(&mut h, "Next: load it down");
    assert!(has_label(&h, "R2 resistance"));
    assert!(has_label(&h, "Add R3"));
    // type the value and Set
    h.get_by_label("R2 resistance").click();
    h.run_steps(1);
    for _ in 0..8 {
        h.key_press(egui::Key::Backspace);
    }
    h.get_by_label("R2 resistance").type_text("10k");
    h.run_steps(1);
    h.get_by_label("Set R2 resistance").click();
    h.run_steps(4);
    let r2 = {
        let run = screen(&mut h).run.as_ref().unwrap();
        let p = run.doc.project().part("r2").unwrap();
        let d = run.doc.lib.get(&p.part).unwrap();
        bb_circuit::Project::prop_f64(d, p, "resistance").unwrap()
    };
    assert!((r2 - 10_000.0).abs() < 1.0, "R2 = {r2}");
    h.get_by_label("Add R3").click();
    h.run_steps(4);
    assert!(
        screen(&mut h)
            .run
            .as_ref()
            .unwrap()
            .doc
            .project()
            .part("r3")
            .is_some(),
        "R3 added"
    );
    h.get_by_label("Test circuit").click();
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 20.0, |h| has_label_containing(
            h,
            "Problem: Nothing connects R3's lead in column"
        ) || has_label_containing(
            h,
            "Problem: Nothing connects R3"
        )),
        "{:?}",
        text_of(&h, "Problem:")
    );
}

#[test]
fn dragging_a_wire_in_a_lesson_says_which_net_a_hole_joins() {
    let mut h = harness();
    go(&mut h, Route::Lessons);
    h.get_by_label("Lesson 1: What is a circuit?").click();
    h.run_steps(4);
    next(&mut h, "Next: close the loop");
    h.run_steps(5);
    let a = pin_screen(&mut h, "ps1", "VBUS");
    let b = pin_screen(&mut h, "bb1", "b5");
    h.hover_at(a);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: a,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    for i in 1..=10 {
        h.hover_at(a + (b - a) * (i as f32 / 10.0));
        h.run_steps(1);
    }
    h.run_steps(2);
    shot(&mut h, "lessons-drag-hint");
    h.event(egui::Event::PointerButton {
        pos: b,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 30.0, |h| enabled(h, "Next: break the loop")),
        "a jumper into b5 closes the loop"
    );
}
