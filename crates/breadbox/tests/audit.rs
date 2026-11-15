//! GUI tests for spec items the audit found unreachable from the app (`docs/SPEC-AUDIT.md`):
//! the Workbench simulation menu (`SIM-1` / `SIM-3`: single steps, reset MCU only, seed, sync
//! quantum), `.cir` import (`PRJ-4`) and plugin permission violations surfacing as errors
//! (`PLUG-2`).

mod common;

use bb_core::SimTime;
use breadbox::BreadboxApp;
use breadbox::router::{ProTab, Route};
use common::*;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use std::path::PathBuf;
use std::sync::OnceLock;

type H = Harness<'static, BreadboxApp>;

/// One scratch folder per test binary for imported / created projects.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("projects")).unwrap();
        std::fs::create_dir_all(d.join("export")).unwrap();
        // SAFETY: set once, before any test of this binary reads them
        unsafe {
            std::env::set_var("BREADBOX_PROJECTS_DIR", d.join("projects"));
            std::env::set_var("BREADBOX_EXPORT_DIR", d.join("export"));
        }
        d
    })
}

fn open_example(h: &mut H, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(4);
}

fn sim_time(h: &H) -> SimTime {
    h.state().ctx.bench.sim_time
}

fn open_sim_menu(h: &mut H) {
    let l = h
        .query_all_by_label_contains("Speed: ")
        .next()
        .and_then(|n| n.accesskit_node().label())
        .expect("speed menu");
    h.get_by_label(&l).click();
    h.run_steps(2);
}

/// Waits until the bench reports a stable (paused) time; returns it.
fn settle(h: &mut H) -> SimTime {
    let mut last = sim_time(h);
    let mut same = 0;
    assert!(wait_for(h, 30.0, |h| {
        let t = sim_time(h);
        let running = h.state().ctx.bench.running;
        if t == last && !running {
            same += 1;
        } else {
            same = 0;
            last = t;
        }
        same > 10
    }));
    sim_time(h)
}

#[test]
fn simulation_menu_steps_resets_mcu_and_replays_with_a_seed() {
    scratch();
    let mut h = harness();
    open_example(&mut h, "blink");
    assert!(wait_for(&mut h, 30.0, |h| h
        .state_mut()
        .ctx
        .sim()
        .is_some()));
    let t0 = settle(&mut h);
    // one millisecond
    open_sim_menu(&mut h);
    h.get_by_label("Step 1 ms").click();
    h.run_steps(2);
    let t1 = settle(&mut h);
    assert_eq!(t1 - t0, SimTime::from_ms(1), "{t0:?} → {t1:?}");
    // one quantum (1 µs by default)
    h.get_by_label("Step 1 µs").click();
    h.run_steps(2);
    let t2 = settle(&mut h);
    assert_eq!(t2 - t1, SimTime::from_us(1), "{t1:?} → {t2:?}");
    // a coarser sync quantum: the quantum step follows it
    h.get_by_label("10 µs").click();
    h.run_steps(3);
    let p = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .settings
        .clone();
    assert_eq!(p.sync_quantum_us, 10.0);
    let t3 = settle(&mut h); // the bench restarted with the new quantum
    assert!(t3 < t2, "settings change restarts the bench: {t3:?}");
    open_sim_menu_if_closed(&mut h);
    h.get_by_label("Step 10 µs").click();
    h.run_steps(2);
    let t4 = settle(&mut h);
    assert_eq!(t4 - t3, SimTime::from_us(10));
    // run a little, then reset only the MCU: the serial monitor says so, time keeps going
    open_sim_menu_if_closed(&mut h);
    for _ in 0..20 {
        h.get_by_label("Step 1 ms").click();
        h.run_steps(1);
    }
    let before = settle(&mut h);
    h.get_by_label("Reset MCU only · U1").click();
    h.run_steps(2);
    open_sim_menu_if_closed(&mut h);
    h.get_by_label("Step 1 ms").click();
    assert!(
        wait_for(&mut h, 30.0, |h| h
            .state()
            .ctx
            .bench
            .serial
            .iter()
            .any(|l| l.text.contains("MCU reset (bench state kept)"))),
        "no MCU reset line: {:?}",
        h.state().ctx.bench.serial
    );
    assert!(
        settle(&mut h) > before,
        "reset MCU only keeps the bench clock"
    );
    // the seed is stored with the project and restarts the bench
    let seed = h.get_by_label("Seed value");
    seed.set_value_accesskit(42.0);
    h.run_steps(3);
    assert_eq!(
        h.state()
            .ctx
            .circuit
            .as_ref()
            .unwrap()
            .project()
            .settings
            .seed,
        42
    );
    // undo restores the previous settings
    h.state_mut().ctx.command(breadbox::router::Command::Undo);
    h.run_steps(3);
    assert_ne!(
        h.state()
            .ctx
            .circuit
            .as_ref()
            .unwrap()
            .project()
            .settings
            .seed,
        42
    );
}

fn open_sim_menu_if_closed(h: &mut H) {
    if !has_label(h, "Step 1 ms") {
        open_sim_menu(h);
    }
}

#[test]
fn native_menu_step_and_reset_mcu_commands_work() {
    scratch();
    let mut h = harness();
    open_example(&mut h, "blink");
    assert!(wait_for(&mut h, 30.0, |h| h
        .state_mut()
        .ctx
        .sim()
        .is_some()));
    let t0 = settle(&mut h);
    h.state_mut()
        .ctx
        .command(breadbox::router::Command::StepSim(
            breadbox::router::StepSize::Millisecond,
        ));
    h.run_steps(2);
    assert_eq!(settle(&mut h) - t0, SimTime::from_ms(1));
    h.state_mut()
        .ctx
        .command(breadbox::router::Command::ResetMcu);
    h.state_mut()
        .ctx
        .command(breadbox::router::Command::StepSim(
            breadbox::router::StepSize::Millisecond,
        ));
    assert!(wait_for(&mut h, 30.0, |h| h
        .state()
        .ctx
        .bench
        .serial
        .iter()
        .any(|l| l.text.contains("MCU reset"))));
}

/// PRJ-4: an ngspice netlist opens as a new Pro project (auto-placed schematic, its analyses
/// run); Home offers "Import .cir…" and File → Import sends the same command.
#[test]
fn cir_netlist_imports_as_a_pro_project_and_runs() {
    let dir = scratch();
    let mut h = harness();
    assert!(has_label(&h, "Import .cir…"), "Home has an import entry");
    let cir = dir.join("lowpass.cir");
    std::fs::write(
        &cir,
        "* RC low-pass\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 159n\n.ac dec 20 10 100k\n.end\n",
    )
    .unwrap();
    h.state_mut().open_path(&cir);
    h.run_steps(4);
    assert_eq!(h.state().ctx.route, Route::Pro(ProTab::Analysis));
    let p = h.state().ctx.project.as_ref().unwrap().clone();
    assert!(
        p.dir.starts_with(dir.join("projects")),
        "{}",
        p.dir.display()
    );
    assert!(p.dir.join("imported/lowpass.cir").exists());
    let parts = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .parts
        .clone();
    assert!(parts.len() >= 3, "{parts:#?}");
    assert!(parts.iter().all(|x| x.schematic.is_some()));
    // Pro → Analysis shows the netlist synced from the imported schematic; .ac runs → Bode
    assert!(
        wait_for(&mut h, 30.0, |h| has_label_containing(
            h,
            "synced from schematic"
        )),
        "imported projects sync the netlist from their schematic"
    );
    h.state_mut()
        .ctx
        .command(breadbox::router::Command::RunAnalyses);
    assert!(
        wait_for(&mut h, 60.0, |h| has_label_containing(h, ".ac")),
        "no Bode plot"
    );
    // a bad netlist is an error toast, not a crash
    let bad = dir.join("broken.cir");
    std::fs::write(&bad, "* nothing useful\nQ1 a b\n.end\n").unwrap();
    h.state_mut().open_path(&bad);
    h.run_steps(2);
    assert_eq!(h.state().ctx.route, Route::Pro(ProTab::Analysis));
}

/// PLUG-2: a local plugin part whose wasm changes its own load without declaring
/// `inject:load` is stopped by the sandbox — and the user sees it as an error on the
/// Workbench (not only in the Firmware output), while the bench keeps running.
#[test]
fn a_plugin_violating_its_permissions_shows_an_error_on_the_workbench() {
    let dir = scratch().join("sneaky");
    std::fs::create_dir_all(&dir).unwrap();
    let wat = r#"(module
  (import "bb" "bb_set_ext" (func $set_ext (param i32 i32 f64)))
  (memory (export "memory") 1)
  (data (i32.const 16) "r")
  (global $heap (mut i32) (i32.const 4096))
  (func (export "bb_abi_version") (result i32) i32.const 1)
  (func (export "bb_alloc") (param $n i32) (result i32) (local $p i32)
    global.get $heap local.set $p
    global.get $heap local.get $n i32.add global.set $heap
    local.get $p)
  (func (export "bb_on_start") i32.const 16 i32.const 1 f64.const 10 call $set_ext))"#;
    std::fs::write(dir.join("model.wasm"), wat::parse_str(wat).unwrap()).unwrap();
    std::fs::write(
        dir.join("parts.json"),
        r#"[{
  "id": "sneakyres", "name": "Sneaky resistor", "category": "building_blocks",
  "ref_prefix": "R", "description": "test", "pins": ["1:passive", "2:passive"],
  "model": {"spice": ["$(el:R) $(1) $(2) {$(ext:r)}"], "ext": [{"name": "r", "default": 1000}],
            "behaviour": "wasm"},
  "bench": {"layout": "row", "off_board": true, "size": [4, 2]}
}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        r#"{"id": "test.sneaky", "name": "Sneaky resistor", "version": "1.0.0", "kind": ["part"],
            "summary": "changes its resistance without asking",
            "provides": {"parts": "parts.json", "model": "model.wasm"}, "permissions": []}"#,
    )
    .unwrap();
    let mut h = harness();
    open_example(&mut h, "blink");
    h.state_mut().ctx.navigate(Route::Plugins);
    h.run_steps(3);
    // SAFETY: the only test in this binary that answers file dialogs
    unsafe { std::env::set_var("BREADBOX_TEST_PICK", &dir) };
    h.get_by_label("Load local plugin…").click();
    h.run_steps(2);
    h.get_by_label("Plugin folder…").click();
    h.run_steps(3);
    if has_label(&h, "Allow and load") {
        h.get_by_label("Allow and load").click();
    }
    h.run_steps(4);
    // the part is in the open circuit's library: place it across the UNO's 5 V / GND
    h.state_mut().ctx.navigate(Route::Workbench);
    h.run_steps(4);
    {
        let doc = h.state_mut().ctx.circuit.as_mut().unwrap();
        let lib = doc.lib.clone();
        assert!(lib.get("sneakyres").is_some(), "plugin part in the library");
        let r = doc.editor.add_part(&lib, "sneakyres", 40.0, 0.0).unwrap();
        use bb_circuit::project::Endpoint;
        doc.editor
            .add_wire(&lib, Endpoint::pin("u1", "5V"), Endpoint::pin(&r, "1"));
        doc.editor
            .add_wire(&lib, Endpoint::pin("u1", "GND"), Endpoint::pin(&r, "2"));
        doc.touched();
    }
    h.run_steps(3);
    h.get_by_label("Run").click();
    let found = wait_for(&mut h, 60.0, |h| {
        h.state()
            .ctx
            .toasts
            .items()
            .iter()
            .any(|t| t.text.contains("permission denied") && t.text.contains("inject:load"))
    });
    assert!(
        found,
        "no error toast: {:?}",
        h.state()
            .ctx
            .toasts
            .items()
            .iter()
            .map(|t| t.text.clone())
            .collect::<Vec<_>>()
    );
    assert!(h.state().ctx.bench.running, "the bench keeps running");
}

/// APP-5: edits younger than the autosave debounce are not lost — switching projects saves the
/// circuit, and quitting (eframe calls `save` → `BreadboxApp::flush`) writes sketch tabs too.
#[test]
fn quick_edits_survive_a_project_switch_and_quit() {
    let dir = scratch();
    let mut h = harness();
    let proj = breadbox::project::Project::create(
        &dir.join("projects"),
        "quick-edits",
        breadbox::project::ProjectMode::Playground,
    )
    .unwrap();
    h.state_mut().open_path(&proj.dir);
    h.run_steps(3);
    // a circuit edit, then straight to another project (no time for the 1.2 s autosave)
    {
        let doc = h.state_mut().ctx.circuit.as_mut().unwrap();
        let lib = doc.lib.clone();
        doc.editor.add_part(&lib, "led", 30.0, 10.0).unwrap();
        doc.touched();
    }
    h.run_steps(1);
    h.state_mut().open_path(&repo().join("examples/blink"));
    h.run_steps(2);
    let saved = bb_circuit::Project::load(&proj.dir).unwrap();
    assert!(
        saved.parts.iter().any(|p| p.part == "led"),
        "the LED was saved on the switch: {:?}",
        saved.parts.iter().map(|p| &p.part).collect::<Vec<_>>()
    );
    // a sketch edit, then quit
    h.state_mut().open_path(&proj.dir);
    h.run_steps(2);
    h.state_mut().ctx.navigate(Route::Firmware);
    h.run_steps(4);
    let editor = h.get_by_role(eframe::egui::Role::MultilineTextInput);
    editor.focus();
    h.run_steps(1);
    h.get_by_role(eframe::egui::Role::MultilineTextInput)
        .type_text("// quick note\n");
    h.run_steps(1);
    h.state_mut().flush();
    let ino = proj.dir.join("firmware/U1/quick-edits/quick-edits.ino");
    let text = std::fs::read_to_string(&ino).unwrap();
    assert!(text.contains("// quick note"), "{text}");
}

/// PRO-3 / SPICE-3: every analysis card is in the sidebar (Noise and Op point too, "off" until
/// added), and `.meas` / `.tf` results from the netlist show up in a measurements table.
#[test]
fn pro_shows_every_analysis_card_and_meas_tf_results() {
    let dir = scratch();
    let mut h = harness();
    let proj = breadbox::project::Project::create(
        &dir.join("projects"),
        "meas-tf",
        breadbox::project::ProjectMode::Pro,
    )
    .unwrap();
    std::fs::write(
        proj.dir.join("analyses/meas-tf.cir"),
        "* divider with a filter cap\nV1 in 0 PULSE(0 5 0 1u 1u 1m 2m) AC 1\nR1 in out 1k\nR2 out 0 1k\nC1 out 0 100n\n.tran 10u 4m\n.meas tran vmax MAX v(out)\n.meas tran vavg AVG v(out) FROM=1m TO=3m\n.tf v(out) V1\n.end\n",
    )
    .unwrap();
    h.state_mut().open_path(&proj.dir);
    h.run_steps(4);
    assert_eq!(h.state().ctx.route, Route::Pro(ProTab::Analysis));
    for card in [
        "Transient (.tran)",
        "DC sweep",
        "AC",
        "Monte Carlo",
        "Temperature",
        "Noise (.noise) — off",
        "Op point",
    ] {
        assert!(has_label_containing(&h, card), "card {card}");
    }
    h.state_mut()
        .ctx
        .command(breadbox::router::Command::RunAnalyses);
    assert!(
        wait_for(&mut h, 60.0, |h| has_label_containing(
            h,
            ".meas / .tf — measurements"
        )),
        "no measurements table"
    );
    // rows read "<name> <value>"
    assert!(has_label_containing(&h, "vmax "), "vmax row");
    assert!(has_label_containing(&h, "vavg "), "vavg row");
    assert!(
        has_label_containing(&h, ".tf transfer_function 0.5000"),
        "tf gain 0.5"
    );
    screenshot(&mut h, "pro-measurements");
    assert!(
        has_label_containing(&h, "output_impedance_at_v(out) 500"),
        "output impedance R1 ‖ R2"
    );
}

fn bench_view(h: &H) -> breadbox::canvas::pen::View {
    h.state()
        .ctx
        .egui
        .data(|d| d.get_temp::<breadbox::canvas::pen::View>(eframe::egui::Id::unique("bench-view")))
        .expect("bench view")
}

/// Latest `cm=` reading of the sonar sketch.
fn last_cm(h: &H) -> Option<f64> {
    h.state()
        .ctx
        .bench
        .serial
        .iter()
        .rev()
        .find_map(|l| l.text.split("cm=").nth(1)?.trim().parse::<f64>().ok())
}

/// A mouse-wheel scroll over a point (content moves by `dy` points).
fn scroll_at(h: &mut H, at: eframe::egui::Pos2, dy: f32) {
    use eframe::egui;
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(20);
}

fn drag(h: &mut H, from: eframe::egui::Pos2, to: eframe::egui::Pos2) {
    use eframe::egui;
    h.hover_at(from);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    for k in 1..=8 {
        let t = k as f32 / 8.0;
        h.hover_at(from + (to - from) * t);
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
}

/// WORLD-4 / WORLD-5 on a real sketch: an HC-SR04 bench (the `sonar` firmware prints `cm=`)
/// gets a hand from the World panel → the reading follows; dragging the hand on the bench moves
/// it; a world script and a keyframe timeline drive variables; undo removes the timeline.
#[test]
fn hand_object_scripts_and_timelines_drive_the_world() {
    let dir = scratch();
    let proj = breadbox::project::Project::create(
        &dir.join("projects"),
        "sonar-hand",
        breadbox::project::ProjectMode::Playground,
    )
    .unwrap();
    // UNO running the sonar fixture + an HC-SR04 (TRIG D7, ECHO D8) to the right of it
    {
        let lib = bb_circuit::Library::builtin();
        let mut c = bb_circuit::Project::load(&proj.dir).unwrap();
        c.world.noise = false;
        let fw = repo().join("crates/bb-sim/tests/fixtures/sonar.uno.hex");
        c.parts[0].firmware.as_mut().unwrap().image = Some(fw.display().to_string());
        let mut ed = bb_circuit::edit::Editor::new(c);
        let s = ed.add_part(&lib, "hc_sr04", 80.0, 30.0).unwrap();
        use bb_circuit::project::Endpoint;
        for (a, b) in [
            ("5V", "VCC"),
            ("GND", "GND"),
            ("D7", "TRIG"),
            ("D8", "ECHO"),
        ] {
            ed.add_wire(&lib, Endpoint::pin("u1", a), Endpoint::pin(&s, b));
        }
        ed.project.save(&proj.dir).unwrap();
    }
    let mut h = harness();
    h.state_mut().open_path(&proj.dir);
    h.run_steps(4);
    assert_eq!(h.state().ctx.route, Route::Workbench);
    h.get_by_label("Run").click();
    assert!(
        wait_for(&mut h, 60.0, |h| last_cm(h).is_some_and(|c| c > 300.0)),
        "nothing in front of the sensor: {:?}",
        last_cm(&h)
    );
    // the World panel offers the objects the bench's sensors read
    assert!(has_label_containing(&h, "drag them on the bench"));
    h.get_by_label("Add hand").click();
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 60.0, |h| last_cm(h)
            .is_some_and(|c| (20.0..35.0).contains(&c))),
        "hand 30 cm in front: {:?}",
        last_cm(&h)
    );
    // drag the hand 10 cm closer on the bench (≈ 39 bench units down)
    let (hx, hy) = {
        let o = &h
            .state()
            .ctx
            .circuit
            .as_ref()
            .unwrap()
            .project()
            .world
            .objects[0];
        (o.pos.x / 0.00254, o.pos.y / 0.00254)
    };
    let v = bench_view(&h);
    let from = v.to_screen(hx, hy);
    let to = v.to_screen(hx, hy + 39.37);
    assert!(
        h.state().ctx.egui.content_rect().contains(to) && to.y > 100.0,
        "drag stays on the canvas: {to:?}"
    );
    drag(&mut h, from, to);
    let moved = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .world
        .objects[0]
        .pos;
    assert!(
        (moved.y - (hy + 39.37) * 0.00254).abs() < 0.01,
        "the hand's new place is saved: {moved:?}"
    );
    assert!(
        wait_for(&mut h, 60.0, |h| last_cm(h)
            .is_some_and(|c| (12.0..20.0).contains(&c))),
        "reading follows the dragged hand: {:?}",
        last_cm(&h)
    );
    screenshot(&mut h, "world-objects");
    // a script: the room warms 2 °C per world hour
    h.get_by_label("Program the world").click();
    h.run_steps(2);
    h.get_by_label("New script").click();
    h.get_by_label("New script")
        .type_text("air.temp_c = 18 + 2*t_hours");
    h.run_steps(1);
    h.get_by_label("Add script").click();
    h.run_steps(3);
    let w = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .world
        .clone();
    assert_eq!(w.scripts.len(), 1, "{:?}", w.scripts);
    assert!(
        wait_for(&mut h, 30.0, |h| h
            .state_mut()
            .ctx
            .sim()
            .and_then(|s| s.telemetry())
            .is_some_and(|t| (t.world.temp_c - 18.0).abs() < 0.5)),
        "the script drives the air temperature"
    );
    // a bad script is refused with a message, nothing stored
    h.get_by_label("New script").click();
    h.get_by_label("New script").type_text("lux = 3 *");
    h.get_by_label("Add script").click();
    h.run_steps(2);
    assert_eq!(
        h.state()
            .ctx
            .circuit
            .as_ref()
            .unwrap()
            .project()
            .world
            .scripts
            .len(),
        1
    );
    // a timeline on the ambient light: two keyframes, preview drawn (scroll the panel down
    // to it, like a user)
    scroll_at(&mut h, eframe::egui::pos2(1290.0, 400.0), -600.0);
    h.get_by_label("Add timeline: Add timeline…").click();
    h.run_steps(2);
    h.get_by_label("Ambient light (lx)").click();
    h.run_steps(3);
    let w = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .world
        .clone();
    assert_eq!(w.timelines.len(), 1);
    assert_eq!(w.timelines[0].var, "light.ambient_lux");
    assert!(has_label(&h, "Timeline preview light.ambient_lux"));
    h.get_by_label("Keyframe 2 value")
        .set_value_accesskit(900.0);
    h.run_steps(3);
    let w = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .world
        .clone();
    assert_eq!(w.timelines[0].keys[1].value, 900.0, "{:?}", w.timelines[0]);
    screenshot(&mut h, "world-program");
    // undo takes the keyframe edit back
    h.state_mut().ctx.command(breadbox::router::Command::Undo);
    h.run_steps(3);
    let w = h
        .state()
        .ctx
        .circuit
        .as_ref()
        .unwrap()
        .project()
        .world
        .clone();
    assert_ne!(w.timelines[0].keys[1].value, 900.0);
}

/// WORLD-1: "Water automatically" in the World panel schedules waterings in the world model
/// (stored with the project and applied to the running bench).
#[test]
fn world_panel_schedules_watering() {
    scratch();
    let mut h = harness();
    open_example(&mut h, "garden-node");
    assert!(wait_for(&mut h, 30.0, |h| h
        .state_mut()
        .ctx
        .sim()
        .is_some()));
    h.get_by_label("Water automatically: off").click();
    h.run_steps(2);
    h.get_by_label("Every 6 h").click();
    h.run_steps(3);
    let pot = h.state().ctx.circuit.as_ref().unwrap().project().world.pots[0].clone();
    assert_eq!(pot.schedule.map(|s| s.every_h), Some(6.0), "{pot:?}");
    assert!(has_label(&h, "Water automatically: every 6 h · 100 ml"));
    let live = h
        .state_mut()
        .ctx
        .sim()
        .unwrap()
        .with_bench(|b| b.world().config().pots[0].schedule)
        .flatten();
    assert_eq!(
        live.map(|s| s.every_h),
        Some(6.0),
        "the running world waters too"
    );
}

/// RF-8: radio annotations (`wifi: AT+CWJAP … → OK …`, `mqtt: PUB …`) appear in the serial
/// monitor between the firmware's lines, and the "Radio annotations" toggle hides them.
#[test]
fn serial_monitor_radio_annotations_toggle() {
    scratch();
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    assert!(
        wait_for(&mut h, 90.0, |h| has_label_containing(
            h,
            "mqtt: PUB garden/pot1"
        )),
        "no mqtt annotation"
    );
    assert!(has_label_containing(&h, "soil_raw="));
    h.get_by_label("Radio annotations").click();
    h.run_steps(3);
    assert!(!has_label_containing(&h, "mqtt: PUB"), "annotations hidden");
    assert!(!has_label_containing(&h, "wifi: "), "annotations hidden");
    assert!(
        wait_for(&mut h, 30.0, |h| has_label_containing(h, "soil_raw=")),
        "firmware lines stay"
    );
    h.get_by_label("Radio annotations").click();
    h.run_steps(3);
    assert!(has_label_containing(&h, "mqtt: PUB"), "shown again");
}
