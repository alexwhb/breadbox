//! Co-simulation `Session` behaviour (SPEC SPICE-7): ports, ext variables, watches with
//! interpolated crossing times, snapshot/restore, re-solve at discontinuities, fast paths.

use bb_core::PinDrive;
use bb_spice::{CircuitBuilder, ModelKind, Netlist, Session, SessionOptions};

fn rc_with_port() -> (Netlist, bb_spice::PortId) {
    let mut b = CircuitBuilder::new("pwm into RC");
    let pin = b.port("d9", "pin", "0", PinDrive::HighZ);
    b.resistor("1", "pin", "out", 1e3);
    b.capacitor("1", "out", "0", 1e-6);
    (b.build(), pin)
}

const TAU: f64 = 1.025e-3; // (1k + 25 Ω) · 1 µF

#[test]
fn rc_step_response_and_crossing_time() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    let w = s.watch("out", 1.0, 2.0).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    let c = s.advance_to(2e-3).unwrap();
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].rising && c[0].watch == w);
    let expect = TAU * (5.0f64 / 3.0).ln();
    assert!(
        (c[0].time - expect).abs() < 1e-6,
        "crossing at {} expected {expect}",
        c[0].time
    );
    let v = s.voltage("out").unwrap();
    let ve = 5.0 * (1.0 - (-2e-3 / TAU).exp());
    assert!((v - ve).abs() < 2e-3 * 5.0, "{v} vs {ve}");
}

#[test]
fn small_quanta_are_cheap_and_accurate() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    // 1 µs co-simulation quantum for 2 ms
    let mut t = 0.0;
    let mut max_err = 0.0f64;
    for _ in 0..2000 {
        t += 1e-6;
        s.advance_to(t).unwrap();
        let v = s.voltage("out").unwrap();
        let ve = 5.0 * (1.0 - (-t / TAU).exp());
        max_err = max_err.max((v - ve).abs());
    }
    assert!(max_err < 0.02, "max error {max_err}");
    let st = s.stats();
    assert!(
        st.steps < 400,
        "took {} internal steps for 2000 quanta",
        st.steps
    );
}

#[test]
fn discontinuity_resolves_resistive_nodes_immediately() {
    let mut b = CircuitBuilder::new("divider + cap");
    let pin = b.port("p1", "a", "0", PinDrive::HighZ);
    b.resistor("1", "a", "mid", 1e3);
    b.resistor("2", "mid", "0", 1e3);
    b.resistor("3", "mid", "c", 1e3);
    b.capacitor("1", "c", "0", 1e-6);
    let nl = b.build();
    let mut s = Session::new(&nl).unwrap();
    s.advance_to(1e-3).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 4.0, r: 0.0 });
    s.op().unwrap();
    // capacitor still at 0 V at t⁺: mid = 4 * (1k||1k) / (1k + 1k||1k) = 4/3
    let mid = s.voltage("mid").unwrap();
    assert!((mid - 4.0 / 3.0).abs() < 1e-3, "mid = {mid}");
    assert!(s.voltage("c").unwrap().abs() < 1e-6);
    assert!((s.time() - 1e-3).abs() < 1e-15);
}

#[test]
fn change_inside_a_speculative_step_rewinds() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    s.advance_to(0.5e-3).unwrap();
    // switch off in the middle of whatever step the solver is in
    s.set_port(pin, PinDrive::Thevenin { v: 0.0, r: 25.0 });
    s.advance_to(1.5e-3).unwrap();
    let v_half = 5.0 * (1.0 - (-0.5e-3 / TAU).exp());
    let ve = v_half * (-1.0e-3 / TAU).exp();
    let v = s.voltage("out").unwrap();
    assert!((v - ve).abs() < 0.01, "{v} vs {ve}");
}

#[test]
fn pwm_watch_and_falling_edges() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    let w = s.watch("pin", 1.5, 3.0).unwrap();
    let mut edges = Vec::new();
    let period = 1.0 / 490.0;
    for k in 0..10 {
        let t0 = k as f64 * period;
        s.advance_to(t0).unwrap();
        s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
        edges.extend(s.op().unwrap());
        s.advance_to(t0 + 0.478 * period).unwrap();
        s.set_port(pin, PinDrive::Thevenin { v: 0.0, r: 25.0 });
        edges.extend(s.op().unwrap());
    }
    assert_eq!(edges.len(), 20, "{edges:?}");
    for (i, e) in edges.iter().enumerate() {
        assert_eq!(e.watch, w);
        assert_eq!(e.rising, i % 2 == 0);
        let k = (i / 2) as f64;
        let t = if i % 2 == 0 {
            k * period
        } else {
            k * period + 0.478 * period
        };
        assert!(
            (e.time - t).abs() < 1e-12,
            "edge {i} at {} expected {t}",
            e.time
        );
    }
}

#[test]
fn advance_until_crossing_stops_at_the_edge() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.watch("out", 1.0, 2.0).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    let c = s.advance_until_crossing(5e-3).unwrap();
    assert_eq!(c.len(), 1);
    let expect = TAU * (5.0f64 / 3.0).ln();
    assert!((s.time() - expect).abs() < 1e-6, "stopped at {}", s.time());
    assert!((s.voltage("out").unwrap() - 2.0).abs() < 1e-2);
    // continuing reaches the end without further crossings
    let c = s.advance_until_crossing(5e-3).unwrap();
    assert!(c.is_empty());
    assert!((s.time() - 5e-3).abs() < 1e-15);
}

#[test]
fn ext_variables_drive_expressions() {
    let mut b = CircuitBuilder::new("ldr divider");
    b.vsource_dc("1", "vcc", "0", 5.0);
    b.resistor_expr(
        "ldr",
        "vcc",
        "a1",
        "350k*pwr(max(ext(\"J2.lux\"),0.01),-0.7)",
    )
    .unwrap();
    b.resistor("1", "a1", "0", 10e3);
    let nl = b.build();
    let mut s = Session::new(&nl).unwrap();
    let v_at = |lux: f64| {
        let r = 350e3 * lux.powf(-0.7);
        5.0 * 10e3 / (10e3 + r)
    };
    s.set_ext("J2.lux", 180.0);
    s.advance_to(1e-3).unwrap();
    assert!((s.voltage("a1").unwrap() - v_at(180.0)).abs() < 1e-9);
    s.set_ext("J2.lux", 5.0);
    s.advance_to(2e-3).unwrap();
    assert!((s.voltage("a1").unwrap() - v_at(5.0)).abs() < 1e-9);
    // purely resistive: nothing was stepped
    assert_eq!(s.stats().steps, 0);
    assert!(s.stats().free_advances >= 2);
    // unknown ext names are accepted and ignored
    s.set_ext("nobody.uses.this", 1.0);
    s.advance_to(3e-3).unwrap();
}

#[test]
fn resistive_circuit_costs_nothing_per_advance() {
    let mut b = CircuitBuilder::new("static");
    let p = b.port("a", "a", "0", PinDrive::Thevenin { v: 3.3, r: 25.0 });
    b.resistor("1", "a", "b", 1e3);
    b.resistor("2", "b", "0", 2e3);
    b.model(
        "led",
        ModelKind::D,
        &[("is", 1e-19), ("n", 1.8), ("rs", 6.0)],
    );
    b.diode("1", "b", "0", "led");
    let nl = b.build();
    let mut s = Session::new(&nl).unwrap();
    for k in 1..=100_000 {
        s.advance_to(k as f64 * 1e-6).unwrap();
    }
    let st = s.stats();
    assert_eq!(st.steps, 0);
    let n0 = st.newton_iterations;
    s.set_port(p, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    s.advance_to(0.2).unwrap();
    assert!(s.stats().newton_iterations > n0);
    assert!(s.voltage("b").unwrap() > 1.5);
}

#[test]
fn settled_reactive_circuit_stops_stepping() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    s.advance_to(0.05).unwrap();
    let steps = s.stats().steps;
    s.advance_to(10.0).unwrap();
    assert!(
        s.stats().steps - steps < 50,
        "kept stepping: {}",
        s.stats().steps - steps
    );
    assert!((s.voltage("out").unwrap() - 5.0).abs() < 1e-6);
}

#[test]
fn snapshot_and_restore_replay_identically() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.watch("out", 1.0, 2.0).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    s.advance_to(0.3e-3).unwrap();
    let snap = s.snapshot();
    let c1 = s.advance_to(3e-3).unwrap();
    let v1 = s.voltage("out").unwrap();
    s.restore(&snap).unwrap();
    assert_eq!(s.time(), 0.3e-3);
    let c2 = s.advance_to(3e-3).unwrap();
    let v2 = s.voltage("out").unwrap();
    assert_eq!(c1, c2);
    assert_eq!(v1, v2);
    // going back in time without a snapshot is an error
    assert!(s.advance_to(1e-3).is_err());
}

#[test]
fn ports_added_after_start_keep_state() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    s.advance_to(1e-3).unwrap();
    let v_before = s.voltage("out").unwrap();
    let extra = s.add_port("probe", "out", "0").unwrap();
    assert_eq!(s.port_drive(extra), Some(PinDrive::HighZ));
    s.op().unwrap();
    let v_after = s.voltage("out").unwrap();
    assert!((v_before - v_after).abs() < 1e-3, "{v_before} vs {v_after}");
    // pull the node down through the new port
    s.set_port(extra, PinDrive::Pull { v: 0.0, r: 10.0 });
    s.advance_to(2e-3).unwrap();
    assert!(s.voltage("out").unwrap() < 0.1);
    assert!(s.port_current(extra).unwrap() < 0.0);
}

#[test]
fn hi_z_port_reads_node_and_current_port_injects() {
    let mut b = CircuitBuilder::new("pullup");
    b.vsource_dc("1", "vcc", "0", 5.0);
    b.resistor("pu", "vcc", "in", 10e3);
    let p = b.port("in", "in", "0", PinDrive::HighZ);
    let nl = b.build();
    let mut s = Session::new(&nl).unwrap();
    s.advance_to(1e-6).unwrap();
    assert!((s.voltage("in").unwrap() - 5.0).abs() < 1e-6);
    s.set_port(p, PinDrive::Current { i: -0.25e-3 });
    s.advance_to(2e-6).unwrap();
    assert!((s.voltage("in").unwrap() - 2.5).abs() < 1e-6);
    assert!((s.current("rpu").unwrap() - 0.25e-3).abs() < 1e-9);
}

#[test]
fn set_param_and_export() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    s.advance_to(1e-3).unwrap();
    s.set_param("r1", "r", 100.0).unwrap();
    s.advance_to(3e-3).unwrap();
    assert!(s.voltage("out").unwrap() > 4.9);
    assert!(s.set_param("r1", "bogus", 1.0).is_err());
    assert!(s.set_param("nope", "r", 1.0).is_err());
    let text = s.to_spice_text();
    assert!(text.contains("breadbox port pd9"), "{text}");
    assert!(text.contains("DC 5"), "{text}");
    // the exported netlist parses and runs in bb-spice too
    let nl2 = Netlist::parse(&(text.replace(".end\n", ".op\n.end\n"))).unwrap();
    let r = nl2.run().unwrap();
    assert!((r.op("v(out)").unwrap() - 5.0).abs() < 1e-6);
}

#[test]
fn time_dependent_sources_and_tline_in_a_session() {
    let nl = Netlist::parse(
        "t\nV1 in 0 SIN(0 1 1k)\nR1 in a 50\nT1 a 0 b 0 Z0=50 TD=10u\nR2 b 0 50\n.end\n",
    )
    .unwrap();
    let mut s = Session::with_options(
        &nl,
        SessionOptions {
            max_step: 2e-6,
            ..Default::default()
        },
    )
    .unwrap();
    let w = s.watch("b", -0.1, 0.1).unwrap();
    let mut crossings = Vec::new();
    let mut t = 0.0;
    while t < 2e-3 {
        t += 10e-6;
        crossings.extend(s.advance_to(t).unwrap());
    }
    // matched line: b is a/1 delayed by 10 µs, amplitude 0.5
    let tb = 2e-3;
    let expect = 0.5 * (2.0 * std::f64::consts::PI * 1e3 * (tb - 10e-6)).sin();
    assert!(
        (s.voltage("b").unwrap() - expect).abs() < 5e-3,
        "{} vs {expect}",
        s.voltage("b").unwrap()
    );
    assert!(crossings.iter().all(|c| c.watch == w));
    assert!(crossings.len() >= 3);
    let first_rise = crossings.iter().find(|c| c.rising).unwrap().time;
    // 0.5 sin(2π f (t - td)) = 0.1  → t = td + asin(0.2)/(2π f)
    let expect = 10e-6 + (0.2f64).asin() / (2.0 * std::f64::consts::PI * 1e3);
    assert!(
        (first_rise - expect).abs() < 1e-6,
        "{first_rise} vs {expect}"
    );
}

#[test]
fn ext_in_model_parameters_and_temperature() {
    let nl = Netlist::parse(
        "t\nI1 0 a DC 1m\nD1 a 0 dx\n.model dx D(IS={1e-15*ext(\"scale\")} N=1)\n.end\n",
    )
    .unwrap();
    let mut s = Session::new(&nl).unwrap();
    s.set_ext("scale", 1.0);
    s.advance_to(1e-6).unwrap();
    let v1 = s.voltage("a").unwrap();
    s.set_ext("scale", 10.0);
    s.advance_to(2e-6).unwrap();
    let v2 = s.voltage("a").unwrap();
    // ten times the saturation current: V drops by Vt·ln(10)
    let vt = 1.380649e-23 * 300.15 / 1.602176634e-19;
    assert!(((v1 - v2) - vt * 10f64.ln()).abs() < 1e-4, "{v1} {v2}");
    s.set_temperature(77.0);
    s.advance_to(3e-6).unwrap();
    assert!(s.voltage("a").unwrap() < v2);
}

#[test]
fn engine_types_are_send() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<Session>();
    assert_send::<Netlist>();
    assert_send::<bb_spice::Results>();
    assert_send::<bb_spice::SessionSnapshot>();
}

/// A watch added while speculative trail points exist (an instrument re-arming its trigger
/// between small quanta) must not index past the trail's stored values.
#[test]
fn watches_added_mid_trail_are_safe() {
    let (nl, pin) = rc_with_port();
    let mut s = Session::new(&nl).unwrap();
    s.watch("out", 1.0, 2.0).unwrap();
    s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
    let mut t = 0.0;
    let mut crossings = 0;
    for k in 0..400 {
        t += 5e-6;
        crossings += s.advance_to(t).unwrap().len();
        if k % 7 == 0 {
            let w = s.watch("out", 2.4, 2.6).unwrap();
            if k % 14 == 0 {
                s.unwatch(w);
            }
        }
    }
    assert!(crossings >= 1);
    assert!(s.voltage("out").unwrap() > 0.5);
}
