//! Timing-accuracy tests on real arduino-cli compiled firmware.

mod common;
use bb_avr::Pin;
use common::*;

const D13: Pin = Pin::new('B', 5);

#[test]
fn blink_toggles_every_1000ms() {
    let mut avr = load("blink", "uno");
    let ev = run_ms(&mut avr, 10_500.0);
    let e = edges(&ev, D13);
    // First event is pinMode (output low), then HIGH/LOW alternate.
    let toggles: Vec<u64> = e.iter().skip(1).map(|x| x.0).collect();
    assert!(toggles.len() >= 10, "{} toggles", toggles.len());
    for w in toggles.windows(2) {
        let dt = (w[1] - w[0]) as f64;
        let err = (dt - 16_000_000.0) / 16_000_000.0;
        assert!(
            err.abs() < 0.0005,
            "interval {dt} cycles, error {:.4}%",
            err * 100.0
        );
    }
    // Levels alternate starting with HIGH.
    assert!(e[1].1);
    assert!(!e[2].1);
}

fn parse_kv(line: &str, key: &str) -> Option<u64> {
    let i = line.find(&format!("{key}="))? + key.len() + 1;
    let rest = &line[i..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[test]
fn millis_and_micros_track_simulated_time_over_10s() {
    let mut avr = load("millis_micros", "uno");
    let ev = run_ms(&mut avr, 10_200.0);
    let out = usart_text(&mut avr, 0);
    let lines: Vec<&str> = out.lines().collect();
    let marks = edges(&ev, Pin::new('D', 7));
    // marks[0] is pinMode (output low); each toggle afterwards samples millis/micros.
    let toggles: Vec<u64> = marks.iter().skip(1).map(|x| x.0).collect();
    assert_eq!(lines.len(), 10, "{out}");
    assert_eq!(toggles.len(), 10);
    // Timer 0 starts in init(), after the C runtime: a constant offset vs. simulated time.
    let mut off_ms = Vec::new();
    let mut off_us = Vec::new();
    for (line, &t) in lines.iter().zip(&toggles) {
        let m = parse_kv(line, "m").unwrap() as f64;
        let u = parse_kv(line, "u").unwrap() as f64;
        let t_us = t as f64 / 16.0;
        off_ms.push(m - t_us / 1000.0);
        off_us.push(u - t_us);
    }
    assert!(off_us[0].abs() < 200.0, "startup offset {} µs", off_us[0]);
    for (i, o) in off_us.iter().enumerate() {
        // micros(): 4 µs resolution, sampled a few cycles after the marker.
        assert!(
            (o - off_us[0]).abs() <= 8.0,
            "sample {i}: micros offset {o} vs {} ({off_us:?})",
            off_us[0]
        );
        // millis(): 1.024 ms tick with fractional correction → within ~1 ms.
        assert!(
            off_ms[i].abs() <= 1.1,
            "millis offset {} ms ({off_ms:?})",
            off_ms[i]
        );
    }
    // No drift over 10 s.
    let drift = off_us.last().unwrap() - off_us.first().unwrap();
    assert!(drift.abs() <= 8.0, "drift {drift} µs");
}

struct Pwm {
    period: u64,
    high: u64,
}

fn measure_pwm(ev: &[bb_avr::PinEvent], pin: Pin, after: u64) -> Pwm {
    let e: Vec<(u64, bool)> = edges(ev, pin).into_iter().filter(|x| x.0 > after).collect();
    let rises: Vec<u64> = e.iter().filter(|x| x.1).map(|x| x.0).collect();
    let falls: Vec<u64> = e.iter().filter(|x| !x.1).map(|x| x.0).collect();
    assert!(rises.len() >= 4, "{pin}: too few edges ({})", rises.len());
    let period = rises[2] - rises[1];
    for w in rises.windows(2).skip(1) {
        assert_eq!(w[1] - w[0], period, "{pin}: jittery period");
    }
    let r = rises[1];
    let f = *falls.iter().find(|&&f| f > r).unwrap();
    Pwm {
        period,
        high: f - r,
    }
}

#[test]
fn analog_write_on_every_uno_pwm_pin() {
    let mut avr = load("pwm_all", "uno");
    let ev = run_ms(&mut avr, 60.0);
    let start = ms(5.0);
    // (pin, timer prescale, fast?, ocr)
    let cases = [
        ("D3", Pin::new('D', 3), false, 32u64),
        ("D5", Pin::new('D', 5), true, 64),
        ("D6", Pin::new('D', 6), true, 128),
        ("D9", Pin::new('B', 1), false, 192),
        ("D10", Pin::new('B', 2), false, 200),
        ("D11", Pin::new('B', 3), false, 250),
    ];
    for (name, pin, fast, ocr) in cases {
        let p = measure_pwm(&ev, pin, start);
        let tick = 64;
        let (period, high) = if fast {
            (256 * tick, (ocr + 1) * tick)
        } else {
            (510 * tick, 2 * ocr * tick)
        };
        let hz = 16e6 / p.period as f64;
        println!(
            "{name}: {hz:.4} Hz, duty {:.3}%",
            100.0 * p.high as f64 / p.period as f64
        );
        assert_eq!(p.period, period, "{name} period");
        assert!(
            p.high.abs_diff(high) <= tick,
            "{name}: high {} vs {high}",
            p.high
        );
        if fast {
            assert!((hz - 976.5625).abs() < 1e-9);
        } else {
            assert!((hz - 490.196).abs() < 0.001);
        }
    }
}

#[test]
fn servo_pulse_widths() {
    let mut avr = load("servo_sweep", "uno");
    let ev = run_ms(&mut avr, 3_000.0);
    let e = edges(&ev, Pin::new('B', 1));
    let pulses: Vec<(u64, u64)> = e
        .windows(2)
        .filter(|w| w[0].1 && !w[1].1)
        .map(|w| (w[0].0, w[1].0 - w[0].0))
        .collect();
    let in_window = |a: f64, b: f64| -> Vec<f64> {
        pulses
            .iter()
            .filter(|(t, _)| (*t as f64) > ms(a) as f64 && (*t as f64) < ms(b) as f64)
            .map(|(_, w)| *w as f64 / 16.0)
            .collect()
    };
    for (a, b, us) in [
        (100.0, 950.0, 544.0),
        (1100.0, 1950.0, 1472.0),
        (2100.0, 2950.0, 2400.0),
    ] {
        let w = in_window(a, b);
        assert!(w.len() > 30, "pulses in window: {}", w.len());
        // The library programs (us - 2 µs) ticks; ISR + digitalWrite overhead adds a few µs on
        // real hardware too. Every pulse must be identical and within 8 µs of nominal.
        // When the millis ISR happens to run at the compare match the falling edge is delayed
        // by its length (the classic Servo-library jitter); the typical pulse is exact.
        for x in &w {
            assert!((x - us).abs() <= 8.0, "pulse {x} µs, expected {us}");
        }
        let mut sorted = w.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        assert!(
            (median - us).abs() <= 1.0,
            "median pulse {median} µs, expected {us}"
        );
    }
    // Refresh period 20 ms.
    let rises: Vec<u64> = e.iter().filter(|x| x.1).map(|x| x.0).collect();
    let per = (rises[20] - rises[19]) as f64 / 16_000.0;
    assert!((per - 20.0).abs() < 0.05, "servo period {per} ms");
}

#[test]
fn tone_frequency_and_duration() {
    let mut avr = load("tone_test", "uno");
    let ev = run_ms(&mut avr, 1_200.0);
    let e = edges(&ev, Pin::new('B', 0));
    let first: Vec<u64> = e.iter().filter(|x| x.0 < ms(900.0)).map(|x| x.0).collect();
    // 440 Hz: ocr = F_CPU/440/2/128 - 1 = 141 → half period 128*142 = 18176 cycles.
    let halves: Vec<u64> = first.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(halves.len() >= 430);
    // The pin is toggled in the TIMER2_COMPA ISR; the millis ISR occasionally delays it, so
    // single half periods jitter but the compare timing itself is exact.
    let inner = &halves[1..halves.len() - 1];
    for h in inner {
        assert!(h.abs_diff(18_176) < 160, "440 Hz half period {h}");
    }
    let mean = (first[first.len() - 2] - first[1]) as f64 / (first.len() - 3) as f64;
    assert!((mean - 18_176.0).abs() < 0.5, "mean half period {mean}");
    let dur = (first[first.len() - 1] - first[1]) as f64 / 16_000.0;
    assert!((dur - 500.0).abs() < 2.0, "tone lasted {dur} ms");
    let second: Vec<u64> = e
        .iter()
        .filter(|x| x.0 > ms(1_010.0))
        .map(|x| x.0)
        .collect();
    let mean = (second[second.len() - 1] - second[0]) as f64 / (second.len() - 1) as f64;
    assert!(
        (mean - 8_000.0).abs() < 0.5,
        "1 kHz mean half period {mean}"
    );
    for w in second.windows(2) {
        assert!((w[1] - w[0]).abs_diff(8_000) < 160);
    }
}

#[test]
fn timer1_waveform_modes_and_input_capture() {
    let mut avr = load("timer_regs", "uno");
    let mut ev = Vec::new();
    let mut bytes: Vec<bb_avr::UsartByte> = Vec::new();
    while !bytes.iter().any(|b| b.data == b'D' as u16) {
        let until = avr.cycle() + ms(10.0);
        run_until(&mut avr, until, &mut (), &mut ev);
        bytes.extend(avr.take_usart_tx(0));
        assert!(avr.cycle() < ms(2_000.0), "no phase D");
    }
    let at = |c: u8| bytes.iter().find(|b| b.data == c as u16).unwrap().cycle;
    let (ta, tb, tc, td) = (at(b'A'), at(b'B'), at(b'C'), at(b'D'));
    let oc1a = Pin::new('B', 1);
    let oc1b = Pin::new('B', 2);
    // Phase A: mode 14 fast PWM, 50 Hz, 1.5 ms / 5 ms high.
    let a = measure_pwm_window(&ev, oc1a, ta, tb);
    assert_eq!(a.period, 320_000);
    assert_eq!(a.high, 24_000);
    let b = measure_pwm_window(&ev, oc1b, ta, tb);
    assert_eq!(b.high, 80_000);
    // Phase B: 8 kHz phase+frequency correct, 25 % and inverted 75 % (= 25 % high).
    let a = measure_pwm_window(&ev, oc1a, tb, tc);
    assert_eq!(a.period, 2_000);
    assert_eq!(a.high, 500);
    let b = measure_pwm_window(&ev, oc1b, tb, tc);
    assert_eq!(b.period, 2_000);
    assert_eq!(b.high, 500);
    // Phase C: CTC toggle at 1 kHz.
    let c = measure_pwm_window(&ev, oc1a, tc, td);
    assert_eq!(c.period, 16_000);
    assert_eq!(c.high, 8_000);
    let mut out = String::new();
    // Phase D: rising edges on ICP1 (D8) spaced 20000, 30000, 40000 cycles → /8 ticks.
    let icp = Pin::new('B', 0);
    let t0 = avr.cycle() + 20_000;
    let mut t = t0;
    for gap in [0u64, 20_000, 30_000, 40_000] {
        t += gap;
        avr.schedule_pin_input(icp, Some(true), t);
        avr.schedule_pin_input(icp, Some(false), t + 5_000);
    }
    let until = t + ms(20.0);
    run_until(&mut avr, until, &mut (), &mut ev);
    out.push_str(&usart_text(&mut avr, 0));
    assert!(out.contains("cap 2500 3750 5000"), "{out}");
}

fn measure_pwm_window(ev: &[bb_avr::PinEvent], pin: Pin, from: u64, to: u64) -> Pwm {
    let sub: Vec<bb_avr::PinEvent> = ev
        .iter()
        .filter(|e| e.cycle >= from && e.cycle <= to)
        .copied()
        .collect();
    measure_pwm(&sub, pin, from)
}
