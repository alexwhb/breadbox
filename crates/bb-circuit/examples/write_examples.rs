//! Writes the example projects (`examples/<name>/project.json`, PRJ-2/PRJ-3).
//!
//!     cargo run -p bb-circuit --example write_examples [name…]
//!
//! The committed JSON files are the source of truth for the app; this generator keeps their
//! breadboard coordinates consistent (every lead lands in a hole). Sketches live next to each
//! project under `firmware/<ref>/<sketch>/` (written by the firmware/radio side).

use bb_circuit::builder::BenchBuilder;
use bb_circuit::library::Library;
use bb_circuit::part::PropValue;
use bb_circuit::project::{FirmwareRef, Mode, Project, Quantity};
use bb_world::{PotConfig, WorldConfig};
use serde_json::json;
use std::path::PathBuf;

fn n(v: f64) -> PropValue {
    PropValue::Num(v)
}
fn t(v: &str) -> PropValue {
    PropValue::Text(v.into())
}

fn firmware(b: &mut BenchBuilder, part: &str, sketch: &str, image: Option<&str>) {
    firmware_for(b, part, sketch, None, image);
}

fn firmware_for(
    b: &mut BenchBuilder,
    part: &str,
    sketch: &str,
    fqbn: Option<&str>,
    image: Option<&str>,
) {
    let p = b.project.part_mut(part).expect("mcu part");
    p.firmware = Some(FirmwareRef {
        sketch: sketch.into(),
        fqbn: fqbn.map(String::from),
        image: image.map(String::from),
    });
}

pub fn garden_node(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "garden-node", Mode::Playground);
    b.project.description =
        "UNO + ESP-01 garden monitor: soil moisture, light level, a night LED and MQTT.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_830", 32.0, 2.0);
    // rails
    b.wire((&u1, "5V"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    // LDR divider → A1: 5 V — LDR1 — A1 — R1 10k — GND
    b.two_lead("ldr", "LDR1", &bb, "b3", "b6", &[]);
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c6",
        "c10",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "a3"), (&bb, "t+3"), "red");
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    b.wire((&bb, "d6"), (&u1, "A1"), "green");
    // night LED: D9 → R2 220 Ω → D1 amber → GND
    b.wire((&u1, "D9"), (&bb, "a18"), "orange");
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "c18",
        "c22",
        &[("resistance", n(220.0))],
    );
    b.two_lead("led", "D1", &bb, "d22", "d23", &[("color", t("amber"))]);
    b.wire((&bb, "a23"), (&bb, "t-18"), "black");
    // water LED: D8 → R3 330 Ω → D2 red → GND
    b.wire((&u1, "D8"), (&bb, "a28"), "purple");
    b.two_lead(
        "resistor",
        "R3",
        &bb,
        "c28",
        "c32",
        &[("resistance", n(330.0))],
    );
    b.two_lead("led", "D2", &bb, "d32", "d33", &[("color", t("red"))]);
    b.wire((&bb, "a33"), (&bb, "t-26"), "black");
    // ESP-01 on SoftwareSerial (RX = D3, TX = D2), powered from the UNO's 3.3 V pin
    let u2 = b.offboard("esp01", "U2", 84.0, 9.5, &[("baud", n(9600.0))]);
    b.wire((&u1, "3V3"), (&u2, "VCC"), "red");
    b.wire((&u2, "EN"), (&u2, "VCC"), "red");
    b.wire((&u2, "GND"), (&bb, "t-45"), "black");
    b.wire((&u1, "D2"), (&u2, "RX"), "yellow");
    b.wire((&u2, "TX"), (&u1, "D3"), "blue");
    // capacitive soil sensor J1 in pot 1
    b.offboard("pot_of_soil", "POT1", 60.0, 25.0, &[("pot", t("pot1"))]);
    let j1 = b.offboard("capsoil", "J1", 67.0, 23.0, &[]);
    b.project
        .part_mut(&j1)
        .expect("j1")
        .attach
        .insert("pot".into(), "pot1".into());
    b.wire((&j1, "VCC"), (&bb, "t+30"), "red");
    b.wire((&j1, "GND"), (&bb, "t-30"), "black");
    b.wire((&j1, "AOUT"), (&u1, "A0"), "yellow");
    // bench instruments
    let scope = b.offboard("mini_scope", "SCOPE1", 2.0, -12.0, &[]);
    let dmm = b.offboard("dmm", "DMM1", 82.0, 24.0, &[]);
    b.instrument(
        "mini_scope",
        Some(&scope),
        &[("ch1", (&u1, "D9"))],
        json!({"vdiv": 2.0, "timebase": 1e-3, "trigger": {"source": "ch1", "level": 2.5, "edge": "rising", "mode": "auto"}}),
    );
    b.instrument(
        "dmm",
        Some(&dmm),
        &[("plus", (&j1, "AOUT")), ("com", (&bb, "t-1"))],
        json!({"mode": "dc_v"}),
    );
    // live labels (WB-6)
    b.probe(
        "Night LED · D9",
        (&u1, "D9"),
        Quantity::Duty,
        Some((50.0, 4.5)),
    );
    b.probe(
        "Water LED · D8",
        (&u1, "D8"),
        Quantity::Logic,
        Some((70.0, 4.5)),
    );
    b.probe(
        "Light sensor → A1",
        (&u1, "A1"),
        Quantity::Voltage,
        Some((44.0, 16.0)),
    );
    b.probe(
        "Soil sensor → A0",
        (&j1, "AOUT"),
        Quantity::Voltage,
        Some((52.0, 24.5)),
    );
    firmware(
        &mut b,
        &u1,
        "firmware/U1/garden-node",
        Some("../fixtures/garden-node.uno.hex"),
    );
    b.project.world = WorldConfig {
        pots: vec![PotConfig::default()],
        ..WorldConfig::default()
    };
    b.project.spice.directives =
        ".tran 0 10m 0 1u\n.dc lux 1 10k dec 40\n.step param R1v list 4.7k 10k 22k\n.mc 100 tol=5% seed=7\n.op\n".into();
    b.finish()
}

pub fn blink(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "blink", Mode::Playground);
    b.project.description = "Hello, world: the LED on D13 blinks once a second.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    b.wire((&u1, "D13"), (&bb, "a5"), "orange");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(220.0))],
    );
    b.two_lead("led", "D1", &bb, "d9", "d10", &[("color", t("red"))]);
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    b.probe(
        "LED · D13",
        (&u1, "D13"),
        Quantity::Logic,
        Some((36.0, 0.0)),
    );
    firmware(
        &mut b,
        &u1,
        "firmware/U1/blink",
        Some("../fixtures/blink.uno.hex"),
    );
    b.finish()
}

pub fn night_light(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "night-light", Mode::Playground);
    b.project.description = "An LDR on A0 measures the room; after dark the UNO dims an LED up \
         through a 2N2222 on D9 — the darker, the brighter."
        .into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_830", 32.0, 2.0);
    b.wire((&u1, "5V"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    // 5V — LDR1 — A0 — R1 10k — GND
    b.two_lead("ldr", "LDR1", &bb, "b3", "b6", &[]);
    b.wire((&bb, "a3"), (&bb, "t+3"), "red");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c6",
        "c10",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    b.wire((&bb, "d6"), (&u1, "A0"), "yellow");
    // D9 — R2 1k — Q1 base; NPN at columns 53–55 (E B C), emitter to ground. The LED stage
    // sits at the far end of the board so its light does not reach the LDR.
    b.rigid("npn", "Q1", &bb, "E", "e53", 0.0, &[]);
    b.wire((&bb, "a53"), (&bb, "t-42"), "black");
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "b54",
        "b59",
        &[("resistance", n(1e3))],
    );
    b.wire((&u1, "D9"), (&bb, "a59"), "orange");
    // 5V — R3 220 — LED — collector
    b.two_lead("led", "D1", &bb, "d58", "d55", &[("color", t("white"))]);
    b.two_lead(
        "resistor",
        "R3",
        &bb,
        "a58",
        "a63",
        &[("resistance", n(220.0))],
    );
    b.wire((&bb, "b63"), (&bb, "t+49"), "red");
    b.probe(
        "Light sensor → A0",
        (&u1, "A0"),
        Quantity::Voltage,
        Some((36.0, 0.0)),
    );
    b.probe(
        "LED drive · D9",
        (&u1, "D9"),
        Quantity::Duty,
        Some((36.0, 6.0)),
    );
    b.project.world.light.ambient_lux = 300.0;
    firmware(
        &mut b,
        &u1,
        "firmware/U1/night-light",
        Some("../fixtures/night-light.uno.hex"),
    );
    b.finish()
}

pub fn pico_blink(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "pico-blink", Mode::Playground);
    b.project.description = "Raspberry Pi Pico (API-level): the on-board LED blinks at 2 Hz and \
         A0 (GP26) reads a potentiometer."
        .into();
    let u1 = b.offboard("pico", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "3V3"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    b.rigid("potentiometer", "RV1", &bb, "1", "e10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    b.wire((&bb, "a12"), (&bb, "t+10"), "red");
    b.wire((&bb, "a11"), (&u1, "GP26"), "yellow");
    b.probe(
        "Knob → A0 (GP26)",
        (&u1, "GP26"),
        Quantity::Voltage,
        Some((36.0, 0.0)),
    );
    firmware_for(
        &mut b,
        &u1,
        "firmware/U1/pico-blink",
        Some("breadbox:host:pico"),
        None,
    );
    b.finish()
}

pub fn thermometer(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "thermometer", Mode::Playground);
    b.project.description = "TMP36 on A0, temperature on a 16×2 LCD (LiquidCrystal).".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_830", 32.0, 2.0);
    b.wire((&u1, "5V"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    let tmp = b.rigid("tmp36", "U2", &bb, "VS", "e4", 0.0, &[]);
    b.wire((&bb, "a4"), (&bb, "t+4"), "red");
    b.wire((&bb, "a6"), (&bb, "t-5"), "black");
    b.wire((&bb, "a5"), (&u1, "A0"), "yellow");
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
    b.wire((&lcd, "VDD"), (&bb, "t+20"), "red");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "j20",
        "j24",
        &[("resistance", n(1e3))],
    );
    b.wire((&lcd, "V0"), (&bb, "f20"), "green");
    b.wire((&bb, "f24"), (&bb, "b-20"), "black");
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "j28",
        "j32",
        &[("resistance", n(220.0))],
    );
    b.wire((&bb, "f28"), (&bb, "b+24"), "red");
    b.wire((&lcd, "A"), (&bb, "f32"), "red");
    b.wire((&lcd, "K"), (&bb, "b-26"), "black");
    b.wire((&bb, "t+40"), (&bb, "b+40"), "red");
    b.wire((&bb, "t-40"), (&bb, "b-40"), "black");
    b.probe(
        "TMP36 → A0",
        (&tmp, "VOUT"),
        Quantity::Voltage,
        Some((36.0, 0.0)),
    );
    firmware(
        &mut b,
        &u1,
        "firmware/U1/thermometer",
        Some("../fixtures/thermometer.uno.hex"),
    );
    b.finish()
}

pub fn ble_button(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "ble-button", Mode::Playground);
    b.project.description =
        "A push button reported over Bluetooth LE (HM-10) to a virtual phone; the phone switches the LED on D13.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "5V"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    // tactile switch straddling the channel: pins 1/2 in the top half, 3/4 below
    b.rigid("push_button", "SW1", &bb, "1", "e5", 90.0, &[]);
    b.wire((&bb, "a5"), (&u1, "D4"), "green");
    b.wire((&bb, "j5"), (&bb, "b-4"), "black");
    b.wire((&bb, "b-2"), (&bb, "t-2"), "black");
    let ble = b.offboard("hm10", "U2", 72.0, 4.0, &[]);
    b.wire((&ble, "VCC"), (&bb, "t+20"), "red");
    b.wire((&ble, "GND"), (&bb, "t-20"), "black");
    b.wire((&ble, "TXD"), (&u1, "D10"), "blue");
    // D11 → 1k / 2k divider → RXD (the HM-10's input is 3.3 V)
    b.wire((&u1, "D11"), (&bb, "a14"), "yellow");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "b14",
        "b18",
        &[("resistance", n(1e3))],
    );
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "c18",
        "c22",
        &[("resistance", n(2e3))],
    );
    b.wire((&bb, "a22"), (&bb, "t-18"), "black");
    b.wire((&bb, "d18"), (&ble, "RXD"), "yellow");
    b.probe(
        "Button · D4",
        (&u1, "D4"),
        Quantity::Logic,
        Some((36.0, 0.0)),
    );
    firmware(
        &mut b,
        &u1,
        "firmware/U1/ble-button",
        Some("../fixtures/ble-button.uno.hex"),
    );
    b.finish()
}

pub fn servo_sweep(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "servo-sweep", Mode::Playground);
    b.project.description = "An SG90 servo sweeping 0–180° with the Servo library.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let m1 = b.offboard("servo", "M1", 34.0, 6.0, &[]);
    b.wire((&m1, "GND"), (&u1, "GND"), "brown");
    b.wire((&m1, "VCC"), (&u1, "5V"), "red");
    b.wire((&m1, "SIG"), (&u1, "D9"), "orange");
    b.probe("Servo · D9", (&u1, "D9"), Quantity::Duty, Some((30.0, 0.0)));
    firmware(
        &mut b,
        &u1,
        "firmware/U1/servo-sweep",
        Some("../fixtures/servo-sweep.uno.hex"),
    );
    b.finish()
}

pub fn esp32_weather(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "esp32-weather", Mode::Playground);
    b.project.description =
        "ESP32 + DHT22 publishing temperature and humidity over Wi-Fi/MQTT.".into();
    let bb = b.breadboard("breadboard_830", 0.0, 2.0);
    let u1 = b.rigid("esp32_devkit", "U1", &bb, "EN", "a10", 0.0, &[]);
    let _ = u1;
    let dht = b.offboard("dht22", "U2", 40.0, 24.0, &[]);
    b.wire((&bb, "j10"), (&bb, "b+11"), "red"); // 3V3 (second header row lands in row i)
    b.wire((&bb, "j11"), (&bb, "b-12"), "black"); // GND.2
    b.wire((&dht, "VCC"), (&bb, "b+30"), "red");
    b.wire((&dht, "GND"), (&bb, "b-30"), "black");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "g34",
        "g38",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "f34"), (&bb, "b+34"), "red");
    b.wire((&dht, "DATA"), (&bb, "f38"), "yellow");
    b.wire((&bb, "h38"), (&bb, "j14"), "yellow"); // to D4 (third column of the right row)
    firmware_for(
        &mut b,
        "u1",
        "firmware/U1/esp32-weather",
        Some("breadbox:host:esp32"),
        None,
    );
    b.finish()
}

pub fn astable_555(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "555-astable", Mode::Playground);
    b.project.description = "The classic 555 blinker: R1 1k, R2 10k, C1 47 µF → ~1.4 Hz.".into();
    let bt = b.offboard("battery_pack", "BT1", 0.0, 4.0, &[("kind", t("9v"))]);
    let bb = b.breadboard("breadboard_400", 12.0, 2.0);
    b.wire((&bt, "plus"), (&bb, "t+1"), "red");
    b.wire((&bt, "minus"), (&bb, "t-1"), "black");
    // NE555 straddling the channel at columns 10–13: pin 1 GND at f10 … pin 8 VCC at e10
    b.rigid("ne555", "U1", &bb, "GND", "f10", 0.0, &[]);
    b.wire((&bb, "j10"), (&bb, "b-8"), "black"); // GND
    b.wire((&bb, "a10"), (&bb, "t+8"), "red"); // VCC
    b.wire((&bb, "j13"), (&bb, "b+10"), "red"); // RESET (pin 4) high
    b.wire((&bb, "b+2"), (&bb, "t+2"), "red");
    b.wire((&bb, "b-2"), (&bb, "t-2"), "black");
    // R1 VCC → DIS (pin 7 at e11), R2 DIS → THR (pin 6 at e12), THR ↔ TRIG (pin 2 at f11)
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "a6",
        "a11",
        &[("resistance", n(1e3))],
    );
    b.wire((&bb, "b6"), (&bb, "t+5"), "red");
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "b11",
        "b15",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "c15"), (&bb, "c12"), "green");
    b.wire((&bb, "d12"), (&bb, "i11"), "green");
    b.two_lead(
        "capacitor_electrolytic",
        "C1",
        &bb,
        "j15",
        "j16",
        &[("capacitance", n(47e-6))],
    );
    b.wire((&bb, "h15"), (&bb, "h11"), "green");
    b.wire((&bb, "j17"), (&bb, "b-14"), "black");
    b.wire((&bb, "i16"), (&bb, "i17"), "black");
    // OUT (pin 3 at f12) → R3 → LED → GND
    b.two_lead(
        "resistor",
        "R3",
        &bb,
        "g12",
        "g20",
        &[("resistance", n(470.0))],
    );
    b.two_lead("led", "D1", &bb, "h20", "h21", &[("color", t("green"))]);
    b.wire((&bb, "j21"), (&bb, "b-18"), "black");
    b.probe(
        "Timing cap",
        (&bb, "c12"),
        Quantity::Voltage,
        Some((14.0, 0.0)),
    );
    b.probe(
        "555 out",
        (&bb, "f12"),
        Quantity::Frequency,
        Some((28.0, 0.0)),
    );
    b.finish()
}

pub fn rc_filter_bode(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "rc-filter-bode", Mode::Pro);
    b.project.description =
        "First-order RC low-pass: Bode plot with .ac, step response with .tran.".into();
    let v1 = b.schematic_only(
        "vsource",
        "V1",
        10.0,
        14.0,
        0.0,
        &[("spec", t("DC 0 AC 1 PULSE(0 5 0 1u 1u 5m 10m)"))],
    );
    let r1 = b.schematic_only("resistor", "R1", 18.0, 8.0, 0.0, &[("resistance", n(1e3))]);
    let c1 = b.schematic_only(
        "capacitor",
        "C1",
        26.0,
        14.0,
        90.0,
        &[("capacitance", n(100e-9))],
    );
    let g1 = b.schematic_only("gnd", "#GND1", 10.0, 20.0, 0.0, &[]);
    let g2 = b.schematic_only("gnd", "#GND2", 26.0, 20.0, 0.0, &[]);
    let (vp, vn) = (b.anchor(&v1, "plus"), b.anchor(&v1, "minus"));
    let (ra, rb) = (b.anchor(&r1, "1"), b.anchor(&r1, "2"));
    let (ca, cb) = (b.anchor(&c1, "1"), b.anchor(&c1, "2"));
    let (g1a, g2a) = (b.anchor(&g1, "0"), b.anchor(&g2, "0"));
    b.sch_wire(vec![vp, (vp.0, ra.1), ra]);
    b.sch_wire(vec![rb, (ca.0, rb.1), ca]);
    b.sch_wire(vec![vn, g1a]);
    b.sch_wire(vec![cb, g2a]);
    b.project
        .schematic
        .labels
        .push(bb_circuit::project::NetLabel {
            id: "nl_in".into(),
            name: "IN".into(),
            x: vp.0,
            y: ra.1,
            rot: 0.0,
        });
    b.project
        .schematic
        .labels
        .push(bb_circuit::project::NetLabel {
            id: "nl_out".into(),
            name: "OUT".into(),
            x: ca.0,
            y: rb.1,
            rot: 0.0,
        });
    b.project.spice.directives = ".ac dec 20 10 100k\n.tran 10u 20m\n.op\n".into();
    b.finish()
}

pub fn op_amp_amplifier(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "op-amp-amplifier", Mode::Pro);
    b.project.description = "Non-inverting amplifier, gain 11, TL072 on ±12 V.".into();
    let vin = b.schematic_only(
        "vsource",
        "VIN",
        6.0,
        20.0,
        0.0,
        &[("spec", t("SIN(0 0.5 1k) AC 1"))],
    );
    let vp = b.schematic_only("vsource", "VP", 50.0, 10.0, 0.0, &[("spec", t("DC 12"))]);
    let vn = b.schematic_only("vsource", "VN", 56.0, 10.0, 0.0, &[("spec", t("DC 12"))]);
    let u1 = b.schematic_only("opamp_tl072", "U1", 24.0, 20.0, 0.0, &[]);
    let r1 = b.schematic_only("resistor", "R1", 16.0, 34.0, 0.0, &[("resistance", n(1e3))]);
    let r2 = b.schematic_only(
        "resistor",
        "R2",
        30.0,
        34.0,
        0.0,
        &[("resistance", n(10e3))],
    );
    let rl = b.schematic_only(
        "resistor",
        "RL",
        40.0,
        26.0,
        90.0,
        &[("resistance", n(10e3))],
    );
    let _ = (vin, vp, vn, u1, r1, r2, rl);
    // connect everything with net labels at the pin anchors
    let label = |b: &mut BenchBuilder, part: &str, pin: &str, name: &str| {
        let (x, y) = b.anchor(part, pin);
        let id = b.project.next_id("nl");
        b.project
            .schematic
            .labels
            .push(bb_circuit::project::NetLabel {
                id,
                name: name.into(),
                x,
                y,
                rot: 0.0,
            });
    };
    label(&mut b, "vin", "plus", "IN");
    label(&mut b, "vin", "minus", "0");
    label(&mut b, "vp", "plus", "VCC");
    label(&mut b, "vp", "minus", "0");
    label(&mut b, "vn", "plus", "0");
    label(&mut b, "vn", "minus", "VEE");
    label(&mut b, "u1", "IN1+", "IN");
    label(&mut b, "u1", "IN1-", "FB");
    label(&mut b, "u1", "OUT1", "OUT");
    label(&mut b, "u1", "VCC", "VCC");
    label(&mut b, "u1", "VEE", "VEE");
    label(&mut b, "u1", "IN2+", "0");
    label(&mut b, "u1", "IN2-", "OUT2");
    label(&mut b, "u1", "OUT2", "OUT2");
    label(&mut b, "r1", "1", "0");
    label(&mut b, "r1", "2", "FB");
    label(&mut b, "r2", "1", "FB");
    label(&mut b, "r2", "2", "OUT");
    label(&mut b, "rl", "1", "OUT");
    label(&mut b, "rl", "2", "0");
    b.project.spice.directives = ".tran 10u 5m\n.ac dec 20 10 10Meg\n.op\n".into();
    b.finish()
}

fn main() {
    let lib = Library::builtin();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let only: Vec<String> = std::env::args().skip(1).collect();
    let all: Vec<(&str, Project)> = vec![
        ("garden-node", garden_node(&lib)),
        ("blink", blink(&lib)),
        ("night-light", night_light(&lib)),
        ("thermometer", thermometer(&lib)),
        ("ble-button", ble_button(&lib)),
        ("servo-sweep", servo_sweep(&lib)),
        ("esp32-weather", esp32_weather(&lib)),
        ("pico-blink", pico_blink(&lib)),
        ("555-astable", astable_555(&lib)),
        ("rc-filter-bode", rc_filter_bode(&lib)),
        ("op-amp-amplifier", op_amp_amplifier(&lib)),
    ];
    for (name, p) in all {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let problems = p.check(&lib);
        assert!(problems.is_empty(), "{name}: {problems:?}");
        let dir = root.join(name);
        p.save(&dir).expect("write project");
        println!("wrote {}", dir.join("project.json").display());
    }
}
