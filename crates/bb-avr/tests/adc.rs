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

// ------------------------------------------------------------------ WORLD-6: transfer function

/// Every channel sees a voltage that sweeps 0 → 5.4 V (past AVcc, to exercise clamping) and
/// wraps; records what each conversion was handed, in order.
#[derive(Default)]
struct Ramp {
    supplied: Vec<f64>,
}

impl bb_avr::AvrIo for Ramp {
    fn adc_sample(&mut self, _pin: Pin, cycle: u64) -> Option<f64> {
        let v = (cycle as f64 * 3.7e-5) % 5.4;
        self.supplied.push(v);
        Some(v)
    }
}

/// Every printed code of `adc_read` (A0…A5 per line), in conversion order.
fn printed_codes(out: &str) -> Vec<i64> {
    out.lines()
        .filter(|l| l.contains("A5="))
        .flat_map(|l| (0..6).map(move |i| field(l, &format!("A{i}"))))
        .collect()
}

fn ideal(v: f64) -> i64 {
    ((v * 1024.0 / 5.0).floor() as i64).clamp(0, 1023)
}

/// Runs `adc_read` for 1 s on the ramp with an ADC model; (codes, voltages handed over).
fn ramp_run(model: bb_avr::AdcModel, seed: u64) -> (Vec<i64>, Vec<f64>) {
    let mut cfg = AvrConfig::new(Mcu::Atmega328p);
    cfg.stop_on = StopOn {
        pin_change: false,
        ..StopOn::default()
    };
    cfg.adc_model = model;
    cfg.seed = seed;
    let mut avr = Avr::new(cfg);
    avr.load_hex(&std::fs::read_to_string(fixture_path("adc_read", "uno", "hex")).unwrap())
        .unwrap();
    let mut io = Ramp::default();
    let mut ev = Vec::new();
    run_until(&mut avr, ms(1000.0), &mut io, &mut ev);
    let codes = printed_codes(&usart_text(&mut avr, 0));
    assert!(codes.len() >= 100, "only {} codes", codes.len());
    assert!(io.supplied.len() >= codes.len());
    (codes, io.supplied)
}

#[test]
fn ideal_adc_returns_exact_codes_for_a_ramp() {
    let (codes, v) = ramp_run(bb_avr::AdcModel::Ideal, 7);
    for (i, &c) in codes.iter().enumerate() {
        assert_eq!(c, ideal(v[i]), "conversion {i}: {:.4} V", v[i]);
    }
    // the ramp covered the range, including the clamp above AVcc
    assert!(codes.iter().any(|&c| c < 100));
    assert!(codes.contains(&1023));
    // ideal is not seeded: any seed gives the same codes
    assert_eq!(ramp_run(bb_avr::AdcModel::Ideal, 99).0, codes);
}

#[test]
fn realistic_adc_errors_are_bounded_and_seeded() {
    let model = bb_avr::AdcModel::realistic();
    let (a, v) = ramp_run(model, 7);
    let (a2, _) = ramp_run(model, 7);
    let (b, _) = ramp_run(model, 8);
    assert_eq!(a, a2, "same seed → identical codes");
    assert_ne!(a, b, "another chip (seed) → other errors");
    let mut differ = 0;
    for (i, &c) in a.iter().enumerate() {
        let e = c - ideal(v[i]);
        // offset 2 + gain 2 + INL 0.5 + noise 1 LSB (+1 for flooring)
        assert!(
            e.abs() <= 6,
            "conversion {i}: {c} vs ideal {} ({:.4} V)",
            ideal(v[i]),
            v[i]
        );
        assert!((0..=1023).contains(&c));
        differ += (e != 0) as usize;
    }
    assert!(
        differ * 4 > a.len(),
        "only {differ} of {} codes differ from ideal",
        a.len()
    );
}

#[test]
fn realistic_adc_noise_flickers_a_steady_input_by_at_most_one_code() {
    let run = |model: bb_avr::AdcModel| -> Vec<i64> {
        let mut cfg = AvrConfig::new(Mcu::Atmega328p);
        cfg.stop_on = StopOn {
            pin_change: false,
            ..StopOn::default()
        };
        cfg.adc_model = model;
        cfg.seed = 3;
        let mut avr = Avr::new(cfg);
        avr.load_hex(&std::fs::read_to_string(fixture_path("adc_read", "uno", "hex")).unwrap())
            .unwrap();
        let mut io = volts();
        let mut ev = Vec::new();
        run_until(&mut avr, ms(1000.0), &mut io, &mut ev);
        usart_text(&mut avr, 0)
            .lines()
            .filter(|l| l.contains("A2="))
            .map(|l| field(l, "A2"))
            .collect()
    };
    // 2.5 V on A2
    let ideal_codes = run(bb_avr::AdcModel::Ideal);
    assert!(ideal_codes.iter().all(|&c| c == 512), "{ideal_codes:?}");
    let real = run(bb_avr::AdcModel::realistic());
    let (lo, hi) = (*real.iter().min().unwrap(), *real.iter().max().unwrap());
    assert!(hi - lo >= 1, "no conversion noise: {real:?}");
    assert!(hi - lo <= 2, "noise beyond ±1 code: {real:?}");
}

/// Datasheet-style measurement of one chip's transfer function: a slow, noise-free sweep →
/// code transitions → INL / DNL after removing offset and gain (end-point fit).
#[test]
fn realistic_chip_inl_and_dnl_stay_within_half_an_lsb() {
    let e = bb_avr::AdcErrors::DATASHEET;
    let mut offsets = Vec::new();
    for seed in 0..24u64 {
        let chip = bb_avr::AdcChip::new(seed, &e);
        assert!(chip.offset.abs() <= e.offset_lsb && chip.gain.abs() <= e.gain_lsb);
        offsets.push(chip.offset);
        // the same seed is the same chip
        assert_eq!(
            bb_avr::AdcChip::new(seed, &e).convert(300.3, 0.0, 0, 1023),
            chip.convert(300.3, 0.0, 0, 1023)
        );
        // sweep in 1/64 LSB steps: where does each code begin?
        let mut first = vec![None::<f64>; 1024];
        let mut last = 0;
        let steps = 1030 * 64;
        for s in 0..steps {
            let x = s as f64 / 64.0 - 3.0;
            let c = chip.convert(x, 0.0, 0, 1023);
            assert!(c >= last, "seed {seed}: not monotonic at {x}");
            last = c;
            if first[c as usize].is_none() {
                first[c as usize] = Some(x);
            }
        }
        assert!(
            first.iter().all(|f| f.is_some()),
            "seed {seed}: missing codes"
        );
        // remove offset + gain with the end points (codes 1 and 1023), like a datasheet
        let t = |c: usize| first[c].unwrap();
        let lsb = (t(1023) - t(1)) / 1022.0;
        for c in 1..1023 {
            let inl = (t(c) - t(1)) / lsb - (c as f64 - 1.0);
            assert!(
                inl.abs() <= 0.5 + 0.05,
                "seed {seed}: INL {inl:.3} at code {c}"
            );
            let dnl = (t(c + 1) - t(c)) / lsb - 1.0;
            assert!(
                dnl.abs() <= 0.5 + 0.05,
                "seed {seed}: DNL {dnl:.3} at code {c}"
            );
        }
    }
    // chips differ
    offsets.sort_by(f64::total_cmp);
    offsets.dedup();
    assert!(offsets.len() > 20);
}

#[test]
fn the_adc_model_switches_while_running() {
    let mut avr = load("adc_read", "uno");
    avr.set_stop_on(StopOn {
        pin_change: false,
        ..StopOn::default()
    });
    assert_eq!(avr.adc_model(), bb_avr::AdcModel::Ideal);
    let mut io = volts();
    let mut ev = Vec::new();
    run_until(&mut avr, ms(300.0), &mut io, &mut ev);
    avr.set_adc_model(bb_avr::AdcModel::realistic());
    let _ = usart_text(&mut avr, 0);
    run_until(&mut avr, ms(1300.0), &mut io, &mut ev);
    let a2: Vec<i64> = usart_text(&mut avr, 0)
        .lines()
        .filter(|l| l.contains("A2="))
        .map(|l| field(l, "A2"))
        .collect();
    assert!(a2.iter().any(|&c| c != 512), "still ideal: {a2:?}");
}
