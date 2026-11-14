//! ATmega2560 firmware (Serial1, Timer3) and multiple independent MCU instances (MCU-7).

mod common;
use bb_avr::{Avr, FirmwareImage, Mcu, Pin, PinEvent};
use common::*;

#[test]
fn mega_serial1_and_timer3() {
    let mut avr = load("mega_serial1_timer3", "mega");
    assert_eq!(avr.mcu(), Mcu::Atmega2560);
    let ev = run_ms(&mut avr, 1_300.0);
    let s1 = usart_text(&mut avr, 1);
    assert!(s1.starts_with("mega serial1\r\ndone "), "{s1:?}");
    let n: u32 = s1.trim().rsplit(' ').next().unwrap().parse().unwrap();
    // 2 kHz compare rate over delay(1000).
    assert!((1_998..=2_003).contains(&n), "timer3 matches {n}");
    assert_eq!(usart_text(&mut avr, 0), format!("t3={n}\r\n"));
    // Serial1's TX pin (PD3) carries the same text at 9615 baud.
    let frames = decode_frames(
        &line_edges(&ev, Pin::new('D', 3)),
        16e6 / (8.0 * 208.0),
        8,
        None,
        1,
    );
    let text: String = frames.iter().map(|f| f.data as u8 as char).collect();
    assert!(text.starts_with("mega serial1\r\n"), "{text:?}");
    // OC3A (D5 = PE3) toggles every 1000 ticks of clk/8 → 1 kHz square wave.
    let e = edges(&ev, Pin::new('E', 3));
    let toggles: Vec<u64> = e
        .iter()
        .map(|x| x.0)
        .filter(|&c| c > ms(10.0) && c < ms(900.0))
        .collect();
    assert!(toggles.len() > 100);
    for w in toggles.windows(2) {
        assert_eq!(w[1] - w[0], 8_000);
    }
}

#[test]
fn elf_and_hex_load_identically() {
    for (name, board) in [("blink", "uno"), ("mega_serial1_timer3", "mega")] {
        let hex = FirmwareImage::from_hex(
            &std::fs::read_to_string(fixture_path(name, board, "hex")).unwrap(),
        )
        .unwrap();
        let elf =
            FirmwareImage::from_elf(&std::fs::read(fixture_path(name, board, "elf")).unwrap())
                .unwrap();
        let mut a = load(name, board);
        let mut b = load_elf(name, board);
        assert_eq!(a.flash_bytes(), b.flash_bytes(), "{name}");
        assert_eq!(hex.flash_end(), elf.flash_end());
        let ea = run_ms(&mut a, 50.0);
        let eb = run_ms(&mut b, 50.0);
        assert_eq!(ea, eb);
    }
}

/// Loads a fixture with A0 at 2.5 V (the busy sketch samples it).
fn load_env(name: &str, board: &str) -> Avr {
    let mut a = load(name, board);
    if board != "mega" {
        a.set_pin_voltage(Pin::new('C', 0), 2.5);
    }
    a
}

fn solo(name: &str, board: &str, millis: f64) -> (Vec<PinEvent>, String) {
    let mut a = load_env(name, board);
    let ev = run_ms(&mut a, millis);
    let s = usart_text(&mut a, 0);
    (ev, s)
}

#[test]
fn multiple_mcus_interleaved_match_solo_runs() {
    let names = [
        ("blink", "uno"),
        ("serial_hello", "uno"),
        ("mega_serial1_timer3", "mega"),
        ("busy", "uno"),
    ];
    let total = 2_500.0;
    let solos: Vec<(Vec<PinEvent>, String)> =
        names.iter().map(|(n, b)| solo(n, b, total)).collect();
    let mut mcus: Vec<Avr> = names.iter().map(|(n, b)| load_env(n, b)).collect();
    let mut evs: Vec<Vec<PinEvent>> = vec![Vec::new(); mcus.len()];
    let mut outs = vec![String::new(); mcus.len()];
    // Lockstep in 100 µs quanta, round robin.
    let mut t = 0;
    while t < ms(total) {
        t = (t + 1_600).min(ms(total));
        for (i, m) in mcus.iter_mut().enumerate() {
            run_until(m, t, &mut (), &mut evs[i]);
            outs[i].push_str(&usart_text(m, 0));
        }
    }
    for i in 0..mcus.len() {
        assert_eq!(evs[i], solos[i].0, "{} pin events differ", names[i].0);
        assert_eq!(outs[i], solos[i].1, "{} serial differs", names[i].0);
    }
}

#[test]
fn mcus_run_in_parallel_threads() {
    let reference = solo("busy", "uno", 500.0);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let mut a = load_env("busy", "uno");
                let ev = run_ms(&mut a, 500.0);
                (ev, usart_text(&mut a, 0))
            })
        })
        .collect();
    for h in handles {
        let r = h.join().unwrap();
        assert_eq!(r.0, reference.0);
        assert_eq!(r.1, reference.1);
    }
    // Snapshots: a clone continues identically.
    let mut a = load_env("busy", "uno");
    run_ms(&mut a, 100.0);
    let mut b = a.clone();
    let ea = run_ms(&mut a, 100.0);
    let eb = run_ms(&mut b, 100.0);
    assert_eq!(ea, eb);
    assert_eq!(usart_text(&mut a, 0), usart_text(&mut b, 0));
}

#[test]
fn mega_timer5_int3_differential_adc_serial3() {
    let mut avr = load("mega_extras", "mega");
    let (a0, a1, a9) = (Pin::new('F', 0), Pin::new('F', 1), Pin::new('K', 1));
    avr.set_pin_voltage(a0, 3.0);
    avr.set_pin_voltage(a1, 2.9);
    avr.set_pin_voltage(a9, 1.25);
    let d18 = Pin::new('D', 3);
    avr.set_pin_input(d18, Some(false));
    let mut ev = Vec::new();
    let mut out = String::new();
    let line = run_until_line(&mut avr, 0, "d10=", 100.0, &mut out, &mut ev);
    // ADC0-ADC1 at gain 1: 0.1 V · 512 / 5 V = 10.24 → 10; ADC1-ADC0 at gain 10: -1 V → -102.4 → -103.
    assert_eq!(line, "d10=10 d01x10=-103 a9=256");
    let s3 = run_until_line(&mut avr, 3, "serial3", 50.0, &mut String::new(), &mut ev);
    assert_eq!(s3, "serial3");
    // INT3 (D18) rising edges.
    let t = avr.cycle() + 1_000;
    for k in 0..3u64 {
        avr.schedule_pin_input(d18, Some(true), t + k * 4_000);
        avr.schedule_pin_input(d18, Some(false), t + k * 4_000 + 2_000);
    }
    run_until_line(&mut avr, 0, "rises=3", 50.0, &mut out, &mut ev);
    // Timer5 phase-correct PWM at 490 Hz on PL5/PL4/PL3 (D44/45/46).
    run_for(&mut avr, ms(30.0), &mut ev);
    for (pin, ocr) in [
        (Pin::new('L', 5), 64u64),
        (Pin::new('L', 4), 128),
        (Pin::new('L', 3), 192),
    ] {
        let e: Vec<(u64, bool)> = edges(&ev, pin)
            .into_iter()
            .filter(|x| x.0 > ms(10.0))
            .collect();
        let rises: Vec<u64> = e.iter().filter(|x| x.1).map(|x| x.0).collect();
        assert_eq!(rises[2] - rises[1], 510 * 64, "{pin}");
        let r = rises[1];
        let f = e.iter().find(|x| !x.1 && x.0 > r).unwrap().0;
        assert_eq!(f - r, 2 * ocr * 64, "{pin} duty");
    }
}
