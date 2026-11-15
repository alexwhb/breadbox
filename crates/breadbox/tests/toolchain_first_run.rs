//! First run on a Mac without a firmware toolchain (`MCU-4`), from the Firmware screen.
//!
//! The machine state is simulated, never changed: `BREADBOX_ARDUINO_CLI` points at a missing
//! file (so `PATH` / Homebrew are not searched) and Application Support + Caches are scratch
//! folders, so the user's own arduino-cli install is neither found nor touched.
//!
//! - `first_run_offers_the_install_and_reports_a_failed_download` (offline): Verify without a
//!   toolchain opens the install dialog; an install whose download fails says so and can be
//!   retried.
//! - `a_stuck_arduino_cli_shows_a_message_instead_of_detecting_forever`: an arduino-cli that
//!   never answers (how a Finder-launched app's detection hung behind a macOS privacy prompt)
//!   is stopped after its time budget; the Firmware screen counts the wait, then says what
//!   happened and offers Retry / Install; Retry with a working arduino-cli settles.
//! - `first_run_installs_the_toolchain_and_verifies_a_sketch` (`--ignored`, network, minutes):
//!   the one-click install downloads arduino-cli + the arduino:avr core into the scratch
//!   Application Support folder (native avr-gcc + ctags on Rosetta-less Apple Silicon), then
//!   Verify compiles blink with it.

mod common;

use breadbox::BreadboxApp;
use breadbox::router::{Command, Route};
use breadbox::toolchain::ToolchainState;
use common::*;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use std::path::PathBuf;
use std::sync::Mutex;

type H = Harness<'static, BreadboxApp>;

/// The tests change the process environment: one at a time.
static ENV: Mutex<()> = Mutex::new(());

/// A machine without a toolchain: scratch Application Support / Caches, no arduino-cli.
fn first_run_machine(name: &str) -> PathBuf {
    let scratch = breadbox::plugins::temp_dir(name);
    // SAFETY: serialized by ENV; the app reads these when it starts
    unsafe {
        std::env::set_var("BREADBOX_ARDUINO_CLI", scratch.join("no-such/arduino-cli"));
        std::env::set_var("BREADBOX_APP_SUPPORT", scratch.join("support"));
        std::env::set_var("BREADBOX_CACHES", scratch.join("caches"));
        std::env::remove_var("BREADBOX_TOOLCHAIN_DIR");
        std::env::remove_var("BREADBOX_BUILD_CACHE");
    }
    assert!(bb_firmware::Toolchain::app_dir().starts_with(&scratch));
    scratch
}

fn state(h: &H) -> ToolchainState {
    h.state().ctx.toolchain.state.clone()
}

fn open_blink_firmware(h: &mut H) {
    let blink = h
        .state()
        .ctx
        .resources
        .examples()
        .expect("examples")
        .join("blink");
    h.state_mut().open_path(&blink);
    h.run_steps(3);
    h.state_mut().ctx.navigate(Route::Firmware);
    h.run_steps(4);
    assert!(
        wait_for(h, 60.0, |h| !matches!(state(h), ToolchainState::Detecting)),
        "detection never finished"
    );
    assert!(
        matches!(state(h), ToolchainState::Missing { found: None, .. }),
        "the simulated machine has a toolchain: {}",
        h.state().ctx.toolchain.compiler_label()
    );
}

/// Clicks the enabled "Install toolchain" button (the toolchain box or the dialog).
fn click_install(h: &mut H) {
    let node = h
        .query_all_by_label("Install toolchain")
        .rfind(|n| !n.accesskit_node().is_disabled())
        .expect("an Install toolchain button");
    node.click();
    h.run_steps(3);
}

#[test]
fn first_run_offers_the_install_and_reports_a_failed_download() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = first_run_machine("first-run-offline");
    // SAFETY: serialized by ENV
    unsafe {
        std::env::set_var(
            "BREADBOX_ARDUINO_CLI_URL",
            format!(
                "file://{}",
                scratch.join("offline/arduino-cli.tar.gz").display()
            ),
        )
    };
    let mut h = harness();
    open_blink_firmware(&mut h);
    // the toolchain box offers the install
    assert!(has_label(&h, "Install toolchain"));
    // Verify without a toolchain: the output says so and the install dialog opens
    h.state_mut().ctx.command(Command::Verify);
    h.run_steps(4);
    assert!(has_label_containing(&h, "No toolchain installed"));
    assert!(has_label(&h, "Install the firmware toolchain"));
    assert!(has_label_containing(&h, "Downloads arduino-cli"));
    // the download fails (offline): the dialog says why and the button comes back
    click_install(&mut h);
    assert!(
        wait_for(&mut h, 60.0, |h| matches!(
            state(h),
            ToolchainState::Failed(_)
        )),
        "the install never failed"
    );
    h.run_steps(3);
    assert!(has_label_containing(&h, "The last install failed"));
    assert!(
        h.query_all_by_label("Install toolchain")
            .any(|n| !n.accesskit_node().is_disabled()),
        "retry is possible"
    );
    // nothing was installed outside the scratch folders
    assert!(!scratch.join("support/toolchain/bin/arduino-cli").exists());
    // SAFETY: serialized by ENV
    unsafe { std::env::remove_var("BREADBOX_ARDUINO_CLI_URL") };
}

/// A fake arduino-cli: `stuck` sleeps forever; otherwise it answers `version` and lists no
/// cores.
fn fake_cli(path: &std::path::Path, stuck: bool) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = if stuck {
        "sleep 100000"
    } else {
        "case \"$3\" in version) echo '{\"VersionString\":\"9.9.9-fake\"}' ;; *) echo '{\"platforms\":[]}' ;; esac"
    };
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_stuck_arduino_cli_shows_a_message_instead_of_detecting_forever() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = first_run_machine("stuck-cli");
    let cli = scratch.join("bin/arduino-cli");
    fake_cli(&cli, true);
    // SAFETY: serialized by ENV
    unsafe {
        std::env::set_var("BREADBOX_ARDUINO_CLI", &cli);
        std::env::set_var("BREADBOX_CLI_TIMEOUT_MS", "4500");
    }
    let t0 = std::time::Instant::now();
    let mut h = harness();
    let blink = h
        .state()
        .ctx
        .resources
        .examples()
        .expect("examples")
        .join("blink");
    h.state_mut().open_path(&blink);
    h.run_steps(3);
    h.state_mut().ctx.navigate(Route::Firmware);
    h.run_steps(4);
    assert!(matches!(state(&h), ToolchainState::Detecting));
    // a slow detection counts the wait instead of a bare "detecting…"
    assert!(
        wait_for(&mut h, 4.4, |h| has_label_containing(
            h,
            "detecting toolchain… 3 s"
        ) || has_label_containing(
            h,
            "detecting toolchain… 4 s"
        )),
        "no progress while detecting"
    );
    assert!(
        wait_for(&mut h, 20.0, |h| matches!(
            state(h),
            ToolchainState::Unresponsive(_)
        )),
        "detection never settled"
    );
    let dt = t0.elapsed();
    assert!(dt < std::time::Duration::from_secs(15), "took {dt:?}");
    h.run_steps(3);
    assert!(has_label_containing(&h, "arduino-cli: no answer"));
    assert!(has_label_containing(
        &h,
        "`arduino-cli version` did not finish within 4.5 s and was stopped"
    ));
    assert!(has_label(&h, "Retry"));
    assert!(has_label(&h, "Install toolchain"));
    screenshot(&mut h, "firmware-toolchain-unresponsive");
    // Verify explains instead of waiting
    h.state_mut().ctx.command(Command::Verify);
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains("did not finish within 4.5 s")
            .count()
            >= 2,
        "the output panel repeats why"
    );
    // the user fixes arduino-cli and retries: detection settles on the working one
    fake_cli(&cli, false);
    h.get_by_label("Retry").click();
    h.run_steps(2);
    assert!(
        wait_for(&mut h, 20.0, |h| !matches!(
            state(h),
            ToolchainState::Detecting
        )),
        "retry never settled"
    );
    match state(&h) {
        ToolchainState::Missing {
            found: Some(tc),
            reason,
        } => {
            assert_eq!(tc.cli_version, "9.9.9-fake");
            assert_eq!(reason, "the arduino:avr core is not installed");
        }
        _ => panic!("{}", h.state().ctx.toolchain.compiler_label()),
    }
    // SAFETY: serialized by ENV
    unsafe { std::env::remove_var("BREADBOX_CLI_TIMEOUT_MS") };
    drop(h);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
#[ignore = "downloads arduino-cli + the arduino:avr core (~150 MB) and builds ctags: minutes, network"]
fn first_run_installs_the_toolchain_and_verifies_a_sketch() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = first_run_machine("first-run-install");
    // SAFETY: serialized by ENV
    unsafe { std::env::remove_var("BREADBOX_ARDUINO_CLI_URL") };
    let mut h = harness();
    open_blink_firmware(&mut h);
    click_install(&mut h);
    assert!(h.state().ctx.toolchain.is_installing());
    let mut last_step = String::new();
    let ok = wait_for(&mut h, 1800.0, |h| {
        if let Some(p) = h.state().ctx.toolchain.install_progress()
            && p.step != last_step
        {
            last_step = p.step.clone();
            eprintln!("install: {last_step}");
        }
        !h.state().ctx.toolchain.is_installing()
    });
    assert!(ok, "the install did not finish");
    match state(&h) {
        ToolchainState::Ready(tc) => {
            eprintln!(
                "installed: {} · arduino-cli {} in {}",
                tc.compiler_label(),
                tc.cli_version,
                tc.arduino_cli.display()
            );
            assert!(tc.arduino_cli.starts_with(&scratch), "{:?}", tc.arduino_cli);
        }
        other => panic!(
            "not ready after the install: {}",
            match other {
                ToolchainState::Failed(e) => e,
                ToolchainState::Missing { reason, .. } => reason,
                _ => "?".into(),
            }
        ),
    }
    // Verify compiles blink with the freshly installed toolchain
    h.state_mut().ctx.command(Command::Verify);
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 600.0, |h| has_label_containing(h, "Sketch uses ")),
        "blink did not compile"
    );
    // the scratch toolchain is ~400 MB
    drop(h);
    let _ = std::fs::remove_dir_all(&scratch);
}
