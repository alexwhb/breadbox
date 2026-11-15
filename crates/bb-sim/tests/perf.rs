//! Performance (TEST-6, SIM-4): run with
//!
//!     cargo test -p bb-sim --release --test perf -- --ignored --nocapture

mod common;
use bb_circuit::project::Project;
use bb_core::SimTime;
use bb_sim::{Bench, BenchOptions};
use common::*;
use std::time::Instant;

fn factor(mut b: Bench, secs: u64, what: &str) -> f64 {
    // warm-up second (boot, Wi-Fi join) not counted
    b.run_until(SimTime::from_secs(1)).unwrap();
    let t0 = Instant::now();
    b.run_until(SimTime::from_secs(1 + secs)).unwrap();
    let wall = t0.elapsed().as_secs_f64();
    let f = secs as f64 / wall;
    let s = b.stats;
    println!(
        "{what}: {secs} s simulated in {wall:.3} s → {f:.1}× real time \
         (sync points {}, MCU stops {}, ADC inline {}/stops {}, edges {}, device calls {})",
        s.sync_points, s.mcu_stops, s.adc_answered_inline, s.adc_stops, s.edges, s.device_calls
    );
    f
}

#[test]
#[ignore]
fn garden_node_runs_faster_than_real_time() {
    let dir = root().join("examples/garden-node");
    let project = Project::load(&dir).unwrap();
    let opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(&project, &dir),
        ..Default::default()
    };
    let mut b = Bench::new(project, lib(), opts).unwrap();
    // keep the telemetry path busy too, like the GUI does at 60 Hz
    b.run_until(SimTime::from_ms(10)).unwrap();
    let f = factor(b, 20, "garden-node");
    assert!(
        f >= 1.0,
        "SIM-4: garden-node must run at ≥ 1× real time, got {f:.2}×"
    );
}

#[test]
#[ignore]
fn garden_node_with_telemetry_at_60_hz() {
    let dir = root().join("examples/garden-node");
    let project = Project::load(&dir).unwrap();
    let opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(&project, &dir),
        ..Default::default()
    };
    let mut b = Bench::new(project, lib(), opts).unwrap();
    b.run_until(SimTime::from_secs(1)).unwrap();
    let t0 = Instant::now();
    let frames = 600; // 10 s of simulated time at 60 telemetry frames per simulated second
    for k in 1..=frames {
        b.run_until(SimTime::from_secs(1) + SimTime::from_secs_f64(k as f64 / 60.0))
            .unwrap();
        let _ = b.telemetry();
    }
    let wall = t0.elapsed().as_secs_f64();
    let f = 10.0 / wall;
    println!("garden-node + 60 Hz telemetry: 10 s in {wall:.3} s → {f:.1}× real time");
    assert!(f >= 1.0);
}

#[test]
#[ignore]
fn reactive_pwm_rc_bench() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "pwm-rc");
    b.wire((&u1, "D9"), (&bb, "a5"), "orange");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c10",
        &[("resistance", n(10e3))],
    );
    b.two_lead(
        "capacitor",
        "C1",
        &bb,
        "d10",
        "t-9",
        &[("capacitance", n(10e-6))],
    );
    b.wire((&bb, "a10"), (&u1, "A0"), "yellow");
    let bench = start(b.finish(), &[(&u1, "pwm_rc")]);
    let f = factor(bench, 5, "PWM → RC + ADC");
    assert!(f >= 1.0);
}
