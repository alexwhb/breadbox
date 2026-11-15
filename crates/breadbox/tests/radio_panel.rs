//! The Radios panel on the Firmware screen (`RF-1`, `RF-3`, `RF-4`, `RF-7`): dragging the
//! bench away from the access point lowers the Wi-Fi card's RSSI and is stored with the
//! project; jitter and "drop the AP at t = N s" reach the network; the packet log exports as
//! pcap-ng and CSV; "Add BLE hat" wires an HM-10 to a fresh project's UNO and the virtual
//! phone sees `HMSoft`. Saves `docs/screenshots/radios-placement.png` with
//! `BREADBOX_UPDATE_SCREENSHOTS=1`.

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
                |r| r.conditions.drop_ap_at_s == Some(30.0) && r.conditions.jitter_ms == 15.0
            )),
        "conditions: {:?}",
        h.state()
            .ctx
            .bench
            .radio
            .as_ref()
            .map(|r| r.conditions.clone())
    );
    assert!(has_label_containing(&h, "Latency 40 ms ± 15"));
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
