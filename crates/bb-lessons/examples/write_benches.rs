//! Writes the start circuits of the built-in lessons (`lessons/<track>/<lesson>.bench.json`):
//! real bb-circuit projects built with `BenchBuilder` (with the Handheld DMM plugin's part), or
//! snapshots of example projects with their sketches and prebuilt firmware copied next to the
//! lessons so every track folder is self-contained.
//!
//!   cargo run -p bb-lessons --example write_benches [name-filter]

use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::{FirmwareRef, Mode, NetLabel};
use bb_circuit::{Library, Project, PropValue};
use std::path::{Path, PathBuf};

fn n(v: f64) -> PropValue {
    PropValue::Num(v)
}
fn t(s: &str) -> PropValue {
    PropValue::Text(s.into())
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// USB 5 V supply wired to the top rails of a 400-hole breadboard.
fn powered<'a>(lib: &'a Library, name: &str) -> (BenchBuilder<'a>, String, String) {
    let mut b = BenchBuilder::new(lib, name, Mode::Playground);
    let ps = b.offboard("usb_power", "PS1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 16.0, 2.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    (b, ps, bb)
}

fn dmm(b: &mut BenchBuilder, mode: &str) -> String {
    b.offboard("dmm_handheld", "DMM1", 52.0, 2.0, &[("mode", t(mode))])
}

// ---------------------------------------------------------------- Track 1 · Electricity

fn what_is_a_circuit(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "What is a circuit?", Mode::Playground);
    b.project.description =
        "A USB supply, a resistor and an LED — one wire short of a circuit.".into();
    let ps = b.offboard("usb_power", "PS1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 16.0, 2.0);
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
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
    b.finish()
}

fn volts_amps_ohms(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Volts, amps, ohms");
    b.project.description =
        "5 V across a 1 kΩ resistor, and a handheld meter to look at it.".into();
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(1000.0))],
    );
    b.wire((&bb, "a5"), (&bb, "t+3"), "red");
    b.wire((&bb, "b9"), (&bb, "t-8"), "black");
    let m = dmm(&mut b, "off");
    b.wire((&m, "VO"), (&bb, "d5"), "red");
    b.wire((&m, "COM"), (&bb, "t-12"), "black");
    b.finish()
}

fn ohms_law_led(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Ohm's law with an LED");
    b.project.description =
        "5 V → R1 → red LED → meter (mA) → GND: change R1 and watch the current.".into();
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(220.0))],
    );
    b.wire((&bb, "a5"), (&bb, "t+3"), "red");
    b.two_lead("led", "D1", &bb, "d9", "d10", &[("color", t("red"))]);
    let m = dmm(&mut b, "ma");
    b.wire((&bb, "a10"), (&m, "MA"), "yellow");
    b.wire((&m, "COM"), (&bb, "t-12"), "black");
    b.finish()
}

fn series_parallel(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "Series & parallel", Mode::Playground);
    b.project.description = "1 kΩ and 2.2 kΩ in series, with an ohmmeter across both.".into();
    let bb = b.breadboard("breadboard_400", 16.0, 2.0);
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(1000.0))],
    );
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "e9",
        "e13",
        &[("resistance", n(2200.0))],
    );
    let m = dmm(&mut b, "off");
    b.wire((&m, "VO"), (&bb, "a5"), "red");
    b.wire((&m, "COM"), (&bb, "a13"), "black");
    b.finish()
}

fn voltage_dividers(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Voltage dividers");
    b.project.description =
        "R1 on top, R2 below: the meter reads the voltage in the middle.".into();
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "a5"), (&bb, "t+3"), "red");
    b.two_lead(
        "resistor",
        "R2",
        &bb,
        "e9",
        "e13",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "b13"), (&bb, "t-12"), "black");
    let m = dmm(&mut b, "dc_v");
    b.wire((&m, "VO"), (&bb, "a9"), "red");
    b.wire((&m, "COM"), (&bb, "t-16"), "black");
    b.finish()
}

fn capacitors_time(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Capacitors & time");
    b.project.description = "10 kΩ and 100 µF: one wire away from charging.".into();
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(10e3))],
    );
    b.two_lead(
        "capacitor_electrolytic",
        "C1",
        &bb,
        "d9",
        "d12",
        &[("capacitance", n(100e-6))],
    );
    b.wire((&bb, "b12"), (&bb, "t-10"), "black");
    let m = dmm(&mut b, "dc_v");
    b.wire((&m, "VO"), (&bb, "a9"), "red");
    b.wire((&m, "COM"), (&bb, "t-16"), "black");
    b.finish()
}

fn buttons_pullups(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Buttons & pull-ups");
    b.project.description = "A 10 kΩ pull-up and a push button to ground.".into();
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "a5"), (&bb, "t+3"), "red");
    b.rigid("push_button", "SW1", &bb, "1", "e9", 90.0, &[]);
    b.wire((&bb, "j9"), (&bb, "b-8"), "black");
    b.wire((&bb, "b-2"), (&bb, "t-2"), "black");
    let m = dmm(&mut b, "dc_v");
    b.wire((&m, "VO"), (&bb, "a9"), "red");
    b.wire((&m, "COM"), (&bb, "t-16"), "black");
    b.finish()
}

fn first_sketch(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "Your first sketch: blink", Mode::Playground);
    b.project.description = "An UNO, a resistor and an LED, waiting for code.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
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
    sketch(&mut b, &u1, "blink", None);
    b.finish()
}

// ---------------------------------------------------------------- Track 2 · Sensors

fn light_sensor(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Seeing light with an LDR");
    b.project.description =
        "A 10 kΩ resistor over an LDR: the middle voltage follows the light.".into();
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(10e3))],
    );
    b.wire((&bb, "a5"), (&bb, "t+3"), "red");
    b.two_lead("ldr", "LDR1", &bb, "e9", "e13", &[]);
    b.wire((&bb, "b13"), (&bb, "t-12"), "black");
    let m = dmm(&mut b, "dc_v");
    b.wire((&m, "VO"), (&bb, "a9"), "red");
    b.wire((&m, "COM"), (&bb, "t-16"), "black");
    b.project.world.noise = false;
    b.finish()
}

fn temperature(lib: &Library) -> Project {
    let (mut b, _ps, bb) = powered(lib, "Measuring temperature");
    b.project.description = "A TMP36 on 5 V; the meter reads its output.".into();
    b.rigid("tmp36", "U1", &bb, "VS", "e5", 0.0, &[("tau", n(0.5))]);
    let (vs, vo, gnd) = (
        pin_col(&b, "u1", "VS"),
        pin_col(&b, "u1", "VOUT"),
        pin_col(&b, "u1", "GND"),
    );
    b.wire((&bb, &format!("a{vs}")), (&bb, "t+3"), "red");
    b.wire((&bb, &format!("a{gnd}")), (&bb, "t-10"), "black");
    let m = dmm(&mut b, "dc_v");
    b.wire((&m, "VO"), (&bb, &format!("b{vo}")), "yellow");
    b.wire((&m, "COM"), (&bb, "t-16"), "black");
    b.project.world.noise = false;
    b.finish()
}

/// Breadboard column a rigid part's pin landed in.
fn pin_col(b: &BenchBuilder, part: &str, pin: &str) -> u32 {
    let lib = Library::builtin();
    let p = b.project.part(part).unwrap();
    let def = lib.get(&p.part).unwrap();
    let lead = def.bench.leads.iter().find(|l| l.pin == pin).unwrap();
    let (x, _) = p.bench.as_ref().unwrap().at.apply(lead.x, lead.y);
    let bbp = b.project.part("bb1").unwrap();
    let bbx = bbp.bench.as_ref().unwrap().at.x;
    let bdef = lib.get(&bbp.part).unwrap();
    // hole a1's x is the origin of column numbering
    let a1 = bdef.bench.leads.iter().find(|l| l.pin == "a1").unwrap().x + bbx;
    let a2 = bdef.bench.leads.iter().find(|l| l.pin == "a2").unwrap().x + bbx;
    ((x - a1) / (a2 - a1)).round() as u32 + 1
}

// ---------------------------------------------------------------- Track 3 · Microcontrollers

fn meet_the_uno(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "Meet the UNO", Mode::Playground);
    b.project.description = "An Arduino UNO on USB and a meter on its power pins.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let m = dmm(&mut b, "off");
    b.wire((&m, "VO"), (&u1, "5V"), "red");
    b.wire((&m, "COM"), (&u1, "GND"), "black");
    b.finish()
}

fn led_on_pin(lib: &Library, name: &str, pin: &str, sk: &str, meter: bool) -> Project {
    let mut b = BenchBuilder::new(lib, name, Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    b.wire((&u1, pin), (&bb, "a5"), "orange");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(220.0))],
    );
    b.two_lead("led", "D1", &bb, "d9", "d10", &[("color", t("green"))]);
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    if meter {
        let m = dmm(&mut b, "dc_v");
        b.wire((&m, "VO"), (&bb, "b5"), "red");
        b.wire((&m, "COM"), (&bb, "t-16"), "black");
    }
    sketch(&mut b, &u1, sk, None);
    b.finish()
}

fn serial_pot(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "Talking over serial", Mode::Playground);
    b.project.description = "A knob on A0; the UNO prints what it reads.".into();
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "5V"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    b.rigid(
        "potentiometer",
        "POT1",
        &bb,
        "1",
        "e5",
        0.0,
        &[("position", n(0.5))],
    );
    let (c1, cw, c3) = (
        pin_col(&b, "pot1", "1"),
        pin_col(&b, "pot1", "w"),
        pin_col(&b, "pot1", "3"),
    );
    b.wire((&bb, &format!("a{c1}")), (&bb, "t-4"), "black");
    b.wire((&bb, &format!("a{c3}")), (&bb, "t+8"), "red");
    b.wire((&bb, &format!("a{cw}")), (&u1, "A0"), "yellow");
    sketch(&mut b, &u1, "serial_pot", None);
    b.finish()
}

fn sketch(b: &mut BenchBuilder, part: &str, name: &str, image: Option<&str>) {
    b.project.part_mut(part).unwrap().firmware = Some(FirmwareRef {
        sketch: format!("sketches/{name}"),
        fqbn: None,
        image: image.map(String::from),
    });
}

// ---------------------------------------------------------------- Track 4 · Radios

fn lora_node(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "LoRa: kilometres on a coin cell", Mode::Playground);
    b.project.description =
        "An ESP32 talks to a RYLR896 LoRa module over Serial2; a gateway forwards to MQTT.".into();
    let u1 = b.offboard("esp32_devkit", "U1", 0.0, 0.0, &[]);
    let m = b.offboard("rylr896", "U2", 20.0, 0.0, &[]);
    b.wire((&u1, "3V3"), (&m, "VDD"), "red");
    b.wire((&u1, "GND"), (&m, "GND"), "black");
    b.wire((&u1, "TX2"), (&m, "RXD"), "blue");
    b.wire((&u1, "RX2"), (&m, "TXD"), "yellow");
    let mut p = b.finish();
    p.part_mut("u1").unwrap().firmware = Some(FirmwareRef {
        sketch: "sketches/lora_node".into(),
        fqbn: Some("breadbox:host:esp32".into()),
        image: None,
    });
    p
}

// ---------------------------------------------------------------- Track 5 · SPICE

fn pro_divider(lib: &Library) -> Project {
    let mut b = BenchBuilder::new(lib, "The operating point", Mode::Pro);
    b.project.description = "V1 5 V, R1 10 kΩ over R2 4.7 kΩ: what is V(OUT)?".into();
    let v1 = b.schematic_only("vsource", "V1", 10.0, 14.0, 0.0, &[("spec", t("DC 5"))]);
    let r1 = b.schematic_only("resistor", "R1", 18.0, 8.0, 0.0, &[("resistance", n(10e3))]);
    let r2 = b.schematic_only(
        "resistor",
        "R2",
        26.0,
        14.0,
        90.0,
        &[("resistance", n(4.7e3))],
    );
    let g1 = b.schematic_only("gnd", "#GND1", 10.0, 20.0, 0.0, &[]);
    let g2 = b.schematic_only("gnd", "#GND2", 26.0, 20.0, 0.0, &[]);
    let (vp, vn) = (b.anchor(&v1, "plus"), b.anchor(&v1, "minus"));
    let (ra, rb) = (b.anchor(&r1, "1"), b.anchor(&r1, "2"));
    let (ca, cb) = (b.anchor(&r2, "1"), b.anchor(&r2, "2"));
    let (g1a, g2a) = (b.anchor(&g1, "0"), b.anchor(&g2, "0"));
    b.sch_wire(vec![vp, (vp.0, ra.1), ra]);
    b.sch_wire(vec![rb, (ca.0, rb.1), ca]);
    b.sch_wire(vec![vn, g1a]);
    b.sch_wire(vec![cb, g2a]);
    for (id, name, x, y) in [("nl_in", "IN", vp.0, ra.1), ("nl_out", "OUT", ca.0, rb.1)] {
        b.project.schematic.labels.push(NetLabel {
            id: id.into(),
            name: name.into(),
            x,
            y,
            rot: 0.0,
        });
    }
    b.project.spice.directives = ".op\n".into();
    b.finish()
}

// ---------------------------------------------------------------- snapshots of examples

/// Copies `examples/<ex>/project.json` into the track folder, its sketches to
/// `sketches/<name>/` and its prebuilt AVR image to `firmware/`, rewriting the paths.
fn snapshot(lib: &Library, ex: &str, track: &Path, title: &str) -> Project {
    let dir = root().join("examples").join(ex);
    let mut p = Project::load(&dir).unwrap_or_else(|e| panic!("{ex}: {e}"));
    p.name = title.into();
    for part in &mut p.parts {
        let Some(fw) = &mut part.firmware else {
            continue;
        };
        let src = dir.join(&fw.sketch);
        let name = src.file_name().unwrap().to_string_lossy().to_string();
        copy_dir(&src, &track.join("sketches").join(&name));
        fw.sketch = format!("sketches/{name}");
        if let Some(img) = &fw.image {
            let from = dir.join(img);
            let file = from.file_name().unwrap().to_string_lossy().to_string();
            std::fs::create_dir_all(track.join("firmware")).unwrap();
            std::fs::copy(&from, track.join("firmware").join(&file)).unwrap();
            fw.image = Some(format!("firmware/{file}"));
        }
    }
    let problems = p.check(lib);
    assert!(problems.is_empty(), "{ex}: {problems:?}");
    p
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// Builds one start circuit.
type Maker<'a> = Box<dyn Fn() -> Project + 'a>;

fn main() {
    let host = bb_plugin::PluginHost::from_dirs(
        &[
            root().join("plugins/dmm-handheld"),
            root().join("plugins/lora-gateway"),
        ],
        None,
    );
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let lib = host.library();
    let lessons = root().join("lessons");
    let only: Vec<String> = std::env::args().skip(1).collect();
    let t1 = lessons.join("1-electricity");
    let t2 = lessons.join("2-sensors");
    let t3 = lessons.join("3-microcontrollers");
    let t4 = lessons.join("4-radios");
    let t5 = lessons.join("5-spice");
    let all: Vec<(&Path, &str, Maker)> = vec![
        (
            &t1,
            "what-is-a-circuit",
            Box::new(|| what_is_a_circuit(&lib)),
        ),
        (&t1, "volts-amps-ohms", Box::new(|| volts_amps_ohms(&lib))),
        (&t1, "ohms-law-led", Box::new(|| ohms_law_led(&lib))),
        (&t1, "series-parallel", Box::new(|| series_parallel(&lib))),
        (&t1, "voltage-dividers", Box::new(|| voltage_dividers(&lib))),
        (&t1, "capacitors-time", Box::new(|| capacitors_time(&lib))),
        (&t1, "buttons-pullups", Box::new(|| buttons_pullups(&lib))),
        (&t1, "first-sketch-blink", Box::new(|| first_sketch(&lib))),
        (&t2, "light-sensor", Box::new(|| light_sensor(&lib))),
        (&t2, "temperature", Box::new(|| temperature(&lib))),
        (
            &t2,
            "night-light",
            Box::new(|| {
                let mut p = snapshot(&lib, "night-light", &t2, "A night light that thinks");
                p.world.noise = false;
                p
            }),
        ),
        (
            &t2,
            "thermometer",
            Box::new(|| {
                let mut p = snapshot(&lib, "thermometer", &t2, "A thermometer that talks");
                // a quick sensor so the lesson doesn't wait 40 s for the TMP36 to warm up
                p.part_mut("u2").unwrap().props.insert("tau".into(), n(1.0));
                p.world.noise = false;
                p
            }),
        ),
        (&t3, "meet-the-uno", Box::new(|| meet_the_uno(&lib))),
        (
            &t3,
            "digital-outputs",
            Box::new(|| led_on_pin(&lib, "Digital outputs", "D8", "fast_blink", false)),
        ),
        (
            &t3,
            "pwm-dimming",
            Box::new(|| led_on_pin(&lib, "PWM: dimming an LED", "D9", "dim", true)),
        ),
        (&t3, "serial-monitor", Box::new(|| serial_pot(&lib))),
        (
            &t4,
            "ble-button",
            Box::new(|| snapshot(&lib, "ble-button", &t4, "Bluetooth: a button on your phone")),
        ),
        (
            &t4,
            "wifi-mqtt",
            Box::new(|| snapshot(&lib, "garden-node", &t4, "Wi-Fi and MQTT")),
        ),
        (
            &t4,
            "esp32-weather",
            Box::new(|| snapshot(&lib, "esp32-weather", &t4, "An ESP32 weather station")),
        ),
        (&t4, "lora", Box::new(|| lora_node(&lib))),
        (&t5, "operating-point", Box::new(|| pro_divider(&lib))),
        (
            &t5,
            "rc-transient",
            Box::new(|| snapshot(&lib, "rc-filter-bode", &t5, "Transient: an RC step")),
        ),
        (
            &t5,
            "ac-bode",
            Box::new(|| snapshot(&lib, "rc-filter-bode", &t5, "AC sweep: the −3 dB point")),
        ),
        (
            &t5,
            "opamp-gain",
            Box::new(|| snapshot(&lib, "op-amp-amplifier", &t5, "Op-amp gain")),
        ),
    ];
    // sketches that come from elsewhere in the repository
    copy_dir(
        &root().join("crates/bb-plugin/tests/firmware/lora_node"),
        &t4.join("sketches/lora_node"),
    );
    copy_dir(
        &root().join("examples/blink/firmware/U1/blink"),
        &t1.join("sketches/blink"),
    );
    std::fs::create_dir_all(t1.join("firmware")).unwrap();
    std::fs::copy(
        root().join("examples/fixtures/blink.uno.hex"),
        t1.join("firmware/blink.uno.hex"),
    )
    .unwrap();
    for (dir, name, make) in all {
        if !only.is_empty() && !only.iter().any(|o| name.contains(o.as_str())) {
            continue;
        }
        let p = make();
        let problems = p.check(&lib);
        assert!(problems.is_empty(), "{name}: {problems:?}");
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join(format!("{name}.bench.json"));
        std::fs::write(&file, serde_json::to_string_pretty(&p).unwrap() + "\n").unwrap();
        println!("wrote {}", file.display());
    }
}
