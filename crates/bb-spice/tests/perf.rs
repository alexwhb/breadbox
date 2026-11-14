//! Performance tests (SPEC SPICE-5 / TEST-6). Ignored by default; run with
//! `cargo test -p bb-spice --release --test perf -- --ignored --nocapture`.

use bb_core::PinDrive;
use bb_spice::{CircuitBuilder, ModelKind, Netlist, Session};
use std::fmt::Write;
use std::time::Instant;

/// 32×32 resistor mesh (1 024 nodes) with a diode from every node to ground, driven from two
/// corners: a nonlinear `.op` that needs junction limiting.
fn mesh_netlist(k: usize) -> String {
    let mut s = String::from("1000-node resistor/diode mesh\n");
    let id = |i: usize, j: usize| format!("n{i}_{j}");
    for i in 0..k {
        for j in 0..k {
            if i + 1 < k {
                let _ = writeln!(s, "Rv{i}_{j} {} {} 100", id(i, j), id(i + 1, j));
            }
            if j + 1 < k {
                let _ = writeln!(s, "Rh{i}_{j} {} {} 100", id(i, j), id(i, j + 1));
            }
            let _ = writeln!(s, "D{i}_{j} {} 0 dm", id(i, j));
        }
    }
    let _ = writeln!(s, "V1 in1 0 DC 5\nRs1 in1 {} 10", id(0, 0));
    let _ = writeln!(s, "V2 in2 0 DC 3\nRs2 in2 {} 10", id(k - 1, k - 1));
    let _ = writeln!(s, "I1 0 {} DC 20m", id(k / 2, k / 4));
    s.push_str(".model dm D(IS=1e-14 N=1.5)\n.op\n.end\n");
    s
}

#[test]
#[ignore]
fn mesh_1000_nodes_op_under_50ms() {
    let text = mesh_netlist(32);
    let nl = Netlist::parse(&text).unwrap();
    // warm-up (allocations, page faults)
    nl.run().unwrap();
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let t0 = Instant::now();
        let r = nl.run().unwrap();
        let dt = t0.elapsed().as_secs_f64();
        best = best.min(dt);
        let v = r.op("v(n16_16)").unwrap();
        assert!(v > 0.3 && v < 1.5, "v = {v}");
    }
    eprintln!("1024-node mesh .op: {:.2} ms (best of 5)", best * 1e3);
    assert!(best < 0.050, "mesh .op took {:.1} ms", best * 1e3);
}

#[test]
#[ignore]
fn pwm_session_much_faster_than_real_time() {
    // MCU pin (25 Ω push-pull) → 220 Ω → LED, plus an RC filter on the same pin.
    let mut b = CircuitBuilder::new("pwm led + rc");
    let pin = b.port("d9", "d9", "0", PinDrive::HighZ);
    b.resistor("2", "d9", "n1", 220.0);
    b.model(
        "led_amber",
        ModelKind::D,
        &[("is", 1e-19), ("n", 1.8), ("rs", 6.0)],
    );
    b.diode("1", "n1", "0", "led_amber");
    b.resistor("f", "d9", "f", 10e3);
    b.capacitor("f", "f", "0", 1e-6);
    let nl = b.build();
    let mut s = Session::new(&nl).unwrap();
    let w = s.watch("f", 1.0, 2.0).unwrap();
    let n1 = s.node("n1").unwrap();
    let period = 1.0 / 490.0;
    let duty = 0.478;
    let quantum = 1e-6; // bb-sim's default sync quantum
    let t0 = Instant::now();
    let mut t = 0.0;
    let mut cycle = 0u64;
    let mut high = false;
    let mut next_edge = 0.0;
    let mut k = 0u64;
    let mut crossings = 0;
    while t < 1.0 {
        t = (k as f64 + 1.0) * quantum;
        k += 1;
        if t >= next_edge {
            high = !high;
            s.set_port(
                pin,
                PinDrive::Thevenin {
                    v: if high { 5.0 } else { 0.0 },
                    r: 25.0,
                },
            );
            if high {
                next_edge = (cycle as f64 + duty) * period;
            } else {
                cycle += 1;
                next_edge = cycle as f64 * period;
            }
        }
        crossings += s.advance_to(t).unwrap().len();
        let _ = s.voltage_at(n1);
    }
    let wall = t0.elapsed().as_secs_f64();
    let st = s.stats();
    eprintln!(
        "1 s of 490 Hz PWM at 1 µs quanta: {:.1} ms wall ({:.0}× real time), {} steps, {} commits, {} Newton iterations",
        wall * 1e3,
        1.0 / wall,
        st.steps,
        st.commits,
        st.newton_iterations
    );
    // the RC filter settles around duty·5 V: no watch toggling once settled
    let vf = s.voltage("f").unwrap();
    assert!((vf - duty * 5.0).abs() < 0.3, "filter output {vf}");
    assert!(crossings >= 1 && s.watch_state(w) == Some(true));
    // LED current while on ≈ (5 - Vf)/245 Ω
    assert!(wall < 0.25, "took {wall:.3} s for 1 s of simulated time");
}
