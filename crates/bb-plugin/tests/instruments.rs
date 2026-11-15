//! First-party instrument plugins end to end (PLUG-5): the compiled wasm modules on real
//! benches, read through the public engine API (readings, panel state).

mod common;
use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::{Endpoint, Mode, Wire};
use bb_circuit::{Project, PropValue};
use bb_core::{SimTime, UserInput};
use bb_plugin::PluginHost;
use bb_sim::Bench;
use common::*;

fn host(dirs: &[&str]) -> PluginHost {
    let h = PluginHost::from_dirs(
        &dirs
            .iter()
            .map(|d| root().join("plugins").join(d))
            .collect::<Vec<_>>(),
        None,
    );
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    h
}

fn wire(p: &mut Project, id: &str, a: (&str, &str), b: (&str, &str)) {
    p.wires.retain(|w| w.id != id);
    p.wires.push(Wire {
        id: id.into(),
        a: Endpoint::pin(a.0, a.1),
        b: Endpoint::pin(b.0, b.1),
        color: "green".into(),
        bends: vec![],
    });
}

fn reading(b: &Bench, part: &str, key: &str) -> f64 {
    b.readings(part)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|x| x.1)
        .unwrap_or(f64::NAN)
}

fn panel(b: &mut Bench, part: &str) -> serde_json::Value {
    b.telemetry()
        .parts
        .iter()
        .find(|p| p.part == part)
        .and_then(|p| p.panel.clone())
        .expect("panel")
}

/// garden-node plus an instrument part `id` of `def`.
fn garden_with(
    h: &PluginHost,
    id: &str,
    def: &str,
    props: &[(&str, PropValue)],
) -> (Bench, Project) {
    let dir = root().join("examples/garden-node");
    let mut p = Project::load(&dir).unwrap();
    let mut inst =
        bb_circuit::project::PartInstance::new(id, &id.to_ascii_uppercase(), def).at(80.0, 10.0);
    for (k, v) in props {
        inst.props.insert(k.to_string(), v.clone());
    }
    p.parts.push(inst);
    let opts = {
        let mut o = h.bench_options();
        o.firmware = bb_sim::firmware::project_images(&p, &dir);
        o
    };
    (Bench::new(p.clone(), h.library(), opts).unwrap(), p)
}

#[test]
fn scope_triggers_on_the_d9_pwm_and_its_duty_tracks_the_light() {
    let h = host(&["scope-4ch"]);
    let (mut b, mut p) = garden_with(&h, "scope2", "scope_4ch", &[]);
    wire(&mut p, "ws1", ("scope2", "CH1"), ("u1", "D9"));
    wire(&mut p, "ws2", ("scope2", "GND"), ("bb1", "t-1"));
    b.update_project(p).unwrap();
    b.run_until(SimTime::from_secs(3)).unwrap();
    assert_eq!(reading(&b, "scope2", "triggered"), 1.0);
    let f = reading(&b, "scope2", "frequency");
    assert!((f - 490.2).abs() < 2.0, "f {f}");
    let d0 = reading(&b, "scope2", "duty");
    assert!((d0 - 0.475).abs() < 0.03, "duty {d0}");
    let pnl = panel(&mut b, "scope2");
    let trace = &pnl["screens"]["screen"]["traces"][0];
    assert_eq!(trace["channel"], "CH1");
    assert!(trace["points"].as_array().unwrap().len() > 20);
    assert!(
        pnl["screens"]["screen"]["header"]
            .as_str()
            .unwrap()
            .contains("Trig'd"),
        "{}",
        pnl["screens"]["screen"]["header"]
    );
    // brighter room → the sketch dims the night LED: the duty follows
    b.set_world("light.ambient_lux", 900.0);
    b.run_until(SimTime::from_secs(6)).unwrap();
    let d1 = reading(&b, "scope2", "duty");
    assert!(d1 < d0 - 0.1, "duty {d0} → {d1}");
    assert!((reading(&b, "scope2", "frequency") - 490.2).abs() < 2.0);
    // single-shot freezes the picture
    b.input(
        "scope2",
        &UserInput::Value {
            key: "single".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(200)).unwrap();
    assert_eq!(reading(&b, "scope2", "frozen"), 1.0);
    // a mis-adjusted 10× probe overshoots on the edges
    b.input(
        "scope2",
        &UserInput::Value {
            key: "trig_mode".into(),
            value: 0.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(200)).unwrap();
    let vmax = reading(&b, "scope2", "vmax");
    b.input(
        "scope2",
        &UserInput::Value {
            key: "comp1".into(),
            value: 0.5,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(200)).unwrap();
    let vmax_bad = reading(&b, "scope2", "vmax");
    assert!(vmax_bad > vmax + 1.0, "overshoot {vmax} → {vmax_bad}");
    // FFT: the peak sits at the PWM fundamental
    b.input(
        "scope2",
        &UserInput::Value {
            key: "comp1".into(),
            value: 0.0,
        },
    )
    .unwrap();
    b.input(
        "scope2",
        &UserInput::Value {
            key: "timebase".into(),
            value: 5e-3,
        },
    )
    .unwrap();
    b.input(
        "scope2",
        &UserInput::Value {
            key: "fft".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(300)).unwrap();
    let peak = reading(&b, "scope2", "fft_peak");
    assert!(
        (peak - 490.0).abs() < 25.0,
        "FFT peak {peak}: {}",
        panel(&mut b, "scope2")["readouts"]
    );
}

#[test]
fn logic_analyzer_decodes_the_softwareserial_bytes_to_the_esp01() {
    let h = host(&["logic-analyzer"]);
    let (mut b, mut p) = garden_with(
        &h,
        "la1",
        "logic_analyzer",
        &[(
            "decoders",
            PropValue::Text("uart:D0@9600, uart:D1@9600".into()),
        )],
    );
    // D0 on the sketch's SoftwareSerial TX (D2 → ESP RX), D1 on the ESP's answers (D3)
    wire(&mut p, "wl1", ("la1", "D0"), ("u1", "D2"));
    wire(&mut p, "wl2", ("la1", "D1"), ("u1", "D3"));
    wire(&mut p, "wl3", ("la1", "GND"), ("bb1", "t-1"));
    b.update_project(p).unwrap();
    b.run_until(SimTime::from_secs(5)).unwrap();
    let pnl = panel(&mut b, "la1");
    let rows: Vec<(String, String)> = serde_json::from_value(pnl["readouts"].clone()).unwrap();
    let tx = &rows
        .iter()
        .find(|r| r.0 == "UART D0 @ 9600")
        .expect("D0 decoder")
        .1;
    let rx = &rows
        .iter()
        .find(|r| r.0 == "UART D1 @ 9600")
        .expect("D1 decoder")
        .1;
    assert!(
        tx.contains("AT+CWJAP=\"SIM-AP\",\"breadbox\"\r\n"),
        "{tx:?}"
    );
    assert!(
        tx.contains("AT+CIPSTART=\"TCP\",\"10.0.0.1\",1883"),
        "{tx:?}"
    );
    assert!(rx.contains("WIFI GOT IP"), "{rx:?}");
    // byte for byte what the modem received
    let sent = b.radio().unwrap().uart_sent_text("u2");
    let n = sent.len().min(tx.len()).min(200);
    assert_eq!(&tx[..n], &sent[..n]);
    assert!(reading(&b, "la1", "edges") > 1000.0);
    b.input(
        "la1",
        &UserInput::Value {
            key: "window".into(),
            value: 2.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(100)).unwrap();
    let pnl = panel(&mut b, "la1");
    let markers = pnl["screens"]["screen"]["markers"].as_array().unwrap();
    assert!(!markers.is_empty());
}

fn psu_bench(h: &PluginHost, load_ohms: f64) -> Bench {
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "psu", Mode::Playground);
    let psu = bb.offboard(
        "bench_psu",
        "PSU1",
        0.0,
        0.0,
        &[
            ("voltage", PropValue::Num(5.0)),
            ("current_limit", PropValue::Num(1.0)),
        ],
    );
    let r = bb.offboard(
        "resistor",
        "R1",
        30.0,
        0.0,
        &[
            ("resistance", PropValue::Num(load_ohms)),
            ("power", PropValue::Num(100.0)),
        ],
    );
    bb.wire((&psu, "POS"), (&r, "1"), "red");
    bb.wire((&psu, "NEG"), (&r, "2"), "black");
    Bench::new(bb.finish(), lib, h.bench_options()).unwrap()
}

#[test]
fn psu_regulates_limits_and_folds_back_on_a_short() {
    let h = host(&["bench-psu"]);
    // CV into 10 Ω: 5 V minus the 50 mV/A sag at 0.5 A
    let mut b = psu_bench(&h, 10.0);
    b.run_until(SimTime::from_ms(200)).unwrap();
    let (v, i) = (
        reading(&b, "psu1", "voltage"),
        reading(&b, "psu1", "current"),
    );
    assert!((v - 4.975).abs() < 0.02, "CV {v} V");
    assert!((i - v / 10.0).abs() < 0.005, "{i} A");
    assert_eq!(reading(&b, "psu1", "cc"), 0.0);
    // 2 Ω would need 2.5 A: constant current at the 1 A limit
    let mut b = psu_bench(&h, 2.0);
    b.run_until(SimTime::from_ms(200)).unwrap();
    assert!((reading(&b, "psu1", "current") - 1.0).abs() < 0.02);
    assert!((reading(&b, "psu1", "voltage") - 2.0).abs() < 0.05);
    assert_eq!(reading(&b, "psu1", "cc"), 1.0);
    assert_eq!(reading(&b, "psu1", "folded"), 0.0);
    // a dead short folds the limit back to 10 %
    let mut b = psu_bench(&h, 0.01);
    b.run_until(SimTime::from_ms(200)).unwrap();
    assert_eq!(reading(&b, "psu1", "folded"), 1.0);
    let i = reading(&b, "psu1", "current");
    assert!((i - 0.1).abs() < 0.01, "foldback current {i}");
    let pnl = panel(&mut b, "psu1");
    assert_eq!(pnl["leds"]["cc"], 1.0);
    assert!(
        pnl["displays"]["amps"]["annunciators"]
            .to_string()
            .contains("FOLD")
    );
    // foldback off: the short gets the full limit
    b.input(
        "psu1",
        &UserInput::Value {
            key: "foldback".into(),
            value: 0.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(100)).unwrap();
    assert!((reading(&b, "psu1", "current") - 1.0).abs() < 0.02);
    // output off
    b.input(
        "psu1",
        &UserInput::Value {
            key: "output".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(50)).unwrap();
    assert!(reading(&b, "psu1", "current").abs() < 1e-6);
}

/// Fits A·sin(ωt + φ) + c to samples: (A, φ in degrees).
fn fit(samples: &[(f64, f64)], f: f64) -> (f64, f64) {
    let w = 2.0 * std::f64::consts::PI * f;
    let n = samples.len() as f64;
    let (mut s, mut c) = (0.0, 0.0);
    let mean = samples.iter().map(|x| x.1).sum::<f64>() / n;
    for (t, v) in samples {
        s += (v - mean) * (w * t).sin();
        c += (v - mean) * (w * t).cos();
    }
    let (a, b) = (2.0 * s / n, 2.0 * c / n);
    ((a * a + b * b).sqrt(), b.atan2(a).to_degrees())
}

#[test]
fn function_generator_into_an_rc_filter_matches_spice() {
    let h = host(&["function-generator"]);
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "rc", Mode::Playground);
    let fg = bb.offboard(
        "function_generator",
        "FG1",
        0.0,
        0.0,
        &[
            ("frequency", PropValue::Num(1000.0)),
            ("amplitude", PropValue::Num(2.0)),
        ],
    );
    let r = bb.offboard(
        "resistor",
        "R1",
        30.0,
        0.0,
        &[("resistance", PropValue::Num(1000.0))],
    );
    let c = bb.offboard(
        "capacitor",
        "C1",
        40.0,
        0.0,
        &[("capacitance", PropValue::Num(100e-9))],
    );
    bb.wire((&fg, "OUT"), (&r, "1"), "red");
    let cpins: Vec<String> = lib
        .get("capacitor")
        .unwrap()
        .pins
        .iter()
        .map(|p| p.id.clone())
        .collect();
    bb.wire((&r, "2"), (&c, &cpins[0]), "yellow");
    bb.wire((&c, &cpins[1]), (&fg, "COM"), "black");
    let mut b = Bench::new(bb.finish(), lib, h.bench_options()).unwrap();
    b.run_until(SimTime::from_ms(10)).unwrap();
    let mut bench = Vec::new();
    for k in 0..1000 {
        b.run_until(SimTime::from_ms(10) + SimTime::from_us(10 * k))
            .unwrap();
        bench.push((b.time().as_secs_f64(), b.pin_voltage("r1", "2").unwrap()));
    }
    // the same circuit as a plain SPICE transient
    let nl = bb_spice::Netlist::parse(
        "fg into rc\nV1 g 0 SIN(0 1 1000)\nRO g a 50\nR1 a out 1k\nC1 out 0 100n\n.tran 1u 20m 0 1u\n.end\n",
    )
    .unwrap();
    let res = nl.run().unwrap();
    let plot = res.runs[0].plot(bb_spice::results::PlotKind::Tran).unwrap();
    let t = plot.vectors[0].reals();
    let v = plot
        .vectors
        .iter()
        .find(|x| x.name == "v(out)")
        .unwrap()
        .reals();
    let spice: Vec<(f64, f64)> = t
        .iter()
        .zip(v.iter())
        .filter(|(t, _)| **t >= 0.01)
        .map(|(t, v)| (*t, *v))
        .collect();
    let (a_b, p_b) = fit(&bench, 1000.0);
    let (a_s, p_s) = fit(&spice, 1000.0);
    // analytic: |H| = 1/√(1+(ωRC)²), R = 1050 Ω
    let wrc = 2.0 * std::f64::consts::PI * 1000.0 * 1050.0 * 100e-9;
    let a_th = 1.0 / (1.0 + wrc * wrc).sqrt();
    assert!((a_b - a_s).abs() < 0.01 * a_s, "bench {a_b} vs SPICE {a_s}");
    assert!((a_s - a_th).abs() < 0.01 * a_th, "SPICE {a_s} vs {a_th}");
    assert!(
        (p_b - p_s).abs() < 1.0,
        "phase bench {p_b}° vs SPICE {p_s}°"
    );
    // square wave: the filtered output swings symmetrically
    b.input(
        "fg1",
        &UserInput::Value {
            key: "shape".into(),
            value: 1.0,
        },
    )
    .unwrap();
    b.run_for(SimTime::from_ms(10)).unwrap();
    let mut vmin = f64::INFINITY;
    let mut vmax = f64::NEG_INFINITY;
    for _ in 0..200 {
        b.run_for(SimTime::from_us(10)).unwrap();
        let v = b.pin_voltage("r1", "2").unwrap();
        vmin = vmin.min(v);
        vmax = vmax.max(v);
    }
    assert!(
        vmax > 0.6 && vmin < -0.6 && (vmax + vmin).abs() < 0.1,
        "{vmin}..{vmax}"
    );
    assert_eq!(reading(&b, "fg1", "shape"), 1.0);
}

// ------------------------------------------------------------------ test depth (closeout-b)

/// An UNO running bb-sim's test sketch `sketch` (fixture), plus whatever `build` adds.
fn uno_with(h: &PluginHost, sketch: &str, build: impl FnOnce(&mut BenchBuilder, &str)) -> Bench {
    let lib = h.library();
    let mut b = BenchBuilder::new(&lib, sketch, Mode::Playground);
    b.project.world.noise = false;
    let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
    build(&mut b, &u1);
    let mut opts = h.bench_options();
    let hex = root().join(format!("crates/bb-sim/tests/fixtures/{sketch}.uno.hex"));
    opts.firmware
        .insert(u1, bb_sim::firmware::load_image(&hex).unwrap());
    Bench::new(b.finish(), lib, opts).unwrap()
}

/// A decoder's readout row (`I2C D0/D1` → its text).
fn decoded(b: &mut Bench, la: &str, row: &str) -> String {
    let pnl = panel(b, la);
    let rows: Vec<(String, String)> = serde_json::from_value(pnl["readouts"].clone()).unwrap();
    rows.into_iter()
        .find(|r| r.0 == row)
        .map(|r| r.1)
        .unwrap_or_else(|| panic!("no {row} decoder: {}", pnl["readouts"]))
}

/// Runs in 10 ms steps (up to `secs`) until the decoder row contains `want`.
fn wait_decoded(b: &mut Bench, la: &str, row: &str, want: &str, secs: f64) -> String {
    let end = b.time() + SimTime::from_secs_f64(secs);
    loop {
        let text = decoded(b, la, row);
        if text.contains(want) || b.time() >= end {
            return text;
        }
        b.run_for(SimTime::from_ms(10)).unwrap();
    }
}

fn la_part(b: &mut BenchBuilder, decoders: &str) -> String {
    b.offboard(
        "logic_analyzer",
        "LA1",
        60.0,
        0.0,
        &[("decoders", PropValue::Text(decoders.into()))],
    )
}

/// PLUG-5 logic analyzer, I²C decoder on the AVR's real TWI waveforms to a BME280: address +
/// direction + ACK/NACK, register pointer, repeated START, the chip id read back, a write, and
/// a probe of an address nobody answers.
#[test]
fn logic_analyzer_decodes_i2c_to_a_bme280() {
    let h = host(&["logic-analyzer"]);
    let mut b = uno_with(&h, "la_i2c", |b, u1| {
        let s = b.offboard("bme280", "U2", 30.0, 0.0, &[]);
        b.wire((&s, "VIN"), (u1, "5V"), "red");
        b.wire((&s, "GND"), (u1, "GND"), "black");
        b.wire((&s, "SDA"), (u1, "A4"), "blue");
        b.wire((&s, "SCL"), (u1, "A5"), "yellow");
        let la = la_part(b, "i2c:D0/D1");
        b.wire((&la, "D0"), (u1, "A4"), "blue");
        b.wire((&la, "D1"), (u1, "A5"), "yellow");
        b.wire((&la, "GND"), (u1, "GND"), "black");
    });
    let cycle = "START · addr 0x76 W ACK · 0xD0 ACK · START · addr 0x76 R ACK · 0x60 NACK · STOP · \
                 START · addr 0x76 W ACK · 0xF4 ACK · 0x27 ACK · STOP · \
                 START · addr 0x40 W NACK · STOP";
    b.run_until(SimTime::from_ms(100)).unwrap();
    let text = wait_decoded(&mut b, "la1", "I2C D0/D1", cycle, 1.0);
    assert!(text.contains(cycle), "decoded: {text}");
    // the sketch saw the same bus
    let fw: Vec<String> = b
        .serial("u1")
        .unwrap()
        .data_lines()
        .map(|l| l.text.clone())
        .collect();
    assert!(fw.contains(&"id=0x60 ghost=2".to_string()), "{fw:?}");
    assert!(reading(&b, "la1", "decoded") >= 15.0);
}

/// SPI decoder on the AVR's hardware SPI (mode 0, 1 MHz) into a 74HC595 whose serial out
/// feeds MISO: MOSI and MISO bytes per transfer, framed by chip select; MISO matches what the
/// sketch read (the byte leaving the shift register).
#[test]
fn logic_analyzer_decodes_spi_mosi_and_miso() {
    let h = host(&["logic-analyzer"]);
    let mut b = uno_with(&h, "la_spi", |b, u1| {
        let s = b.offboard("hc595", "U2", 30.0, 0.0, &[]);
        b.wire((&s, "VCC"), (u1, "5V"), "red");
        b.wire((&s, "GND"), (u1, "GND"), "black");
        b.wire((&s, "MR"), (u1, "5V"), "red");
        b.wire((&s, "OE"), (u1, "GND"), "black");
        b.wire((&s, "DS"), (u1, "D11"), "blue");
        b.wire((&s, "SHCP"), (u1, "D13"), "yellow");
        b.wire((&s, "STCP"), (u1, "D10"), "green");
        b.wire((&s, "Q7S"), (u1, "D12"), "white");
        let la = la_part(b, "spi:D0/D1/D2/D3");
        b.wire((&la, "D0"), (u1, "D13"), "yellow");
        b.wire((&la, "D1"), (u1, "D11"), "blue");
        b.wire((&la, "D2"), (u1, "D12"), "white");
        b.wire((&la, "D3"), (u1, "D10"), "green");
        b.wire((&la, "GND"), (u1, "GND"), "black");
    });
    let frame = "MOSI 0x5A · MISO 0xC3 · MOSI 0xC3 · MISO 0x5A";
    b.run_until(SimTime::from_ms(100)).unwrap();
    let text = wait_decoded(&mut b, "la1", "SPI D0/D1", frame, 1.0);
    assert!(text.contains(frame), "decoded: {text}");
    let fw: Vec<String> = b
        .serial("u1")
        .unwrap()
        .data_lines()
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(fw.first().map(String::as_str), Some("rx=0,5A"), "{fw:?}");
    assert!(fw.iter().skip(1).all(|l| l == "rx=C3,5A"), "{fw:?}");
}

/// 1-Wire decoder on DallasTemperature talking to a DS18B20 (4.7 k pull-up): reset with the
/// presence pulse, Skip ROM (0xCC), Convert T (0x44), the status polling while it converts,
/// Read Scratchpad (0xBE) and the temperature bytes (19.25 °C = 0x0134 → 0x34, 0x01, LSB
/// first). Found: the polling read slots used to be packed into bytes — hundreds of `0x00`
/// items that pushed the transaction out of the readout.
#[test]
fn logic_analyzer_decodes_one_wire_to_a_ds18b20() {
    let h = host(&["logic-analyzer"]);
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "1wire", Mode::Playground);
    bb.project.world.noise = false;
    bb.project.world.air.temp_c = 19.25;
    let u1 = bb.offboard("uno", "U1", 0.0, 0.0, &[]);
    let s = bb.offboard("ds18b20", "U2", 30.0, 0.0, &[]);
    let r = bb.offboard(
        "resistor",
        "R1",
        40.0,
        0.0,
        &[("resistance", PropValue::Num(4700.0))],
    );
    bb.wire((&s, "GND"), (&u1, "GND"), "black");
    bb.wire((&s, "VDD"), (&u1, "5V"), "red");
    bb.wire((&s, "DQ"), (&u1, "D4"), "yellow");
    bb.wire((&r, "1"), (&u1, "5V"), "red");
    bb.wire((&r, "2"), (&u1, "D4"), "yellow");
    let la = la_part(&mut bb, "onewire:D0");
    bb.wire((&la, "D0"), (&u1, "D4"), "yellow");
    bb.wire((&la, "GND"), (&u1, "GND"), "black");
    let mut opts = h.bench_options();
    let hex = root().join("crates/bb-sim/tests/fixtures/ds18b20.uno.hex");
    opts.firmware
        .insert(u1.clone(), bb_sim::firmware::load_image(&hex).unwrap());
    let mut b = Bench::new(bb.finish(), lib, opts).unwrap();
    b.run_until(SimTime::from_ms(200)).unwrap();
    // every distinct readout over 3 s (it shows the last 24 items)
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..300 {
        b.run_for(SimTime::from_ms(10)).unwrap();
        let t = decoded(&mut b, "la1", "1-Wire D0");
        if seen.last() != Some(&t) {
            seen.push(t);
        }
    }
    let any = |want: &str| seen.iter().any(|t| t.contains(want));
    // requestTemperatures: Skip ROM + Convert T, then status slots until the next reset
    assert!(
        any("RESET · presence · 0xCC Skip ROM · 0x44 Convert T · busy ×"),
        "{seen:#?}"
    );
    // getTempCByIndex: Search ROM (64 bits × 3 slots), then Match ROM with the ROM code
    // (family 0x28, CRC 0x29) and Read Scratchpad: 19.25 °C = 0x0134
    assert!(any("0xF0 Search ROM · search · 192 slots"), "{seen:#?}");
    assert!(
        any(
            "RESET · presence · 0x55 Match ROM · 0x28 · 0x01 · 0x00 · 0x00 · 0x00 · 0x00 · 0x00 · 0x29 · 0xBE Read Scratchpad · 0x34 · 0x01"
        ),
        "{seen:#?}"
    );
    let busy: u32 = seen
        .iter()
        .find_map(|t| t.split("busy ×").nth(1))
        .and_then(|x| x.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert!(busy > 1000, "~750 ms of polling: {busy} slots");
    assert!(
        !any("0x00 · 0x00 · 0x00 · 0x00 · 0x00 · 0x00 · 0x00"),
        "polling slots packed into bytes: {seen:#?}"
    );
    let fw: Vec<String> = b
        .serial(&u1)
        .unwrap()
        .data_lines()
        .map(|l| l.text.clone())
        .collect();
    assert!(fw.iter().any(|l| l == "t=19.25"), "{fw:?}");
}

fn set(b: &mut Bench, part: &str, key: &str, value: f64) {
    b.input(
        part,
        &UserInput::Value {
            key: key.into(),
            value,
        },
    )
    .unwrap();
}

/// FG1 (1 kHz, 4 Vpp, +1 V offset) into a load resistor `load` Ω.
fn fgen_bench(h: &PluginHost, load: f64) -> Bench {
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "fgen", Mode::Playground);
    let fg = bb.offboard(
        "function_generator",
        "FG1",
        0.0,
        0.0,
        &[
            ("frequency", PropValue::Num(1000.0)),
            ("amplitude", PropValue::Num(4.0)),
            ("offset", PropValue::Num(1.0)),
        ],
    );
    let r = bb.offboard(
        "resistor",
        "R1",
        30.0,
        0.0,
        &[
            ("resistance", PropValue::Num(load)),
            ("power", PropValue::Num(10.0)),
        ],
    );
    bb.wire((&fg, "OUT"), (&r, "1"), "red");
    bb.wire((&r, "2"), (&fg, "COM"), "black");
    Bench::new(bb.finish(), lib, h.bench_options()).unwrap()
}

/// Samples the load voltage every `dt` for `n` samples: (t, v).
fn sample_load(b: &mut Bench, dt: SimTime, n: usize) -> Vec<(f64, f64)> {
    (0..n)
        .map(|_| {
            b.run_for(dt).unwrap();
            let v = b.pin_voltage("r1", "1").unwrap() - b.pin_voltage("r1", "2").unwrap();
            (b.time().as_secs_f64(), v)
        })
        .collect()
}

struct Stats {
    mean: f64,
    max: f64,
    min: f64,
    rms_ac: f64,
    /// Fraction of the samples above the middle of min…max.
    high: f64,
    /// From upward crossings of the middle (Hz).
    freq: f64,
}

fn stats(s: &[(f64, f64)]) -> Stats {
    let n = s.len() as f64;
    let mean = s.iter().map(|x| x.1).sum::<f64>() / n;
    let max = s.iter().map(|x| x.1).fold(f64::MIN, f64::max);
    let min = s.iter().map(|x| x.1).fold(f64::MAX, f64::min);
    let rms_ac = (s.iter().map(|x| (x.1 - mean).powi(2)).sum::<f64>() / n).sqrt();
    let mid = 0.5 * (max + min);
    let high = s.iter().filter(|x| x.1 > mid).count() as f64 / n;
    let ups: Vec<f64> = s
        .windows(2)
        .filter(|w| w[0].1 <= mid && w[1].1 > mid)
        .map(|w| w[0].0 + (w[1].0 - w[0].0) * (mid - w[0].1) / (w[1].1 - w[0].1))
        .collect();
    let freq = if ups.len() >= 2 {
        (ups.len() - 1) as f64 / (ups[ups.len() - 1] - ups[0])
    } else {
        0.0
    };
    Stats {
        mean,
        max,
        min,
        rms_ac,
        high,
        freq,
    }
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// PLUG-5 function generator, every waveform into a 50 Ω load: amplitude, offset, frequency,
/// shape (RMS / duty), the 50 Ω source (half the open-circuit voltage), output off.
#[test]
fn function_generator_every_waveform_into_a_50_ohm_load() {
    let h = host(&["function-generator"]);
    let us = SimTime::from_us;
    // the 50 Ω source: 4 Vpp + 1 V into 1 MΩ is the open-circuit voltage, into 50 Ω half of it
    let mut open = fgen_bench(&h, 1e6);
    open.run_until(SimTime::from_ms(5)).unwrap();
    let so = stats(&sample_load(&mut open, us(5), 1000));
    assert!(
        near(so.max, 3.0, 0.02) && near(so.min, -1.0, 0.02),
        "open: {}..{}",
        so.min,
        so.max
    );
    let mut b = fgen_bench(&h, 50.0);
    b.run_until(SimTime::from_ms(5)).unwrap();
    // sine: 1 V amplitude, +0.5 V offset at the load, 1 kHz
    let s = stats(&sample_load(&mut b, us(5), 1000));
    let a = (s.max - s.min) / 2.0;
    assert!(near(a, 1.0, 0.01), "sine amplitude {a}");
    let r_src = 50.0 * ((so.max - so.min) / (s.max - s.min) - 1.0);
    assert!(near(r_src, 50.0, 1.0), "source resistance {r_src} Ω");
    assert!(near(s.mean, 0.5, 0.01), "offset {}", s.mean);
    assert!(near(s.freq, 1000.0, 2.0), "f {}", s.freq);
    assert!(
        near(s.rms_ac, 1.0 / 2f64.sqrt(), 0.01),
        "sine RMS {}",
        s.rms_ac
    );
    // square: ±1 V around the offset, 50 % duty, RMS = amplitude
    set(&mut b, "fg1", "shape", 1.0);
    b.run_for(SimTime::from_ms(2)).unwrap();
    let s = stats(&sample_load(&mut b, us(5), 1000));
    assert!(
        near(s.max, 1.5, 0.01) && near(s.min, -0.5, 0.01),
        "square {}..{}",
        s.min,
        s.max
    );
    assert!(near(s.high, 0.5, 0.02), "square duty {}", s.high);
    assert!(near(s.rms_ac, 1.0, 0.02), "square RMS {}", s.rms_ac);
    assert!(near(s.freq, 1000.0, 2.0), "f {}", s.freq);
    // triangle: same peaks, RMS = amplitude / √3
    set(&mut b, "fg1", "shape", 2.0);
    b.run_for(SimTime::from_ms(2)).unwrap();
    let s = stats(&sample_load(&mut b, us(5), 1000));
    // the vertices are sharp: the solver's 10 µs steps (no breakpoints in a behavioural
    // source) round them by a few tens of mV
    assert!(
        near(s.max, 1.5, 0.03) && near(s.min, -0.5, 0.03),
        "triangle {}..{}",
        s.min,
        s.max
    );
    assert!(
        near(s.rms_ac, 1.0 / 3f64.sqrt(), 0.01),
        "triangle RMS {}",
        s.rms_ac
    );
    assert!(near(s.high, 0.5, 0.02));
    // pulse, 20 % duty at 2.5 kHz
    set(&mut b, "fg1", "shape", 3.0);
    set(&mut b, "fg1", "duty", 0.2);
    set(&mut b, "fg1", "frequency", 2500.0);
    b.run_for(SimTime::from_ms(2)).unwrap();
    let s = stats(&sample_load(&mut b, us(2), 2000));
    assert!(
        near(s.max, 1.5, 0.01) && near(s.min, -0.5, 0.01),
        "pulse {}..{}",
        s.min,
        s.max
    );
    assert!(near(s.high, 0.2, 0.02), "pulse duty {}", s.high);
    assert!(near(s.freq, 2500.0, 10.0), "f {}", s.freq);
    // noise: around the offset, σ = amplitude / 3, never past ±amplitude, not periodic
    set(&mut b, "fg1", "shape", 4.0);
    set(&mut b, "fg1", "frequency", 1000.0);
    b.run_for(SimTime::from_ms(2)).unwrap();
    let samples = sample_load(&mut b, us(25), 4000);
    let s = stats(&samples);
    assert!(near(s.mean, 0.5, 0.03), "noise mean {}", s.mean);
    assert!(near(s.rms_ac, 1.0 / 3.0, 0.05), "noise σ {}", s.rms_ac);
    assert!(
        s.max <= 1.5 + 1e-6 && s.min >= -0.5 - 1e-6,
        "{}..{}",
        s.min,
        s.max
    );
    // arbitrary at 100 Hz: the built-in 16-point "heartbeat" (peak +1 at 1/4 of the period,
    // dip -0.6 right after), then a point changed from the panel
    set(&mut b, "fg1", "shape", 5.0);
    set(&mut b, "fg1", "frequency", 100.0);
    b.run_for(SimTime::from_ms(20)).unwrap();
    let samples = sample_load(&mut b, us(25), 800);
    let s = stats(&samples);
    assert!(near(s.max, 1.5, 0.01), "arb peak {}", s.max);
    assert!(near(s.min, 0.5 - 0.6, 0.01), "arb dip {}", s.min);
    let t_peak = samples.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
    let phase = (t_peak * 100.0).fract();
    assert!(near(phase, 0.25, 0.005), "peak at phase {phase}");
    set(&mut b, "fg1", "a4", -1.0);
    b.run_for(SimTime::from_ms(10)).unwrap();
    let s = stats(&sample_load(&mut b, us(25), 800));
    assert!(near(s.min, -0.5, 0.01), "a4 = -1 → {}", s.min);
    // output off: nothing on the load (the offset too)
    set(&mut b, "fg1", "output", 0.0);
    b.run_for(SimTime::from_ms(1)).unwrap();
    let s = stats(&sample_load(&mut b, us(10), 200));
    assert!(
        s.max.abs() < 1e-3 && s.min.abs() < 1e-3,
        "output off: {}..{}",
        s.min,
        s.max
    );
    assert_eq!(reading(&b, "fg1", "output"), 0.0);
}

/// The scope's CH1 trace value (V) at screen position `x` (divisions).
fn trace_at(b: &mut Bench, scope: &str, x: f64) -> f64 {
    let pnl = panel(b, scope);
    let vdiv = pnl["controls"]["vdiv1"].as_f64().unwrap();
    let pts = pnl["screens"]["screen"]["traces"][0]["points"]
        .as_array()
        .unwrap()
        .clone();
    let y = pts
        .iter()
        .filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
        .take_while(|(px, _)| *px <= x)
        .last()
        .map(|p| p.1)
        .unwrap();
    y * vdiv
}

/// PLUG-5 4-channel scope on the function generator: DC / AC / GND coupling, the 10× probe's
/// compensation trimmer (over- and under-compensated edges; 1× unaffected), and the
/// pulse-width trigger picking the wide or the narrow pulse of an arbitrary waveform (normal
/// mode: no matching pulse → not triggered).
#[test]
fn scope_coupling_probe_compensation_and_pulse_width_trigger() {
    let h = host(&["scope-4ch", "function-generator"]);
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "scope", Mode::Playground);
    let fg = bb.offboard(
        "function_generator",
        "FG1",
        0.0,
        0.0,
        &[
            ("shape", PropValue::Text("square".into())),
            ("frequency", PropValue::Num(1000.0)),
            ("amplitude", PropValue::Num(4.0)),
            ("offset", PropValue::Num(1.0)),
        ],
    );
    let sc = bb.offboard(
        "scope_4ch",
        "SCOPE1",
        30.0,
        0.0,
        &[
            ("timebase", PropValue::Num(1e-3)),
            ("trigger_level", PropValue::Num(1.0)),
        ],
    );
    bb.wire((&sc, "CH1"), (&fg, "OUT"), "yellow");
    bb.wire((&sc, "GND"), (&fg, "COM"), "black");
    let mut b = Bench::new(bb.finish(), lib, h.bench_options()).unwrap();
    let r = |b: &Bench, k: &str| reading(b, "scope1", k);
    let ms = SimTime::from_ms;
    // DC: -1 … +3 V, average +1 V, 1 kHz (counted over the 10 periods of a 1 ms/div screen)
    b.run_until(ms(100)).unwrap();
    assert_eq!(r(&b, "triggered"), 1.0);
    assert!(near(r(&b, "vmax"), 3.0, 0.05) && near(r(&b, "vmin"), -1.0, 0.05));
    assert!(near(r(&b, "vavg"), 1.0, 0.05), "vavg {}", r(&b, "vavg"));
    assert!(
        near(r(&b, "frequency"), 1000.0, 2.0) && near(r(&b, "duty"), 0.5, 0.01),
        "f {} duty {}",
        r(&b, "frequency"),
        r(&b, "duty")
    );
    set(&mut b, "scope1", "timebase", 200e-6);
    // AC: the offset is gone, the swing stays
    set(&mut b, "scope1", "trig_level", 0.0);
    set(&mut b, "scope1", "coupling1", 1.0);
    b.run_for(ms(100)).unwrap();
    assert!(near(r(&b, "vavg"), 0.0, 0.1), "AC vavg {}", r(&b, "vavg"));
    assert!(near(r(&b, "vpp"), 4.0, 0.2), "AC vpp {}", r(&b, "vpp"));
    // GND: a flat line at 0 V
    set(&mut b, "scope1", "coupling1", 2.0);
    b.run_for(ms(100)).unwrap();
    assert!(r(&b, "vpp").abs() < 1e-9 && r(&b, "vavg").abs() < 1e-9);
    // 10× probe compensation (DC): adjusted → flat tops; over-compensated → overshoot at the
    // edges; under-compensated → rounded leading edges; a 1× probe has no trimmer
    set(&mut b, "scope1", "coupling1", 0.0);
    set(&mut b, "scope1", "vdiv1", 1.0);
    b.run_for(ms(100)).unwrap();
    let flat = trace_at(&mut b, "scope1", 5.1);
    assert!(near(flat, 3.0, 0.1), "20 µs after the edge: {flat}");
    assert!(near(r(&b, "vmax"), 3.0, 0.05));
    set(&mut b, "scope1", "comp1", 0.5);
    b.run_for(ms(100)).unwrap();
    assert!(r(&b, "vmax") > 4.5, "overshoot: vmax {}", r(&b, "vmax"));
    set(&mut b, "scope1", "comp1", -0.5);
    b.run_for(ms(100)).unwrap();
    let rounded = trace_at(&mut b, "scope1", 5.1);
    assert!(
        rounded < 2.0,
        "20 µs after the edge: {rounded} (adjusted: {flat})"
    );
    assert!(
        near(r(&b, "vmax"), 3.0, 0.1),
        "it settles: {}",
        r(&b, "vmax")
    );
    set(&mut b, "scope1", "probe1", 0.0);
    b.run_for(ms(100)).unwrap();
    assert!(near(trace_at(&mut b, "scope1", 5.1), 3.0, 0.1), "1× probe");
    set(&mut b, "scope1", "probe1", 1.0);
    set(&mut b, "scope1", "comp1", 0.0);

    // pulse-width trigger: a 100 Hz arbitrary waveform with a 2.5 ms and a 0.625 ms pulse
    let arb = [
        -1.0, 1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0, 1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0,
    ];
    for (k, a) in arb.iter().enumerate() {
        set(&mut b, "fg1", &format!("a{k}"), *a);
    }
    set(&mut b, "fg1", "offset", 0.0);
    set(&mut b, "fg1", "frequency", 100.0);
    set(&mut b, "fg1", "shape", 5.0);
    set(&mut b, "scope1", "vdiv1", 2.0);
    set(&mut b, "scope1", "timebase", 1e-3);
    set(&mut b, "scope1", "trig_level", 0.0);
    set(&mut b, "scope1", "trig_mode", 1.0); // normal
    // wider than 1 ms: the trigger (x = 5) is the wide pulse's falling edge
    set(&mut b, "scope1", "trig_pulse", 1e-3);
    b.run_for(ms(150)).unwrap();
    assert_eq!(r(&b, "triggered"), 1.0);
    let (before, just_before) = (
        trace_at(&mut b, "scope1", 3.5),
        trace_at(&mut b, "scope1", 4.8),
    );
    assert!(
        before > 1.0 && just_before > 0.3,
        "wide pulse: {before} / {just_before}"
    );
    // narrower than 1 ms: the narrow pulse's falling edge, with the line low 1.5 ms earlier
    set(&mut b, "scope1", "trig_pulse", -1e-3);
    b.run_for(ms(150)).unwrap();
    assert_eq!(r(&b, "triggered"), 1.0);
    let (before, just_before) = (
        trace_at(&mut b, "scope1", 3.5),
        trace_at(&mut b, "scope1", 4.7),
    );
    assert!(
        before < -1.0 && just_before > 1.0,
        "narrow pulse: {before} / {just_before}"
    );
    // wider than 5 ms: nothing matches — normal mode keeps the last picture, not triggered
    set(&mut b, "scope1", "trig_pulse", 5e-3);
    b.run_for(ms(150)).unwrap();
    assert_eq!(r(&b, "triggered"), 0.0, "stale trigger status");
    let pnl = panel(&mut b, "scope1");
    let header = pnl["screens"]["screen"]["header"].as_str().unwrap();
    assert!(!header.contains("Trig'd"), "{header}");
    assert!(
        !pnl["screens"]["screen"]["traces"][0]["points"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the last picture stays"
    );
}
