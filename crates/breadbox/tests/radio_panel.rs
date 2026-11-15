//! The Radios panel on the Firmware screen (`RF-1`, `RF-3`, `RF-4`, `RF-7`): dragging the
//! bench away from the access point lowers the Wi-Fi card's RSSI and is stored with the
//! project; jitter and "drop the AP at t = N s" reach the network; placement, conditions and
//! the LAN bridge are saved in `project.json`, restored on open and undoable (the running
//! bench follows undo / redo); the packet log exports as pcap-ng and CSV; "Add BLE hat" wires
//! an HM-10 to a fresh project's UNO and the virtual phone sees `HMSoft`. Saves
//! `docs/screenshots/radios-placement.png` with `BREADBOX_UPDATE_SCREENSHOTS=1`.

mod common;

use breadbox::BreadboxApp;
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn open_example(h: &mut H, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(4);
}

fn go(h: &mut H, section: &str) {
    h.get_by_label("Switch screen").click();
    h.run_steps(2);
    h.get_by_label(section).click();
    h.run_steps(3);
}

fn first_label(h: &H, part: &str) -> Option<String> {
    h.query_all_by_label_contains(part).next().and_then(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    })
}

/// The Wi-Fi card's RSSI (`-54 dBm (3.2 m, 1 wall)` → -54).
fn wifi_rssi(h: &H) -> Option<i32> {
    let r = h.state().ctx.bench.radio.as_ref()?;
    r.cards
        .iter()
        .flat_map(|c| c.rows.iter())
        .find(|(k, _)| k == "RSSI")
        .and_then(|(_, v)| v.split_whitespace().next()?.parse().ok())
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
    for i in 1..=8 {
        let t = i as f32 / 8.0;
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

#[test]
fn placement_conditions_and_packet_log_export_on_garden_node() {
    let out = std::env::temp_dir().join(format!("bb-radio-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    // SAFETY: set before the app reads it; no other test in this binary reads it
    unsafe { std::env::set_var("BREADBOX_EXPORT_DIR", &out) };
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    h.run_steps(2);
    go(&mut h, "Firmware");
    // the ESP-01 joins SIM-AP from the default spot: 3.2 m, one wall
    assert!(
        wait_for(&mut h, 120.0, |h| wifi_rssi(h).is_some()),
        "the Wi-Fi card never showed an RSSI"
    );
    let near = wifi_rssi(&h).unwrap();
    assert!(
        (-56..=-52).contains(&near),
        "{near} dBm at the default spot"
    );
    assert!(has_label_containing(
        &h,
        "SIM-AP · 3.2 m · 1 wall · -54 dBm"
    ));
    assert!(has_label(&h, "Floor plan"));

    // drag the bench ~2.5 m further from the AP
    let marker = h.get_by_label_contains("Bench position 3.2 m").rect();
    let from = marker.center();
    drag(&mut h, from, from + egui::vec2(95.0, 0.0));
    assert!(
        wait_for(&mut h, 30.0, |h| wifi_rssi(h)
            .is_some_and(|r| r <= near - 5)),
        "RSSI did not drop: {:?} (was {near})",
        wifi_rssi(&h)
    );
    let pos = |h: &H| {
        let doc = h.state().ctx.circuit.as_ref().unwrap();
        bb_sim::radio::ProjectRadios::from_project(doc.project()).bench_pos
    };
    let moved = pos(&h);
    assert!(moved.x > 5.0, "stored with the project: {moved:?}");
    assert!(
        first_label(&h, "Bench position ").is_some_and(|l| !l.contains(" 3.2 m")),
        "the marker moved"
    );
    // keyboard: the focused marker nudges by 0.5 m
    h.get_by_label_contains("Bench position ").focus();
    h.run_steps(1);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert!(
        (pos(&h).x - (moved.x + 0.5)).abs() < 1e-6,
        "{:?} after → (was {moved:?})",
        pos(&h)
    );
    // the engine's readout catches up with the marker
    let want = pos(&h).x;
    assert!(
        wait_for(&mut h, 30.0, |h| h
            .state()
            .ctx
            .bench
            .radio
            .as_ref()
            .and_then(|r| r.placement.as_ref())
            .is_some_and(|p| (p.bench.0 - want).abs() < 1e-6)),
        "placement readout"
    );
    assert!(has_label_containing(
        &h,
        &format!("SIM-AP · {want:.1} m · 1 wall · ")
    ));
    h.run_steps(4);
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "radios-placement");

    // RF-4: export the packet log both ways
    assert!(wait_for(&mut h, 60.0, |h| h
        .state()
        .ctx
        .bench
        .radio
        .as_ref()
        .is_some_and(|r| !r.log.is_empty())));
    h.run_steps(2);
    h.get_by_label("Export pcap-ng…").click();
    h.run_steps(3);
    let pcap = std::fs::read(out.join("garden-node-packets.pcapng")).expect("pcap-ng written");
    assert_eq!(
        u32::from_le_bytes(pcap[0..4].try_into().unwrap()),
        0x0A0D_0D0A,
        "section header block"
    );
    assert!(pcap.len() > 100);
    h.get_by_label("Export CSV…").click();
    h.run_steps(3);
    let csv = std::fs::read_to_string(out.join("garden-node-packets.csv")).expect("CSV written");
    assert!(
        csv.starts_with("time_s,dir,radio,proto,src,dst,bytes,frame,dropped\n"),
        "{}",
        &csv[..csv.len().min(200)]
    );
    assert!(csv.lines().count() > 1);
    assert!(
        h.state()
            .ctx
            .toasts
            .items()
            .iter()
            .any(|t| t.text.contains("garden-node-packets.csv")),
        "toast with the saved path"
    );

    // RF-3: jitter slider and "drop the AP at t = N s"
    let jitter = h.get_by_label_contains("Jitter ");
    jitter.scroll_to_me();
    h.run_steps(2);
    h.get_by_label_contains("Jitter ").focus();
    h.run_steps(1);
    for _ in 0..5 {
        h.key_press(egui::Key::ArrowRight);
    }
    h.run_steps(2);
    let field = h.get_by_label("Drop the AP at (s)");
    field.scroll_to_me();
    h.run_steps(2);
    h.get_by_label("Drop the AP at (s)").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("Drop the AP at (s)").type_text("30");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    h.get_by_label("Drop the AP at t =").click();
    h.run_steps(2);
    assert!(
        wait_for(&mut h, 30.0, |h| h
            .state()
            .ctx
            .bench
            .radio
            .as_ref()
            .is_some_and(
                |r| r.conditions.drop_ap_at_s == Some(30.0) && r.conditions.jitter_ms == 6.0
            )),
        "conditions: {:?}",
        h.state()
            .ctx
            .bench
            .radio
            .as_ref()
            .map(|r| r.conditions.clone())
    );
    // the engine's defaults (3 ms ± 1) until changed; jitter nudged 1 → 6
    assert!(has_label_containing(&h, "Latency 3 ms ± 6"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn add_ble_hat_wires_an_hm10_and_the_phone_sees_hmsoft() {
    let tmp = std::env::temp_dir().join(format!("bb-ble-hat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let p = breadbox::project::Project::create(
        &tmp,
        "ble-hat",
        breadbox::project::ProjectMode::Playground,
    )
    .unwrap();
    let mut h = harness();
    h.state_mut().open_path(&p.dir);
    h.run_steps(4);
    go(&mut h, "Firmware");
    assert!(has_label_containing(&h, "No BLE radio on this bench"));
    let add = h.get_by_label("Add BLE hat");
    add.scroll_to_me();
    h.run_steps(2);
    h.get_by_label("Add BLE hat").click();
    h.run_steps(3);
    // one undoable edit: the module + its four wires to the UNO
    {
        let doc = h.state().ctx.circuit.as_ref().unwrap();
        let hat = doc
            .project()
            .parts
            .iter()
            .find(|p| p.part == "hm10")
            .expect("HM-10 placed");
        assert_eq!(hat.reference, "U2");
        let wired: Vec<String> = doc
            .project()
            .wires
            .iter()
            .filter(|w| w.a.part() == Some(hat.id.as_str()))
            .map(|w| format!("{:?}", w.b))
            .collect();
        assert_eq!(wired.len(), 4, "{wired:?}");
        assert_eq!(doc.editor.undo_label(), Some("Add BLE hat"));
    }
    assert!(
        h.state()
            .ctx
            .toasts
            .items()
            .iter()
            .any(|t| t.text.contains("HM-10")),
        "toast"
    );
    // the bench runs and the phone scans the module
    assert!(
        wait_for(&mut h, 120.0, |h| h
            .state()
            .ctx
            .bench
            .radio
            .as_ref()
            .and_then(|r| r.ble.as_ref())
            .and_then(|b| b.phone.as_ref())
            .is_some_and(|ph| ph.scan.iter().any(|d| d.name == "HMSoft"))),
        "the virtual phone never saw HMSoft: {:?}",
        h.state().ctx.bench.radio.as_ref().map(|r| r.ble.clone())
    );
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 10.0, |h| has_label(h, "Connect HMSoft")),
        "scan list in the Bluetooth LE card"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// The open project's saved radio settings.
fn saved(h: &H) -> bb_sim::radio::ProjectRadios {
    let doc = h.state().ctx.circuit.as_ref().unwrap();
    bb_sim::radio::ProjectRadios::from_project(doc.project())
}

/// The running bench's network conditions and bench position (telemetry).
fn engine(h: &H) -> Option<(breadbox::bench::NetConditionsView, (f64, f64))> {
    let r = h.state().ctx.bench.radio.as_ref()?;
    Some((r.conditions.clone(), r.placement.as_ref()?.bench))
}

fn nudge(h: &mut H, slider: &str, key: egui::Key, n: usize) {
    let s = h.get_by_label_contains(slider);
    s.scroll_to_me();
    h.run_steps(2);
    h.get_by_label_contains(slider).focus();
    h.run_steps(1);
    for _ in 0..n {
        h.key_press(key);
        h.run_steps(1);
    }
    h.run_steps(2);
}

fn undo(h: &mut H, redo: bool) {
    // a button keeps the focus (not a text field): ⌘Z is the project's undo
    h.get_by_label_contains("Bench position ").focus();
    h.run_steps(1);
    let m = if redo {
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
    } else {
        egui::Modifiers::COMMAND
    };
    h.key_press_modifiers(m, egui::Key::Z);
    h.run_steps(3);
}

/// RF-1 / RF-3: the bench's floor-plan position, the network conditions and the LAN bridge
/// are part of the project — every change is one undoable edit that the running bench follows
/// (also on undo / redo), saved in `project.json` and restored when the project is reopened.
#[test]
fn radio_settings_are_saved_with_the_project_and_undoable() {
    let tmp = std::env::temp_dir().join(format!("bb-radio-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dir = tmp.join("radios.breadbox");
    copy_dir(&repo().join("examples/garden-node"), &dir);
    // a dummy "LAN broker" so the bridge never reaches a real one on this machine
    let broker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let broker_port = broker.local_addr().unwrap().port();
    // checking the box first bridges to the default localhost:1883: hold that port too (when
    // a real broker runs here, the bridge part is skipped)
    let default_port = std::net::TcpListener::bind("127.0.0.1:1883").ok();

    let mut h = harness();
    h.state_mut().open_path(&dir);
    h.run_steps(4);
    h.get_by_label("Run").click();
    h.run_steps(2);
    go(&mut h, "Firmware");
    assert!(
        wait_for(&mut h, 120.0, |h| wifi_rssi(h).is_some()
            && engine(h).is_some()),
        "the bench never reported its radios"
    );
    // nothing saved yet: the panel shows what the bench runs
    assert!(has_label_containing(&h, "Latency 3 ms ± 1"));
    assert!(saved(&h) == bb_sim::radio::ProjectRadios::default());

    // conditions: latency 3 → 7 ms, loss 0 → 2 %, drop the AP at 45 s
    nudge(&mut h, "Latency ", egui::Key::ArrowRight, 4);
    nudge(&mut h, "Packet loss ", egui::Key::ArrowRight, 2);
    h.get_by_label("Drop the AP at (s)").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("Drop the AP at (s)").type_text("45");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    h.get_by_label("Drop the AP at t =").click();
    h.run_steps(2);
    let c = saved(&h).settings.conditions;
    assert_eq!(
        (c.latency_ms, c.jitter_ms, c.loss_pct, c.drop_ap_at_s),
        (7.0, 1.0, 2.0, Some(45.0))
    );
    let mut steps = 4 + 2 + 1;
    // LAN bridge: on, then pointed at the dummy broker
    let bridge = format!("127.0.0.1:{broker_port}");
    if default_port.is_some() {
        let cb = h.get_by_label("Bridge to my real LAN broker");
        cb.scroll_to_me();
        h.run_steps(2);
        h.get_by_label("Bridge to my real LAN broker").click();
        h.run_steps(2);
        h.get_by_label("LAN broker address").click();
        h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label("LAN broker address").type_text(&bridge);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        let b = saved(&h).settings.lan_bridge.expect("bridge saved");
        assert_eq!(format!("{}:{}", b.host, b.port), bridge);
        steps += 2;
    }
    // placement: drag the bench away from the AP
    let marker = h.get_by_label_contains("Bench position 3.2 m");
    marker.scroll_to_me();
    h.run_steps(12);
    let from = h
        .get_by_label_contains("Bench position 3.2 m")
        .rect()
        .center();
    drag(&mut h, from, from + egui::vec2(95.0, 0.0));
    let moved = saved(&h).bench_pos;
    assert!(moved.x > 5.0, "{moved:?}");
    steps += 1;
    {
        let doc = h.state().ctx.circuit.as_ref().unwrap();
        assert_eq!(doc.editor.undo_label(), Some("Move the bench"));
    }
    // the running bench has all of it
    assert!(
        wait_for(&mut h, 30.0, |h| engine(h).is_some_and(|(c, b)| c
            .latency_ms
            == 7.0
            && c.loss_pct == 2.0
            && c.drop_ap_at_s == Some(45.0)
            && (b.0 - moved.x).abs() < 1e-6)),
        "engine: {:?}",
        engine(&h)
    );

    // ⌘Z: the bench goes back to 3.2 m — in the project, on the floor plan and in the engine
    undo(&mut h, false);
    assert!((saved(&h).bench_pos.x - 3.2).abs() < 1e-9);
    assert!(
        wait_for(&mut h, 30.0, |h| engine(h)
            .is_some_and(|(_, b)| (b.0 - 3.2).abs() < 1e-6)),
        "engine after undo: {:?}",
        engine(&h)
    );
    assert!(has_label_containing(&h, "Bench position 3.2 m"));
    // ⌘⇧Z: moved again
    undo(&mut h, true);
    assert!((saved(&h).bench_pos.x - moved.x).abs() < 1e-9);
    assert!(
        wait_for(&mut h, 30.0, |h| engine(h)
            .is_some_and(|(_, b)| (b.0 - moved.x).abs() < 1e-6)),
        "engine after redo: {:?}",
        engine(&h)
    );
    // undo everything: defaults everywhere (the sliders too)
    for _ in 0..steps {
        undo(&mut h, false);
    }
    assert!(
        h.state()
            .ctx
            .circuit
            .as_ref()
            .unwrap()
            .editor
            .undo_label()
            .is_none()
    );
    assert!(
        h.state()
            .ctx
            .circuit
            .as_ref()
            .unwrap()
            .project()
            .radios
            .is_null()
    );
    assert!(
        wait_for(&mut h, 30.0, |h| engine(h).is_some_and(|(c, b)| c
            .latency_ms
            == 3.0
            && c.loss_pct == 0.0
            && c.drop_ap_at_s.is_none()
            && c.bridge.is_none()
            && (b.0 - 3.2).abs() < 1e-6)),
        "engine after undoing everything: {:?}",
        engine(&h)
    );
    assert!(has_label_containing(&h, "Latency 3 ms ± 1"));
    assert!(has_label_containing(&h, "Packet loss 0 %"));
    // … and redo everything
    for _ in 0..steps {
        undo(&mut h, true);
    }
    assert!((saved(&h).bench_pos.x - moved.x).abs() < 1e-9);
    assert!(has_label_containing(&h, "Latency 7 ms ± 1"));
    assert!(
        wait_for(&mut h, 30.0, |h| engine(h).is_some_and(|(c, b)| c
            .latency_ms
            == 7.0
            && c.drop_ap_at_s == Some(45.0)
            && (b.0 - moved.x).abs() < 1e-6)),
        "engine after redoing everything: {:?}",
        engine(&h)
    );

    // saved in project.json …
    h.state_mut().flush();
    let on_disk = bb_circuit::Project::load(&dir).unwrap();
    let r = bb_sim::radio::ProjectRadios::from_project(&on_disk);
    assert_eq!(r.bench_pos, moved);
    assert_eq!(r.settings.conditions.latency_ms, 7.0);
    assert_eq!(r.settings.conditions.drop_ap_at_s, Some(45.0));
    drop(h);

    // … and restored when the project is opened again
    let mut h = harness();
    h.state_mut().open_path(&dir);
    h.run_steps(4);
    h.get_by_label("Run").click();
    h.run_steps(2);
    go(&mut h, "Firmware");
    assert!(has_label_containing(&h, "Latency 7 ms ± 1"));
    assert!(has_label_containing(&h, "Packet loss 2 %"));
    assert!(
        h.query_by_label("Drop the AP at (s)")
            .and_then(|n| n.accesskit_node().value())
            .is_some_and(|v| v == "45"),
        "drop-AP field"
    );
    if default_port.is_some() {
        assert!(
            h.query_by_label("LAN broker address")
                .and_then(|n| n.accesskit_node().value())
                .is_some_and(|v| v == bridge),
            "LAN bridge address"
        );
    }
    assert!(
        wait_for(&mut h, 120.0, |h| engine(h).is_some_and(|(c, b)| c
            .latency_ms
            == 7.0
            && c.loss_pct == 2.0
            && c.drop_ap_at_s == Some(45.0)
            && (default_port.is_none()
                || c.bridge.as_deref() == Some(bridge.as_str()))
            && (b.0 - moved.x).abs() < 1e-6)),
        "reopened bench: {:?}",
        engine(&h)
    );
    assert!(has_label_containing(
        &h,
        &format!("Bench position {:.1} m", moved.x)
    ));
    drop(h);
    drop(broker);
    let _ = std::fs::remove_dir_all(&tmp);
}
