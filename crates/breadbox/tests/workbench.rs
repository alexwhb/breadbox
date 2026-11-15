//! Workbench GUI end-to-end tests (`TEST-5`, `WB-*`): the real app with the co-simulation
//! bench (`SimBench`), driven through AccessKit labels and pointer events with
//! `egui_kittest`. Saves `docs/screenshots/workbench*.png`.

mod common;

use breadbox::BreadboxApp;
use breadbox::canvas::pen::View;
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

type H = Harness<'static, BreadboxApp>;

fn open_example(h: &mut H, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(4);
}

fn serial_has(h: &H, prefix: &str) -> bool {
    h.state()
        .ctx
        .bench
        .serial
        .iter()
        .any(|l| l.text.starts_with(prefix))
}

fn view(h: &H) -> View {
    h.state()
        .ctx
        .egui
        .data(|d| d.get_temp::<View>(egui::Id::unique("bench-view")))
        .expect("bench view")
}

/// Screen position of a part pin / breadboard hole.
fn pin_screen(h: &H, part: &str, pin: &str) -> egui::Pos2 {
    let doc = h.state().ctx.circuit.as_ref().expect("circuit");
    let p = doc.project().part(part).expect("part");
    let def = doc.lib.get(&p.part).expect("def");
    let leads = bb_circuit::Project::bench_leads(def, p).expect("leads");
    let (_, x, y) = leads.iter().find(|(n, _, _)| n == pin).expect("pin");
    view(h).to_screen(*x, *y)
}

/// Clicks the first button with this label (the header comes before the palette).
fn click_first(h: &H, label: &str) {
    h.query_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("no button {label}"))
        .click();
}

#[test]
fn garden_node_runs_on_the_bench_with_serial_and_world_slider() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Workbench);
    assert!(has_label(&h, "Bench canvas"));
    assert!(has_label_containing(&h, "Bench · BB1 + U1 + pot 1"));
    h.get_by_label("Run").click();
    assert!(
        wait_for(&mut h, 120.0, |h| serial_has(h, "soil_raw=")
            && serial_has(h, "mqtt: PUB")),
        "serial output never arrived"
    );
    // a few lines more, like the design's monitor
    assert!(wait_for(&mut h, 120.0, |h| h
        .state()
        .ctx
        .bench
        .serial
        .iter()
        .filter(|l| l.text.starts_with("mqtt: PUB"))
        .count()
        >= 2));
    h.run_steps(4);
    assert!(has_label_containing(&h, "soil_raw="));
    // select the soil sensor: the inspector shows its datasheet values (design)
    let j1 = {
        let d = doc(&h);
        let p = d.project().part("j1").unwrap();
        let leads = bb_circuit::Project::bench_leads(d.lib.get("capsoil").unwrap(), p).unwrap();
        view(&h).to_screen(leads[1].1, leads[1].2 + 1.6)
    };
    h.hover_at(j1);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: j1,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: j1,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    assert!(has_label_containing(&h, "INSPECTOR · J1"));
    // the bench shows live probes and the scope trace
    let tel = h
        .state_mut()
        .ctx
        .bench
        .sim()
        .and_then(|s| s.telemetry())
        .expect("telemetry");
    assert!(
        tel.probes
            .iter()
            .any(|p| p.label.starts_with("Soil sensor"))
    );
    assert!(!tel.scopes.is_empty());
    for p in &tel.parts {
        if let bb_core::Look::Led {
            brightness,
            current,
            ..
        } = &p.visual.look
        {
            eprintln!("LED {} b={brightness} i={current}", p.reference);
        }
    }
    screenshot(&mut h, "workbench");
}

#[test]
fn breadboard_and_schematic_views_toggle_and_pro_schematic_renders() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    assert!(wait_for(&mut h, 120.0, |h| serial_has(h, "soil_raw=")));
    click_first(&h, "Schematic");
    h.run_steps(4);
    assert!(has_label(&h, "Schematic canvas"));
    assert!(!has_label(&h, "Bench canvas"));
    assert!(has_label_containing(&h, "Sheet 1 · garden-node.sch"));
    screenshot(&mut h, "workbench-schematic");
    click_first(&h, "Breadboard");
    h.run_steps(3);
    assert!(has_label(&h, "Bench canvas"));
    // Pro → Schematic
    h.state_mut().ctx.navigate(breadbox::router::Route::Pro(
        breadbox::router::ProTab::Schematic,
    ));
    h.run_steps(5);
    assert!(has_label(&h, "Schematic canvas"));
    assert!(has_label_containing(&h, "same netlist as the bench"));
    screenshot(&mut h, "pro-schematic");
}

/// Releases the pointer at `to` (and only then removes the cursor, so drop targets see it).
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

fn doc(h: &H) -> &breadbox::circuit::CircuitDoc {
    h.state().ctx.circuit.as_ref().expect("circuit")
}

#[test]
fn palette_drag_wire_drawing_inspector_and_undo_redo() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    let n_parts = doc(&h).project().parts.len();
    // drag a resistor from the palette onto the breadboard, first lead into hole g40
    let tile = h
        .query_all_by_role_and_label(egui::accesskit::Role::Button, "Resistor")
        .next()
        .expect("resistor tile")
        .rect()
        .center();
    let hole = pin_screen(&h, "bb1", "g40");
    press_drag(&mut h, tile, hole);
    let (rid, conn_ok) = {
        let d = doc(&h);
        assert_eq!(d.project().parts.len(), n_parts + 1, "resistor added");
        let r = d.project().parts.last().unwrap();
        assert_eq!(r.part, "resistor");
        let conn = bb_circuit::Connectivity::compute(d.project(), &d.lib);
        (
            r.id.clone(),
            conn.connected_on_bench((&r.id, "1"), ("bb1", "j40")),
        )
    };
    assert!(conn_ok, "the dropped resistor's lead sits in column 40");
    // draw a jumper wire from hole a50 to the + rail hole t+40
    let wires = doc(&h).project().wires.len();
    let a = pin_screen(&h, "bb1", "a50");
    let b = pin_screen(&h, "bb1", "b44");
    press_drag(&mut h, a, b);
    {
        let d = doc(&h);
        assert_eq!(d.project().wires.len(), wires + 1, "wire added");
        let conn = bb_circuit::Connectivity::compute(d.project(), &d.lib);
        let w = d.project().wires.last().unwrap();
        assert!(
            conn.connected(("bb1", "e50"), ("bb1", "e44")),
            "columns 44 and 50 joined: {w:?}"
        );
    }
    // undo / redo through the shortcuts
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(
        doc(&h).project().wires.len(),
        wires,
        "undo removed the wire"
    );
    h.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    h.run_steps(3);
    assert_eq!(doc(&h).project().wires.len(), wires + 1, "redo restored it");
    // a schematic-only connection shows as ratsnest on the bench
    {
        let st = h.state_mut();
        let d = st.ctx.circuit.as_mut().unwrap();
        let lib = d.lib.clone();
        let p = d.project().clone();
        let pins = |id: &str| {
            let part = p.part(id).unwrap();
            bb_circuit::Project::schematic_pins(lib.get(&part.part).unwrap(), part).unwrap()
        };
        let a = pins(&rid)[0].clone();
        let b = pins("u1").into_iter().find(|x| x.0 == "D7").unwrap();
        d.editor
            .add_schematic_wire(vec![(a.1, a.2), (b.1, a.2), (b.1, b.2)]);
        d.touched();
        let conn = d.connectivity();
        assert!(
            !conn.ratsnest_bench.is_empty(),
            "ratsnest line for the schematic-only join"
        );
    }
    h.run_steps(2);
    // select R1 on the bench and change its value in the inspector
    let r1 = {
        let d = doc(&h);
        let p = d.project().part("r1").unwrap();
        let leads = bb_circuit::Project::bench_leads(d.lib.get("resistor").unwrap(), p).unwrap();
        let (x, y) = ((leads[0].1 + leads[1].1) / 2.0, leads[0].2);
        view(&h).to_screen(x, y)
    };
    h.hover_at(r1);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: r1,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: r1,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    assert!(has_label_containing(&h, "INSPECTOR · R1"), "R1 selected");
    // hide the World panel (rail) so the inspector's fields are on screen
    click_first(&h, "World");
    h.run_steps(3);
    assert!(has_label_containing(
        &h,
        "Colour bands brown black orange gold"
    ));
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Resistance")
        .click();
    h.run_steps(1);
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Resistance")
        .type_text("4k7");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let v = doc(&h)
        .project()
        .part("r1")
        .and_then(|p| p.props.get("resistance").and_then(|v| v.as_f64()));
    assert_eq!(v, Some(4700.0), "R1 changed to 4.7 kΩ");
    assert!(
        has_label_containing(&h, "Colour bands yellow violet red gold"),
        "bands follow the value"
    );
    screenshot(&mut h, "workbench-edit");
}

#[test]
fn soil_slider_changes_moisture_on_serial() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    assert!(wait_for(&mut h, 120.0, |h| serial_has(h, "soil_raw=")));
    let before: Vec<String> = h
        .state()
        .ctx
        .bench
        .serial
        .iter()
        .filter(|l| l.text.contains("moisture="))
        .map(|l| l.text.clone())
        .collect();
    assert!(
        before.iter().any(|l| l.contains("moisture=4")),
        "{before:?}"
    );
    // drag the soil slider to bone dry
    let track = h
        .query_all_by_label_contains("Soil moisture")
        .find(|n| n.rect().height() < 30.0)
        .expect("slider")
        .rect();
    let from = egui::pos2(track.left() + track.width() * 0.7, track.center().y);
    let to = egui::pos2(track.left() - 20.0, track.center().y);
    press_drag(&mut h, from, to);
    let n0 = h.state().ctx.bench.serial.len();
    assert!(
        wait_for(&mut h, 120.0, |h| h.state().ctx.bench.serial
            [n0.min(h.state().ctx.bench.serial.len())..]
            .iter()
            .any(|l| l.text.contains(" moisture=0%"))),
        "moisture never dropped"
    );
    // the world change is part of the document (undoable)
    let vwc = doc(&h).project().world.pots[0].vwc;
    assert!(vwc < 2.0, "vwc {vwc}");
}

#[test]
fn led_without_resistor_burns_out_with_smoke() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    // bypass R2 (D9 → D1 directly): a wire from D9's strip to the LED anode strip
    {
        let st = h.state_mut();
        let d = st.ctx.circuit.as_mut().unwrap();
        d.editor.delete(&["r2".to_string()], &[]);
        d.touched();
    }
    h.run_steps(2);
    // the LED anode straight onto the 5 V rail
    let a = pin_screen(&h, "bb1", "t+5");
    let b = pin_screen(&h, "bb1", "b22");
    press_drag(&mut h, a, b);
    assert!(has_label_containing(&h, "ERC"));
    h.get_by_label("Run").click();
    let damaged = |h: &mut H| {
        h.state_mut()
            .ctx
            .bench
            .sim()
            .and_then(|s| s.telemetry())
            .is_some_and(|t| {
                t.parts.iter().any(|p| {
                    p.reference == "D1" && p.visual.health.is_damaged() && p.visual.smoke > 0.0
                })
            })
    };
    assert!(wait_for(&mut h, 120.0, damaged), "D1 never burnt out");
    h.run_steps(4);
    screenshot(&mut h, "workbench-smoke");
    // repair
    let ok = h.state_mut().ctx.bench.sim().map(|s| {
        s.send(bb_sim::Command::RepairAll);
        true
    });
    assert_eq!(ok, Some(true));
}

#[test]
fn upload_on_the_live_bench_then_pro_synced_netlist_with_bench_probes() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    assert!(wait_for(&mut h, 120.0, |h| serial_has(h, "soil_raw=")));
    // Firmware → Upload to sim on the running bench
    h.state_mut()
        .ctx
        .navigate(breadbox::router::Route::Firmware);
    h.run_steps(4);
    assert!(wait_for(&mut h, 30.0, |h| !matches!(
        h.state().ctx.toolchain.state,
        breadbox::toolchain::ToolchainState::Detecting
    )));
    h.get_by_label("Upload to sim").click();
    assert!(
        wait_for(&mut h, 240.0, |h| serial_has(h, "Flashed virtual U1")),
        "upload never reached the bench"
    );
    let n = h.state().ctx.bench.serial.len();
    assert!(wait_for(&mut h, 120.0, |h| h.state().ctx.bench.serial
        [n.min(h.state().ctx.bench.serial.len())..]
        .iter()
        .any(|l| l.text.starts_with("soil_raw="))));
    // Pro → Analysis: synced netlist; the bench-probe column has live values
    h.state_mut().ctx.navigate(breadbox::router::Route::Pro(
        breadbox::router::ProTab::Analysis,
    ));
    h.run_steps(4);
    assert!(has_label_containing(&h, "synced from schematic"));
    assert!(
        wait_for(&mut h, 120.0, |h| has_label(h, "tran plot")
            && has_label(h, "dc plot")),
        "plots"
    );
    assert!(h.state().ctx.bench.probes.contains_key("V(A1)"));
    h.run_steps(3);
    let a1 = h.state().ctx.bench.probes["V(A1)"];
    assert!((a1 - 2.6).abs() < 0.2, "bench V(A1) {a1}");
    screenshot(&mut h, "pro-analysis-synced");
}

#[test]
fn share_bundle_exports_and_reopens() {
    let tmp = std::env::temp_dir().join(format!("bb-share-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // SAFETY: set before the app reads them; tests in this binary don't read them elsewhere
    unsafe {
        std::env::set_var("BREADBOX_EXPORT_DIR", &tmp);
        std::env::set_var("BREADBOX_PROJECTS_DIR", tmp.join("projects"));
    }
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Share").click();
    h.run_steps(2);
    h.get_by_label("Project bundle (.breadbox.zip)").click();
    h.run_steps(3);
    let zip = tmp.join("garden-node.breadbox.zip");
    assert!(zip.exists(), "bundle written");
    for (label, file) in [
        ("SPICE netlist (.cir)", "garden-node.cir"),
        ("Bill of materials (.csv)", "garden-node-bom.csv"),
        ("Schematic (.svg)", "garden-node.svg"),
        ("Schematic (.png)", "garden-node-schematic.png"),
    ] {
        h.get_by_label("Share").click();
        h.run_steps(2);
        h.get_by_label(label).click();
        h.run_steps(3);
        assert!(tmp.join(file).exists(), "{file}");
    }
    // re-open the bundle
    h.state_mut().open_path(&zip);
    h.run_steps(4);
    let p = h.state().ctx.project.as_ref().expect("project");
    assert!(
        p.dir.starts_with(tmp.join("projects")),
        "{}",
        p.dir.display()
    );
    assert_eq!(doc(&h).project().parts.len(), 13);
    assert!(!p.read_only, "an imported bundle is a normal project");
    let _ = std::fs::remove_dir_all(&tmp);
}

fn have_cxx() -> bool {
    std::process::Command::new(std::env::var("BREADBOX_CXX").unwrap_or_else(|_| "clang++".into()))
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn esp32_host_board_runs_and_uploads_through_flash_host() {
    if !have_cxx() {
        eprintln!("skipping: no C++ compiler for API-level boards");
        return;
    }
    let mut h = harness();
    open_example(&mut h, "esp32-weather");
    h.get_by_label("Run").click();
    // the bench host-compiles the ESP32 sketch in the background and runs it
    assert!(
        wait_for(&mut h, 60.0, |h| h
            .state()
            .ctx
            .bench
            .serial
            .iter()
            .any(|l| l.kind == breadbox::bench::LineKind::Normal)),
        "ESP32 serial never arrived: {:#?} events {:#?}",
        h.state().ctx.bench.serial,
        h.state().ctx.bench.events
    );
    // Firmware → Upload: Verify (host C++), then FlashHost into the running bench
    h.state_mut()
        .ctx
        .navigate(breadbox::router::Route::Firmware);
    h.run_steps(4);
    h.get_by_label("Upload to sim").click();
    assert!(
        wait_for(&mut h, 240.0, |h| serial_has(h, "Loaded new sketch into")),
        "FlashHost never reached the bench"
    );
}

#[test]
fn virtual_phone_connects_to_the_hm10_of_ble_button() {
    let mut h = harness();
    open_example(&mut h, "ble-button");
    h.get_by_label("Run").click();
    h.run_steps(2);
    h.state_mut()
        .ctx
        .navigate(breadbox::router::Route::Firmware);
    h.run_steps(4);
    assert!(
        wait_for(&mut h, 120.0, |h| h
            .query_all_by_label_contains("Connect ")
            .next()
            .is_some()),
        "the phone never saw an advertiser"
    );
    h.query_all_by_label_contains("Connect ")
        .next()
        .unwrap()
        .click();
    assert!(
        wait_for(&mut h, 60.0, |h| has_label_containing(
            h,
            "Virtual phone · connected to"
        )),
        "phone did not connect"
    );
    screenshot(&mut h, "firmware-phone");
}

/// UI cost of the Workbench with garden-node running at 1× real time (`cargo test --release
/// -p breadbox --test workbench -- --ignored perf --nocapture`).
#[test]
#[ignore]
fn perf_workbench_frame_time_with_garden_node_running() {
    let opts = breadbox::AppOptions {
        sim_speed: breadbox::bench::SimSpeed::RealTime(1.0),
        ..breadbox::AppOptions::test()
    };
    let mut h = app_harness(opts);
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    assert!(wait_for(&mut h, 60.0, |h| serial_has(h, "soil_raw=")));
    // warm up, then time 300 frames (UI + layout + tessellation, as the app does per frame)
    h.run_steps(30);
    let t0 = std::time::Instant::now();
    let sim0 = h.state().ctx.bench.sim_time;
    let n = 300;
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    let mut tris = 0usize;
    for _ in 0..n {
        let f = std::time::Instant::now();
        h.run_steps(1);
        // tessellate like the wgpu integration does every frame
        let shapes = h.output().shapes.clone();
        let prims = h.state().ctx.egui.tessellate(shapes, 1.0);
        tris = prims
            .iter()
            .map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.indices.len() / 3,
                _ => 0,
            })
            .sum();
        let e = f.elapsed();
        total += e;
        worst = worst.max(e);
        // pace like a 60 Hz display
        std::thread::sleep(std::time::Duration::from_millis(16).saturating_sub(f.elapsed()));
    }
    let wall = t0.elapsed().as_secs_f64();
    let sim = (h.state().ctx.bench.sim_time - sim0).as_secs_f64();
    let rt = h
        .state_mut()
        .ctx
        .bench
        .sim()
        .and_then(|s| s.telemetry())
        .map(|t| t.realtime_factor)
        .unwrap_or(0.0);
    let busy = wall - n as f64 * 0.016;
    eprintln!(
        "workbench: {n} frames in {wall:.2} s · CPU per frame avg {:.2} ms / worst {:.1} ms (≈ {:.0} fps headroom) · {tris} triangles · sim {sim:.2} s ({:.2}× real time, engine reports {rt:.2}×)",
        total.as_secs_f64() * 1000.0 / n as f64,
        worst.as_secs_f64() * 1000.0,
        n as f64 / total.as_secs_f64(),
        sim / wall
    );
    let _ = busy;
    assert!(
        sim / wall > 0.9,
        "garden-node fell behind real time with the UI open"
    );
}
