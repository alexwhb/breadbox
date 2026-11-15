//! The non-radio AVR example projects (`PRJ-2`) run on the real AVR core: blink timing,
//! night-light PWM vs the LDR, thermometer TMP36 → 16×2 LCD (decoded from the HD44780 bus),
//! servo-sweep pulse widths.

mod common;

use bb_avr::{Avr, AvrIo, Pin, PinEvent, PinState, StopReason};
use common::*;
use std::collections::HashMap;

struct Analog(HashMap<Pin, f64>);

impl AvrIo for Analog {
    fn adc_sample(&mut self, pin: Pin, _cycle: u64) -> Option<f64> {
        Some(*self.0.get(&pin).unwrap_or(&0.0))
    }
}

fn run(avr: &mut Avr, ms: f64, io: &mut Analog) -> Vec<PinEvent> {
    let until = avr.cycle() + (ms * 16_000.0) as u64;
    let mut ev = vec![];
    loop {
        let r = avr.run(until, io);
        ev.extend(avr.take_pin_events());
        match r {
            StopReason::ReachedCycle => return ev,
            StopReason::PinChange => {}
            other => panic!("{other:?}"),
        }
    }
}

fn levels(ev: &[PinEvent], pin: Pin) -> Vec<(u64, bool)> {
    let mut out: Vec<(u64, bool)> = vec![];
    for e in ev.iter().filter(|e| e.pin == pin) {
        let h = matches!(
            e.state,
            PinState::Output { high: true } | PinState::Input { pullup: true }
        );
        if out.last().map(|l| l.1) != Some(h) {
            out.push((e.cycle, h));
        }
    }
    out
}

fn serial(avr: &mut Avr) -> String {
    avr.take_usart_tx(0)
        .iter()
        .map(|b| b.data as u8 as char)
        .collect()
}

#[test]
fn blink_toggles_every_second() {
    let mut avr = load_example("blink");
    let ev = run(&mut avr, 6_500.0, &mut Analog(HashMap::new()));
    let e = levels(&ev, uno(13));
    assert!(e.len() >= 6, "{e:?}");
    for w in e.windows(2).skip(1) {
        let ms = (w[1].0 - w[0].0) as f64 / 16_000.0;
        assert!((ms - 1000.0).abs() < 2.0, "{ms} ms");
    }
}

#[test]
fn night_light_dims_with_the_room() {
    let a0 = uno(14);
    let mut io = Analog(HashMap::from([(a0, 4.0)]));
    let mut avr = load_example("night-light");
    run(&mut avr, 300.0, &mut io);
    let text = serial(&mut avr);
    assert!(text.contains("light=819 led=0\r\n"), "{text}");
    // Dusk: raw 200 → map(200, 300, 100, 40, 255) = 147 → 57.6 % duty at 490 Hz on D9.
    io.0.insert(a0, 200.0 * 5.0 / 1024.0 + 0.001);
    run(&mut avr, 250.0, &mut io);
    let ev = run(&mut avr, 100.0, &mut io);
    assert!(serial(&mut avr).contains("light=200 led=147"));
    let e = levels(&ev, uno(9));
    let mut high = 0u64;
    let mut total = 0u64;
    for w in e.windows(2) {
        if w[0].1 {
            high += w[1].0 - w[0].0;
        }
        total += w[1].0 - w[0].0;
    }
    let duty = high as f64 / total as f64;
    assert!((duty - 147.0 / 255.0).abs() < 0.01, "duty {duty}");
    let rises: Vec<u64> = e.iter().filter(|x| x.1).map(|x| x.0).collect();
    let period_ms = (rises[1] - rises[0]) as f64 / 16_000.0;
    assert!((period_ms - 1000.0 / 490.196).abs() < 0.01, "{period_ms}");
    // Night: full on.
    io.0.insert(a0, 0.3);
    run(&mut avr, 300.0, &mut io);
    assert!(serial(&mut avr).contains("led=255"));
    assert_eq!(avr.pin_info(uno(9)).state, PinState::Output { high: true });
}

/// Minimal HD44780 in 4-bit mode, fed from pin events (RS, E, D4–D7).
struct Lcd {
    ddram: [u8; 128],
    addr: usize,
    nibbles: u32,
    pending: Option<u8>,
}

impl Lcd {
    fn decode(ev: &[PinEvent], rs: Pin, e: Pin, d: [Pin; 4]) -> Lcd {
        let mut lcd = Lcd {
            ddram: [b' '; 128],
            addr: 0,
            nibbles: 0,
            pending: None,
        };
        let mut level: HashMap<Pin, bool> = HashMap::new();
        let mut e_high = false;
        for ev in ev {
            let h = matches!(ev.state, PinState::Output { high: true });
            if ev.pin == e {
                if e_high && !h {
                    let nib = d.iter().enumerate().fold(0u8, |acc, (i, p)| {
                        acc | ((*level.get(p).unwrap_or(&false) as u8) << i)
                    });
                    lcd.nibble(*level.get(&rs).unwrap_or(&false), nib);
                }
                e_high = h;
            } else {
                level.insert(ev.pin, h);
            }
        }
        lcd
    }

    fn nibble(&mut self, rs: bool, nib: u8) {
        self.nibbles += 1;
        // LiquidCrystal::begin sends four lone nibbles (0x3, 0x3, 0x3, 0x2) to enter 4-bit mode.
        if self.nibbles <= 4 {
            return;
        }
        match self.pending.take() {
            None => self.pending = Some(nib),
            Some(hi) => self.byte(rs, hi << 4 | nib),
        }
    }

    fn byte(&mut self, rs: bool, b: u8) {
        if rs {
            self.ddram[self.addr & 0x7f] = b;
            self.addr = (self.addr + 1) & 0x7f;
        } else if b == 0x01 {
            self.ddram = [b' '; 128];
            self.addr = 0;
        } else if b & 0xfe == 0x02 {
            self.addr = 0;
        } else if b & 0x80 != 0 {
            self.addr = (b & 0x7f) as usize;
        }
    }

    fn line(&self, n: usize) -> String {
        let start = if n == 0 { 0 } else { 0x40 };
        self.ddram[start..start + 16]
            .iter()
            .map(|&c| if c == 223 { '°' } else { c as char })
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

#[test]
fn thermometer_shows_tmp36_on_the_lcd() {
    let a0 = uno(14);
    // 21.3 °C → TMP36 0.713 V → ADC 146 → (146·5/1024 − 0.5)·100 = 21.29 °C.
    let mut io = Analog(HashMap::from([(a0, 0.713)]));
    let mut avr = load_example("thermometer");
    let ev = run(&mut avr, 1_800.0, &mut io);
    let lcd = Lcd::decode(&ev, uno(12), uno(11), [uno(5), uno(4), uno(3), uno(2)]);
    assert_eq!(lcd.line(0), "Temp: 21.3°C");
    assert_eq!(lcd.line(1), "lo 21.3 hi 21.3");
    assert!(serial(&mut avr).contains("21.3 C\r\n"));
    // Warmer: min/max tracking.
    io.0.insert(a0, 0.80); // ADC 163 → 29.6 °C
    let ev2 = run(&mut avr, 1_200.0, &mut io);
    let all: Vec<PinEvent> = ev.into_iter().chain(ev2).collect();
    let lcd = Lcd::decode(&all, uno(12), uno(11), [uno(5), uno(4), uno(3), uno(2)]);
    assert_eq!(lcd.line(0), "Temp: 29.6°C");
    assert_eq!(lcd.line(1), "lo 21.3 hi 29.6");
}

#[test]
fn servo_sweep_pulses() {
    let mut avr = load_example("servo-sweep");
    let ev = run(&mut avr, 6_000.0, &mut Analog(HashMap::new()));
    let e = levels(&ev, uno(9));
    let pulses: Vec<(u64, f64)> = e
        .windows(2)
        .filter(|w| w[0].1 && !w[1].1)
        .map(|w| (w[0].0, (w[1].0 - w[0].0) as f64 / 16.0))
        .collect();
    assert!(pulses.len() > 250);
    let min = pulses.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let max = pulses.iter().map(|p| p.1).fold(0.0, f64::max);
    // Servo library: 0° = 544 µs, 180° = 2400 µs, frames every 20 ms.
    assert!((min - 544.0).abs() < 8.0, "min {min} µs");
    assert!((max - 2400.0).abs() < 8.0, "max {max} µs");
    let period = (pulses[11].0 - pulses[10].0) as f64 / 16_000.0;
    assert!((period - 20.0).abs() < 0.2, "{period} ms");
    // A full sweep 0→180→0 takes 2 × 181 × 15 ms ≈ 5.4 s: the max shows up once in the middle.
    let t_max = pulses.iter().find(|p| p.1 > 2390.0).unwrap().0 as f64 / 16e6;
    assert!((2.4..3.2).contains(&t_max), "{t_max}");
}
