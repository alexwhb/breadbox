//! Blink: the LED on D13 toggles at 1 Hz (± 0.1 %) with real firmware.

mod common;
use bb_circuit::project::Quantity;
use bb_core::{Look, SimTime};
use common::*;

#[test]
fn blink_led_toggles_at_one_hertz() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "blink");
    b.wire((&u1, "D13"), (&bb, "a5"), "orange");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(220.0))],
    );
    let d1 = b.two_lead("led", "D1", &bb, "d9", "d10", &[]);
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    b.probe("D13", (&u1, "D13"), Quantity::Frequency, None);
    b.instrument(
        "mini_scope",
        None,
        &[("ch1", (&u1, "D13"))],
        serde_json::json!({"timebase": 0.5, "vdiv": 2.0}),
    );
    let project = b.finish();
    let mut bench = start(project, &[(&u1, "blink")]);

    // sample the LED visual every 50 ms for 6 s and find its on/off transitions
    let mut transitions = Vec::new();
    let mut last_on = false;
    for k in 1..=120 {
        bench.run_until(SimTime::from_ms(k * 50)).unwrap();
        let on = match bench.visual(&d1).unwrap().look {
            Look::Led { brightness, .. } => brightness > 0.3,
            other => panic!("{other:?}"),
        };
        if on != last_on {
            transitions.push((k * 50, on));
            last_on = on;
        }
    }
    assert!(transitions.len() >= 10, "{transitions:?}");
    // LED current ≈ (5 − 0.025·I − 1.9) / 220
    let i = bench.readings(&d1)[0].1;
    assert!(i == 0.0 || (0.010..0.016).contains(&i), "{i}");
    // exact period from the scope's edge record on D13
    let scope = &bench.telemetry().scopes[0];
    assert!(scope.frequency > 0.0);
    let edges = rising_edges(&mut bench, &u1);
    assert!(edges.len() >= 4, "{edges:?}");
    let periods: Vec<f64> = edges.windows(2).map(|w| w[1] - w[0]).collect();
    for p in &periods {
        assert!((p - 1.0).abs() < 1e-3, "period {p} s");
    }
    let lines = lines(&bench, &u1);
    assert_eq!(lines.first().map(String::as_str), Some("blink"));
    // the board's L LED follows D13 too
    let tel = bench.telemetry();
    let board = tel.parts.iter().find(|p| p.part == u1).unwrap();
    assert!(matches!(board.visual.look, Look::Board { power: true, .. }));
}

fn rising_edges(bench: &mut bb_sim::Bench, u1: &str) -> Vec<f64> {
    let _ = u1;
    let tel = bench.telemetry();
    let p = tel.probes.iter().find(|p| p.label == "D13").unwrap();
    assert!((p.value - 1.0).abs() < 0.2, "probe frequency {}", p.value);
    // reconstruct edges from the scope trace (step plot over the last 2 s window)
    let s = &tel.scopes[0];
    let mut out = Vec::new();
    for w in s.traces[0].windows(2) {
        if w[0].1 < 2.5 && w[1].1 >= 2.5 {
            out.push(w[1].0);
        }
    }
    out
}
