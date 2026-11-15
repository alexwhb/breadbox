//! Headless bench scenarios with real compiled firmware (SIM-*, WB-13).

mod common;
use bb_circuit::project::Quantity;
use bb_core::{DeviceEvent, Health, Look, SimTime, UserInput};
use common::*;

fn ms(v: u64) -> SimTime {
    SimTime::from_ms(v)
}

#[test]
fn button_with_input_pullup_and_rc_debounce() {
    let run = |with_cap: bool| {
        let lib = lib();
        let (mut b, u1, bb) = uno_bench(&lib, "button");
        b.wire((&bb, "b-2"), (&bb, "t-2"), "black");
        let sw = b.rigid(
            "push_button",
            "SW1",
            &bb,
            "1",
            "e10",
            90.0,
            &[("bounce_ms", n(2.0))],
        );
        b.wire((&u1, "D2"), (&bb, "c10"), "green");
        b.wire((&bb, "j10"), (&bb, "b-9"), "black");
        if with_cap {
            b.two_lead(
                "capacitor",
                "C1",
                &bb,
                "b10",
                "t-9",
                &[("capacitance", n(100e-9))],
            );
        }
        let mut bench = start(b.finish(), &[(&u1, "button")]);
        bench.run_until(ms(300)).unwrap();
        for k in 0..3u64 {
            bench.run_until(ms(500 + 600 * k)).unwrap();
            bench.input(&sw, &UserInput::Press(true)).unwrap();
            bench.run_until(ms(800 + 600 * k)).unwrap();
            bench.input(&sw, &UserInput::Press(false)).unwrap();
        }
        bench.run_until(ms(2200)).unwrap();
        let l = lines(&bench, &u1);
        let presses = l
            .iter()
            .filter_map(|x| {
                x.strip_prefix("presses=")
                    .and_then(|v| v.parse::<u32>().ok())
            })
            .next_back()
            .unwrap_or(0);
        (presses, l)
    };
    let (with_rc, l) = run(true);
    assert_eq!(with_rc, 3, "the RC filter swallows the bounce: {l:?}");
    assert!(
        l.contains(&"level=1".to_string()) && l.contains(&"level=0".to_string()),
        "{l:?}"
    );
    let (bare, l) = run(false);
    assert!(
        bare > 3,
        "without the capacitor the contact bounce counts several times: {bare} {l:?}"
    );
}

#[test]
fn analog_write_into_rc_filter_matches_spice() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "pwm-rc");
    b.wire((&u1, "D9"), (&bb, "a5"), "orange");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c10",
        &[("resistance", n(10e3)), ("tempco", n(0.0))],
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
    b.probe("A0", (&u1, "A0"), Quantity::Voltage, None);
    b.instrument(
        "mini_scope",
        None,
        &[("ch1", (&u1, "D9")), ("ch2", (&u1, "A0"))],
        serde_json::json!({"timebase": 0.001, "vdiv": 2.0}),
    );
    let mut bench = start(b.finish(), &[(&u1, "pwm_rc")]);
    bench.run_until(ms(800)).unwrap();
    let tel = bench.telemetry();
    let s = &tel.scopes[0];
    assert!((s.frequency - 490.2).abs() < 2.0, "{}", s.frequency);
    assert!((s.duty - 64.0 / 255.0).abs() < 0.01, "{}", s.duty);
    let v_bench = bench.voltage("A0").unwrap();
    let vcc = bench.voltage("VCC").unwrap();
    // the same circuit by hand in SPICE: the pin as a 25 Ω / 22 Ω Thevenin source
    let period: f64 = 1.0 / (16e6 / 64.0 / 510.0);
    let high = period * 64.0 / 255.0;
    let net = format!(
        "pwm rc by hand\nV1 drv 0 PULSE(0 {vcc} 0 1n 1n {high} {period})\nRpin drv pin 24\nR1 pin out 10k\nC1 out 0 10u\n.tran 10u 0.8\n.end\n"
    );
    let res = bb_spice::Netlist::parse(&net).unwrap().run().unwrap();
    let tran = res.plot(bb_spice::PlotKind::Tran).unwrap();
    let out = tran.vector("v(out)").unwrap();
    let v_spice = out.real_at(out.len() - 1);
    assert!(
        (v_bench - v_spice).abs() < 0.01 * v_spice + 0.005,
        "bench {v_bench} vs spice {v_spice}"
    );
    // the firmware reads the same through the ADC
    let a0 = lines(&bench, &u1)
        .iter()
        .filter_map(|l| l.strip_prefix("a0=").and_then(|v| v.parse::<f64>().ok()))
        .next_back()
        .unwrap();
    assert!(
        (a0 - v_spice / vcc * 1024.0).abs() < 4.0,
        "adc {a0} vs {}",
        v_spice / vcc * 1024.0
    );
}

#[test]
fn led_without_resistor_burns_out_and_repair_all_fixes_it() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "burn");
    b.wire((&u1, "D13"), (&bb, "a5"), "orange");
    let d1 = b.two_lead("led", "D1", &bb, "b5", "b6", &[]);
    b.wire((&bb, "a6"), (&bb, "t-6"), "black");
    let project = b.finish();
    let mut bench = start(project, &[(&u1, "blink")]);
    let tel = bench.telemetry();
    assert!(tel.erc.iter().any(|e| e.code == "led_no_resistor"));
    bench.run_until(ms(700)).unwrap();
    assert!(
        matches!(bench.health(&d1), Health::Damaged { .. }),
        "{:?}",
        bench.health(&d1)
    );
    let ev = bench.take_events();
    assert!(
        ev.iter().any(|e| e.source == "D1" && matches!(&e.event, DeviceEvent::Damage { reason } if reason.contains("LED burnt out"))),
        "{ev:?}"
    );
    let tel = bench.telemetry();
    let part = tel.parts.iter().find(|p| p.part == d1).unwrap();
    assert!(part.visual.smoke > 0.0);
    assert!(matches!(part.visual.health, Health::Damaged { .. }));
    // a burnt LED is open: no current any more, the pin survived
    bench.run_until(ms(1700)).unwrap();
    assert!(
        bench.element_current(&d1, "D").is_none(),
        "damaged LED is out of the circuit"
    );
    let board = tel.parts.iter().find(|p| p.part == u1).unwrap();
    assert_eq!(board.visual.health, Health::Ok);
    bench.repair_all().unwrap();
    assert_eq!(bench.health(&d1), Health::Ok);
    assert!(bench.element_current(&d1, "D").is_some());
}

#[test]
fn usb_short_trips_the_polyfuse_and_it_resets_when_removed() {
    let lib = lib();
    let mut b = bb_circuit::builder::BenchBuilder::new(
        &lib,
        "short",
        bb_circuit::project::Mode::Playground,
    );
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    b.two_lead("resistor", "R1", &bb, "a5", "a9", &[("resistance", n(1e3))]);
    b.wire((&bb, "b5"), (&bb, "t+4"), "red");
    b.wire((&bb, "b9"), (&bb, "t-8"), "black");
    let short = b.wire((&bb, "t+20"), (&bb, "t-20"), "yellow");
    let mut project = b.finish();
    let mut bench = start(project.clone(), &[]);
    bench.run_until(ms(200)).unwrap();
    let ev = bench.take_events();
    assert!(
        ev.iter().any(|e| matches!(&e.event, DeviceEvent::Stress { reason } if reason.contains("polyfuse tripped"))),
        "{ev:?}"
    );
    let i = bench
        .readings(&ps)
        .iter()
        .find(|r| r.0 == "usb_current_a")
        .unwrap()
        .1;
    assert!(i < 0.15, "tripped fuse limits the short to {i}");
    assert_eq!(
        bench
            .readings(&ps)
            .iter()
            .find(|r| r.0 == "fuse_tripped")
            .unwrap()
            .1,
        1.0
    );
    // remove the short: the PTC cools and resets, 5 V comes back
    project.wires.retain(|w| w.id != short);
    bench.update_project(project).unwrap();
    bench.run_until(ms(2000)).unwrap();
    assert_eq!(
        bench
            .readings(&ps)
            .iter()
            .find(|r| r.0 == "fuse_tripped")
            .unwrap()
            .1,
        0.0
    );
    let v = bench.pin_voltage(&ps, "VBUS").unwrap();
    assert!((v - 4.98).abs() < 0.03, "{v}");
}

#[test]
fn shorted_uno_rail_browns_out_the_mcu() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "brownout");
    b.wire((&bb, "t+10"), (&bb, "t-10"), "yellow");
    let mut bench = start(b.finish(), &[(&u1, "blink")]);
    bench.run_until(ms(300)).unwrap();
    let tel = bench.telemetry();
    let m = &tel.mcus[0];
    assert!(m.vcc < 1.0, "rail collapsed: {}", m.vcc);
    assert!(m.in_reset, "held in reset below the BOD level");
}

#[test]
fn dht22_with_the_adafruit_library_reads_the_world() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "dht");
    b.project.world.noise = false;
    b.project.world.air.temp_c = 23.4;
    b.project.world.air.rh = 61.0;
    let dht = b.offboard("dht22", "U2", 40.0, 24.0, &[]);
    b.wire((&dht, "VCC"), (&bb, "t+10"), "red");
    b.wire((&dht, "GND"), (&bb, "t-10"), "black");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "a12",
        "a16",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "b12"), (&bb, "t+12"), "red");
    b.wire((&dht, "DATA"), (&bb, "c16"), "yellow");
    b.wire((&bb, "d16"), (&u1, "D2"), "yellow");
    let mut bench = start(b.finish(), &[(&u1, "dht22")]);
    bench.run_until(ms(2600)).unwrap();
    bench.set_world("air.temp_c", -5.5);
    bench.set_world("air.rh", 33.3);
    bench.run_until(ms(9000)).unwrap();
    let l = lines(&bench, &u1);
    let reads: Vec<(f64, f64)> = l
        .iter()
        .filter_map(|x| Some((field(x, "t")?, field(x, "h")?)))
        .collect();
    assert!(!l.iter().any(|x| x.contains("failed")), "{l:?}");
    assert_eq!(reads.first(), Some(&(23.4, 61.0)), "{l:?}");
    assert_eq!(reads.last(), Some(&(-5.5, 33.3)), "{l:?}");
}

#[test]
fn lcd_shows_liquidcrystal_text() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "lcd");
    let lcd = b.offboard("lcd_1602", "LCD1", 40.0, 24.0, &[]);
    for (pin, to) in [
        ("RS", "D12"),
        ("E", "D11"),
        ("D4", "D5"),
        ("D5", "D4"),
        ("D6", "D3"),
        ("D7", "D2"),
    ] {
        b.wire((&lcd, pin), (&u1, to), "blue");
    }
    b.wire((&lcd, "VSS"), (&bb, "t-20"), "black");
    b.wire((&lcd, "RW"), (&bb, "t-21"), "black");
    b.wire((&lcd, "V0"), (&bb, "t-22"), "black");
    b.wire((&lcd, "VDD"), (&bb, "t+20"), "red");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "a24",
        "a28",
        &[("resistance", n(220.0))],
    );
    b.wire((&bb, "b24"), (&bb, "t+23"), "red");
    b.wire((&lcd, "A"), (&bb, "b28"), "red");
    b.wire((&lcd, "K"), (&bb, "t-25"), "black");
    let mut bench = start(b.finish(), &[(&u1, "lcd")]);
    bench.run_until(ms(2500)).unwrap();
    match bench.visual(&lcd).unwrap().look {
        Look::TextLcd {
            text,
            display_on,
            backlight,
            ..
        } => {
            assert_eq!(text[0], "Hello, Breadbox!");
            assert!(text[1].starts_with("t=2s °"), "{:?}", text[1]);
            assert!(display_on && backlight);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn servo_sweep_angles_follow_the_pulses() {
    let lib = lib();
    let (mut b, u1, _bb) = uno_bench(&lib, "servo");
    let m1 = b.offboard("servo", "M1", 40.0, 24.0, &[]);
    b.wire((&m1, "GND"), (&u1, "GND.2"), "brown");
    b.wire((&m1, "VCC"), (&u1, "5V"), "red");
    b.wire((&m1, "SIG"), (&u1, "D9"), "orange");
    let mut bench = start(b.finish(), &[(&u1, "servo")]);
    let mut seen = Vec::new();
    for k in 0..7u64 {
        // each step is held 400 ms; look near its end
        bench.run_until(ms(380 + 400 * k)).unwrap();
        let r = bench.readings(&m1);
        let angle = r.iter().find(|x| x.0 == "angle_deg").unwrap().1;
        seen.push(angle);
    }
    let want = [0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0];
    for (a, w) in seen.iter().zip(want) {
        assert!((a - w).abs() <= 1.5, "{seen:?}");
    }
    let l = lines(&bench, &u1);
    assert!(l.contains(&"angle=180".to_string()), "{l:?}");
    match bench.visual(&m1).unwrap().look {
        Look::Servo { .. } => {}
        other => panic!("{other:?}"),
    }
}

/// SIM-6 / WB-13: an MCU pin sourcing more than its 40 mA rating is flagged (stress), and a
/// pin shorted to GND is destroyed (damage event, serial warning) while the rest of the board
/// keeps running. Firmware: the real blink sketch (D13 high every other second).
#[test]
fn mcu_pin_over_40ma_is_stressed_and_a_short_destroys_it() {
    use bb_circuit::PropValue;
    let lib = lib();
    // D13 → 68 Ω → GND: ≈ 5 V / (25 + 68) Ω ≈ 50 mA while high — over the rating, not fatal
    let (mut b, u1, _bb) = uno_bench(&lib, "pin-50ma");
    let r = b.offboard(
        "resistor",
        "R1",
        40.0,
        30.0,
        &[("resistance", PropValue::Num(68.0))],
    );
    b.wire((&u1, "D13"), (&r, "1"), "orange");
    b.wire((&r, "2"), (&u1, "GND"), "black");
    let mut bench = start(b.finish(), &[(&u1, "blink")]);
    bench.run_until(SimTime::from_ms(2500)).unwrap();
    let ev = bench.take_events();
    assert!(
        ev.iter().any(|e| matches!(&e.event, DeviceEvent::Stress { reason } if reason.contains("pin D13 over 40 mA"))),
        "{ev:?}"
    );
    assert!(
        !ev.iter().any(|e| matches!(e.event, DeviceEvent::Damage { .. })),
        "{ev:?}"
    );
    // D13 straight to GND: the pin burns out
    let (mut b, u1, _bb) = uno_bench(&lib, "pin-short");
    b.wire((&u1, "D13"), (&u1, "GND"), "black");
    let mut bench = start(b.finish(), &[(&u1, "blink")]);
    bench.run_until(SimTime::from_ms(2500)).unwrap();
    let ev = bench.take_events();
    assert!(
        ev.iter().any(|e| matches!(&e.event, DeviceEvent::Damage { reason } if reason.contains("pin D13 destroyed"))),
        "{ev:?}"
    );
    let warned = bench
        .serial_mut(&u1)
        .is_some_and(|s| s.take_new().iter().any(|l| l.text.contains("pin D13 destroyed")));
    assert!(warned, "serial monitor warns");
}

/// SIM-6 / WB-13: an electrolytic capacitor wired backwards across 5 V pops (damage + sound)
/// and stays damaged.
#[test]
fn reversed_electrolytic_pops() {
    use bb_circuit::PropValue;
    let lib = lib();
    let (mut b, u1, _bb) = uno_bench(&lib, "reverse-cap");
    let c = b.offboard(
        "capacitor_electrolytic",
        "C1",
        40.0,
        30.0,
        &[("capacitance", PropValue::Num(100e-6))],
    );
    // + to GND, − to 5 V
    b.wire((&c, "plus"), (&u1, "GND"), "black");
    b.wire((&c, "minus"), (&u1, "5V"), "red");
    let mut bench = start(b.finish(), &[]);
    bench.run_until(SimTime::from_ms(500)).unwrap();
    let ev = bench.take_events();
    assert!(
        ev.iter().any(|e| matches!(&e.event, DeviceEvent::Damage { reason } if reason.contains("electrolytic reversed"))),
        "{ev:?}"
    );
    assert!(
        ev.iter()
            .any(|e| matches!(e.event, DeviceEvent::Sound(bb_core::Sound::Pop))),
        "pop"
    );
    assert!(bench.health(&c).is_damaged(), "{:?}", bench.health(&c));
    bench.run_until(SimTime::from_ms(1000)).unwrap();
    assert!(bench.health(&c).is_damaged(), "stays damaged");
}
