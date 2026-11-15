//! The lessons' end-to-end test: a scripted solver completes **every** built-in lesson through
//! the public engine API — build edits with `bb_circuit::edit::Editor`, choices and predictions
//! through the `Session`, bench runs and instrument readings (the Handheld DMM plugin) through
//! `bb_sim::Bench` — and every step's goals pass.
//!
//! `LESSON=<id>` limits a run to one lesson.

use bb_lessons::{Catalog, Runner};
use bb_plugin::PluginHost;
use std::path::PathBuf;
use std::time::Instant;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn host() -> PluginHost {
    let h = PluginHost::from_dirs(
        &[
            root().join("plugins/dmm-handheld"),
            root().join("plugins/lora-gateway"),
        ],
        None,
    );
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    h
}

fn solve_track(track: &str) {
    let cat = Catalog::builtin();
    assert!(cat.errors.is_empty(), "{:?}", cat.errors);
    let t = cat
        .track(track)
        .unwrap_or_else(|| panic!("no track {track}"));
    let only = std::env::var("LESSON").ok();
    let h = host();
    let mut failures = Vec::new();
    for l in &t.lessons {
        if only.as_ref().is_some_and(|o| *o != l.lesson.id) {
            continue;
        }
        let t0 = Instant::now();
        let r = Runner::new(l, &h).and_then(|mut r| r.solve().map(|_| r));
        match r {
            Ok(r) => {
                eprintln!(
                    "{:>28}: {} steps solved in {:.1} s",
                    l.lesson.id,
                    r.session.lesson.steps.len(),
                    t0.elapsed().as_secs_f64()
                );
                if std::env::var("VERBOSE").is_ok() {
                    for (step, checks) in &r.transcript {
                        for c in checks {
                            eprintln!("    {step}: {} → {} ({})", c.label, c.ok, c.detail);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("{:>28}: FAILED {e}", l.lesson.id);
                failures.push(format!("{}: {e}", l.lesson.id));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn solver_completes_track_electricity() {
    solve_track("electricity");
}

#[test]
fn solver_completes_track_sensors() {
    solve_track("sensors");
}

#[test]
fn solver_completes_track_microcontrollers() {
    solve_track("microcontrollers");
}

#[test]
fn solver_completes_track_radios() {
    solve_track("radios");
}

#[test]
fn solver_completes_track_spice() {
    solve_track("spice");
}

/// "Sketch compiles": every code step's starter and reference solution build with the Arduino
/// toolchain, and the freshly compiled solution (not the prebuilt image) passes the step's
/// goals on the bench. Skipped when no toolchain is installed.
#[test]
fn code_steps_compile_and_the_compiled_solution_passes() {
    if bb_firmware::Toolchain::detect().is_err() {
        eprintln!("no Arduino toolchain; skipping");
        return;
    }
    let cat = Catalog::builtin();
    let h = host();
    let lib = h.library();
    let mut n = 0;
    for l in cat.all() {
        let Some(at) = l.lesson.steps.iter().position(|s| s.code.is_some()) else {
            continue;
        };
        let code = l.lesson.steps[at].code.clone().unwrap();
        let mut r = Runner::new(l, &h).unwrap();
        // solve up to the code step
        while r.session.step < at {
            let res = r.solve_step().unwrap();
            assert!(res.iter().all(|c| c.ok), "{}: {res:?}", l.lesson.id);
            r.next_step();
        }
        let fqbn = bb_lessons::code::fqbn_for(r.session.project(), &lib, &code.mcu)
            .unwrap_or_else(|| panic!("{}: {} is not an AVR board", l.lesson.id, code.mcu));
        let starter = bb_lessons::code::compile(&code.starter, &fqbn).unwrap();
        assert!(starter.success, "{} starter:\n{}", l.lesson.id, starter.log);
        let build = bb_lessons::code::compile(&code.solution, &fqbn).unwrap();
        assert!(build.success, "{} solution:\n{}", l.lesson.id, build.log);
        r.upload(&code.mcu, build.image.as_ref().unwrap()).unwrap();
        let step = r.session.current().clone();
        r.run(step.run.max(1.0)).unwrap();
        let res = r.evaluate();
        assert!(
            res.iter().all(|c| c.ok),
            "{}: compiled solution: {res:?}",
            l.lesson.id
        );
        eprintln!(
            "{:>28}: starter + solution compile; compiled solution passes",
            l.lesson.id
        );
        n += 1;
    }
    assert_eq!(n, 4, "code steps");
}
