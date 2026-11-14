//! TWI (I²C) master and slave at the bit level on the SDA/SCL pins (open drain).
//!
//! The master is a small sequencer of micro-operations (drive/release a line, wait half a bit
//! period, wait for SCL to actually go high — clock stretching —, sample SDA). SCL period is
//! `16 + 2·TWBR·4^TWPS` CPU cycles. The slave watches the lines for START/STOP and SCL edges,
//! matches TWAR/TWAMR/general call, ACKs, stretches SCL while TWINT is set, and transmits on
//! SLA+R. Status codes follow the datasheet tables (0x08 … 0xF8).

use crate::PinOwner;
use crate::avr::{Avr, NEVER, NO_VEC};
use crate::io::IoReg;
use crate::variant::{Irq, Variant};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MOp {
    SdaLow,
    SdaRelease,
    /// Drive SDA for a data bit (release for 1).
    SdaBit(bool),
    SclLow,
    SclRelease,
    WaitLow,
    WaitHigh,
    WaitSclHigh,
    /// Sample SDA after sending `bit`; arbitration lost if we released and read low.
    SampleArb(bool),
    SampleAck,
    SampleBit,
    /// Transfer finished: set TWINT with a status computed from what was sampled.
    Done(DoneKind),
    StopDone,
    WaitBusFree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoneKind {
    Start,
    RepStart,
    Addr,
    DataTx,
    DataRx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SState {
    Idle,
    Addr,
    AddrAck {
        rw: bool,
        gc: bool,
    },
    RxWait,
    RxData,
    RxAck {
        acked: bool,
    },
    TxWait,
    TxData,
    TxAck,
    /// Not addressed any more until the next START (after NACK / last byte).
    Done,
}

#[derive(Debug, Clone)]
pub(crate) struct Twi {
    pub twbr: u8,
    pub twps: u8,
    pub twar: u8,
    pub twdr: u8,
    pub twamr: u8,
    pub twcr: u8,
    pub twint: bool,
    pub twwc: bool,
    pub status: u8,
    pub sda_low: bool,
    pub scl_low: bool,
    pub pins_owned: bool,
    pub bus_busy: bool,
    // master
    pub mops: VecDeque<MOp>,
    pub m_wait_until: u64,
    pub m_parked_scl: bool,
    pub m_running: bool,
    pub m_active: bool,
    pub m_read: bool,
    pub m_shift: u8,
    pub m_ack: bool,
    pub m_arb_lost: bool,
    // slave
    pub s_state: SState,
    pub s_shift: u8,
    pub s_bits: u8,
    pub s_tx: u8,
    pub s_last: bool,
    pub s_ack: bool,
    pub s_gc: bool,
    pub s_stretch: bool,
    pub next_event: u64,
    pub vec: u8,
}

impl Twi {
    pub fn new(v: &Variant) -> Twi {
        Twi {
            twbr: 0,
            twps: 0,
            twar: 0xFE,
            twdr: 0xFF,
            twamr: 0,
            twcr: 0,
            twint: false,
            twwc: false,
            status: 0xF8,
            sda_low: false,
            scl_low: false,
            pins_owned: false,
            bus_busy: false,
            mops: VecDeque::new(),
            m_wait_until: NEVER,
            m_parked_scl: false,
            m_running: false,
            m_active: false,
            m_read: false,
            m_shift: 0,
            m_ack: false,
            m_arb_lost: false,
            s_state: SState::Idle,
            s_shift: 0,
            s_bits: 0,
            s_tx: 0,
            s_last: false,
            s_ack: false,
            s_gc: false,
            s_stretch: false,
            next_event: NEVER,
            vec: v.vector_of(Irq::Twi).unwrap_or(NO_VEC),
        }
    }
    pub fn reset(&mut self) {
        let vec = self.vec;
        *self = Twi {
            vec,
            ..Twi::new(&crate::variant::M328P)
        };
    }
    fn enabled(&self) -> bool {
        self.twcr & 0x04 != 0
    }
    fn half_periods(&self) -> (u64, u64) {
        let t = 16 + 2 * self.twbr as u64 * (1u64 << (2 * self.twps as u64));
        let low = t.div_ceil(2);
        (low, t - low)
    }
}

impl Avr {
    fn twi_update_irq(&mut self) {
        let on = self.twi.twint && self.twi.twcr & 0x01 != 0;
        let v = self.twi.vec;
        self.set_irq(v, on);
    }

    fn twi_apply_drive(&mut self, at: u64) {
        let (sda, scl) = (self.v.twi_sda, self.v.twi_scl);
        if !self.twi.enabled() {
            if self.twi.pins_owned {
                self.twi.pins_owned = false;
                self.clear_pin_override(sda, PinOwner::Twi, at);
                self.clear_pin_override(scl, PinOwner::Twi, at);
            }
            return;
        }
        self.twi.pins_owned = true;
        let (sl, cl) = (self.twi.sda_low, self.twi.scl_low);
        self.set_pin_override(sda, PinOwner::Twi, Some(sl), Some(false), at);
        self.set_pin_override(scl, PinOwner::Twi, Some(cl), Some(false), at);
    }

    fn twi_set_sda(&mut self, low: bool, at: u64) {
        if self.twi.sda_low != low {
            self.twi.sda_low = low;
            self.twi_apply_drive(at);
        }
    }

    fn twi_set_scl(&mut self, low: bool, at: u64) {
        if self.twi.scl_low != low {
            self.twi.scl_low = low;
            self.twi_apply_drive(at);
        }
    }

    fn twi_finish(&mut self, status: u8) {
        self.twi.status = status;
        self.twi.twint = true;
        self.twi_update_irq();
    }

    // ------------------------------------------------------------------ master sequencer

    fn twi_queue_start(&mut self, repeated: bool) {
        use MOp::*;
        let q = &mut self.twi.mops;
        if repeated {
            q.extend([
                SdaRelease,
                WaitLow,
                SclRelease,
                WaitSclHigh,
                WaitHigh,
                SdaLow,
                WaitHigh,
                SclLow,
                Done(DoneKind::RepStart),
            ]);
        } else {
            q.extend([WaitBusFree, SdaLow, WaitHigh, SclLow, Done(DoneKind::Start)]);
        }
    }

    fn twi_queue_stop(&mut self) {
        use MOp::*;
        self.twi.mops.extend([
            SdaLow,
            WaitLow,
            SclRelease,
            WaitSclHigh,
            WaitHigh,
            SdaRelease,
            StopDone,
        ]);
    }

    fn twi_queue_tx(&mut self, byte: u8, kind: DoneKind) {
        use MOp::*;
        self.twi.m_shift = byte;
        for i in 0..8 {
            let b = byte >> (7 - i) & 1 != 0;
            self.twi.mops.extend([
                SdaBit(b),
                WaitLow,
                SclRelease,
                WaitSclHigh,
                WaitHigh,
                SampleArb(b),
                SclLow,
            ]);
        }
        self.twi.mops.extend([
            SdaRelease,
            WaitLow,
            SclRelease,
            WaitSclHigh,
            WaitHigh,
            SampleAck,
            SclLow,
            Done(kind),
        ]);
    }

    fn twi_queue_rx(&mut self, ack: bool) {
        use MOp::*;
        self.twi.m_shift = 0;
        for _ in 0..8 {
            self.twi.mops.extend([
                SdaRelease,
                WaitLow,
                SclRelease,
                WaitSclHigh,
                WaitHigh,
                SampleBit,
                SclLow,
            ]);
        }
        self.twi.m_ack = ack;
        self.twi.mops.extend([
            SdaBit(!ack),
            WaitLow,
            SclRelease,
            WaitSclHigh,
            WaitHigh,
            SclLow,
            SdaRelease,
            Done(DoneKind::DataRx),
        ]);
    }

    fn twi_master_run(&mut self, now: u64) {
        if self.twi.m_running {
            return;
        }
        self.twi.m_running = true;
        let (low_t, high_t) = self.twi.half_periods();
        let (low_t, high_t) = (low_t << self.sys.clk_shift, high_t << self.sys.clk_shift);
        let sda_pin = self.v.twi_sda;
        let scl_pin = self.v.twi_scl;
        while let Some(op) = self.twi.mops.front().copied() {
            match op {
                MOp::WaitLow | MOp::WaitHigh => {
                    if self.twi.m_wait_until == NEVER {
                        self.twi.m_wait_until =
                            now + if op == MOp::WaitLow { low_t } else { high_t };
                    }
                    if now < self.twi.m_wait_until {
                        break;
                    }
                    self.twi.m_wait_until = NEVER;
                }
                MOp::WaitSclHigh => {
                    if !self.pin_level(scl_pin) {
                        self.twi.m_parked_scl = true;
                        break;
                    }
                    self.twi.m_parked_scl = false;
                }
                MOp::WaitBusFree => {
                    if self.twi.bus_busy && !self.twi.m_active {
                        break;
                    }
                    self.twi.m_active = true;
                    self.twi.bus_busy = true;
                }
                MOp::SdaLow => self.twi_set_sda(true, now),
                MOp::SdaRelease => self.twi_set_sda(false, now),
                MOp::SdaBit(b) => self.twi_set_sda(!b, now),
                MOp::SclLow => self.twi_set_scl(true, now),
                MOp::SclRelease => self.twi_set_scl(false, now),
                MOp::SampleArb(b) => {
                    if b && !self.pin_level(sda_pin) {
                        // Lost arbitration: release the bus.
                        self.twi.mops.clear();
                        self.twi.m_active = false;
                        self.twi_set_sda(false, now);
                        self.twi_set_scl(false, now);
                        self.twi.m_arb_lost = true;
                        self.twi_finish(0x38);
                        break;
                    }
                }
                MOp::SampleAck => self.twi.m_ack = !self.pin_level(sda_pin),
                MOp::SampleBit => {
                    let b = self.pin_level(sda_pin) as u8;
                    self.twi.m_shift = (self.twi.m_shift << 1) | b;
                }
                MOp::Done(kind) => {
                    let ack = self.twi.m_ack;
                    let status = match kind {
                        DoneKind::Start => 0x08,
                        DoneKind::RepStart => 0x10,
                        DoneKind::Addr => match (self.twi.m_read, ack) {
                            (false, true) => 0x18,
                            (false, false) => 0x20,
                            (true, true) => 0x40,
                            (true, false) => 0x48,
                        },
                        DoneKind::DataTx => {
                            if ack {
                                0x28
                            } else {
                                0x30
                            }
                        }
                        DoneKind::DataRx => {
                            self.twi.twdr = self.twi.m_shift;
                            if ack { 0x50 } else { 0x58 }
                        }
                    };
                    self.twi.mops.pop_front();
                    self.twi_finish(status);
                    continue;
                }
                MOp::StopDone => {
                    self.twi.twcr &= !0x10;
                    self.twi.m_active = false;
                    self.twi.bus_busy = false;
                    self.twi.status = 0xF8;
                }
            }
            self.twi.mops.pop_front();
        }
        self.twi.m_running = false;
        let t = if self.twi.m_wait_until != NEVER {
            self.twi.m_wait_until
        } else {
            NEVER
        };
        self.twi.next_event = t;
        self.schedule(t);
    }

    pub(crate) fn twi_event(&mut self, t: u64) {
        self.twi.next_event = NEVER;
        self.twi_master_run(t);
    }

    // ------------------------------------------------------------------ line monitor / slave

    pub(crate) fn twi_line_change(&mut self, is_sda: bool, level: bool, at: u64) {
        if !self.twi.enabled() {
            return;
        }
        let scl_high = self.pin_level(self.v.twi_scl);
        if is_sda {
            if scl_high {
                if !level {
                    self.twi.bus_busy = true;
                    if !self.twi.m_active {
                        self.twi_slave_start(at);
                    }
                } else {
                    self.twi.bus_busy = false;
                    if !self.twi.m_active {
                        self.twi_slave_stop(at);
                    }
                    if self.twi.mops.front() == Some(&MOp::WaitBusFree) {
                        self.twi_master_run(at);
                    }
                }
            }
            return;
        }
        // SCL edge
        if level && self.twi.m_parked_scl && !self.twi.m_running {
            self.twi.m_parked_scl = false;
            self.twi_master_run(at);
            return;
        }
        if self.twi.m_active {
            return;
        }
        if level {
            self.twi_slave_scl_rise(at);
        } else {
            self.twi_slave_scl_fall(at);
        }
    }

    fn twi_slave_addressed(&self) -> bool {
        !matches!(self.twi.s_state, SState::Idle | SState::Addr | SState::Done)
    }

    fn twi_slave_start(&mut self, at: u64) {
        if self.twi_slave_addressed() {
            self.twi_release_slave(at);
            self.twi_finish(0xA0);
        }
        self.twi.s_state = SState::Addr;
        self.twi.s_shift = 0;
        self.twi.s_bits = 0;
    }

    fn twi_slave_stop(&mut self, at: u64) {
        let was = self.twi_slave_addressed()
            && !matches!(
                self.twi.s_state,
                SState::TxWait | SState::TxData | SState::TxAck
            );
        self.twi_release_slave(at);
        self.twi.s_state = SState::Idle;
        if was {
            self.twi_finish(0xA0);
        }
    }

    fn twi_release_slave(&mut self, at: u64) {
        self.twi.s_stretch = false;
        if !self.twi.m_active {
            self.twi_set_sda(false, at);
            self.twi_set_scl(false, at);
        }
    }

    fn twi_slave_scl_rise(&mut self, _at: u64) {
        let sda = self.pin_level(self.v.twi_sda);
        match self.twi.s_state {
            SState::Addr | SState::RxData => {
                if self.twi.s_bits < 8 {
                    self.twi.s_shift = (self.twi.s_shift << 1) | sda as u8;
                    self.twi.s_bits += 1;
                }
            }
            SState::TxAck => self.twi.s_ack = !sda,
            _ => {}
        }
    }

    fn twi_slave_scl_fall(&mut self, at: u64) {
        match self.twi.s_state {
            SState::Addr if self.twi.s_bits == 8 => {
                let byte = self.twi.s_shift;
                let addr = byte >> 1;
                let rw = byte & 1 != 0;
                let mask = self.twi.twamr >> 1;
                let own = self.twi.twar >> 1;
                let ea = self.twi.twcr & 0x40 != 0;
                let gc = addr == 0 && !rw && self.twi.twar & 1 != 0;
                let hit = ea && (gc || (addr & !mask) == (own & !mask));
                if hit {
                    self.twi_set_sda(true, at);
                    self.twi.s_gc = gc;
                    self.twi.s_state = SState::AddrAck { rw, gc };
                } else {
                    self.twi.s_state = SState::Done;
                }
            }
            SState::AddrAck { rw, gc } => {
                self.twi_set_sda(false, at);
                self.twi.s_stretch = true;
                self.twi_set_scl(true, at);
                if rw {
                    self.twi.s_state = SState::TxWait;
                    self.twi_finish(0xA8);
                } else {
                    self.twi.s_state = SState::RxWait;
                    self.twi_finish(if gc { 0x70 } else { 0x60 });
                }
            }
            SState::RxData if self.twi.s_bits == 8 => {
                let ack = self.twi.twcr & 0x40 != 0;
                if ack {
                    self.twi_set_sda(true, at);
                }
                self.twi.s_state = SState::RxAck { acked: ack };
            }
            SState::RxAck { acked } => {
                self.twi_set_sda(false, at);
                self.twi.twdr = self.twi.s_shift;
                self.twi.s_stretch = true;
                self.twi_set_scl(true, at);
                self.twi.s_state = SState::RxWait;
                let gc = self.twi.s_gc;
                let status = match (gc, acked) {
                    (false, true) => 0x80,
                    (false, false) => 0x88,
                    (true, true) => 0x90,
                    (true, false) => 0x98,
                };
                if !acked {
                    self.twi.s_state = SState::Done;
                }
                self.twi_finish(status);
            }
            SState::TxData => {
                self.twi.s_bits += 1;
                if self.twi.s_bits < 8 {
                    let b = self.twi.s_tx >> (7 - self.twi.s_bits) & 1 != 0;
                    self.twi_set_sda(!b, at);
                } else {
                    self.twi_set_sda(false, at);
                    self.twi.s_state = SState::TxAck;
                }
            }
            SState::TxAck => {
                self.twi.s_stretch = true;
                self.twi_set_scl(true, at);
                let status = if !self.twi.s_ack {
                    0xC0
                } else if self.twi.s_last {
                    0xC8
                } else {
                    0xB8
                };
                self.twi.s_state = if status == 0xB8 {
                    SState::TxWait
                } else {
                    SState::Done
                };
                self.twi_finish(status);
            }
            _ => {}
        }
    }

    /// Software cleared TWINT while the slave was stretching SCL.
    fn twi_slave_continue(&mut self, at: u64) {
        match self.twi.s_state {
            SState::TxWait => {
                self.twi.s_tx = self.twi.twdr;
                self.twi.s_last = self.twi.twcr & 0x40 == 0;
                self.twi.s_bits = 0;
                let b = self.twi.s_tx >> 7 & 1 != 0;
                self.twi_set_sda(!b, at);
                self.twi.s_state = SState::TxData;
            }
            SState::RxWait => {
                self.twi.s_bits = 0;
                self.twi.s_shift = 0;
                self.twi.s_state = SState::RxData;
            }
            _ => {}
        }
        if self.twi.s_stretch {
            self.twi.s_stretch = false;
            self.twi_set_scl(false, at);
        }
    }

    // ------------------------------------------------------------------ registers

    pub(crate) fn twi_peek(&self, r: IoReg) -> u8 {
        let t = &self.twi;
        match r {
            IoReg::Twbr => t.twbr,
            IoReg::Twsr => (t.status & 0xF8) | t.twps,
            IoReg::Twar => t.twar,
            IoReg::Twdr => t.twdr,
            IoReg::Twamr => t.twamr,
            IoReg::Twcr => (t.twcr & 0x75) | (t.twint as u8) << 7 | (t.twwc as u8) << 3,
            _ => 0,
        }
    }

    pub(crate) fn twi_read(&mut self, r: IoReg) -> u8 {
        self.twi_peek(r)
    }

    pub(crate) fn twi_write(&mut self, r: IoReg, v: u8) {
        let now = self.cycle;
        match r {
            IoReg::Twbr => self.twi.twbr = v,
            IoReg::Twsr => self.twi.twps = v & 3,
            IoReg::Twar => self.twi.twar = v,
            IoReg::Twamr => self.twi.twamr = v & 0xFE,
            IoReg::Twdr => {
                if self.twi.twint {
                    self.twi.twdr = v;
                    self.twi.twwc = false;
                } else {
                    self.twi.twwc = true;
                }
            }
            IoReg::Twcr => self.twcr_write(v, now),
            _ => {}
        }
    }

    fn twcr_write(&mut self, v: u8, now: u64) {
        let was_en = self.twi.enabled();
        self.twi.twcr = v & 0x75;
        if !self.twi.enabled() {
            let vec = self.twi.vec;
            let keep = (
                self.twi.twbr,
                self.twi.twps,
                self.twi.twar,
                self.twi.twamr,
                self.twi.twdr,
            );
            self.twi.reset();
            self.twi.vec = vec;
            (
                self.twi.twbr,
                self.twi.twps,
                self.twi.twar,
                self.twi.twamr,
                self.twi.twdr,
            ) = keep;
            self.twi_apply_drive(now);
            self.twi_update_irq();
            return;
        }
        if !was_en {
            self.twi_apply_drive(now);
        }
        if v & 0x80 != 0 && self.twi.twint {
            self.twi.twint = false;
            self.twi_update_irq();
            let sta = v & 0x20 != 0;
            let sto = v & 0x10 != 0;
            if self.twi.m_active {
                let st = self.twi.status;
                if sto {
                    self.twi_queue_stop();
                    if sta {
                        self.twi_queue_start(false);
                    }
                } else if sta {
                    self.twi_queue_start(true);
                } else {
                    match st {
                        0x08 | 0x10 => {
                            let b = self.twi.twdr;
                            self.twi.m_read = b & 1 != 0;
                            self.twi_queue_tx(b, DoneKind::Addr);
                        }
                        0x18 | 0x28 | 0x30 => {
                            let b = self.twi.twdr;
                            self.twi_queue_tx(b, DoneKind::DataTx);
                        }
                        0x20 | 0x48 => {}
                        0x40 | 0x50 => {
                            let ack = v & 0x40 != 0;
                            self.twi_queue_rx(ack);
                        }
                        _ => {}
                    }
                }
                self.twi_master_run(now);
            } else if self.twi.s_stretch
                || matches!(self.twi.s_state, SState::TxWait | SState::RxWait)
            {
                if sto {
                    // Recover from error / leave addressed mode.
                    self.twi.s_state = SState::Idle;
                    self.twi_release_slave(now);
                    self.twi.twcr &= !0x10;
                } else {
                    self.twi_slave_continue(now);
                }
                if sta && !sto {
                    self.twi_queue_start(false);
                    self.twi_master_run(now);
                }
            } else {
                if sto {
                    self.twi.twcr &= !0x10;
                }
                if sta {
                    self.twi_queue_start(false);
                    self.twi_master_run(now);
                }
                if matches!(self.twi.s_state, SState::Done) {
                    self.twi.s_state = SState::Idle;
                }
            }
        } else if v & 0x80 != 0
            && !self.twi.twint
            && v & 0x20 != 0
            && !self.twi.m_active
            && self.twi.mops.is_empty()
        {
            // START requested from idle (TWINT was already clear).
            self.twi_queue_start(false);
            self.twi_master_run(now);
        } else if v & 0x80 != 0 && v & 0x10 != 0 && !self.twi.m_active {
            self.twi.twcr &= !0x10;
        }
    }
}
