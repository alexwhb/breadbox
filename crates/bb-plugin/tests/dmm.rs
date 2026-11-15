//! Handheld DMM plugin end to end (PLUG-5, TEST-4 "plugin DMM loading the node"): the
//! compiled `plugins/dmm-handheld/dmm.wasm` on the real garden-node bench and on a bench of
//! resistors, through the public engine API.

mod common;
use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::{Endpoint, Mode, Wire};
use bb_circuit::{Project, PropValue};
use bb_core::{SimTime, UserInput};
use bb_plugin::PluginHost;
use bb_plugin::host::loading_callout;
use bb_sim::Bench;
use common::*;

fn host() -> PluginHost {
    let h = PluginHost::from_dirs(&[root().join("plugins/dmm-handheld")], None);
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    h
}

fn wire(p: &mut Project, id: &str, a: (&str, &str), b: (&str, &str)) {
    p.wires.retain(|w| w.id != id);
    p.wires.push(Wire {
        id: id.into(),
        a: Endpoint::pin(a.0, a.1),
        b: Endpoint::pin(b.0, b.1),
        color: "red".into(),
        bends: vec![],
    });
}

/// garden-node with the bench meter replaced by the plugin DMM clipped to J1 out / GND rail.
fn garden(h: &PluginHost, with_meter: bool) -> Bench {
    let dir = root().join("examples/garden-node");
    let mut p = Project::load(&dir).unwrap();
    p.world.noise = false;
    p.instruments.retain(|i| i.kind != "dmm");
    if with_meter {
        p.part_mut("dmm1").unwrap().part = "dmm_handheld".into();
        wire(&mut p, "wdmm1", ("dmm1", "VO"), ("j1", "AOUT"));
        wire(&mut p, "wdmm2", ("dmm1", "COM"), ("bb1", "t-1"));
    } else {
        p.parts.retain(|x| x.id != "dmm1");
    }
    let mut opts = h.bench_options();
    opts.firmware = bb_sim::firmware::project_images(&p, &dir);
    Bench::new(p, h.library(), opts).unwrap()
}

fn reading(b: &Bench, part: &str, key: &str) -> f64 {
    b.readings(part)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|x| x.1)
        .unwrap_or(f64::NAN)
}

fn display(b: &mut Bench, part: &str) -> (String, String, Vec<String>) {
    let tel = b.telemetry();
    let p = tel.parts.iter().find(|p| p.part == part).unwrap();
    let panel = p.panel.as_ref().expect("panel state");
    let lcd = &panel["displays"]["lcd"];
    (
        lcd["text"].as_str().unwrap_or("").to_string(),
        lcd["unit"].as_str().unwrap_or("").to_string(),
        lcd["annunciators"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn probe_avg(b: &mut Bench, label: &str) -> f64 {
    b.telemetry()
        .probes
        .iter()
        .find(|p| p.label == label)
        .unwrap()
        .avg
}

#[test]
fn dmm_on_garden_node_a0_reads_the_loaded_node_and_shows_the_design_callout() {
    let h = host();
    let mut bare = garden(&h, false);
    bare.run_until(SimTime::from_secs(3)).unwrap();
    let mut b = garden(&h, true);
    b.run_until(SimTime::from_secs(3)).unwrap();
    // the design callout, computed from the operating point with and without the meter
    let c = loading_callout(&mut b, "dmm1").expect("callout");
    assert_eq!(
        c.text,
        "Probing A0 (J1 out): meter load shifts the node by −0.4 mV — shown as it would be on a real bench."
    );
    assert!(
        (c.shift * 1e3 + 0.41).abs() < 0.05,
        "shift {} mV",
        c.shift * 1e3
    );
    // the node really moved by that much compared with the bench without a meter
    let a0_bare = bare.pin_voltage("j1", "AOUT").unwrap();
    let a0 = b.pin_voltage("j1", "AOUT").unwrap();
    assert!(((a0 - a0_bare) - c.shift).abs() < 2e-5, "{a0} vs {a0_bare}");
    // the meter reads the loaded node within its accuracy: ±(0.05 % + 1 count) (+1 count of
    // flicker)
    let v = reading(&b, "dmm1", "value");
    let tol = a0 * 0.0005 + 0.002;
    assert!((v - a0).abs() <= tol, "DMM {v} vs node {a0} (±{tol})");
    let model = 3.03 - 0.02737 * 42.0;
    assert!((v - model).abs() < 0.01, "DMM {v} vs sensor model {model}");
    let (text, unit, ann) = display(&mut b, "dmm1");
    assert_eq!(unit, "V");
    assert!(text.starts_with("1.8") && text.len() == 5, "{text}");
    assert!(
        ann.contains(&"AUTO".to_string()) && ann.contains(&"DC".to_string()),
        "{ann:?}"
    );
    // the probe label agrees with the meter
    let probe = probe_avg(&mut b, "Soil sensor → A0");
    assert!((probe - v).abs() <= tol, "probe {probe} vs DMM {v}");
}

#[test]
fn dmm_autoranges_with_a_350_ms_settling_delay_and_holds() {
    let h = host();
    let mut b = garden(&h, true);
    b.run_until(SimTime::from_secs(2)).unwrap();
    assert_eq!(reading(&b, "dmm1", "range_full"), 6.0);
    // move the red probe to ground: the meter ranges down to 600 mV
    let mut p = b.project().clone();
    wire(&mut p, "wdmm1", ("dmm1", "VO"), ("bb1", "t-2"));
    let t_move = b.time();
    b.update_project(p.clone()).unwrap();
    let mut first_valid = None;
    let mut saw_settling = false;
    while b.time() < t_move + SimTime::from_ms(1500) {
        b.run_for(SimTime::from_ms(10)).unwrap();
        if reading(&b, "dmm1", "settling") == 1.0 {
            saw_settling = true;
        }
        if reading(&b, "dmm1", "range_full") == 0.6 && reading(&b, "dmm1", "settling") == 0.0 {
            first_valid = Some(b.time());
            break;
        }
    }
    assert!(saw_settling, "the display blanks while ranging");
    let dt = (first_valid.expect("settled on 600 mV") - t_move).as_secs_f64();
    assert!(
        (0.35..0.75).contains(&dt),
        "range change + 350 ms settle took {dt} s"
    );
    let (text, unit, _) = display(&mut b, "dmm1");
    assert_eq!(unit, "mV");
    assert!(text.parse::<f64>().unwrap().abs() < 0.5, "{text}");
    // HOLD freezes the display while the probe moves back to 1.88 V
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "hold".into(),
            value: 1.0,
        },
    )
    .unwrap();
    let held = display(&mut b, "dmm1").0;
    wire(&mut p, "wdmm1", ("dmm1", "VO"), ("j1", "AOUT"));
    b.update_project(p).unwrap();
    b.run_for(SimTime::from_secs(1)).unwrap();
    let (text, _, ann) = display(&mut b, "dmm1");
    assert_eq!(text, held);
    assert!(ann.contains(&"HOLD".to_string()));
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "hold".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_secs(1)).unwrap();
    let (text, unit, _) = display(&mut b, "dmm1");
    assert_eq!(unit, "V");
    assert!(text.starts_with("1.8"), "{text}");
    // RANGE: manual ranges, MANU annunciator
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "range".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "range".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_secs(1)).unwrap();
    let (text, _, ann) = display(&mut b, "dmm1");
    assert!(ann.contains(&"MANU".to_string()), "{ann:?}");
    assert_eq!(reading(&b, "dmm1", "range_full"), 60.0);
    assert!(
        text.starts_with("1.8") && text.len() == 4,
        "60 V range shows 2 decimals: {text}"
    );
}

#[test]
fn dmm_ac_true_rms_and_frequency_on_the_night_led_pwm() {
    let h = host();
    let mut b = garden(&h, true);
    let mut p = b.project().clone();
    wire(&mut p, "wdmm1", ("dmm1", "VO"), ("u1", "D9"));
    b.update_project(p).unwrap();
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "dial".into(),
            value: 1.0,
        },
    )
    .unwrap(); // V~
    b.run_until(SimTime::from_secs(3)).unwrap();
    let vac = reading(&b, "dmm1", "value");
    // 490 Hz PWM between 0 and the pin's sagging high level: AC-coupled true RMS =
    // V·√(d(1−d)), checked against the mini scope's view of the same node
    let tel = b.telemetry();
    let s = &tel.scopes[0];
    assert!((s.frequency - 490.2).abs() < 2.0, "scope {}", s.frequency);
    let expect = s.vpp * (s.duty * (1.0 - s.duty)).sqrt();
    assert!((vac - expect).abs() < expect * 0.03, "V~ {vac} vs {expect}");
    assert!(display(&mut b, "dmm1").2.contains(&"AC".to_string()));
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "dial".into(),
            value: 9.0,
        },
    )
    .unwrap(); // Hz
    b.run_for(SimTime::from_secs(2)).unwrap();
    let f = reading(&b, "dmm1", "value");
    assert!((f - 490.2).abs() < 1.0, "Hz {f}");
}

/// UNO 5V → R1 → (A jack | mA jack) → COM → GND, and a resistor on its own for Ω.
fn resistor_bench(h: &PluginHost, series_jack: &str, dial: &str, r_ohms: f64) -> Bench {
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "dmm-series", Mode::Playground);
    let u1 = bb.offboard("uno", "U1", 0.0, 0.0, &[]);
    let r1 = bb.offboard(
        "resistor",
        "R1",
        20.0,
        0.0,
        &[("resistance", PropValue::Num(r_ohms))],
    );
    let m = bb.offboard(
        "dmm_handheld",
        "DMM1",
        30.0,
        0.0,
        &[("mode", PropValue::Text(dial.into()))],
    );
    let rx = bb.offboard(
        "resistor",
        "R2",
        40.0,
        0.0,
        &[("resistance", PropValue::Num(4700.0))],
    );
    if series_jack.is_empty() {
        // Ω: the meter across R2 alone
        bb.wire((&m, "VO"), (&rx, "1"), "red");
        bb.wire((&m, "COM"), (&rx, "2"), "black");
    } else {
        bb.wire((&u1, "5V"), (&r1, "1"), "red");
        bb.wire((&r1, "2"), (&m, series_jack), "red");
        bb.wire((&m, "COM"), (&u1, "GND"), "black");
    }
    let p = bb.finish();
    Bench::new(p, lib, h.bench_options()).unwrap()
}

#[test]
fn dmm_ohms_continuity_and_series_current() {
    let h = host();
    // Ω with its test current
    let mut b = resistor_bench(&h, "", "ohm", 1000.0);
    b.run_until(SimTime::from_secs(2)).unwrap();
    let r = reading(&b, "dmm1", "value");
    assert!((r - 4700.0).abs() < 4700.0 * 0.002 + 2.0, "Ω {r}");
    let (text, unit, _) = display(&mut b, "dmm1");
    assert_eq!(unit, "kΩ");
    let shown: f64 = text.parse().unwrap();
    assert!((shown - 4.700).abs() <= 0.0035, "{text}");
    let tel = b.telemetry();
    let panel = tel
        .parts
        .iter()
        .find(|p| p.part == "dmm1")
        .unwrap()
        .panel
        .clone()
        .unwrap();
    let readouts = panel["readouts"].to_string();
    assert!(readouts.contains("Test current"), "{readouts}");
    // continuity: beeps on a short, not on 4.7 kΩ
    b.input(
        "dmm1",
        &UserInput::Value {
            key: "dial".into(),
            value: 5.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_secs(1)).unwrap();
    assert_eq!(reading(&b, "dmm1", "beep"), 0.0);
    let ev = b.take_events();
    assert!(
        !ev.iter()
            .any(|e| matches!(e.event, bb_core::DeviceEvent::Sound(_)))
    );
    let mut p = b.project().clone();
    wire(&mut p, "short", ("r2", "1"), ("r2", "2"));
    b.update_project(p).unwrap();
    b.run_for(SimTime::from_secs(1)).unwrap();
    assert_eq!(reading(&b, "dmm1", "beep"), 1.0);
    assert!(b.take_events().iter().any(|e| matches!(
        e.event,
        bb_core::DeviceEvent::Sound(bb_core::Sound::Beep { .. })
    )));
    // current: 5 V through 330 Ω in series with the mA jack (1.8 Ω shunt)
    let mut b = resistor_bench(&h, "MA", "ma", 330.0);
    b.run_until(SimTime::from_secs(2)).unwrap();
    let i = reading(&b, "dmm1", "value");
    let expect = b.pin_voltage("u1", "5V").unwrap() / (330.0 + 1.8);
    assert!(
        (i - expect).abs() < expect * 0.003 + 2e-5,
        "mA {i} vs {expect}"
    );
    assert_eq!(display(&mut b, "dmm1").1, "mA");
    // the same current through the 10 A jack: 0.03 Ω burden, 6 A range shows 3 decimals
    let mut b = resistor_bench(&h, "A", "a", 330.0);
    b.run_until(SimTime::from_secs(2)).unwrap();
    let (text, unit, _) = display(&mut b, "dmm1");
    assert_eq!((text.as_str(), unit.as_str()), ("0.015", "A"));
    // A mode with the leads in VΩ/COM reads nothing: current needs series wiring
    let mut b = resistor_bench(&h, "VO", "a", 330.0);
    b.run_until(SimTime::from_secs(2)).unwrap();
    assert!(reading(&b, "dmm1", "value").abs() < 1e-3);
    // a short through the mA jack blows its fuse
    let mut b = resistor_bench(&h, "MA", "ma", 0.1);
    b.run_until(SimTime::from_secs(1)).unwrap();
    assert_eq!(reading(&b, "dmm1", "fuse_ma"), 1.0);
    assert_eq!(display(&mut b, "dmm1").0, "FUSE");
}

/// ⊣⊢ capacitance (PLUG-5): the meter times the RC charge of the capacitor through its range
/// resistor and auto-ranges from 60 nF up to the right range.
#[test]
fn dmm_capacitance_mode_measures_and_autoranges() {
    let h = host();
    for (farads, unit, shown, tol) in [
        (100e-9, "nF", 100e-9 * 1e9, 0.02),
        (4.7e-6, "µF", 4.7, 0.02),
        (47e-6, "µF", 47.0, 0.02),
    ] {
        let lib = h.library();
        let mut bb = BenchBuilder::new(&lib, "dmm-cap", Mode::Playground);
        let m = bb.offboard(
            "dmm_handheld",
            "DMM1",
            30.0,
            0.0,
            &[("mode", PropValue::Text("cap".into()))],
        );
        let c = bb.offboard(
            "capacitor",
            "C1",
            40.0,
            0.0,
            &[("capacitance", PropValue::Num(farads))],
        );
        bb.wire((&m, "VO"), (&c, "1"), "red");
        bb.wire((&m, "COM"), (&c, "2"), "black");
        let mut b = Bench::new(bb.finish(), lib, h.bench_options()).unwrap();
        b.run_until(SimTime::from_secs(6)).unwrap();
        let v = reading(&b, "dmm1", "value");
        assert!((v / farads - 1.0).abs() < tol, "{farads} F read as {v} F");
        let (text, u, ann) = display(&mut b, "dmm1");
        assert_eq!(u, unit, "{text} {u}");
        let x: f64 = text.parse().unwrap();
        assert!((x / shown - 1.0).abs() < tol, "{text} {u}");
        assert!(ann.iter().any(|a| a == "⊣⊢"), "{ann:?}");
    }
}
