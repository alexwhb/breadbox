//! A minimal co-simulation harness (no bb-sim): one real AVR running compiled firmware, wired
//! to one UART radio module through `UartWire` with exact cycle timing, ADC voltages from a
//! closure, USART0 captured as the serial monitor.
#![allow(dead_code)]

use bb_avr::{Avr, AvrConfig, AvrIo, Mcu, Pin, PinEvent, PinState, StopOn, StopReason};
use bb_core::SimTime;
use bb_radio::{Annotation, DeviceId, RadioSpace, UartWire};
use std::path::PathBuf;

pub const F_CPU: u64 = 16_000_000;

pub fn example_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/fixtures")
        .join(name)
}

pub fn load_example(name: &str) -> Avr {
    let mut cfg = AvrConfig::new(Mcu::Atmega328p);
    cfg.stop_on = StopOn {
        pin_change: true,
        ..StopOn::default()
    };
    let mut avr = Avr::new(cfg);
    let hex = std::fs::read_to_string(example_fixture(&format!("{name}.uno.hex")))
        .expect("fixture missing; run scripts/build-test-firmware.sh");
    avr.load_hex(&hex).unwrap();
    avr
}

/// UNO pin by Arduino number (digital 0–13, A0 = 14 …).
pub fn uno(n: u8) -> Pin {
    match n {
        0..=7 => Pin::new('D', n),
        8..=13 => Pin::new('B', n - 8),
        _ => Pin::new('C', n - 14),
    }
}

type Volts = Box<dyn FnMut(Pin, SimTime) -> f64>;

struct Io<'a> {
    volts: &'a mut Volts,
}

impl AvrIo for Io<'_> {
    fn adc_sample(&mut self, pin: Pin, cycle: u64) -> Option<f64> {
        Some((self.volts)(pin, SimTime::from_cycles(cycle, F_CPU)))
    }
}

pub struct Bench {
    pub avr: Avr,
    pub space: RadioSpace,
    pub wire: UartWire,
    pub module: DeviceId,
    /// MCU pin driving the module's RX.
    pub mcu_tx: Pin,
    /// MCU pin fed by the module's TX.
    pub mcu_rx: Pin,
    pub volts: Volts,
    /// USART0 text with the time each line completed.
    pub lines: Vec<(SimTime, String)>,
    partial: String,
    pub notes: Vec<Annotation>,
    /// Every pin event except the UART lines.
    pub pins: Vec<PinEvent>,
    /// Upper bound on one AVR run chunk.
    pub quantum: SimTime,
}

impl Bench {
    pub fn new(avr: Avr, space: RadioSpace, module: DeviceId, mcu_tx: Pin, mcu_rx: Pin) -> Self {
        let wire = UartWire::new(&space, module);
        let mut avr = avr;
        avr.set_pin_input(mcu_rx, Some(true));
        Bench {
            avr,
            space,
            wire,
            module,
            mcu_tx,
            mcu_rx,
            volts: Box::new(|_, _| 0.0),
            lines: vec![],
            partial: String::new(),
            notes: vec![],
            pins: vec![],
            quantum: SimTime::from_ms(1),
        }
    }

    pub fn now(&self) -> SimTime {
        self.avr.time()
    }

    /// Runs the bench until `end`.
    pub fn run_until(&mut self, end: SimTime) {
        while self.avr.time() < end {
            let now = self.avr.time();
            let mut target = (now + self.quantum).min(end);
            if let Some(w) = self.space.next_wake() {
                target = target.min(w.max(now));
            }
            if let Some(d) = self.wire.next_decision() {
                target = target.min(d.max(now));
            }
            let until = self.avr.cycle_at(target).max(self.avr.cycle() + 1);
            let mut io = Io {
                volts: &mut self.volts,
            };
            let r = self.avr.run(until, &mut io);
            match r {
                StopReason::ReachedCycle | StopReason::PinChange => {}
                other => panic!("unexpected stop {other:?} at {}", self.avr.time()),
            }
            for e in self.avr.take_pin_events() {
                let t = SimTime::from_cycles(e.cycle, F_CPU);
                if e.pin == self.mcu_tx {
                    let high = match e.state {
                        PinState::Output { high } => high,
                        PinState::Input { .. } => true,
                    };
                    self.wire.mcu_edge(t, high);
                } else {
                    self.pins.push(e);
                }
            }
            let t = self.avr.time();
            self.wire.pump(&mut self.space, t);
            self.space.advance(t);
            let cyc = self.avr.cycle();
            for (et, lvl) in self.wire.module_edges(&mut self.space) {
                let c = self.avr.cycle_at(et);
                // The core may overshoot a stop by one instruction (+ interrupt entry).
                assert!(
                    c + 32 >= cyc,
                    "module edge at {et} lands in the MCU's past ({})",
                    self.avr.time()
                );
                self.avr.schedule_pin_input(self.mcu_rx, Some(lvl), c);
            }
            for b in self.avr.take_usart_tx(0) {
                let c = b.data as u8 as char;
                if c == '\n' {
                    let line = std::mem::take(&mut self.partial);
                    self.lines.push((
                        SimTime::from_cycles(b.cycle, F_CPU),
                        line.trim_end_matches('\r').to_string(),
                    ));
                } else {
                    self.partial.push(c);
                }
            }
            self.notes.extend(self.space.take_annotations());
        }
    }

    pub fn serial_text(&self) -> String {
        self.lines.iter().map(|l| format!("{}\n", l.1)).collect()
    }
}
