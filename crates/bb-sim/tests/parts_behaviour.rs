//! Parts the library ships that only had the generic every-part `.op` smoke test: their
//! electrical behaviour on a bench (PART-3, PART-4, PART-5).

mod common;
use bb_circuit::PropValue;
use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::Mode;
use bb_core::SimTime;
use bb_sim::Bench;
use common::*;

/// A USB supply (PS1: VBUS / GND) and nothing else.
fn supply<'a>(lib: &'a bb_circuit::Library, name: &str) -> (BenchBuilder<'a>, String) {
    let mut b = BenchBuilder::new(lib, name, Mode::Playground);
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    (b, ps)
}

fn settle(b: bb_circuit::Project) -> Bench {
    let mut bench = start(b, &[]);
    bench.run_until(SimTime::from_ms(50)).unwrap();
    bench
}

fn v(b: &Bench, part: &str, pin: &str) -> f64 {
    b.pin_voltage(part, pin).unwrap_or(f64::NAN)
}

/// PART-3: every LED colour has its own forward voltage — red lowest, blue / white highest —
/// so the same 220 Ω gives each a different current.
#[test]
fn led_colours_have_their_own_forward_voltage() {
    let lib = lib();
    let (mut b, ps) = supply(&lib, "led-colours");
    let colours = ["red", "amber", "yellow", "green", "blue", "white"];
    let mut ids = Vec::new();
    for (k, c) in colours.iter().enumerate() {
        let x = 20.0 + 10.0 * k as f64;
        let r = b.offboard(
            "resistor",
            &format!("R{}", k + 1),
            x,
            0.0,
            &[("resistance", PropValue::Num(220.0))],
        );
        let d = b.offboard(
            "led",
            &format!("D{}", k + 1),
            x,
            8.0,
            &[("color", PropValue::Text((*c).into()))],
        );
        b.wire((&ps, "VBUS"), (&r, "1"), "red");
        b.wire((&r, "2"), (&d, "a"), "orange");
        b.wire((&d, "k"), (&ps, "GND"), "black");
        ids.push(d);
    }
    let bench = settle(b.finish());
    let vf: Vec<f64> = ids
        .iter()
        .map(|d| v(&bench, d, "a") - v(&bench, d, "k"))
        .collect();
    // red < amber ≤ yellow < green < blue, white ≈ blue
    assert!(
        vf[0] < vf[1] && vf[1] <= vf[2] + 0.05 && vf[2] < vf[3] && vf[3] < vf[4],
        "{vf:?}"
    );
    assert!((1.6..2.2).contains(&vf[0]), "red Vf {vf:?}");
    assert!((2.6..3.5).contains(&vf[4]), "blue Vf {vf:?}");
    assert!((vf[5] - vf[4]).abs() < 0.3, "white ≈ blue {vf:?}");
}

/// PART-5: a 1N4733A Zener clamps at ≈ 5.1 V; a 1N5819 Schottky drops less than a 1N4148.
#[test]
fn zener_clamps_and_schottky_drops_less_than_silicon() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "diodes", Mode::Playground);
    let bat = b.offboard(
        "battery_pack",
        "BT1",
        0.0,
        0.0,
        &[("kind", PropValue::Text("9v".into()))],
    );
    let r = b.offboard(
        "resistor",
        "R1",
        20.0,
        0.0,
        &[("resistance", PropValue::Num(1000.0))],
    );
    let z = b.offboard(
        "diode",
        "D1",
        30.0,
        0.0,
        &[("model", PropValue::Text("1N4733A".into()))],
    );
    b.wire((&bat, "plus"), (&r, "1"), "red");
    b.wire((&r, "2"), (&z, "k"), "orange");
    b.wire((&z, "a"), (&bat, "minus"), "black");
    // the same 1 kΩ + forward diode for the two small-signal types
    let mut fwd = Vec::new();
    for (k, m) in ["1N4148", "1N5819"].iter().enumerate() {
        let x = 40.0 + 10.0 * k as f64;
        let rr = b.offboard(
            "resistor",
            &format!("R{}", k + 2),
            x,
            0.0,
            &[("resistance", PropValue::Num(1000.0))],
        );
        let d = b.offboard(
            "diode",
            &format!("D{}", k + 2),
            x,
            8.0,
            &[("model", PropValue::Text((*m).into()))],
        );
        b.wire((&bat, "plus"), (&rr, "1"), "red");
        b.wire((&rr, "2"), (&d, "a"), "orange");
        b.wire((&d, "k"), (&bat, "minus"), "black");
        fwd.push(d);
    }
    let bench = settle(b.finish());
    let vz = v(&bench, &z, "k") - v(&bench, &z, "a");
    assert!((4.8..5.4).contains(&vz), "zener {vz} V");
    let si = v(&bench, &fwd[0], "a") - v(&bench, &fwd[0], "k");
    let sch = v(&bench, &fwd[1], "a") - v(&bench, &fwd[1], "k");
    assert!((0.55..0.8).contains(&si), "1N4148 {si} V");
    assert!(sch < si - 0.15, "Schottky {sch} V < silicon {si} V");
}

/// PART-5: a 2N7000 / IRLZ44N switches an LED with its gate; an LM358 / TL072 / ideal op-amp
/// as a voltage follower copies a pot divider's 2.5 V.
#[test]
fn mosfets_switch_and_op_amps_follow() {
    let lib = lib();
    for (model, gate_hi) in [("2N7000", true), ("2N7000", false), ("IRLZ44N", true)] {
        let (mut b, ps) = supply(&lib, "mosfet");
        let r = b.offboard(
            "resistor",
            "R1",
            20.0,
            0.0,
            &[("resistance", PropValue::Num(220.0))],
        );
        let d = b.offboard("led", "D1", 30.0, 0.0, &[]);
        let q = b.offboard(
            "nmos",
            "Q1",
            40.0,
            0.0,
            &[("model", PropValue::Text(model.into()))],
        );
        b.wire((&ps, "VBUS"), (&r, "1"), "red");
        b.wire((&r, "2"), (&d, "a"), "orange");
        b.wire((&d, "k"), (&q, "D"), "orange");
        b.wire((&q, "S"), (&ps, "GND"), "black");
        b.wire(
            (&q, "G"),
            (&ps, if gate_hi { "VBUS" } else { "GND" }),
            "yellow",
        );
        let bench = settle(b.finish());
        let i = bench.element_current(&d, "D").unwrap_or(0.0).abs();
        if gate_hi {
            assert!(i > 8e-3, "{model} on: LED {i} A");
        } else {
            assert!(i < 1e-5, "{model} off: LED {i} A");
        }
    }
    for amp in ["opamp_lm358", "opamp_tl072", "opamp_ideal"] {
        let (mut b, ps) = supply(&lib, "follower");
        let pot = b.offboard("potentiometer", "RV1", 20.0, 0.0, &[]);
        let u = b.offboard(amp, "U1", 30.0, 0.0, &[]);
        b.wire((&ps, "VBUS"), (&pot, "1"), "red");
        b.wire((&pot, "3"), (&ps, "GND"), "black");
        let out = if amp == "opamp_ideal" {
            b.wire((&pot, "w"), (&u, "+"), "yellow");
            b.wire((&u, "out"), (&u, "-"), "green");
            "out"
        } else {
            b.wire((&ps, "VBUS"), (&u, "VCC"), "red");
            b.wire((&u, "VEE"), (&ps, "GND"), "black");
            b.wire((&pot, "w"), (&u, "IN1+"), "yellow");
            b.wire((&u, "OUT1"), (&u, "IN1-"), "green");
            "OUT1"
        };
        let bench = settle(b.finish());
        let vin = v(&bench, &pot, "w");
        let vout = v(&bench, &u, out);
        assert!((vin - 2.5).abs() < 0.05, "{amp} divider {vin}");
        assert!((vout - vin).abs() < 0.05, "{amp} follower {vin} → {vout}");
    }
}

/// PART-5: 74HC04 / 74HC08 / 74HC32 gates give their truth tables on a 5 V supply.
#[test]
fn logic_gates_truth_tables() {
    let lib = lib();
    for (part, two_inputs) in [("hc04", false), ("hc08", true), ("hc32", true)] {
        for (a, bb) in [(false, false), (false, true), (true, false), (true, true)] {
            if !two_inputs && bb {
                continue;
            }
            let (mut b, ps) = supply(&lib, "gates");
            let u = b.offboard(part, "U1", 20.0, 0.0, &[]);
            b.wire((&ps, "VBUS"), (&u, "VCC"), "red");
            b.wire((&u, "GND"), (&ps, "GND"), "black");
            let lvl = |x: bool| if x { "VBUS" } else { "GND" };
            b.wire((&ps, lvl(a)), (&u, "1A"), "yellow");
            if two_inputs {
                b.wire((&ps, lvl(bb)), (&u, "1B"), "yellow");
            }
            let bench = settle(b.finish());
            let y = v(&bench, &u, "1Y") > 2.5;
            let want = match part {
                "hc04" => !a,
                "hc08" => a && bb,
                _ => a || bb,
            };
            assert_eq!(y, want, "{part} {a} {bb}");
        }
    }
}

/// PART-4: a 10 kΩ NTC (β 3950) reads 10 kΩ at 25 °C and less when the room warms.
#[test]
fn thermistor_follows_the_air_temperature() {
    let lib = lib();
    let (mut b, ps) = supply(&lib, "ntc");
    let r = b.offboard(
        "resistor",
        "R1",
        20.0,
        0.0,
        &[("resistance", PropValue::Num(10_000.0))],
    );
    let t = b.offboard(
        "thermistor",
        "TH1",
        30.0,
        0.0,
        &[("tau", PropValue::Num(0.01))],
    );
    b.wire((&ps, "VBUS"), (&r, "1"), "red");
    b.wire((&r, "2"), (&t, "1"), "yellow");
    b.wire((&t, "2"), (&ps, "GND"), "black");
    let mut p = b.finish();
    p.world.air.temp_c = 25.0;
    p.world.noise = false;
    let mut bench = start(p, &[]);
    bench.run_until(SimTime::from_ms(500)).unwrap();
    let vmid = v(&bench, &t, "1");
    let vbus = v(&bench, &r, "1");
    let r_ntc = 10_000.0 * vmid / (vbus - vmid);
    assert!((r_ntc - 10_000.0).abs() < 300.0, "R(25 °C) {r_ntc}");
    bench.set_world("air.temp_c", 40.0);
    bench.run_until(SimTime::from_ms(1500)).unwrap();
    let vmid = v(&bench, &t, "1");
    let r40 = 10_000.0 * vmid / (vbus - vmid);
    // β model: R = R25·exp(β(1/T − 1/T25)) ≈ 5.3 kΩ at 40 °C
    assert!((r40 - 5_300.0).abs() < 400.0, "R(40 °C) {r40}");
}
