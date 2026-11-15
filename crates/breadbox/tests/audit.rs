//! GUI tests for spec items the audit found unreachable from the app (`docs/SPEC-AUDIT.md`):
//! the Workbench simulation menu (`SIM-1` / `SIM-3`: single steps, reset MCU only, seed, sync
//! quantum) and `.cir` import (`PRJ-4`).

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
    assert!(p.dir.join("analyses/lowpass.original.cir").exists());
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
    // the AC analysis runs → a Bode plot
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
