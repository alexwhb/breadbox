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
