//! USART0–3 in asynchronous mode: 5–9 data bits, none/even/odd parity, 1–2 stop bits, U2X,
//! multi-processor mode, double-buffered transmitter, 2-level receive FIFO with FE/DOR/UPE,
//! RXC/UDRE/TXC interrupts.
//!
//! TX drives the TXD pin bit by bit (exact edge cycles) unless the waveform is disabled; every
//! byte is also logged for the fast-path serial monitor. RX samples the RXD pin level in the
//! middle of each bit after a falling start edge, or receives injected bytes directly.

use crate::avr::{Avr, NEVER, NO_VEC};
use crate::io::UReg;
use crate::variant::{Irq, UsartDesc, Variant};
use crate::{PinOwner, UsartByte};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TxFrame {
    bits: u16,
    nbits: u8,
    start: u64,
    bit_cycles: u64,
    next_bit: u8,
    data: u16,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RxFrame {
    start: u64,
    bit_cycles: u64,
    parity: u8,
    idx: u8,
    data: u16,
    parity_acc: bool,
    parity_err: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RxEntry {
    data: u16,
    fe: bool,
    dor: bool,
    upe: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Usart {
    pub d: &'static UsartDesc,
    pub ucsra: u8,
    pub ucsrb: u8,
    pub ucsrc: u8,
    pub ubrr: u16,
    pub udr_tx: Option<u16>,
    pub tx: Option<TxFrame>,
    pub txc: bool,
    pub tx_pin_owned: bool,
    pub tx_log: Vec<UsartByte>,
    pub waveform: bool,
    pub fifo: VecDeque<RxEntry>,
    pub rx: Option<RxFrame>,
    pub inject: VecDeque<u8>,
    pub inject_done: u64,
    pub next_event: u64,
    pub vec_rx: u8,
    pub vec_udre: u8,
    pub vec_tx: u8,
    pub gated: bool,
    pub gated_at: u64,
    pub clk_shift: u8,
}

impl Usart {
    pub fn new(d: &'static UsartDesc, v: &Variant) -> Usart {
        let vo = |irq| v.vector_of(irq).unwrap_or(NO_VEC);
        Usart {
            d,
            ucsra: 0,
            ucsrb: 0,
            ucsrc: 0x06,
            ubrr: 0,
            udr_tx: None,
            tx: None,
            txc: false,
            tx_pin_owned: false,
            tx_log: Vec::new(),
            waveform: true,
            fifo: VecDeque::new(),
            rx: None,
            inject: VecDeque::new(),
            inject_done: NEVER,
            next_event: NEVER,
            vec_rx: vo(Irq::UsartRx(d.n)),
            vec_udre: vo(Irq::UsartUdre(d.n)),
            vec_tx: vo(Irq::UsartTx(d.n)),
            gated: false,
            gated_at: 0,
            clk_shift: 0,
        }
    }

    /// Register reset; the TX log (already-sent bytes) and waveform setting survive.
    pub fn reset(&mut self) {
        let log = std::mem::take(&mut self.tx_log);
        let wf = self.waveform;
        let (d, vr, vu, vt) = (self.d, self.vec_rx, self.vec_udre, self.vec_tx);
        *self = Usart {
            d,
            ucsra: 0,
            ucsrb: 0,
            ucsrc: 0x06,
            ubrr: 0,
            udr_tx: None,
            tx: None,
            txc: false,
            tx_pin_owned: false,
            tx_log: log,
            waveform: wf,
            fifo: VecDeque::new(),
            rx: None,
            inject: VecDeque::new(),
            inject_done: NEVER,
            next_event: NEVER,
            vec_rx: vr,
            vec_udre: vu,
            vec_tx: vt,
            gated: false,
            gated_at: 0,
            clk_shift: 0,
        };
    }

    /// Oscillator cycles per bit.
    pub fn bit_cycles(&self) -> u64 {
        let per = if self.ucsra & 0x02 != 0 { 8 } else { 16 };
        ((self.ubrr as u64 & 0x0FFF) + 1) * per << self.clk_shift
    }

    fn char_bits(&self) -> u8 {
        match ((self.ucsrb >> 2) & 1) << 2 | ((self.ucsrc >> 1) & 3) {
            0 => 5,
            1 => 6,
            2 => 7,
            7 => 9,
            _ => 8,
        }
    }

    /// 0 = none, 2 = even, 3 = odd.
    fn parity_mode(&self) -> u8 {
        let p = (self.ucsrc >> 4) & 3;
        if p == 1 { 0 } else { p }
    }

    fn stop_bits(&self) -> u8 {
        if self.ucsrc & 0x08 != 0 { 2 } else { 1 }
    }

    fn build_frame(&self, data: u16, start: u64) -> TxFrame {
        let n = self.char_bits();
        let mut bits: u16 = 0;
        let mask = (1u16 << n) - 1;
        let d = data & mask;
        bits |= d << 1;
        let mut pos = 1 + n;
        let pm = self.parity_mode();
        if pm != 0 {
            let mut p = (d.count_ones() & 1) as u16;
            if pm == 3 {
                p ^= 1;
            }
            bits |= p << pos;
            pos += 1;
        }
        for _ in 0..self.stop_bits() {
            bits |= 1 << pos;
            pos += 1;
        }
        TxFrame {
            bits,
            nbits: pos,
            start,
            bit_cycles: self.bit_cycles(),
            next_bit: 1,
            data: d,
        }
    }

    fn txen(&self) -> bool {
        self.ucsrb & 0x08 != 0
    }
    fn rxen(&self) -> bool {
        self.ucsrb & 0x10 != 0
    }
}

impl Avr {
    fn usart_update_irq(&mut self, i: usize) {
        let u = &self.usarts[i];
        let rx = !u.fifo.is_empty() && u.ucsrb & 0x80 != 0;
        let udre = u.udr_tx.is_none() && u.ucsrb & 0x20 != 0;
        let tx = u.txc && u.ucsrb & 0x40 != 0;
        let (a, b, c) = (u.vec_rx, u.vec_udre, u.vec_tx);
        self.set_irq(a, rx);
        self.set_irq(b, udre);
        self.set_irq(c, tx);
    }

    fn usart_update_next(&mut self, i: usize) {
        let u = &mut self.usarts[i];
        let mut t = NEVER;
        if !u.gated {
            if let Some(f) = &u.tx {
                let k = if u.waveform { f.next_bit } else { f.nbits };
                t = t.min(f.start + k as u64 * f.bit_cycles);
            }
            if let Some(r) = &u.rx {
                t = t.min(r.start + r.bit_cycles / 2 + r.idx as u64 * r.bit_cycles);
            }
            t = t.min(u.inject_done);
        }
        u.next_event = t;
        self.schedule(t);
    }

    fn usart_start_tx(&mut self, i: usize, at: u64) {
        let u = &mut self.usarts[i];
        if u.tx.is_some() || !u.txen() {
            return;
        }
        let Some(data) = u.udr_tx.take() else { return };
        u.clk_shift = self.sys.clk_shift;
        let f = u.build_frame(data, at);
        u.tx = Some(f);
        let wf = u.waveform;
        let pin = u.d.tx;
        if wf {
            self.set_override_level(pin, false, at);
        }
        self.usart_update_irq(i);
    }

    pub(crate) fn usart_event(&mut self, i: usize, t: u64) {
        // TX
        if let Some(f) = self.usarts[i].tx {
            let wf = self.usarts[i].waveform;
            let k = if wf { f.next_bit } else { f.nbits };
            let at = f.start + k as u64 * f.bit_cycles;
            if at == t {
                if k >= f.nbits {
                    let u = &mut self.usarts[i];
                    u.tx_log.push(UsartByte {
                        cycle: at,
                        data: f.data,
                    });
                    u.tx = None;
                    let pin = u.d.tx;
                    if wf {
                        self.set_override_level(pin, true, at);
                    }
                    if self.usarts[i].udr_tx.is_some() && self.usarts[i].txen() {
                        self.usart_start_tx(i, at);
                    } else {
                        self.usarts[i].txc = true;
                        if !self.usarts[i].txen() {
                            self.usart_release_tx(i, at);
                        }
                    }
                    self.usart_update_irq(i);
                } else {
                    let level = f.bits & (1 << k) != 0;
                    let pin = self.usarts[i].d.tx;
                    self.set_override_level(pin, level, at);
                    if let Some(f) = &mut self.usarts[i].tx {
                        f.next_bit += 1;
                    }
                }
            }
        }
        // RX (pin-level frame)
        if let Some(r) = self.usarts[i].rx {
            let at = r.start + r.bit_cycles / 2 + r.idx as u64 * r.bit_cycles;
            if at == t {
                let level = self.pin_level(self.usarts[i].d.rx);
                self.usart_rx_sample(i, level, at);
            }
        }
        // RX (injected bytes)
        if self.usarts[i].inject_done == t {
            if let Some(b) = self.usarts[i].inject.pop_front() {
                self.usart_push_rx(
                    i,
                    RxEntry {
                        data: b as u16,
                        fe: false,
                        dor: false,
                        upe: false,
                    },
                );
            }
            self.usart_schedule_inject(i, t);
        }
        self.usart_update_next(i);
    }

    fn usart_rx_sample(&mut self, i: usize, level: bool, at: u64) {
        let Some(mut r) = self.usarts[i].rx else {
            return;
        };
        let nchar = self.usarts[i].char_bits();
        let idx = r.idx;
        if idx == 0 {
            if level {
                // False start bit.
                self.usarts[i].rx = None;
                return;
            }
        } else if idx <= nchar {
            if level {
                r.data |= 1 << (idx - 1);
                r.parity_acc ^= true;
            }
        } else if r.parity != 0 && idx == nchar + 1 {
            let expect_odd = r.parity == 3;
            if (r.parity_acc ^ level) != expect_odd {
                r.parity_err = true;
            }
        } else {
            // First stop bit: frame complete.
            self.usarts[i].rx = None;
            let mpcm = self.usarts[i].ucsra & 0x01 != 0;
            if mpcm {
                let addr = if nchar == 9 {
                    r.data & 0x100 != 0
                } else {
                    level
                };
                if !addr {
                    let _ = at;
                    return;
                }
            }
            self.usart_push_rx(
                i,
                RxEntry {
                    data: r.data,
                    fe: !level,
                    dor: false,
                    upe: r.parity_err,
                },
            );
            // The line may already be low again (back-to-back start bit seen at the stop bit).
            return;
        }
        r.idx += 1;
        self.usarts[i].rx = Some(r);
    }

    fn usart_push_rx(&mut self, i: usize, e: RxEntry) {
        let u = &mut self.usarts[i];
        if !u.rxen() {
            return;
        }
        if u.fifo.len() >= 2 {
            if let Some(last) = u.fifo.back_mut() {
                last.dor = true;
            }
        } else {
            u.fifo.push_back(e);
        }
        self.usart_update_irq(i);
    }

    pub(crate) fn usart_rx_edge(&mut self, i: usize, level: bool, at: u64) {
        if i >= self.usarts.len() {
            return;
        }
        let u = &mut self.usarts[i];
        if level || !u.rxen() || u.rx.is_some() || u.gated {
            return;
        }
        u.clk_shift = self.sys.clk_shift;
        u.rx = Some(RxFrame {
            start: at,
            bit_cycles: u.bit_cycles(),
            parity: u.parity_mode(),
            idx: 0,
            data: 0,
            parity_acc: false,
            parity_err: false,
        });
        self.usart_update_next(i);
    }

    pub(crate) fn usart_inject_bytes(&mut self, i: usize, bytes: &[u8], now: u64) {
        self.usarts[i].inject.extend(bytes.iter().copied());
        if self.usarts[i].inject_done == NEVER {
            self.usart_schedule_inject(i, now);
        }
        self.usart_update_next(i);
    }

    fn usart_schedule_inject(&mut self, i: usize, from: u64) {
        let u = &mut self.usarts[i];
        u.clk_shift = self.sys.clk_shift;
        u.inject_done = if u.inject.is_empty() {
            NEVER
        } else {
            let len = 1 + u.char_bits() as u64 + (u.parity_mode() != 0) as u64 + 1;
            from + len * u.bit_cycles()
        };
    }

    fn usart_release_tx(&mut self, i: usize, at: u64) {
        if self.usarts[i].tx_pin_owned {
            self.usarts[i].tx_pin_owned = false;
            let pin = self.usarts[i].d.tx;
            let n = self.usarts[i].d.n;
            self.clear_pin_override(pin, PinOwner::UsartTx(n), at);
        }
    }

    pub(crate) fn usart_ack_txc(&mut self, i: usize) {
        if let Some(u) = self.usarts.get_mut(i) {
            u.txc = false;
            self.usart_update_irq(i);
        }
    }

    pub(crate) fn usart_set_gate(&mut self, i: usize, gated: bool, now: u64) {
        let u = &mut self.usarts[i];
        if u.gated == gated {
            return;
        }
        if gated {
            u.gated = true;
            u.gated_at = now;
        } else {
            u.gated = false;
            let dt = now - u.gated_at;
            if let Some(f) = &mut u.tx {
                f.start += dt;
            }
            if let Some(r) = &mut u.rx {
                r.start += dt;
            }
            if u.inject_done != NEVER {
                u.inject_done += dt;
            }
        }
        self.usart_update_next(i);
    }

    fn ucsra_value(&self, i: usize) -> u8 {
        let u = &self.usarts[i];
        let mut v = u.ucsra & 0x03;
        if let Some(e) = u.fifo.front() {
            v |= 0x80;
            if e.fe {
                v |= 0x10;
            }
            if e.dor {
                v |= 0x08;
            }
            if e.upe {
                v |= 0x04;
            }
        }
        if u.txc {
            v |= 0x40;
        }
        if u.udr_tx.is_none() {
            v |= 0x20;
        }
        v
    }

    pub(crate) fn usart_peek(&self, i: usize, r: UReg) -> u8 {
        let Some(u) = self.usarts.get(i) else {
            return 0;
        };
        match r {
            UReg::A => self.ucsra_value(i),
            UReg::B => {
                let rxb8 = u.fifo.front().map(|e| (e.data >> 8) as u8 & 1).unwrap_or(0);
                (u.ucsrb & !0x02) | (rxb8 << 1)
            }
            UReg::C => u.ucsrc,
            UReg::BrrL => u.ubrr as u8,
            UReg::BrrH => (u.ubrr >> 8) as u8 & 0x0F,
            UReg::Udr => u.fifo.front().map(|e| e.data as u8).unwrap_or(0),
        }
    }

    pub(crate) fn usart_read(&mut self, i: usize, r: UReg) -> u8 {
        if i >= self.usarts.len() {
            return 0;
        }
        if r == UReg::Udr {
            let v = self.usarts[i]
                .fifo
                .pop_front()
                .map(|e| e.data as u8)
                .unwrap_or(0);
            self.usart_update_irq(i);
            return v;
        }
        self.usart_peek(i, r)
    }

    pub(crate) fn usart_write(&mut self, i: usize, r: UReg, v: u8) {
        if i >= self.usarts.len() {
            return;
        }
        let now = self.cycle;
        match r {
            UReg::A => {
                let u = &mut self.usarts[i];
                if v & 0x40 != 0 {
                    u.txc = false;
                }
                u.ucsra = v & 0x03;
            }
            UReg::B => {
                let old = self.usarts[i].ucsrb;
                self.usarts[i].ucsrb = v & !0x02;
                let (rxp, txp, n) = (self.usarts[i].d.rx, self.usarts[i].d.tx, self.usarts[i].d.n);
                if v & 0x08 != 0 && old & 0x08 == 0 {
                    self.usarts[i].tx_pin_owned = true;
                    self.set_pin_override(txp, PinOwner::UsartTx(n), Some(true), Some(true), now);
                    self.usart_start_tx(i, now);
                } else if v & 0x08 == 0
                    && old & 0x08 != 0
                    && self.usarts[i].tx.is_none()
                    && self.usarts[i].udr_tx.is_none()
                {
                    self.usart_release_tx(i, now);
                }
                if v & 0x10 != 0 && old & 0x10 == 0 {
                    self.set_pin_override(rxp, PinOwner::UsartRx(n), Some(false), None, now);
                } else if v & 0x10 == 0 && old & 0x10 != 0 {
                    let u = &mut self.usarts[i];
                    u.rx = None;
                    u.fifo.clear();
                    self.clear_pin_override(rxp, PinOwner::UsartRx(n), now);
                }
            }
            UReg::C => self.usarts[i].ucsrc = v,
            UReg::BrrL => {
                let u = &mut self.usarts[i];
                u.ubrr = (u.ubrr & 0x0F00) | v as u16;
            }
            UReg::BrrH => {
                let u = &mut self.usarts[i];
                u.ubrr = (u.ubrr & 0x00FF) | (((v & 0x0F) as u16) << 8);
            }
            UReg::Udr => {
                let u = &mut self.usarts[i];
                if u.udr_tx.is_none() {
                    let b8 = (u.ucsrb & 1) as u16;
                    u.udr_tx = Some(v as u16 | (b8 << 8));
                    self.usart_start_tx(i, now);
                }
            }
        }
        self.usart_update_irq(i);
        self.usart_update_next(i);
    }
}
