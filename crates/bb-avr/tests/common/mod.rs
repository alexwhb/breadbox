//! Shared helpers for real-firmware tests.
#![allow(dead_code)]

use bb_avr::{Avr, AvrConfig, AvrIo, Mcu, Pin, PinEvent, PinState, StopOn, StopReason};
use std::path::PathBuf;

pub const F: u64 = 16_000_000;

pub fn fixture_path(name: &str, board: &str, ext: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.{board}.{ext}"))
}

/// Loads `<name>.<board>.hex` into a fresh MCU. Pin-change stops are off (events are logged).
pub fn load(name: &str, board: &str) -> Avr {
    let mcu = if board == "mega" {
        Mcu::Atmega2560
    } else {
        Mcu::Atmega328p
    };
    let mut cfg = AvrConfig::new(mcu);
    cfg.stop_on = StopOn {
        pin_change: false,
        ..StopOn::default()
    };
    let mut avr = Avr::new(cfg);
    let hex = std::fs::read_to_string(fixture_path(name, board, "hex"))
        .expect("fixture missing; run scripts/build-test-firmware.sh");
    avr.load_hex(&hex).unwrap();
    avr
}

pub fn load_elf(name: &str, board: &str) -> Avr {
    let mcu = if board == "mega" {
        Mcu::Atmega2560
    } else {
        Mcu::Atmega328p
    };
    let mut cfg = AvrConfig::new(mcu);
    cfg.stop_on = StopOn {
        pin_change: false,
        ..StopOn::default()
    };
    let mut avr = Avr::new(cfg);
    let elf = std::fs::read(fixture_path(name, board, "elf")).unwrap();
    avr.load_elf(&elf).unwrap();
    avr
}

pub fn ms(x: f64) -> u64 {
    (x * 1e-3 * F as f64).round() as u64
}

/// Runs until `until` cycles, panicking on unexpected stops; collects pin events.
pub fn run_until(avr: &mut Avr, until: u64, io: &mut dyn AvrIo, events: &mut Vec<PinEvent>) {
    loop {
        let r = avr.run(until, io);
        events.extend(avr.take_pin_events());
        match r {
            StopReason::ReachedCycle => return,
            StopReason::PinChange => continue,
            other => panic!(
                "unexpected stop {other:?} at cycle {} pc 0x{:X}",
                avr.cycle(),
                avr.pc()
            ),
        }
    }
}

pub fn run_ms(avr: &mut Avr, millis: f64) -> Vec<PinEvent> {
    let mut ev = Vec::new();
    let until = avr.cycle() + ms(millis);
    run_until(avr, until, &mut (), &mut ev);
    ev
}

/// Output level transitions of one pin: (cycle, high).
pub fn edges(events: &[PinEvent], pin: Pin) -> Vec<(u64, bool)> {
    let mut out: Vec<(u64, bool)> = Vec::new();
    let mut last: Option<bool> = None;
    for e in events.iter().filter(|e| e.pin == pin) {
        let lvl = match e.state {
            PinState::Output { high } => high,
            PinState::Input { pullup } => pullup,
        };
        if last != Some(lvl) {
            out.push((e.cycle, lvl));
            last = Some(lvl);
        }
    }
    out
}

/// Decodes an 8N1 UART stream from line transitions (idle high) at `baud`.
pub fn decode_uart(edges: &[(u64, bool)], baud: f64) -> Vec<u8> {
    let bit = F as f64 / baud;
    let level_at = |t: f64| -> bool {
        let mut lvl = true;
        for &(c, l) in edges {
            if (c as f64) <= t {
                lvl = l;
            } else {
                break;
            }
        }
        lvl
    };
    let mut out = Vec::new();
    let mut i = 0;
    let mut busy_until = 0.0f64;
    while i < edges.len() {
        let (c, l) = edges[i];
        if !l && (c as f64) >= busy_until {
            let start = c as f64;
            let mut byte = 0u8;
            for b in 0..8 {
                if level_at(start + bit * (1.5 + b as f64)) {
                    byte |= 1 << b;
                }
            }
            let stop_ok = level_at(start + bit * 9.5);
            assert!(stop_ok, "framing error at cycle {c}");
            out.push(byte);
            busy_until = start + bit * 9.5;
        }
        i += 1;
    }
    out
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A fixed analog environment for ADC tests.
#[derive(Default)]
pub struct Volts {
    pub map: std::collections::HashMap<Pin, f64>,
    pub samples: Vec<(Pin, u64)>,
}

impl AvrIo for Volts {
    fn adc_sample(&mut self, pin: Pin, cycle: u64) -> Option<f64> {
        self.samples.push((pin, cycle));
        Some(*self.map.get(&pin).unwrap_or(&0.0))
    }
}

pub fn usart_text(avr: &mut Avr, n: usize) -> String {
    let bytes: Vec<u8> = avr.take_usart_tx(n).iter().map(|b| b.data as u8).collect();
    text(&bytes)
}
