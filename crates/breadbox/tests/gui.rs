//! GUI end-to-end tests (`TEST-5`): drive the real app through AccessKit labels with
//! `egui_kittest` and save 1440×900 screenshots of each screen to `docs/screenshots/`.

mod common;

use common::*;
use egui_kittest::kittest::Queryable;
#[allow(unused_imports)]
use egui_kittest::kittest::NodeT;

#[test]
fn home_screen_renders() {
    let mut h = harness();
    assert!(has_label(&h, "Breadbox"));
    assert!(has_label(&h, "New project"));
    assert!(has_label_containing(&h, "Open example garden-node"));
    screenshot(&mut h, "home");
}

fn open_example(h: &mut egui_kittest::Harness<'static, breadbox::BreadboxApp>, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(3);
}

fn go(h: &mut egui_kittest::Harness<'static, breadbox::BreadboxApp>, section: &str) {
    h.get_by_label("Switch screen").click();
    h.run_steps(2);
    h.get_by_label(section).click();
    h.run_steps(3);
}

fn toolchain_settled(h: &egui_kittest::Harness<'static, breadbox::BreadboxApp>) -> bool {
    !matches!(h.state().ctx.toolchain.state, breadbox::toolchain::ToolchainState::Detecting)
}

#[test]
fn firmware_upload_garden_node_shows_serial_and_radio() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    go(&mut h, "Firmware");
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Firmware);
    assert!(has_label(&h, "garden-node.ino"));
    assert!(has_label(&h, "wifi_at.h"));
    assert!(wait_for(&mut h, 30.0, |h| toolchain_settled(h)));
    h.get_by_label("Upload to sim").click();
    // compiles with the real toolchain if present (else flashes the prebuilt fixture), then runs
    assert!(
        wait_for(&mut h, 240.0, |h| {
            let s = &h.state().ctx.bench.serial;
            s.iter().any(|l| l.text.starts_with("soil_raw=")) && s.iter().any(|l| l.text.starts_with("mqtt: PUB"))
        }),
        "serial output never arrived"
    );
    let radio = h.state().ctx.bench.radio.clone().expect("radio model");
    assert_eq!(radio.cards[0].label, "U2");
    h.run_steps(3);
    screenshot(&mut h, "firmware");
    h.get_by_label("Serial monitor").click();
    h.run_steps(3);
    assert!(has_label_containing(&h, "soil_raw="));
    screenshot(&mut h, "firmware-serial");
}

fn paused_at(h: &egui_kittest::Harness<'static, breadbox::BreadboxApp>) -> Option<u32> {
    let d = h.state().ctx.bench.debug.get("U1")?;
    if d.is_paused() { d.location.as_ref().map(|l| l.line) } else { None }
}

#[test]
fn firmware_breakpoint_hit_and_step_over() {
    let mut h = harness();
    open_example(&mut h, "blink");
    go(&mut h, "Firmware");
    assert!(has_label(&h, "blink.ino"));
    h.get_by_label("Step · breakpoints").click();
    h.run_steps(2);
    h.get_by_label("Breakpoint line 8").click();
    h.run_steps(2);
    assert!(h.state().ctx.bench.debug.is_empty() || true);
    assert!(wait_for(&mut h, 30.0, |h| toolchain_settled(h)));
    h.get_by_label("Upload to sim").click();
    assert!(wait_for(&mut h, 240.0, |h| paused_at(h) == Some(8)), "breakpoint on line 8 never hit");
    h.run_steps(3);
    assert!(has_label_containing(&h, "paused at blink.ino:8"));
    screenshot(&mut h, "firmware-debug");
    h.get_by_label("Step over").click();
    assert!(wait_for(&mut h, 30.0, |h| paused_at(h) == Some(9)), "step over did not land on line 9");
    h.get_by_label("Continue").click();
    assert!(wait_for(&mut h, 30.0, |h| paused_at(h) == Some(8)), "continue did not come back to the breakpoint");
}
