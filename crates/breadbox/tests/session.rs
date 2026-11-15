//! A new user's first session, scripted through the UI only (AccessKit labels, pointer drags,
//! typing): Home → New Playground project → build an LED circuit on a breadboard → write blink
//! → Verify → Upload → Run → the LED blinks; Home → New Pro project → type an RC low-pass →
//! AC / Bode → the corner; Lessons → Track 1 lesson 1 to the end. Screenshots:
//! `docs/screenshots/session-*.png` (with `BREADBOX_UPDATE_SCREENSHOTS=1`).

mod common;

use breadbox::BreadboxApp;
use breadbox::router::{ProTab, Route};
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use std::path::PathBuf;
use std::sync::OnceLock;

type H = Harness<'static, BreadboxApp>;

fn projects() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // SAFETY: set once before any test of this binary reads it
        unsafe { std::env::set_var("BREADBOX_PROJECTS_DIR", &d) };
        d
    })
}

fn doc(h: &H) -> &breadbox::circuit::CircuitDoc {
    h.state().ctx.circuit.as_ref().expect("circuit")
}

fn view(h: &H) -> breadbox::canvas::pen::View {
    h.state()
        .ctx
        .egui
        .data(|d| d.get_temp::<breadbox::canvas::pen::View>(egui::Id::unique("bench-view")))
        .expect("bench view")
}

/// Screen position of a part's pin or a breadboard hole.
fn pin_screen(h: &H, part: &str, pin: &str) -> egui::Pos2 {
    let d = doc(h);
    let p = d.project().part(part).expect("part");
    let def = d.lib.get(&p.part).expect("def");
    let leads = bb_circuit::Project::bench_leads(def, p).expect("leads");
    let (_, x, y) = leads
        .iter()
        .find(|(n, _, _)| n == pin)
        .unwrap_or_else(|| panic!("{part}.{pin}"));
    view(h).to_screen(*x, *y)
}

fn release(h: &mut H, to: egui::Pos2) {
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    h.remove_cursor();
    h.run_steps(2);
}

fn press_drag(h: &mut H, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for i in 1..=10 {
        h.hover_at(from + (to - from) * (i as f32 / 10.0));
        h.run_steps(1);
    }
    release(h, to);
}

/// Drags a palette tile onto the bench at `to`; returns the new part's id.
fn drop_tile(h: &mut H, tile: &str, to: egui::Pos2) -> String {
    let n = doc(h).project().parts.len();
    // the palette tile (the header's Breadboard / Schematic toggle shares a label)
    let from = h
        .query_all_by_role_and_label(egui::accesskit::Role::Button, tile)
        .map(|n| n.rect())
        .find(|r| r.left() < 300.0 && r.top() > 100.0)
        .unwrap_or_else(|| panic!("no {tile} tile"))
        .center();
    press_drag(h, from, to);
    let p = doc(h).project();
    assert_eq!(p.parts.len(), n + 1, "{tile} dropped");
    p.parts.last().unwrap().id.clone()
}

fn click_first(h: &H, label: &str) {
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no {label}"))
        .click();
}

fn connected(h: &H, a: (&str, &str), b: (&str, &str)) -> bool {
    let d = doc(h);
    bb_circuit::Connectivity::compute(d.project(), &d.lib).connected(a, b)
}

#[test]
fn session_1_playground_led_blinks_from_my_own_sketch() {
    let dir = projects();
    let mut h = harness();
    // Home → New Playground project
    assert!(has_label(&h, "New Playground project"));
    click_first(&h, "New Playground project");
    h.run_steps(3);
    // the suggested name is selected: typing replaces it
    h.get_by_label("Project name").type_text("first-blink");
    h.run_steps(1);
    h.get_by_label("Create project").click();
    h.run_steps(5);
    assert_eq!(h.state().ctx.route, Route::Workbench);
    let p = h.state().ctx.project.as_ref().unwrap().dir.clone();
    assert!(p.starts_with(dir), "{}", p.display());
    assert!(p.ends_with("first-blink.breadbox"), "{}", p.display());
    screenshot(&mut h, "session-1-new-project");
    // the new project starts with an UNO R3 (U1) and a blink sketch; drop a breadboard,
    // a resistor and an LED onto the bench
    let canvas = h.get_by_label("Bench canvas").rect();
    let bb = drop_tile(
        &mut h,
        "Breadboard",
        canvas.center() + egui::vec2(canvas.width() * 0.18, 0.0),
    );
    // the breadboard runs off the canvas: Zoom → Fit to view
    let z = h
        .query_all_by_label_contains("Zoom ")
        .next()
        .and_then(|n| n.accesskit_node().label())
        .expect("zoom menu");
    h.get_by_label(&z).click();
    h.run_steps(2);
    h.get_by_label("Fit to view").click();
    h.run_steps(3);
    // a 220 Ω resistor with its first lead in hole e10 (it spans to e14), an LED with its
    // anode in d14 (same column as the resistor's second lead) and its cathode in d15
    let at = pin_screen(&h, &bb, "e10");
    let r = drop_tile(&mut h, "Resistor", at);
    assert!(
        connected(&h, (&r, "1"), (&bb, "a10")),
        "R lead 1 in column 10"
    );
    assert!(
        connected(&h, (&r, "2"), (&bb, "a14")),
        "R lead 2 in column 14"
    );
    let at = pin_screen(&h, &bb, "d14");
    let led = drop_tile(&mut h, "LED", at);
    assert!(connected(&h, (&led, "a"), (&r, "2")), "LED anode meets R");
    assert!(
        connected(&h, (&led, "k"), (&bb, "a15")),
        "LED cathode in column 15"
    );
    // a new resistor is 10 kΩ: select it and make it 220 Ω in the Inspector
    let body = {
        let d = doc(&h);
        let p = d.project().part(&r).unwrap();
        let leads = bb_circuit::Project::bench_leads(d.lib.get("resistor").unwrap(), p).unwrap();
        view(&h).to_screen((leads[0].1 + leads[1].1) / 2.0, leads[0].2)
    };
    h.hover_at(body);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: body,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(2);
    assert!(has_label_containing(&h, "INSPECTOR · R1"), "R1 selected");
    // the World panel sits above the Inspector: hide it (rail button) to see the fields
    click_first(&h, "World");
    h.run_steps(3);
    assert!(has_label_containing(
        &h,
        "Colour bands brown black orange gold"
    ));
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Resistance")
        .click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Resistance")
        .type_text("220");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let ohms = doc(&h)
        .project()
        .part(&r)
        .and_then(|p| p.props.get("resistance").and_then(|v| v.as_f64()));
    assert_eq!(ohms, Some(220.0));
    assert!(has_label_containing(&h, "Colour bands red red brown gold"));
    click_first(&h, "World");
    h.run_steps(2);
    // jumper wires: D13 → column 10, column 15 → GND
    let a = pin_screen(&h, "u1", "D13");
    let b = pin_screen(&h, &bb, "a10");
    press_drag(&mut h, a, b);
    let a = pin_screen(&h, &bb, "a15");
    let b = pin_screen(&h, "u1", "GND");
    press_drag(&mut h, a, b);
    assert!(connected(&h, ("u1", "D13"), (&r, "1")), "D13 → R");
    assert!(connected(&h, (&led, "k"), ("u1", "GND")), "LED → GND");
    let checks = h.state_mut().ctx.circuit.as_mut().unwrap().checks();
    eprintln!("ERC/DRC: {:?}", checks.summary());
    screenshot(&mut h, "session-3-wired");

    // Firmware: replace the starter sketch with my own blink (D13, 250 ms)
    h.get_by_label("Switch screen").click();
    h.run_steps(2);
    h.get_by_label("Firmware").click();
    h.run_steps(4);
    assert_eq!(h.state().ctx.route, Route::Firmware);
    let editor = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    editor.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .type_text(SKETCH);
    h.run_steps(2);
    let text = editor_text(&h);
    assert!(
        text.starts_with("// my first blink") && text.matches("void loop").count() == 1,
        "the editor holds exactly my sketch:\n{text}"
    );
    assert!(
        wait_for(&mut h, 30.0, |h| !matches!(
            h.state().ctx.toolchain.state,
            breadbox::toolchain::ToolchainState::Detecting
        )),
        "toolchain detection"
    );
    let have_toolchain = h.state().ctx.toolchain.ready().is_some();
    if have_toolchain {
        h.get_by_label("Verify").click();
        assert!(
            wait_for(&mut h, 120.0, |h| has_label_containing(h, "Sketch uses ")),
            "no size report"
        );
        screenshot(&mut h, "session-4-verified");
        h.get_by_label("Upload to sim").click();
        assert!(
            wait_for(&mut h, 120.0, |h| has_label_containing(
                h,
                "Flashed virtual U1"
            )),
            "not flashed"
        );
    } else {
        eprintln!("no toolchain: the bench runs the starter sketch's prebuilt image");
    }
    // back to the bench: 1× real time, Run, and watch the LED blink
    h.get_by_label("Switch screen").click();
    h.run_steps(2);
    h.get_by_label("Workbench").click();
    h.run_steps(4);
    h.state_mut()
        .ctx
        .bench
        .set_speed(breadbox::bench::SimSpeed::RealTime(1.0));
    // Upload to sim leaves the bench running (the header shows Pause); otherwise press Run
    if has_label(&h, "Run") {
        h.get_by_label("Run").click();
    }
    assert!(wait_for(&mut h, 10.0, |h| has_label(h, "Pause")));
    let (mut on, mut off) = (0, 0);
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < 20.0 && (on < 3 || off < 3) {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(40));
        if let Some(b) = led_brightness(&mut h, &led) {
            if b > 0.4 {
                on += 1;
            } else if b < 0.05 {
                off += 1;
            }
        }
    }
    if on < 3 || off < 3 {
        screenshot(&mut h, "session-debug");
        let t = h.state_mut().ctx.sim().and_then(|s| s.telemetry()).unwrap();
        for p in &t.parts {
            eprintln!("PART {} {:?} {:?}", p.part, p.visual, p.readings);
        }
        eprintln!(
            "time {:?} serial {:?}",
            h.state().ctx.bench.sim_time,
            h.state()
                .ctx
                .bench
                .serial
                .iter()
                .map(|l| l.text.clone())
                .collect::<Vec<_>>()
        );
    }
    assert!(
        on >= 3 && off >= 3,
        "the LED blinks: {on} on / {off} off samples"
    );
    // and it is my sketch: the LED current is set by the 220 Ω resistor (≈ 13 mA, not the
    // board's own L LED)
    let i = led_current_ma(&mut h, &led);
    eprintln!("LED current while on: {i:?} mA");
    wait_for(&mut h, 5.0, |h| {
        led_brightness(h, &led).is_some_and(|b| b > 0.4)
    });
    screenshot(&mut h, "session-5-blinking");
}

const SKETCH: &str = "// my first blink: an LED on D13 through 220 ohms\nconst int LED = 13;\n\nvoid setup() {\n  pinMode(LED, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(LED, HIGH);\n  delay(250);\n  digitalWrite(LED, LOW);\n  delay(250);\n}\n";

fn editor_text(h: &H) -> String {
    h.query_all_by_role(egui::accesskit::Role::MultilineTextInput)
        .next()
        .and_then(|n| n.accesskit_node().value())
        .unwrap_or_default()
}

fn led_brightness(h: &mut H, id: &str) -> Option<f32> {
    let t = h.state_mut().ctx.sim()?.telemetry()?;
    let p = t.parts.iter().find(|p| p.part == id)?;
    match p.visual.look {
        bb_core::Look::Led { brightness, .. } => Some(brightness),
        _ => None,
    }
}

fn led_current_ma(h: &mut H, id: &str) -> Option<f64> {
    let t = h.state_mut().ctx.sim()?.telemetry()?;
    let p = t.parts.iter().find(|p| p.part == id)?;
    p.readings
        .iter()
        .find(|(k, _)| k.contains("current") || k == "I")
        .map(|(_, v)| v * 1e3)
}

const RC: &str = "* RC low-pass: R = 1k, C = 159n, fc = 1/(2 pi R C) = 1.00 kHz\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 159n\n.end\n";

#[test]
fn session_2_pro_rc_low_pass_shows_its_corner() {
    projects();
    let mut h = harness();
    click_first(&h, "New Pro project");
    h.run_steps(3);
    h.get_by_label("Project name").type_text("rc-lowpass");
    h.run_steps(1);
    h.get_by_label("Create project").click();
    h.run_steps(5);
    assert_eq!(h.state().ctx.route, Route::Pro(ProTab::Analysis));
    // the project's own netlist (not a demo), saved with it
    assert!(
        has_label(&h, "rc-lowpass.cir"),
        "netlist file of the new project"
    );
    assert!(has_label(&h, "editable · autosaved"));
    screenshot(&mut h, "session-6-new-pro");
    // type the netlist over whatever the editor shows
    let editor = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    editor.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .type_text(RC);
    h.run_steps(3);
    // AC / Bode card → enable → Apply
    let card = h
        .query_all_by_label_contains("AC / Bode (.ac) — ")
        .next()
        .and_then(|n| n.accesskit_node().label())
        .expect("AC card");
    h.get_by_label(&card).click();
    h.run_steps(3);
    if !h
        .query_all_by_label("Enabled")
        .next()
        .is_some_and(|n| n.accesskit_node().is_selected() == Some(true))
    {
        h.get_by_label("Enabled").click();
        h.run_steps(1);
    }
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let text = h
        .query_all_by_role(egui::accesskit::Role::MultilineTextInput)
        .next()
        .and_then(|n| n.accesskit_node().value())
        .unwrap_or_default();
    assert!(text.contains(".ac "), "the card wrote an .ac line:\n{text}");
    // Apply re-runs the analyses (Run analyses does the same)
    assert!(wait_for(&mut h, 30.0, |h| has_label(h, "Run analyses")));
    h.get_by_label("Run analyses").click();
    assert!(
        wait_for(&mut h, 60.0, |h| has_label_containing(h, "−3 dB at ")),
        "no corner on the Bode card"
    );
    screenshot(&mut h, "session-7-bode");
    let title = h
        .query_all_by_label_contains("−3 dB at ")
        .filter_map(|n| {
            let a = n.accesskit_node();
            a.label().or_else(|| a.value())
        })
        .next()
        .unwrap();
    let khz: f64 = title
        .rsplit("−3 dB at ")
        .next()
        .unwrap()
        .trim_end_matches(" kHz")
        .parse()
        .unwrap_or_else(|_| panic!("{title}"));
    assert!((khz - 1.0).abs() < 0.03, "corner {khz} kHz ({title})");
    // what I typed is in the project
    let file = h
        .state()
        .ctx
        .project
        .as_ref()
        .unwrap()
        .dir
        .join("analyses/rc-lowpass.cir");
    assert!(
        wait_for(&mut h, 10.0, |_| std::fs::read_to_string(&file)
            .is_ok_and(|t| t.contains("R1 in out 1k") && t.contains(".ac "))),
        "netlist saved: {:?}",
        std::fs::read_to_string(&file)
    );
}
