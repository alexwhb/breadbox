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

/// Line level transitions where a released (input) pin counts as idle-high (UART lines).
pub fn line_edges(events: &[PinEvent], pin: Pin) -> Vec<(u64, bool)> {
    let mut out: Vec<(u64, bool)> = Vec::new();
    let mut last: Option<bool> = None;
    for e in events.iter().filter(|e| e.pin == pin) {
        let lvl = match e.state {
            PinState::Output { high } => high,
            PinState::Input { .. } => true,
        };
        if last != Some(lvl) {
            out.push((e.cycle, lvl));
            last = Some(lvl);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub start: u64,
    pub data: u16,
    pub parity_ok: bool,
    pub stop_ok: bool,
}

/// Generic UART decoder: `parity` None / Some(false)=even / Some(true)=odd.
pub fn decode_frames(
    edges: &[(u64, bool)],
    baud: f64,
    bits: u32,
    parity: Option<bool>,
    stops: u32,
) -> Vec<Frame> {
    let bit = F as f64 / baud;
    let level_at = |t: f64| -> bool {
        let i = edges.partition_point(|&(c, _)| (c as f64) <= t);
        if i == 0 { true } else { edges[i - 1].1 }
    };
    let mut out = Vec::new();
    let mut busy_until = -1.0f64;
    for &(c, l) in edges {
        if l || (c as f64) < busy_until {
            continue;
        }
        let start = c as f64;
        let mut data = 0u16;
        let mut ones = 0;
        for b in 0..bits {
            if level_at(start + bit * (1.5 + b as f64)) {
                data |= 1 << b;
                ones += 1;
            }
        }
        let mut pos = 1 + bits;
        let parity_ok = match parity {
            None => true,
            Some(odd) => {
                let p = level_at(start + bit * (pos as f64 + 0.5)) as u32;
                pos += 1;
                ((ones + p) % 2 == 1) == odd
            }
        };
        let mut stop_ok = true;
        for s in 0..stops {
            stop_ok &= level_at(start + bit * ((pos + s) as f64 + 0.5));
        }
        out.push(Frame {
            start: c,
            data,
            parity_ok,
            stop_ok,
        });
        busy_until = start + bit * ((pos + stops) as f64 - 0.5);
    }
    out
}

/// Bit-bangs UART frames onto `pin` starting at `start` (cycles), one byte every `gap` cycles.
/// `parity_override` forces a wrong parity bit; `bad_stop` sends a low stop bit.
#[allow(clippy::too_many_arguments)]
pub fn bitbang(
    avr: &mut Avr,
    pin: Pin,
    start: u64,
    baud: f64,
    bytes: &[u8],
    gap: u64,
    parity: Option<bool>,
    corrupt: &[(usize, &str)],
) -> u64 {
    let bit = F as f64 / baud;
    let mut t0 = start as f64;
    for (i, &b) in bytes.iter().enumerate() {
        let mode = corrupt.iter().find(|c| c.0 == i).map(|c| c.1).unwrap_or("");
        let mut bits = vec![false];
        for k in 0..8 {
            bits.push(b >> k & 1 != 0);
        }
        if let Some(odd) = parity {
            let ones = b.count_ones() % 2 == 1;
            let mut p = ones ^ odd;
            if mode == "parity" {
                p = !p;
            }
            bits.push(p);
        }
        bits.push(mode != "stop");
        for (k, &lvl) in bits.iter().enumerate() {
            avr.schedule_pin_input(pin, Some(lvl), (t0 + k as f64 * bit).round() as u64);
        }
        let end = t0 + bits.len() as f64 * bit;
        avr.schedule_pin_input(pin, Some(true), end.round() as u64);
        t0 += gap as f64;
    }
    t0 as u64
}

/// Runs until the fast-path serial output of USART `n` contains `needle` (or panics at `limit_ms`).
pub fn run_until_text(
    avr: &mut Avr,
    n: usize,
    needle: &str,
    limit_ms: f64,
    out: &mut String,
    events: &mut Vec<PinEvent>,
) {
    let limit = avr.cycle() + ms(limit_ms);
    while !out.contains(needle) {
        let until = (avr.cycle() + ms(1.0)).min(limit);
        run_until(avr, until, &mut (), events);
        out.push_str(&usart_text(avr, n));
        assert!(
            avr.cycle() < limit,
            "timed out waiting for {needle:?}; got {out:?}"
        );
    }
}

/// Runs `cycles` more cycles with no analog IO, collecting pin events.
pub fn run_for(avr: &mut Avr, cycles: u64, events: &mut Vec<PinEvent>) {
    let until = avr.cycle() + cycles;
    run_until(avr, until, &mut (), events);
}

/// Runs until a complete line starting with `prefix` has been printed on USART `n`; returns it.
pub fn run_until_line(
    avr: &mut Avr,
    n: usize,
    prefix: &str,
    limit_ms: f64,
    out: &mut String,
    events: &mut Vec<PinEvent>,
) -> String {
    let limit = avr.cycle() + ms(limit_ms);
    loop {
        if let Some(i) = out.rfind(prefix)
            && let Some(end) = out[i..].find("\r\n")
        {
            return out[i..i + end].to_string();
        }
        let until = (avr.cycle() + ms(0.5)).min(limit);
        run_until(avr, until, &mut (), events);
        out.push_str(&usart_text(avr, n));
        assert!(
            avr.cycle() < limit,
            "timed out waiting for a line {prefix:?}; got {out:?}"
        );
    }
}
