//! I²C (Wire) and SPI on real firmware, with bus partners emulated at the pin level.

mod common;
use bb_avr::{Avr, Pin, PinState, StopOn, StopReason};
use common::*;
use std::collections::VecDeque;

const SDA: Pin = Pin::new('C', 4);
const SCL: Pin = Pin::new('C', 5);

fn drives_low(avr: &Avr, pin: Pin) -> bool {
    avr.pin_info(pin).state == PinState::Output { high: false }
}

fn output_level(avr: &Avr, pin: Pin) -> bool {
    match avr.pin_info(pin).state {
        PinState::Output { high } => high,
        PinState::Input { pullup } => pullup,
    }
}

// ------------------------------------------------------------------ I²C slave model

#[derive(Debug, Clone, Copy, PartialEq)]
enum S {
    Idle,
    Addr,
    AckAddr(bool),
    Rx,
    AckRx,
    Tx,
    TxAck,
}

struct I2cSlave {
    addr: u8,
    sda_low: bool,
    state: S,
    bits: u8,
    shift: u8,
    received: Vec<u8>,
    to_send: VecDeque<u8>,
    master_acked: bool,
    prev: (bool, bool),
    starts: usize,
    stops: usize,
    addresses: Vec<u8>,
}

impl I2cSlave {
    fn new(addr: u8, reply: &[u8]) -> Self {
        I2cSlave {
            addr,
            sda_low: false,
            state: S::Idle,
            bits: 0,
            shift: 0,
            received: vec![],
            to_send: reply.iter().copied().collect(),
            master_acked: false,
            prev: (true, true),
            starts: 0,
            stops: 0,
            addresses: vec![],
        }
    }

    fn tx_bit(&mut self) {
        let b = self.to_send.front().copied().unwrap_or(0xFF) >> (7 - self.bits) & 1;
        self.sda_low = b == 0;
    }

    /// Lines changed (one at a time) to (sda, scl).
    fn on_lines(&mut self, sda: bool, scl: bool) {
        let (psda, pscl) = self.prev;
        self.prev = (sda, scl);
        if scl && pscl && sda != psda {
            if !sda {
                self.starts += 1;
                self.state = S::Addr;
                self.bits = 0;
                self.shift = 0;
            } else {
                self.stops += 1;
                self.state = S::Idle;
                self.sda_low = false;
            }
            return;
        }
        if scl && !pscl {
            // rising: sample
            match self.state {
                S::Addr | S::Rx => {
                    self.shift = (self.shift << 1) | sda as u8;
                    self.bits += 1;
                }
                S::TxAck => self.master_acked = !sda,
                _ => {}
            }
        } else if !scl && pscl {
            // falling: drive
            match self.state {
                S::Addr if self.bits == 8 => {
                    self.addresses.push(self.shift);
                    if self.shift >> 1 == self.addr {
                        self.sda_low = true;
                        self.state = S::AckAddr(self.shift & 1 != 0);
                    } else {
                        self.state = S::Idle;
                    }
                }
                S::AckAddr(read) => {
                    self.bits = 0;
                    self.shift = 0;
                    if read {
                        self.state = S::Tx;
                        self.tx_bit();
                    } else {
                        self.sda_low = false;
                        self.state = S::Rx;
                    }
                }
                S::Rx if self.bits == 8 => {
                    self.received.push(self.shift);
                    self.sda_low = true;
                    self.state = S::AckRx;
                }
                S::AckRx => {
                    self.sda_low = false;
                    self.bits = 0;
                    self.shift = 0;
                    self.state = S::Rx;
                }
                S::Tx => {
                    self.bits += 1;
                    if self.bits == 8 {
                        self.sda_low = false;
                        self.state = S::TxAck;
                    } else {
                        self.tx_bit();
                    }
                }
                S::TxAck => {
                    self.to_send.pop_front();
                    self.bits = 0;
                    if self.master_acked {
                        self.state = S::Tx;
                        self.tx_bit();
                    } else {
                        self.state = S::Idle;
                        self.sda_low = false;
                    }
                }
                _ => {}
            }
        }
    }
}

#[test]
fn wire_master_against_pin_level_slave() {
    let mut avr = load("wire_master", "uno");
    avr.set_stop_on(StopOn {
        pin_change: true,
        ..StopOn::default()
    });
    let mut slave = I2cSlave::new(0x42, &[0xA1, 0xB2, 0xC3]);
    let mut lines = (true, true);
    avr.set_pin_input(SDA, Some(true));
    avr.set_pin_input(SCL, Some(true));
    let mut out = String::new();
    let mut scl_rises = Vec::new();
    while !out.contains("absent=") || !out.ends_with("\r\n") {
        let r = avr.run(avr.cycle() + ms(1.0), &mut ());
        for e in avr.take_pin_events() {
            if e.pin == SCL && matches!(e.state, PinState::Input { .. }) {
                scl_rises.push(e.cycle);
            }
        }
        out.push_str(&usart_text(&mut avr, 0));
        assert!(
            matches!(r, StopReason::PinChange | StopReason::ReachedCycle),
            "{r:?}"
        );
        assert!(avr.cycle() < ms(200.0), "timeout: {out}");
        // Resolve the open-drain lines (external pull-ups) until stable.
        loop {
            let sda = !(drives_low(&avr, SDA) || slave.sda_low);
            let scl = !drives_low(&avr, SCL);
            if (sda, scl) == lines {
                break;
            }
            // Apply one line change at a time (SCL first if both moved).
            let next = if scl != lines.1 {
                (lines.0, scl)
            } else {
                (sda, lines.1)
            };
            lines = next;
            slave.on_lines(lines.0, lines.1);
        }
        avr.set_pin_input(SDA, Some(lines.0));
        avr.set_pin_input(SCL, Some(lines.1));
    }
    assert_eq!(slave.received, vec![0x10, 0x20, 0x30]);
    assert_eq!(
        slave.addresses,
        vec![0x84, 0x85, 0xA0],
        "SLA+W, SLA+R, absent SLA+W"
    );
    assert_eq!(slave.starts, 3);
    assert_eq!(slave.stops, 3);
    assert_eq!(out, "tx=0\r\nrx3 A1 B2 C3\r\nabsent=2\r\n");
    // 100 kHz: SCL period = 16 + 2·TWBR(72) = 160 cycles inside a byte.
    let periods: Vec<u64> = scl_rises
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d < 200)
        .collect();
    assert!(periods.len() > 20);
    // (+ up to 4 cycles: the pull-up "rise" is applied at the next instruction boundary)
    assert!(
        periods.iter().all(|p| (160..=164).contains(p)),
        "{periods:?}"
    );
}

#[test]
fn two_avrs_wire_master_and_slave_on_one_bus() {
    let mut m = load("wire_master", "uno");
    let mut s = load("wire_slave", "uno");
    for a in [&mut m, &mut s] {
        a.set_pin_input(SDA, Some(true));
        a.set_pin_input(SCL, Some(true));
    }
    // Power the slave 5 ms earlier so its Wire.begin(0x42) precedes the master's first START.
    let mut ev = Vec::new();
    run_until(&mut s, ms(5.0), &mut (), &mut ev);
    let off = ms(5.0);
    let q = 8;
    let mut t = 0;
    let mut mout = String::new();
    let mut sout = String::new();
    while !mout.contains("absent=") || !mout.ends_with("\r\n") {
        t += q;
        run_until(&mut m, t, &mut (), &mut ev);
        run_until(&mut s, t + off, &mut (), &mut ev);
        ev.clear();
        let sda = !(drives_low(&m, SDA) || drives_low(&s, SDA));
        let scl = !(drives_low(&m, SCL) || drives_low(&s, SCL));
        for a in [&mut m, &mut s] {
            a.set_pin_input(SDA, Some(sda));
            a.set_pin_input(SCL, Some(scl));
        }
        mout.push_str(&usart_text(&mut m, 0));
        sout.push_str(&usart_text(&mut s, 0));
        assert!(t < ms(300.0), "timeout\nmaster: {mout}\nslave: {sout}");
    }
    run_until(&mut s, t + off + ms(3.0), &mut (), &mut ev);
    sout.push_str(&usart_text(&mut s, 0));
    assert_eq!(
        mout, "tx=0\r\nrx3 4F 4B 21\r\nabsent=2\r\n",
        "\"OK!\" from the slave"
    );
    assert_eq!(sout, "got 10 20 30\r\n");
}

// ------------------------------------------------------------------ SPI

#[test]
fn spi_master_transfer_against_pin_level_slave() {
    let (ss, mosi, miso, sck) = (
        Pin::new('B', 2),
        Pin::new('B', 3),
        Pin::new('B', 4),
        Pin::new('B', 5),
    );
    let mut avr = load("spi_master", "uno");
    avr.set_stop_on(StopOn {
        pin_change: true,
        ..StopOn::default()
    });
    let reply = [0x5Au8, 0xC3];
    let (mut idx, mut bits, mut shift) = (0usize, 0u8, 0u8);
    let mut got = Vec::new();
    let mut prev_ss = true;
    let mut prev_sck = false;
    let mut sck_edges = Vec::new();
    let mut out = String::new();
    avr.set_pin_input(miso, Some(true));
    while !out.ends_with("\r\n") {
        let r = avr.run(avr.cycle() + ms(1.0), &mut ());
        assert!(matches!(
            r,
            StopReason::PinChange | StopReason::ReachedCycle
        ));
        avr.take_pin_events();
        out.push_str(&usart_text(&mut avr, 0));
        assert!(avr.cycle() < ms(100.0), "timeout {out}");
        let ssl = output_level(&avr, ss);
        let sckl = output_level(&avr, sck);
        if ssl != prev_ss {
            prev_ss = ssl;
            if !ssl {
                bits = 0;
                avr.set_pin_input(miso, Some(reply[idx] & 0x80 != 0));
            }
        }
        if !ssl && sckl != prev_sck {
            sck_edges.push(avr.cycle());
            if sckl {
                shift = (shift << 1) | output_level(&avr, mosi) as u8;
                bits += 1;
            } else if bits == 8 {
                got.push(shift);
                bits = 0;
                idx += 1;
                let next = reply.get(idx).copied().unwrap_or(0xFF);
                avr.set_pin_input(miso, Some(next & 0x80 != 0));
            } else {
                let b = reply[idx] >> (7 - bits) & 1;
                avr.set_pin_input(miso, Some(b != 0));
            }
        }
        prev_sck = sckl;
    }
    assert_eq!(got, vec![0xA5, 0x3C]);
    assert_eq!(out, "spi 5A C3\r\n");
    // 1 MHz SCK (SPI clock /16): edges 8 cycles apart inside a byte.
    assert!(
        sck_edges.windows(2).take(15).all(|w| w[1] - w[0] == 8),
        "{:?}",
        &sck_edges[..16]
    );
}

#[test]
fn spi_slave_mode() {
    let (ss, mosi, miso, sck) = (
        Pin::new('B', 2),
        Pin::new('B', 3),
        Pin::new('B', 4),
        Pin::new('B', 5),
    );
    let mut avr = load("spi_slave", "uno");
    for p in [ss, sck, mosi] {
        avr.set_pin_input(p, Some(p != sck));
    }
    let mut ev = Vec::new();
    run_for(&mut avr, ms(20.0), &mut ev);
    avr.set_pin_input(ss, Some(false));
    run_for(&mut avr, 800, &mut ev);
    let half = 160; // 10 µs
    let mut answers = Vec::new();
    for byte in [0x11u8, 0x22] {
        let mut got = 0u8;
        for i in 0..8 {
            avr.set_pin_input(mosi, Some(byte >> (7 - i) & 1 != 0));
            run_for(&mut avr, half, &mut ev);
            got = (got << 1) | output_level(&avr, miso) as u8;
            avr.set_pin_input(sck, Some(true));
            run_for(&mut avr, half, &mut ev);
            avr.set_pin_input(sck, Some(false));
        }
        answers.push(got);
        run_for(&mut avr, 1_600, &mut ev);
    }
    avr.set_pin_input(ss, Some(true));
    run_for(&mut avr, ms(5.0), &mut ev);
    assert_eq!(
        answers,
        vec![0x77, 0x12],
        "preloaded byte, then received + 1"
    );
    assert_eq!(usart_text(&mut avr, 0), "slave got 11\r\nslave got 22\r\n");
}

// ------------------------------------------------------------------ analog comparator

#[test]
fn analog_comparator_edges_and_output() {
    let (ain0, ain1) = (Pin::new('D', 6), Pin::new('D', 7));
    let mut avr = load("acomp", "uno");
    avr.set_pin_voltage(ain0, 1.0);
    avr.set_pin_voltage(ain1, 2.0);
    let mut ev = Vec::new();
    run_for(&mut avr, ms(5.0), &mut ev);
    let mut ask = |avr: &mut Avr| -> String {
        avr.usart_inject(0, b"?");
        let mut ev = Vec::new();
        run_for(avr, ms(3.0), &mut ev);
        usart_text(avr, 0)
    };
    assert_eq!(ask(&mut avr), "aco=0 rises=0\r\n");
    avr.set_pin_voltage(ain0, 3.0);
    assert_eq!(ask(&mut avr), "aco=1 rises=1\r\n");
    // Each change gets time for the ISR (a second edge while ACI is still set counts once, as on
    // hardware).
    for (pin, v) in [(ain0, 1.5), (ain0, 2.5), (ain1, 2.6), (ain1, 0.1)] {
        avr.set_pin_voltage(pin, v);
        run_for(&mut avr, 1_000, &mut ev);
    }
    assert_eq!(ask(&mut avr), "aco=1 rises=3\r\n");
}
