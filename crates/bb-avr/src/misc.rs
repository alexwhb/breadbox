//! EEPROM (with programming time), watchdog (interrupt / reset / both), external interrupts
//! INTn and pin-change interrupts, system control registers (MCUCR, MCUSR, SMCR, CLKPR, PRR)
//! and self-programming (SPM page buffer / erase / write, signature & fuse reads).

use crate::avr::{Avr, CpuState, NEVER, NO_VEC};
use crate::variant::{Irq, Variant};
use crate::{ResetCause, SleepMode};

// ---------------------------------------------------------------------------------- EEPROM

#[derive(Debug, Clone)]
pub(crate) struct Eeprom {
    pub eecr: u8,
    pub eedr: u8,
    pub eear: u16,
    pub mpe_until: u64,
    pub busy_until: u64,
    pub pending: Option<(u16, u8, u8)>,
    pub next_event: u64,
}

impl Default for Eeprom {
    fn default() -> Self {
        Eeprom {
            eecr: 0,
            eedr: 0,
            eear: 0,
            mpe_until: 0,
            busy_until: 0,
            pending: None,
            next_event: NEVER,
        }
    }
}

impl Eeprom {
    pub fn reset(&mut self) {
        // A write in progress completes (the EEPROM is not part of the reset domain).
        let (busy, pending, ne) = (self.busy_until, self.pending, self.next_event);
        *self = Eeprom::default();
        self.busy_until = busy;
        self.pending = pending;
        self.next_event = ne;
    }
}

impl Avr {
    fn ee_busy(&self) -> bool {
        self.ee.pending.is_some() && self.cycle < self.ee.busy_until
    }

    fn ee_update_irq(&mut self) {
        let on = self.ee.eecr & 0x08 != 0 && self.ee.pending.is_none();
        let v = self.vector_of(Irq::EeReady);
        self.set_irq(v, on);
    }

    pub(crate) fn ee_peek_cr(&self) -> u8 {
        let mut v = self.ee.eecr & 0x38;
        if self.cycle <= self.ee.mpe_until {
            v |= 0x04;
        }
        if self.ee.pending.is_some() {
            v |= 0x02;
        }
        v
    }

    pub(crate) fn ee_write_cr(&mut self, v: u8) {
        let now = self.cycle;
        let mpe_open = now <= self.ee.mpe_until;
        self.ee.eecr = v & 0x38;
        if v & 0x04 != 0 && v & 0x02 == 0 {
            self.ee.mpe_until = now + (4u64 << self.sys.clk_shift);
        }
        if v & 0x02 != 0 && mpe_open && self.ee.pending.is_none() {
            let mode = (v >> 4) & 3;
            let ms = match mode {
                1 | 2 => 1.8e-3,
                _ => 3.4e-3,
            };
            let dur = (ms * self.cfg.f_cpu as f64).round() as u64;
            let addr = self.ee.eear % self.eeprom.len() as u16;
            self.ee.pending = Some((addr, self.ee.eedr, mode));
            self.ee.busy_until = now + dur;
            self.ee.next_event = self.ee.busy_until;
            self.ee.mpe_until = 0;
            self.schedule(self.ee.busy_until);
            // CPU halted 2 cycles.
            self.cycle += 2;
        }
        if v & 0x01 != 0 && self.ee.pending.is_none() {
            let a = self.ee.eear as usize % self.eeprom.len();
            self.ee.eedr = self.eeprom[a];
            // CPU halted 4 cycles.
            self.cycle += 4;
        }
        self.ee_update_irq();
    }

    pub(crate) fn ee_write_ar(&mut self, v: u16) {
        if !self.ee_busy() {
            let mask = (self.eeprom.len() - 1) as u16;
            self.ee.eear = v & mask;
        }
    }

    pub(crate) fn ee_event(&mut self, t: u64) {
        if let Some((a, d, mode)) = self.ee.pending.take() {
            let cell = &mut self.eeprom[a as usize];
            *cell = match mode {
                1 => 0xFF,
                2 => *cell & d,
                _ => d,
            };
        }
        let _ = t;
        self.ee.next_event = NEVER;
        self.ee_update_irq();
    }
}

// ---------------------------------------------------------------------------------- watchdog

#[derive(Debug, Clone)]
pub(crate) struct Wdt {
    pub wdtcsr: u8,
    pub wdif: bool,
    pub wdce_until: u64,
    pub epoch: u64,
    pub next_event: u64,
    pub vec: u8,
}

impl Wdt {
    pub fn new(v: &Variant) -> Wdt {
        Wdt {
            wdtcsr: 0,
            wdif: false,
            wdce_until: 0,
            epoch: 0,
            next_event: NEVER,
            vec: v.vector_of(Irq::Wdt).unwrap_or(NO_VEC),
        }
    }
    pub fn reset(&mut self, now: u64, after_wdt_reset: bool) {
        self.wdtcsr = if after_wdt_reset { 0x08 } else { 0 };
        self.wdif = false;
        self.wdce_until = 0;
        self.epoch = now;
        self.next_event = NEVER;
    }
    pub fn disable(&mut self) {
        self.wdtcsr = 0;
        self.wdif = false;
        self.next_event = NEVER;
    }
    fn active(&self) -> bool {
        self.wdtcsr & 0x48 != 0
    }
    fn prescale(&self) -> u32 {
        let p = (((self.wdtcsr >> 2) & 0x08) | (self.wdtcsr & 0x07)) as u32;
        p.min(9)
    }
}

impl Avr {
    /// Watchdog timeout in CPU cycles (128 kHz oscillator, 2K…1024K cycles).
    pub(crate) fn wdt_timeout_cycles(&self) -> u64 {
        let wdt_cycles = 2048u64 << self.wdt.prescale();
        wdt_cycles * self.cfg.f_cpu / 128_000
    }

    fn wdt_reschedule(&mut self) {
        self.wdt.next_event = if self.wdt.active() {
            self.wdt.epoch + self.wdt_timeout_cycles()
        } else {
            NEVER
        };
        let t = self.wdt.next_event;
        self.schedule(t);
    }

    fn wdt_update_irq(&mut self) {
        let on = self.wdt.wdif && self.wdt.wdtcsr & 0x40 != 0;
        let v = self.wdt.vec;
        self.set_irq(v, on);
    }

    pub(crate) fn wdt_reset_counter(&mut self) {
        self.wdt.epoch = self.cycle;
        if self.wdt.active() {
            self.wdt_reschedule();
        }
    }

    pub(crate) fn wdt_peek(&self) -> u8 {
        let mut v = self.wdt.wdtcsr & 0x6F;
        if self.wdt.wdif {
            v |= 0x80;
        }
        if self.cycle <= self.wdt.wdce_until {
            v |= 0x10;
        }
        v
    }

    pub(crate) fn wdt_write(&mut self, v: u8) {
        let now = self.cycle;
        let was_active = self.wdt.active();
        if v & 0x80 != 0 {
            self.wdt.wdif = false;
        }
        let in_window = now <= self.wdt.wdce_until;
        let wdrf = self.sys.mcusr & 0x08 != 0;
        if v & 0x18 == 0x18 && !in_window {
            // Start of the timed sequence.
            self.wdt.wdce_until = now + (4u64 << self.sys.clk_shift);
            self.wdt.wdtcsr = (self.wdt.wdtcsr & !0x40) | (v & 0x40);
        } else if in_window {
            let mut nv = v & 0x6F;
            if wdrf {
                nv |= 0x08;
            }
            self.wdt.wdtcsr = nv;
            self.wdt.wdce_until = 0;
        } else {
            // Outside the sequence: WDIE free, WDE can only be set.
            let mut nv = (self.wdt.wdtcsr & !0x40) | (v & 0x40);
            if v & 0x08 != 0 {
                nv |= 0x08;
            }
            self.wdt.wdtcsr = nv;
        }
        if !was_active && self.wdt.active() {
            self.wdt.epoch = now;
        }
        self.wdt_reschedule();
        self.wdt_update_irq();
    }

    pub(crate) fn wdt_event(&mut self, t: u64) {
        if !self.wdt.active() {
            self.wdt.next_event = NEVER;
            return;
        }
        self.wdt.epoch = t;
        if self.wdt.wdtcsr & 0x40 != 0 {
            self.wdt.wdif = true;
            self.wdt_update_irq();
            self.wdt_reschedule();
        } else {
            // WDE only: system reset.
            self.reset(ResetCause::Watchdog);
        }
    }

    pub(crate) fn wdt_ack(&mut self) {
        self.wdt.wdif = false;
        // Interrupt + reset mode: hardware clears WDIE so the next timeout resets.
        if self.wdt.wdtcsr & 0x08 != 0 {
            self.wdt.wdtcsr &= !0x40;
        }
        self.wdt_update_irq();
    }
}

// ---------------------------------------------------------------------------------- INTn / PCINT

#[derive(Debug, Clone)]
pub(crate) struct ExtInt {
    pub eicr: [u8; 2],
    pub eimsk: u8,
    pub eifr: u8,
    pub pcicr: u8,
    pub pcifr: u8,
    pub pcmsk: [u8; 4],
    pub level_low: u8,
    pub count: usize,
    pub vec_int: [u8; 8],
    pub vec_pc: [u8; 3],
}

impl ExtInt {
    pub fn new(v: &Variant) -> ExtInt {
        let mut vec_int = [NO_VEC; 8];
        for (n, slot) in vec_int.iter_mut().enumerate().take(v.ext_ints.len()) {
            *slot = v.vector_of(Irq::Int(n as u8)).unwrap_or(NO_VEC);
        }
        let mut vec_pc = [NO_VEC; 3];
        for (g, slot) in vec_pc.iter_mut().enumerate() {
            *slot = v.vector_of(Irq::Pcint(g as u8)).unwrap_or(NO_VEC);
        }
        ExtInt {
            eicr: [0; 2],
            eimsk: 0,
            eifr: 0,
            pcicr: 0,
            pcifr: 0,
            pcmsk: [0; 4],
            level_low: 0xFF,
            count: v.ext_ints.len(),
            vec_int,
            vec_pc,
        }
    }
    pub fn reset(&mut self) {
        self.eicr = [0; 2];
        self.eimsk = 0;
        self.eifr = 0;
        self.pcicr = 0;
        self.pcifr = 0;
        self.pcmsk = [0; 4];
    }
    fn sense(&self, n: u8) -> u8 {
        (self.eicr[(n / 4) as usize] >> (2 * (n % 4))) & 3
    }
}

impl Avr {
    fn exint_update_irq(&mut self, n: u8) {
        let e = &self.exint;
        let m = 1u8 << n;
        let on = e.eimsk & m != 0
            && if e.sense(n) == 0 {
                e.level_low & m != 0
            } else {
                e.eifr & m != 0
            };
        let v = e.vec_int[n as usize];
        self.set_irq(v, on);
    }

    fn exint_update_all(&mut self) {
        for n in 0..self.exint.count as u8 {
            self.exint_update_irq(n);
        }
        for g in 0..3u8 {
            let on = self.exint.pcifr & self.exint.pcicr & (1 << g) != 0;
            let v = self.exint.vec_pc[g as usize];
            self.set_irq(v, on);
        }
    }

    /// Edge detection for INTn needs the I/O clock except on asynchronous INT pins.
    fn exint_edges_detectable(&self, n: u8) -> bool {
        match self.state {
            CpuState::Sleeping(SleepMode::Idle) | CpuState::Running | CpuState::Halted { .. } => {
                true
            }
            CpuState::Sleeping(_) => self.v.ext_int_async_edges & (1 << n) != 0,
            CpuState::InReset { .. } => false,
        }
    }

    pub(crate) fn exint_pin_change(&mut self, n: u8, level: bool, at: u64) {
        if n as usize >= self.exint.count {
            return;
        }
        let m = 1u8 << n;
        if level {
            self.exint.level_low &= !m;
        } else {
            self.exint.level_low |= m;
        }
        let fire = match self.exint.sense(n) {
            0 => false,
            1 => true,
            2 => !level,
            _ => level,
        };
        if fire && self.exint_edges_detectable(n) {
            let rose = self.exint.eifr & m == 0;
            self.exint.eifr |= m;
            if n == 0 && rose {
                self.adc_auto_trigger(2, at);
            }
        }
        self.exint_update_irq(n);
    }

    pub(crate) fn pcint_pin_change(&mut self, g: u8, b: u8, _at: u64) {
        if self.exint.pcmsk[g as usize] & (1 << b) != 0 {
            self.exint.pcifr |= 1 << g;
            let on = self.exint.pcicr & (1 << g) != 0;
            let v = self.exint.vec_pc[g as usize];
            self.set_irq(v, on);
        }
    }

    pub(crate) fn exint_ack(&mut self, n: u8) {
        if self.exint.sense(n) != 0 {
            self.exint.eifr &= !(1 << n);
        }
        self.exint_update_irq(n);
    }

    pub(crate) fn pcint_ack(&mut self, g: u8) {
        self.exint.pcifr &= !(1 << g);
        let v = self.exint.vec_pc[g as usize];
        self.set_irq(v, false);
    }

    pub(crate) fn eicr_write(&mut self, i: usize, v: u8) {
        self.exint.eicr[i] = v;
        // Level mode keeps INTFn cleared.
        for n in 0..self.exint.count as u8 {
            if self.exint.sense(n) == 0 {
                self.exint.eifr &= !(1 << n);
            }
        }
        self.exint_update_all();
    }
    pub(crate) fn eimsk_write(&mut self, v: u8) {
        self.exint.eimsk = v;
        self.exint_update_all();
    }
    pub(crate) fn eifr_write(&mut self, v: u8) {
        self.exint.eifr &= !v;
        self.exint_update_all();
    }
    pub(crate) fn pcicr_write(&mut self, v: u8) {
        self.exint.pcicr = v & 7;
        self.exint_update_all();
    }
    pub(crate) fn pcifr_write(&mut self, v: u8) {
        self.exint.pcifr &= !v;
        self.exint_update_all();
    }
}

// ---------------------------------------------------------------------------------- system

#[derive(Debug, Clone)]
pub(crate) struct Sys {
    pub mcusr: u8,
    pub mcucr: u8,
    pub smcr: u8,
    pub clkpr: u8,
    pub clk_shift: u8,
    pub clkpce_until: u64,
    pub prr: [u8; 2],
    pub gtccr: u8,
    pub assr: u8,
    pub psr_sync_epoch: u64,
    pub psr_async_epoch: u64,
    pub spmcsr: u8,
    pub spm_window_until: u64,
    pub spm_busy_until: u64,
    pub spm_buf: Vec<u16>,
    pub aref_tracks_vcc: bool,
    pub ivce_until: u64,
}

impl Default for Sys {
    fn default() -> Self {
        Sys {
            mcusr: 0,
            mcucr: 0,
            smcr: 0,
            clkpr: 0,
            clk_shift: 0,
            clkpce_until: 0,
            prr: [0; 2],
            gtccr: 0,
            assr: 0,
            psr_sync_epoch: 0,
            psr_async_epoch: 0,
            spmcsr: 0,
            spm_window_until: 0,
            spm_busy_until: NEVER,
            spm_buf: Vec::new(),
            aref_tracks_vcc: true,
            ivce_until: 0,
        }
    }
}

impl Sys {
    pub fn reset(&mut self, now: u64) {
        let aref = self.aref_tracks_vcc;
        *self = Sys::default();
        self.aref_tracks_vcc = aref;
        self.psr_sync_epoch = now;
        self.psr_async_epoch = now;
    }
}

impl Avr {
    pub(crate) fn mcusr_write(&mut self, v: u8) {
        self.sys.mcusr &= v;
    }

    pub(crate) fn mcucr_write(&mut self, v: u8) {
        let now = self.cycle;
        let old = self.sys.mcucr;
        let mut nv = (old & 0x03) | (v & 0x70);
        if v & 0x01 != 0 {
            self.sys.ivce_until = now + 4;
            nv |= 0x01;
        } else if now <= self.sys.ivce_until {
            nv = (nv & !0x03) | (v & 0x02);
        }
        self.sys.mcucr = nv;
        if (old ^ nv) & 0x10 != 0 {
            self.refresh_all_ports(now);
        }
    }

    pub(crate) fn clkpr_write(&mut self, v: u8) {
        let now = self.cycle;
        if v == 0x80 {
            self.sys.clkpce_until = now + 4;
            return;
        }
        if now <= self.sys.clkpce_until && v & 0x80 == 0 {
            for i in 0..self.timers.len() {
                self.timer_sync(i, now);
            }
            self.sys.clkpr = v & 0x0F;
            self.sys.clk_shift = (v & 0x0F).min(8);
            self.sys.clkpce_until = 0;
            for i in 0..self.timers.len() {
                self.timer_clock_setup(i, now);
            }
        }
    }

    pub(crate) fn prr_write(&mut self, i: usize, v: u8) {
        let now = self.cycle;
        self.sys.prr[i] = v;
        for t in 0..self.timers.len() {
            let (reg, bit) = self.timers[t].d.prr;
            let idx = if reg == crate::variant::reg::PRR0 {
                0
            } else {
                1
            };
            let gated = self.sys.prr[idx] & (1 << bit) != 0;
            self.timer_set_prr_gate(t, gated, now);
        }
    }

    pub(crate) fn spmcsr_write(&mut self, v: u8) {
        let now = self.cycle;
        // RWWSB (bit 6) is read-only; SPMEN with others opens a 4-cycle window for SPM/LPM.
        self.sys.spmcsr = (self.sys.spmcsr & 0x40) | (v & 0xBF);
        if v & 0x01 != 0 {
            self.sys.spm_window_until = now + 4;
        }
        self.spm_update_irq();
    }

    fn spm_update_irq(&mut self) {
        let on = self.sys.spmcsr & 0x80 != 0 && self.sys.spmcsr & 0x01 == 0;
        let v = self.vector_of(Irq::SpmReady);
        self.set_irq(v, on);
    }

    pub(crate) fn spm_execute(&mut self) {
        let now = self.cycle;
        let cr = self.sys.spmcsr;
        if cr & 0x01 == 0 || now > self.sys.spm_window_until + 1 {
            return;
        }
        let page = self.v.page_bytes;
        if self.sys.spm_buf.len() != page / 2 {
            self.sys.spm_buf = vec![0xFFFF; page / 2];
        }
        let z = (((self.rampz as u32) << 16) | self.reg_pair(30) as u32) as usize;
        let page_base = (z & !(page - 1)) % self.v.flash_bytes;
        let prog_cycles = (4.0e-3 * self.cfg.f_cpu as f64) as u64;
        match cr & 0x3E {
            0x02 => {
                // Page erase.
                for w in &mut self.flash[page_base / 2..page_base / 2 + page / 2] {
                    *w = 0xFFFF;
                }
                self.redecode_range(page_base / 2, page_base / 2 + page / 2);
                self.spm_start_busy(now, prog_cycles, page_base);
            }
            0x04 => {
                // Page write from the temporary buffer.
                let buf = std::mem::replace(&mut self.sys.spm_buf, vec![0xFFFF; page / 2]);
                for (i, w) in buf.iter().enumerate() {
                    self.flash[page_base / 2 + i] = *w;
                }
                self.redecode_range(page_base / 2, page_base / 2 + page / 2);
                self.spm_start_busy(now, prog_cycles, page_base);
            }
            0x10 => {
                // RWW section read enable.
                self.sys.spmcsr &= !0x41;
            }
            0x08 => {
                // Lock bit set: accepted, no effect in the simulator.
                self.sys.spmcsr &= !0x09;
            }
            0x00 => {
                let i = (z % page) / 2;
                self.sys.spm_buf[i] = self.reg_pair(0);
                self.sys.spmcsr &= !0x01;
            }
            _ => {
                self.sys.spmcsr &= !0x01;
            }
        }
        self.spm_update_irq();
    }

    fn spm_start_busy(&mut self, now: u64, dur: u64, page_base: usize) {
        self.sys.spm_busy_until = now + dur;
        if page_base < self.v.nrww_start {
            self.sys.spmcsr |= 0x40;
        }
        // The CPU is halted while programming (as when SPM runs from the boot section).
        self.state = CpuState::Halted { until: now + dur };
        self.schedule(now + dur);
    }

    pub(crate) fn spm_done(&mut self, _t: u64) {
        self.sys.spm_busy_until = NEVER;
        self.sys.spmcsr &= !0x07;
        self.spm_update_irq();
    }

    /// LPM after SPMCSR selected the signature row or fuse/lock bits.
    pub(crate) fn spm_lpm_override(&mut self, z: u32) -> Option<u8> {
        let cr = self.sys.spmcsr;
        if cr & 0x01 == 0 || self.cycle > self.sys.spm_window_until + 3 {
            return None;
        }
        let r = if cr & 0x20 != 0 {
            match z & 0x1F {
                0 => Some(self.v.signature[0]),
                2 => Some(self.v.signature[1]),
                4 => Some(self.v.signature[2]),
                1 => Some(0x9F),
                _ => Some(0xFF),
            }
        } else if cr & 0x08 != 0 {
            let f = self.v.fuses;
            match z & 3 {
                0 => Some(f[0]),
                1 => Some(f[3]),
                2 => Some(f[2]),
                _ => Some(f[1]),
            }
        } else {
            None
        };
        if r.is_some() {
            self.sys.spmcsr &= !0x29;
        }
        r
    }
}
