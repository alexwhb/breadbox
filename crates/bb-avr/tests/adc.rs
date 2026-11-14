//! ADC on real firmware: codes, conversion timing, sample protocol, references, auto-trigger.

mod common;
use bb_avr::{Avr, AvrConfig, Mcu, Pin, StopOn, StopReason};
use common::*;

fn a(i: u8) -> Pin {
    Pin::new('C', i)
}

fn volts() -> Volts {
    let mut v = Volts::default();
    for (i, x) in [0.0, 1.0, 2.5, 3.3, 4.99, 5.0].iter().enumerate() {
        v.map.insert(a(i as u8), *x);
    }
    v
}

fn field(line: &str, key: &str) -> i64 {
    let i = line.find(&format!("{key}=")).unwrap() + key.len() + 1;
    let rest = &line[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().unwrap()
}

#[test]
fn analog_read_codes_and_conversion_timing() {
    let mut avr = load("adc_read", "uno");
    let mut io = volts();
    let mut ev = Vec::new();
    run_until(&mut avr, ms(300.0), &mut io, &mut ev);
    let out = usart_text(&mut avr, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() >= 4, "{out}");
    // floor(V · 1024 / 5 V), clamped to 1023.
    let expect = [0, 204, 512, 675, 1021, 1023];
    for (n, line) in lines.iter().enumerate() {
        for (i, e) in expect.iter().enumerate() {
            assert_eq!(field(line, &format!("A{i}")), *e, "{line}");
        }
        // One analogRead: start aligned to the 125 kHz ADC clock + 13 ADC clocks (104 µs); the
        // very first conversion after ADEN takes 25 ADC clocks (200 µs).
        let dt = field(line, "dt");
        let range = if n == 0 { 200..=216 } else { 104..=120 };
        assert!(range.contains(&dt), "analogRead #{n} took {dt} µs");
    }
    // Sample-and-hold instants all share one phase of the ADC clock (prescaler 128), and
    // back-to-back conversions are 13 ADC clocks apart plus start alignment.
    let s: Vec<u64> = io.samples.iter().map(|x| x.1).collect();
    for w in s.windows(2) {
        assert_eq!((w[1] - w[0]) % 128, 0, "S/H off the ADC clock grid");
    }
    let burst: Vec<u64> = io.samples.iter().skip(1).take(5).map(|x| x.1).collect();
    for w in burst.windows(2) {
        let d = w[1] - w[0];
        assert!(
            (1664..=1664 + 2 * 128).contains(&d),
            "conversion spacing {d}"
        );
    }
    assert_eq!(io.samples[0].0, a(0));
    assert_eq!(io.samples[1].0, a(1));
}

#[test]
fn adc_sample_needed_protocol() {
    let mut cfg = AvrConfig::new(Mcu::Atmega328p);
    cfg.stop_on = StopOn {
        pin_change: false,
        ..StopOn::default()
    };
    let mut avr = Avr::new(cfg);
    avr.load_hex(&std::fs::read_to_string(fixture_path("adc_read", "uno", "hex")).unwrap())
        .unwrap();
    let mut requests = Vec::new();
    while avr.cycle() < ms(120.0) {
        match avr.run(ms(120.0), &mut ()) {
            StopReason::AdcSampleNeeded { pin, cycle } => {
                assert!(cycle <= avr.cycle(), "sample instant is in the past");
                assert!(
                    avr.cycle() - cycle < 8,
                    "core stopped right at the S/H instant"
                );
                // Running again without supplying repeats the request.
                assert_eq!(
                    avr.run(ms(120.0), &mut ()),
                    StopReason::AdcSampleNeeded { pin, cycle }
                );
                requests.push(pin);
                avr.supply_adc_sample(pin.bit as f64 * 0.5);
            }
            StopReason::ReachedCycle => break,
            other => panic!("{other:?}"),
        }
    }
    assert!(requests.len() >= 12);
    let out = usart_text(&mut avr, 0);
    let line = out.lines().next().unwrap();
    // 0.5 V steps → floor(k · 0.5 · 204.8)
    for (i, e) in [0, 102, 204, 307, 409, 512].iter().enumerate() {
        assert_eq!(field(line, &format!("A{i}")), *e, "{line}");
    }
}

#[test]
fn adc_falls_back_to_pushed_pin_voltages() {
    let mut avr = load("adc_read", "uno");
    for i in 0..6u8 {
        avr.set_pin_voltage(a(i), 4.0 - i as f64 * 0.5);
    }
    let mut ev = Vec::new();
    run_until(&mut avr, ms(80.0), &mut (), &mut ev);
    let out = usart_text(&mut avr, 0);
    let line = out.lines().next().unwrap();
    for i in 0..6 {
        let v = 4.0 - i as f64 * 0.5;
        assert_eq!(
            field(line, &format!("A{i}")),
            (v * 1024.0 / 5.0).floor() as i64
        );
    }
}

#[test]
fn adc_references_temperature_and_auto_trigger() {
    let mut avr = load("adc_regs", "uno");
    avr.set_aref(2.5);
    avr.set_temperature(25.0);
    let mut io = Volts::default();
    io.map.insert(a(0), 0.55);
    io.map.insert(a(1), 1.25);
    io.map.insert(a(2), 2.5);
    io.map.insert(a(3), 1.0);
    let mut ev = Vec::new();
    run_until(&mut avr, ms(1_700.0), &mut io, &mut ev);
    let out = usart_text(&mut avr, 0);
    let get = |k: &str| -> i64 {
        field(
            out.lines()
                .find(|l| l.starts_with(k))
                .unwrap_or_else(|| panic!("{k} in {out}")),
            k,
        )
    };
    assert_eq!(get("int1v1_a0"), 512, "0.55 V against the 1.1 V reference");
    assert_eq!(get("temp"), 292, "0.314 V at 25 °C");
    assert_eq!(get("bandgap"), 225, "1.1 V against AVcc");
    assert_eq!(get("gnd"), 0);
    assert_eq!(get("aref_a1"), 512, "1.25 V against AREF = 2.5 V");
    assert_eq!(get("adlar_h"), 128, "left-adjusted 512 → ADCH");
    // Free running at 125 kHz: 13 ADC clocks per conversion → 9615.4 / s.
    let free = get("free");
    assert!(
        (9_600..=9_630).contains(&free),
        "free-running conversions {free}"
    );
    // Auto-trigger on Timer0 overflow (1.024 ms) for 500 ms.
    let t0 = get("t0ovf");
    assert!(
        (486..=490).contains(&t0),
        "timer0 triggered conversions {t0}"
    );
}

#[test]
fn temperature_sensor_follows_die_temperature() {
    let mut avr = load("adc_regs", "uno");
    avr.set_temperature(85.0);
    let mut io = Volts::default();
    let mut ev = Vec::new();
    run_until(&mut avr, ms(40.0), &mut io, &mut ev);
    let out = usart_text(&mut avr, 0);
    let line = out.lines().find(|l| l.starts_with("temp")).unwrap();
    assert_eq!(field(line, "temp"), 353, "0.380 V at 85 °C");
}

#[test]
fn adc_noise_is_seeded_and_deterministic() {
    let run = |seed: u64| -> String {
        let mut cfg = AvrConfig::new(Mcu::Atmega328p);
        cfg.stop_on = StopOn {
            pin_change: false,
            ..StopOn::default()
        };
        cfg.adc_noise_lsb = 2.0;
        cfg.seed = seed;
        let mut avr = Avr::new(cfg);
        avr.load_hex(&std::fs::read_to_string(fixture_path("adc_read", "uno", "hex")).unwrap())
            .unwrap();
        let mut io = volts();
        let mut ev = Vec::new();
        run_until(&mut avr, ms(400.0), &mut io, &mut ev);
        usart_text(&mut avr, 0)
    };
    let a1 = run(7);
    let a2 = run(7);
    let b = run(8);
    assert_eq!(a1, a2, "same seed → identical run");
    assert_ne!(a1, b, "different seed → different noise");
    // Codes stay near the ideal value.
    for line in a1.lines() {
        assert!((field(line, "A2") - 512).abs() <= 10, "{line}");
    }
}
