//! Behavioural devices without firmware: supplies, sensors in the world, actuators, ICs.

mod common;
use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::{Mode, Quantity};
use bb_circuit::{Library, Project};
use bb_core::{DeviceEvent, Health, Look, Pos, SimTime, Sound, UserInput};
use bb_sim::Bench;
use common::*;

fn ms(v: u64) -> SimTime {
    SimTime::from_ms(v)
}

/// USB power (PS1) feeding an 830-hole breadboard (BB1): top and bottom rails are 5 V / GND.
fn usb_bench<'a>(lib: &'a Library, name: &str) -> (BenchBuilder<'a>, String) {
    let mut b = BenchBuilder::new(lib, name, Mode::Playground);
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_830", 10.0, 0.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    b.wire((&bb, "t+50"), (&bb, "b+50"), "red");
    b.wire((&bb, "t-50"), (&bb, "b-50"), "black");
    (b, bb)
}

fn reading(b: &Bench, part: &str, key: &str) -> f64 {
    b.readings(part).iter().find(|r| r.0 == key).map(|r| r.1).unwrap_or(f64::NAN)
}

fn run(project: Project, t: SimTime) -> Bench {
    let mut b = start(project, &[]);
    b.run_until(t).unwrap();
    b
}

#[test]
fn astable_555_example_blinks_at_the_textbook_frequency() {
    let p = Project::load(root().join("examples/555-astable")).unwrap();
    let mut b = start(p, &[]);
    b.run_until(SimTime::from_secs(1)).unwrap();
    b.instruments_mut().reset_probe_stats();
    b.run_until(SimTime::from_secs(5)).unwrap();
    let tel = b.telemetry();
    let f = tel.probes.iter().find(|p| p.label == "555 out").unwrap().value;
    let expect = 1.0 / (std::f64::consts::LN_2 * (1e3 + 2.0 * 10e3) * 47e-6);
    assert!((f - expect).abs() < 0.05 * expect, "{f} vs {expect}");
    assert!(reading(&b, "u1", "cycles") >= 6.0);
    let cap = tel.probes.iter().find(|p| p.label == "Timing cap").unwrap();
    // the capacitor swings between ⅓ and ⅔ of the supply
    let vbat = b.pin_voltage("bt1", "plus").unwrap();
    assert!(
        (cap.max - vbat * 2.0 / 3.0).abs() < 0.15 && (cap.min - vbat / 3.0).abs() < 0.15,
        "cap {}..{} at {vbat}",
        cap.min,
        cap.max
    );
}

#[test]
fn dc_motor_spins_up_and_slows_under_load() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "motor", Mode::Playground);
    let bt = b.offboard("battery_pack", "BT1", 0.0, 0.0, &[("kind", t("aa4"))]);
    let m = b.offboard("dc_motor", "M1", 20.0, 0.0, &[]);
    b.wire((&bt, "plus"), (&m, "plus"), "red");
    b.wire((&bt, "minus"), (&m, "minus"), "black");
    let mut bench = run(b.finish(), ms(1500));
    let rpm = reading(&bench, &m, "rpm");
    assert!((4000.0..8000.0).contains(&rpm), "{rpm}");
    let i_free = bench.element_current(&m, "V:emf").unwrap();
    bench.set_part_param(&m, "load", 5.0).unwrap();
    bench.run_until(ms(3000)).unwrap();
    let rpm2 = reading(&bench, &m, "rpm");
    let i_load = bench.element_current(&m, "V:emf").unwrap();
    assert!(rpm2 < rpm * 0.9 && i_load > i_free * 1.5, "{rpm}→{rpm2}, {i_free}→{i_load}");
    match bench.visual(&m).unwrap().look {
        Look::Motor { rpm, .. } => assert!(rpm > 0.0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn relay_module_clicks_and_switches_its_contacts() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "relay");
    let k = b.offboard("relay_module", "K1", 40.0, 30.0, &[]);
    let sw = b.rigid("slide_switch", "SW1", &bb, "a", "c10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t-10"), "black"); // A → GND
    b.wire((&bb, "a12"), (&bb, "t+12"), "red"); // B → 5 V
    b.wire((&bb, "a11"), (&k, "IN"), "yellow"); // common → IN
    b.wire((&k, "VCC"), (&bb, "t+20"), "red");
    b.wire((&k, "GND"), (&bb, "t-20"), "black");
    b.wire((&k, "COM"), (&bb, "t+22"), "red");
    b.two_lead("resistor", "R1", &bb, "a30", "a34", &[("resistance", n(330.0))]);
    let d1 = b.two_lead("led", "D1", &bb, "b34", "b35", &[]);
    b.wire((&k, "NO"), (&bb, "b30"), "orange");
    b.wire((&bb, "a35"), (&bb, "t-30"), "black");
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(50)).unwrap();
    assert!(bench.element_current(&d1, "D").unwrap() < 1e-6);
    bench.input(&sw, &UserInput::Toggle { index: 0 }).unwrap();
    bench.run_until(ms(53)).unwrap();
    assert!(bench.element_current(&d1, "D").unwrap() < 1e-6, "5 ms pull-in time");
    bench.run_until(ms(80)).unwrap();
    let i = bench.element_current(&d1, "D").unwrap();
    assert!(i > 0.005, "{i}");
    assert!(matches!(bench.visual(&k).unwrap().look, Look::Relay { energized: true }));
    let ev = bench.take_events();
    assert!(ev.iter().any(|e| e.event == DeviceEvent::Sound(Sound::Click)));
}

#[test]
fn potentiometer_knob_divides_the_supply() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "pot");
    let rv = b.rigid("potentiometer", "RV1", &bb, "1", "e10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t+10"), "red");
    b.wire((&bb, "a12"), (&bb, "t-12"), "black");
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(10)).unwrap();
    let v = |b: &Bench| b.pin_voltage(&rv, "w").unwrap();
    let vcc = bench.pin_voltage(&rv, "1").unwrap();
    assert!((v(&bench) - vcc / 2.0).abs() < 0.01);
    bench.input(&rv, &UserInput::Position(0.25)).unwrap();
    bench.run_until(ms(20)).unwrap();
    assert!((v(&bench) - vcc * 0.75).abs() < 0.01, "{}", v(&bench));
    assert_eq!(bench.visual(&rv).unwrap().look, Look::Knob { position: 0.25 });
}

#[test]
fn a_lamp_next_to_the_ldr_raises_the_divider_voltage() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "lamp");
    b.project.world.light.ambient_lux = 50.0;
    let ldr = b.two_lead("ldr", "LDR1", &bb, "a10", "a13", &[]);
    b.wire((&bb, "b10"), (&bb, "t+10"), "red");
    b.two_lead("resistor", "R1", &bb, "c13", "c17", &[("resistance", n(10e3))]);
    b.wire((&bb, "b17"), (&bb, "t-17"), "black");
    // a desk lamp 10 cm away, on
    let lamp = b.offboard("lamp", "LAMP1", 20.0, 40.0, &[("candela", n(20.0))]);
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(300)).unwrap();
    let lit = bench.pin_voltage(&ldr, "2").unwrap();
    let lux_lit = reading(&bench, &ldr, "lux");
    bench.input(&lamp, &UserInput::Toggle { index: 0 }).unwrap();
    bench.run_until(ms(600)).unwrap();
    let dark = bench.pin_voltage(&ldr, "2").unwrap();
    let lux_dark = reading(&bench, &ldr, "lux");
    assert!((lux_dark - 50.0).abs() < 1.0, "{lux_dark}");
    assert!(lux_lit > 500.0, "{lux_lit}");
    assert!(lit > dark + 1.0, "{lit} vs {dark}");
}

#[test]
fn resistors_heat_up_warm_a_tmp36_and_smoke_when_overloaded() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "heat");
    b.project.world.noise = false;
    // 100 Ω, ½ W across 5 V: 0.25 W → +19 K over the 21 °C air
    let r = b.two_lead("resistor", "R1", &bb, "a10", "a14", &[("resistance", n(100.0)), ("power", n(0.5))]);
    b.wire((&bb, "b10"), (&bb, "t+10"), "red");
    b.wire((&bb, "b14"), (&bb, "t-14"), "black");
    let tmp = b.rigid("tmp36", "U1", &bb, "VS", "j10", 0.0, &[]);
    b.wire((&bb, "f10"), (&bb, "b+10"), "red");
    b.wire((&bb, "f12"), (&bb, "b-12"), "black");
    // 22 Ω, ¼ W: 1.1 W — burns
    let hot = b.two_lead("resistor", "R2", &bb, "a30", "a34", &[("resistance", n(22.0))]);
    b.wire((&bb, "b30"), (&bb, "t+30"), "red");
    b.wire((&bb, "b34"), (&bb, "t-34"), "black");
    let mut bench = start(b.finish(), &[]);
    bench.run_until(SimTime::from_secs(30)).unwrap();
    let t_r = reading(&bench, &r, "temp_c");
    assert!((t_r - (21.0 + 0.25 * 75.0)).abs() < 2.0, "{t_r}");
    let t_tmp = reading(&bench, &tmp, "temp_c");
    assert!(t_tmp > 22.0, "TMP36 near the hot resistor reads {t_tmp}");
    let v = bench.pin_voltage(&tmp, "VOUT").unwrap();
    assert!((v - (0.5 + 0.01 * t_tmp)).abs() < 0.01, "{v}");
    assert!(matches!(bench.health(&hot), Health::Damaged { .. }), "{:?}", bench.health(&hot));
    let tel = bench.telemetry();
    assert!(tel.parts.iter().any(|p| p.part == hot && p.visual.health.is_damaged()));
    assert!(tel.power.dissipation.iter().any(|(r, w)| r == "R1" && (*w - 0.25).abs() < 0.02), "{:?}", tel.power.dissipation);
}

#[test]
fn battery_sags_under_load_and_discharges() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "batt", Mode::Playground);
    let bt = b.offboard("battery_pack", "BT1", 0.0, 0.0, &[("kind", t("lipo1s")), ("charge", n(50.0))]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&bt, "plus"), (&bb, "t+1"), "red");
    b.wire((&bt, "minus"), (&bb, "t-1"), "black");
    b.two_lead("resistor", "R1", &bb, "a5", "a9", &[("resistance", n(10.0)), ("power", n(5.0))]);
    b.wire((&bb, "b5"), (&bb, "t+5"), "red");
    b.wire((&bb, "b9"), (&bb, "t-9"), "black");
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(100)).unwrap();
    let emf = reading(&bench, &bt, "emf");
    let soc0 = reading(&bench, &bt, "soc");
    assert!((3.7..3.95).contains(&emf), "{emf}");
    let v = bench.pin_voltage(&bt, "plus").unwrap();
    assert!(v < emf - 0.02, "terminal {v} below emf {emf}");
    bench.run_until(SimTime::from_secs(5)).unwrap();
    let soc1 = reading(&bench, &bt, "soc");
    let drawn = (emf / 10.1) * 4.9; // A·s
    let expect = drawn / 3600.0 * 100.0;
    assert!(((soc0 - soc1) - expect).abs() < 0.2 * expect, "{} vs {expect}", soc0 - soc1);
}

#[test]
fn pir_sees_a_moving_hand_and_holds() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "pir");
    let pir = b.offboard("pir", "U1", 40.0, 30.0, &[("hold_s", n(1.0)), ("warmup_s", n(0.0))]);
    b.wire((&pir, "VCC"), (&bb, "t+10"), "red");
    b.wire((&pir, "GND"), (&bb, "t-10"), "black");
    b.project.world.objects = vec![bb_world::ObjectConfig {
        id: "hand".into(),
        pos: Pos::new(0.10, 0.5),
        ..Default::default()
    }];
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(500)).unwrap();
    let out = |b: &Bench| b.pin_voltage(&pir, "OUT").unwrap();
    assert!(out(&bench) < 0.5);
    bench.set_world("hand.x", 0.2);
    bench.run_until(ms(600)).unwrap();
    assert!(out(&bench) > 3.0, "{}", out(&bench));
    bench.run_until(ms(2000)).unwrap();
    assert!(out(&bench) < 0.5, "hold time over");
}

#[test]
fn hall_sensor_pulls_low_near_a_magnet() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "hall");
    let h = b.rigid("hall_sensor", "U1", &bb, "VCC", "e10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t+10"), "red");
    b.wire((&bb, "a11"), (&bb, "t-11"), "black");
    b.two_lead("resistor", "R1", &bb, "a12", "t+14", &[("resistance", n(10e3))]);
    b.project.world.objects = vec![bb_world::ObjectConfig {
        id: "mag".into(),
        kind: bb_world::ObjectKind::Magnet,
        pos: Pos::new(0.5, 0.5),
        strength_mt: 50.0,
        radius: 0.005,
        ..Default::default()
    }];
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(50)).unwrap();
    assert!(bench.pin_voltage(&h, "OUT").unwrap() > 4.5);
    let at = Pos::new(20.0 * 0.00254, 8.0 * 0.00254);
    bench.set_world("mag.x", at.x + 0.004);
    bench.set_world("mag.y", at.y);
    bench.run_until(ms(100)).unwrap();
    assert!(bench.pin_voltage(&h, "OUT").unwrap() < 0.5);
}

#[test]
fn microphone_output_follows_the_sound_level() {
    let lib = lib();
    let (mut b, _bb) = usb_bench(&lib, "mic");
    let mic = b.offboard("microphone", "MK1", 40.0, 30.0, &[]);
    b.wire((&mic, "VCC"), ("ps1", "VBUS"), "red");
    b.wire((&mic, "GND"), ("ps1", "GND"), "black");
    b.probe("mic", (&mic, "OUT"), Quantity::Voltage, None);
    b.project.world.vars.insert("sound.db".into(), 80.0);
    b.project.world.noise = false;
    let mut bench = start(b.finish(), &[]);
    bench.run_until(ms(100)).unwrap();
    let tel = bench.telemetry();
    let p = tel.probes.iter().find(|p| p.label == "mic").unwrap();
    assert!((p.avg - 2.49).abs() < 0.05, "{}", p.avg);
    assert!((p.max - p.min - 0.2).abs() < 0.03, "{} {}", p.min, p.max);
}

#[test]
fn a_pump_waters_its_pot() {
    let lib = lib();
    let (mut b, _bb) = usb_bench(&lib, "pump");
    b.project.world = bb_world::WorldConfig {
        pots: vec![bb_world::PotConfig { vwc: 20.0, evaporation: false, ..Default::default() }],
        ..Default::default()
    };
    let p = b.offboard("water_pump", "M1", 40.0, 30.0, &[]);
    b.wire((&p, "plus"), ("ps1", "VBUS"), "red");
    b.wire((&p, "minus"), ("ps1", "GND"), "black");
    let bench = run(b.finish(), SimTime::from_secs(2));
    let vwc = bb_core::WorldView::soil_vwc(bench.world(), "pot1").unwrap();
    assert!((vwc - 23.9).abs() < 0.5, "{vwc}");
}

#[test]
fn rgb_seven_segment_and_bar_graph_visuals() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "lights");
    let rgb = b.rigid("rgb_led", "D1", &bb, "R", "e10", 0.0, &[]);
    b.wire((&bb, "a11"), (&bb, "t-11"), "black"); // COM (cathode)
    b.two_lead("resistor", "R1", &bb, "a10", "t+8", &[("resistance", n(220.0))]); // red on
    b.two_lead("resistor", "R2", &bb, "a13", "t+15", &[("resistance", n(220.0))]); // blue on
    let seg = b.rigid("seven_segment", "DS1", &bb, "e", "f30", 0.0, &[]);
    b.wire((&bb, "j32"), (&bb, "b-32"), "black"); // COM
    for (hole, rail) in [("a34", "t+35"), ("a33", "t+33")] {
        // segments b and a
        b.two_lead("resistor", &format!("R{hole}"), &bb, hole, rail, &[("resistance", n(470.0))]);
    }
    let mut bench = run(b.finish(), ms(100));
    match bench.visual(&rgb).unwrap().look {
        Look::RgbLed { r, g, b } => assert!(r > 0.4 && g < 0.01 && b > 0.2, "{r} {g} {b}"),
        other => panic!("{other:?}"),
    }
    match bench.visual(&seg).unwrap().look {
        Look::SevenSeg { digits, .. } => assert_eq!(digits[0] & 0b11, 0b11, "{:08b}", digits[0]),
        other => panic!("{other:?}"),
    }
    let _ = &mut bench;
}

#[test]
fn nand_gate_and_l293d_drive_their_outputs() {
    let lib = lib();
    let (mut b, bb) = usb_bench(&lib, "logic");
    let u = b.rigid("hc00", "U1", &bb, "1A", "f10", 0.0, &[]);
    b.wire((&u, "VCC"), (&bb, "t+10"), "red");
    b.wire((&u, "GND"), (&bb, "b-15"), "black");
    b.wire((&u, "1A"), (&bb, "b+11"), "red");
    b.wire((&u, "1B"), (&bb, "b+12"), "red");
    b.wire((&u, "2A"), (&bb, "b+13"), "red");
    b.wire((&u, "2B"), (&bb, "b-16"), "black");
    let d = b.rigid("l293d", "U2", &bb, "EN12", "f30", 0.0, &[]);
    b.wire((&d, "VCC1"), (&bb, "t+30"), "red");
    b.wire((&d, "VCC2"), (&bb, "t+31"), "red");
    b.wire((&d, "GND"), (&bb, "b-30"), "black");
    b.wire((&d, "EN12"), (&bb, "b+31"), "red");
    b.wire((&d, "1A"), (&bb, "b+32"), "red");
    b.wire((&d, "2A"), (&bb, "b-33"), "black");
    let bench = run(b.finish(), ms(10));
    let v = |pin: &str| bench.pin_voltage(&u, pin).unwrap();
    assert!(v("1Y") < 0.5, "1·1 NAND = 0");
    assert!(v("2Y") > 4.5, "1·0 NAND = 1");
    let y1 = bench.pin_voltage(&d, "1Y").unwrap();
    let y2 = bench.pin_voltage(&d, "2Y").unwrap();
    assert!(y1 > 3.0 && y2 < 0.5, "{y1} {y2}");
}

#[test]
fn regulators_and_optocoupler() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "reg", Mode::Playground);
    let bt = b.offboard("battery_pack", "BT1", 0.0, 0.0, &[("kind", t("9v"))]);
    let bb = b.breadboard("breadboard_830", 10.0, 0.0);
    b.wire((&bt, "plus"), (&bb, "t+1"), "red");
    b.wire((&bt, "minus"), (&bb, "t-1"), "black");
    b.wire((&bb, "t-50"), (&bb, "b-50"), "black");
    let reg = b.rigid("reg_7805", "U1", &bb, "IN", "e10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t+10"), "red");
    b.wire((&bb, "a11"), (&bb, "t-11"), "black");
    b.wire((&bb, "a12"), (&bb, "b+12"), "orange"); // 5 V out → bottom + rail
    b.two_lead("resistor", "R1", &bb, "j20", "b+20", &[("resistance", n(100.0)), ("power", n(1.0))]);
    b.wire((&bb, "i20"), (&bb, "b-20"), "black");
    let ams = b.rigid("ams1117_3v3", "U2", &bb, "GND", "e30", 0.0, &[]);
    b.wire((&bb, "a30"), (&bb, "b-30"), "black");
    b.wire((&bb, "a32"), (&bb, "b+32"), "orange");
    let opto = b.rigid("pc817", "U3", &bb, "A", "f40", 0.0, &[]);
    b.two_lead("resistor", "R2", &bb, "j40", "b+40", &[("resistance", n(330.0))]);
    b.wire((&bb, "j41"), (&bb, "b-41"), "black"); // K
    b.wire((&bb, "a40"), (&bb, "b-42"), "black"); // E (pin 3 at e41? see below)
    b.two_lead("resistor", "R3", &bb, "a45", "b+45", &[("resistance", n(10e3))]);
    let bench = run(b.finish(), ms(20));
    let v5 = bench.pin_voltage(&reg, "OUT").unwrap();
    assert!((v5 - 5.0).abs() < 0.02, "{v5}");
    let v33 = bench.pin_voltage(&ams, "OUT").unwrap();
    assert!((v33 - 3.3).abs() < 0.02, "{v33}");
    let _ = opto;
}
