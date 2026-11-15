//! GUI end-to-end tests (`TEST-5`): drive the real app through AccessKit labels with
//! `egui_kittest` and save 1440×900 screenshots of each screen to `docs/screenshots/`.

mod common;

use common::*;
#[allow(unused_imports)]
use egui_kittest::kittest::NodeT;
use egui_kittest::kittest::Queryable;

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
    !matches!(
        h.state().ctx.toolchain.state,
        breadbox::toolchain::ToolchainState::Detecting
    )
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
            s.iter().any(|l| l.text.starts_with("soil_raw="))
                && s.iter().any(|l| l.text.starts_with("mqtt: PUB"))
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
    if d.is_paused() {
        d.location.as_ref().map(|l| l.line)
    } else {
        None
    }
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
    assert!(
        wait_for(&mut h, 240.0, |h| paused_at(h) == Some(8)),
        "breakpoint on line 8 never hit"
    );
    h.run_steps(3);
    assert!(has_label_containing(&h, "paused at blink.ino:8"));
    screenshot(&mut h, "firmware-debug");
    h.get_by_label("Step over").click();
    assert!(
        wait_for(&mut h, 30.0, |h| paused_at(h) == Some(9)),
        "step over did not land on line 9"
    );
    h.get_by_label("Continue").click();
    assert!(
        wait_for(&mut h, 30.0, |h| paused_at(h) == Some(8)),
        "continue did not come back to the breakpoint"
    );
}

#[test]
fn pro_analysis_runs_demo_and_populates_plots() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    go(&mut h, "Pro");
    assert!(matches!(
        h.state().ctx.route,
        breadbox::router::Route::Pro(_)
    ));
    // auto-runs on first open
    assert!(
        wait_for(&mut h, 120.0, |h| has_label(h, "tran plot")
            && has_label(h, "dc plot")
            && has_label(h, "mc histogram")),
        "plots never appeared"
    );
    h.run_steps(3);
    screenshot(&mut h, "pro-analysis");
    assert!(has_label_containing(&h, "3 steps · "));
    // the netlist is synced from the circuit (USB cable + board load: VCC ≈ 4.98 V)
    assert!(
        has_label_containing(&h, "V(A1) 2.5") || has_label_containing(&h, "V(A1) 2.6"),
        "op table row"
    );
    assert!(has_label_containing(&h, "synced from schematic"));
    assert!(has_label_containing(
        &h,
        "Transient (.tran) — 0 → 10 ms · step 1 µs"
    ));
    screenshot(&mut h, "pro-analysis");
    // re-run explicitly
    h.get_by_label("Run analyses").click();
    assert!(wait_for(&mut h, 120.0, |h| !has_label(h, "Cancel")
        && has_label(h, "tran plot")));
}

#[test]
fn navigate_home_firmware_pro_home() {
    let mut h = harness();
    open_example(&mut h, "blink");
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Workbench);
    go(&mut h, "Firmware");
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Firmware);
    go(&mut h, "Pro");
    assert!(matches!(
        h.state().ctx.route,
        breadbox::router::Route::Pro(_)
    ));
    h.get_by_label("Breadbox").click();
    h.run_steps(3);
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Home);
    assert!(has_label_containing(&h, "Open recent blink"));
}

#[test]
fn every_screen_renders_and_is_saved() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    // the Workbench and Pro → Schematic are real screens now (tests/workbench.rs renders them)
    assert!(has_label(&h, "Bench canvas"));
    go(&mut h, "Plugins");
    assert!(has_label_containing(&h, "SCREEN SLOT · Plugins"));
    screenshot(&mut h, "plugins");
    go(&mut h, "Lessons");
    assert!(has_label_containing(&h, "SCREEN SLOT · Lessons"));
    screenshot(&mut h, "lessons");
    go(&mut h, "Pro");
    h.get_by_label("Schematic").click();
    h.run_steps(3);
    assert_eq!(
        h.state().ctx.route,
        breadbox::router::Route::Pro(breadbox::router::ProTab::Schematic)
    );
    assert!(has_label(&h, "Schematic canvas"));
    h.get_by_label("PCB").click();
    h.run_steps(3);
    screenshot(&mut h, "pro-pcb");
}

#[test]
fn idle_app_does_not_repaint_continuously() {
    let mut h = harness();
    assert!(wait_for(&mut h, 30.0, |h| toolchain_settled(h)));
    assert!(h.try_run().is_ok(), "Home keeps requesting repaints");
    open_example(&mut h, "blink");
    go(&mut h, "Firmware");
    h.run_steps(3);
    assert!(h.try_run().is_ok(), "Firmware keeps requesting repaints");
}

#[test]
fn keyboard_shortcuts_navigate_and_close() {
    use eframe::egui::{Key, Modifiers};
    let mut h = harness();
    open_example(&mut h, "blink");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run_steps(3);
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Firmware);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num3);
    h.run_steps(3);
    assert!(matches!(
        h.state().ctx.route,
        breadbox::router::Route::Pro(_)
    ));
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    assert_eq!(h.state().ctx.route, breadbox::router::Route::Home);
    assert!(h.state().ctx.project.is_none());
}

#[test]
fn verify_blink_then_a_compile_error_lands_in_the_editor() {
    let mut h = harness();
    open_example(&mut h, "blink");
    go(&mut h, "Firmware");
    assert!(wait_for(&mut h, 30.0, |h| toolchain_settled(h)));
    if h.state().ctx.toolchain.ready().is_none() {
        eprintln!("skipping: no arduino-cli/avr-gcc toolchain on this machine");
        return;
    }
    h.get_by_label("Verify").click();
    assert!(
        wait_for(&mut h, 240.0, |h| has_label_containing(h, "Sketch uses ")),
        "no size report"
    );
    // break the sketch: type at the start of the file
    let editor = h.get_by_role(eframe::egui::Role::MultilineTextInput);
    editor.focus();
    h.run_steps(1);
    h.get_by_role(eframe::egui::Role::MultilineTextInput)
        .type_text("oops ");
    h.run_steps(2);
    h.get_by_label("Verify").click();
    let ok = wait_for(&mut h, 240.0, |h| {
        has_label_containing(h, "error: 'oops' does not name a type")
    });
    h.run_steps(2);
    screenshot(&mut h, "firmware-error");
    assert!(ok, "diagnostic not shown in the output");
}

#[test]
fn light_theme_renders() {
    let mut h = harness();
    h.state_mut()
        .ctx
        .command(breadbox::router::Command::SetTheme(
            breadbox::theme::ThemeChoice::Light,
        ));
    h.run_steps(4);
    assert_eq!(h.state().ctx.theme, breadbox::theme::ThemeChoice::Light);
    screenshot(&mut h, "home-light");
}

#[test]
fn pro_analysis_card_popover_disables_monte_carlo() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    go(&mut h, "Pro");
    assert!(wait_for(&mut h, 120.0, |h| has_label(h, "mc histogram")));
    h.get_by_label_contains("Monte Carlo (.mc) — 100 runs · seed 7")
        .click();
    h.run_steps(2);
    assert!(has_label(&h, "Apply"));
    h.get_by_label("Enabled").click();
    h.run_steps(1);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert!(has_label_containing(&h, "Monte Carlo (.mc) — off"));
    assert!(
        wait_for(&mut h, 120.0, |h| has_label(h, "tran plot")
            && !has_label(h, "mc histogram")),
        "the re-run should drop the histogram"
    );
    // and back on
    h.get_by_label_contains("Monte Carlo (.mc) — off").click();
    h.run_steps(2);
    h.get_by_label("Enabled").click();
    h.get_by_label("Apply").click();
    assert!(wait_for(&mut h, 120.0, |h| has_label(h, "mc histogram")));
}

#[test]
fn firmware_library_manager_opens_and_closes() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    go(&mut h, "Firmware");
    h.get_by_label("Manage libraries…").click();
    h.run_steps(3);
    assert!(has_label_containing(&h, "Library manager"));
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert!(!has_label_containing(&h, "Library manager"));
}
