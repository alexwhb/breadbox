//! The `Avr` machine: CPU state, memories, peripherals, the run loop and the public API.

use crate::adc::{AComp, Adc};
use crate::debug::{CallFrame, Debug, StepFilter, Watchpoint};
use crate::decode::{Op, decode};
use crate::gpio::{PinFunc, Port};
use crate::io::IoReg;
use crate::loader::{FirmwareImage, LoadError};
use crate::misc::{Eeprom, ExtInt, Sys, Wdt};
use crate::spi::Spi;
use crate::timer::Timer;
use crate::twi::Twi;
use crate::usart::Usart;
use crate::variant::{Irq, Variant};
use crate::{
    AvrConfig, AvrIo, Bootloader, Mcu, Pin, PinEvent, PinInfo, ResetCause, SleepMode, StopReason,
    UsartByte,
};
use bb_core::SimTime;

pub(crate) const FLAG_C: u8 = 0x01;
pub(crate) const FLAG_Z: u8 = 0x02;
pub(crate) const FLAG_N: u8 = 0x04;
pub(crate) const FLAG_V: u8 = 0x08;
pub(crate) const FLAG_S: u8 = 0x10;
pub(crate) const FLAG_H: u8 = 0x20;
pub(crate) const FLAG_T: u8 = 0x40;
pub(crate) const FLAG_I: u8 = 0x80;

pub(crate) const NO_VEC: u8 = 0xFF;
pub(crate) const NEVER: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CpuState {
    Running,
    Sleeping(SleepMode),
    /// CPU clock halted until `until` (wake-up start-up, EEPROM/SPM stalls).
    Halted {
        until: u64,
    },
    /// Held in reset (reset pin low, brown-out, power off). `release` = cycle the reset ends,
    /// with the cause recorded in MCUSR.
    InReset {
        release: Option<u64>,
        cause: ResetCause,
    },
}

/// Stop-reason priority: a higher value replaces a pending lower one.
fn stop_priority(r: &StopReason) -> u8 {
    match r {
        StopReason::ReachedCycle => 0,
        StopReason::PinChange => 1,
        StopReason::Sleep { .. } => 2,
        StopReason::Reset { .. } => 3,
        StopReason::Paused => 4,
        StopReason::StepDone => 5,
        StopReason::Watchpoint { .. } => 6,
        StopReason::Breakpoint { .. } => 7,
        StopReason::Break { .. } => 7,
        StopReason::Fault { .. } => 8,
        StopReason::AdcSampleNeeded { .. } => 9,
    }
}

/// One emulated AVR microcontroller. Instances are fully independent (no globals), `Send`, and
/// `Clone` (a clone is a snapshot).
#[derive(Clone)]
pub struct Avr {
    pub(crate) v: &'static Variant,
    pub(crate) cfg: AvrConfig,
    // ---- CPU
    pub(crate) regs: [u8; 32],
    pub(crate) sreg: u8,
    pub(crate) sp: u16,
    /// Word address.
    pub(crate) pc: u32,
    pub(crate) pc_mask: u32,
    pub(crate) rampz: u8,
    pub(crate) eind: u8,
    pub(crate) cycle: u64,
    pub(crate) loop_limit: u64,
    pub(crate) state: CpuState,
    pub(crate) irq_skip_one: bool,
    /// Bit n set = interrupt vector n requested (flag & enable).
    pub(crate) pending: u64,
    pub(crate) instret: u64,
    // ---- memories
    pub(crate) flash: Vec<u16>,
    pub(crate) ops: Vec<Op>,
    /// Data space 0..=RAMEND (registers live in `regs`; I/O plain registers and SRAM here).
    pub(crate) data: Vec<u8>,
    pub(crate) eeprom: Vec<u8>,
    pub(crate) io_map: Vec<IoReg>,
    pub(crate) sram_start: usize,
    // ---- peripherals
    pub(crate) ports: Vec<Port>,
    pub(crate) pin_funcs: Vec<Vec<PinFunc>>,
    pub(crate) analog: Vec<f64>,
    pub(crate) timers: Vec<Timer>,
    pub(crate) usarts: Vec<Usart>,
    pub(crate) adc: Adc,
    pub(crate) acomp: AComp,
    pub(crate) spi: Spi,
    pub(crate) twi: Twi,
    pub(crate) ee: Eeprom,
    pub(crate) wdt: Wdt,
    pub(crate) exint: ExtInt,
    pub(crate) sys: Sys,
    /// Scheduled external input changes (cycle, pin, level), sorted by cycle.
    pub(crate) sched_inputs: std::collections::VecDeque<(u64, Pin, Option<bool>)>,
    // ---- environment
    pub(crate) vcc: f64,
    pub(crate) aref: f64,
    pub(crate) temperature_c: f64,
    pub(crate) reset_pin_low: bool,
    // ---- outputs / control
    pub(crate) next_event: u64,
    pub(crate) pin_events: Vec<PinEvent>,
    pub(crate) stop: Option<StopReason>,
    pub(crate) dbg: Debug,
    pub(crate) rng: u64,
    pub(crate) last_reset: ResetCause,
    pub(crate) reset_count: u64,
    pub(crate) fault_reported_sp: bool,
}

impl std::fmt::Debug for Avr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Avr")
            .field("mcu", &self.v.mcu)
            .field("pc", &format_args!("0x{:X}", self.pc * 2))
            .field("sp", &format_args!("0x{:X}", self.sp))
            .field("sreg", &format_args!("0x{:02X}", self.sreg))
            .field("cycle", &self.cycle)
            .finish()
    }
}

impl Avr {
    /// A fresh MCU with erased flash (all `0xFF`), powered on at cycle 0.
    pub fn new(cfg: AvrConfig) -> Avr {
        let v = cfg.mcu.variant();
        let words = v.flash_words();
        let erased = decode(0xFFFF, 0xFFFF, v.ext);
        let mut avr = Avr {
            v,
            regs: [0; 32],
            sreg: 0,
            sp: v.ramend as u16,
            pc: 0,
            pc_mask: (words as u32) - 1,
            rampz: 0,
            eind: 0,
            cycle: 0,
            loop_limit: 0,
            state: CpuState::Running,
            irq_skip_one: false,
            pending: 0,
            instret: 0,
            flash: vec![0xFFFF; words],
            ops: vec![erased; words],
            data: vec![0; v.ramend + 1],
            eeprom: vec![0xFF; v.eeprom_bytes],
            io_map: crate::io::build_io_map(v),
            sram_start: v.sram_start,
            ports: (0..11)
                .map(|i| Port::new(v.ports.contains(&(i as u8))))
                .collect(),
            pin_funcs: crate::gpio::build_pin_funcs(v),
            analog: vec![f64::NAN; 11 * 8],
            timers: v.timers.iter().map(|d| Timer::new(d, v)).collect(),
            usarts: v.usarts.iter().map(|d| Usart::new(d, v)).collect(),
            adc: Adc::new(v),
            acomp: AComp::new(v),
            spi: Spi::new(v),
            twi: Twi::new(v),
            ee: Eeprom::default(),
            wdt: Wdt::new(v),
            exint: ExtInt::new(v),
            sys: Sys::default(),
            sched_inputs: Default::default(),
            vcc: cfg.vcc,
            aref: cfg.vcc,
            temperature_c: 25.0,
            reset_pin_low: false,
            next_event: 0,
            pin_events: Vec::new(),
            stop: None,
            dbg: Debug::new(words),
            rng: cfg.seed ^ 0x9E37_79B9_7F4A_7C15,
            last_reset: ResetCause::PowerOn,
            reset_count: 0,
            fault_reported_sp: false,
            cfg,
        };
        avr.reset_internal(ResetCause::PowerOn, true);
        avr.pin_events.clear();
        avr.stop = None;
        avr
    }

    /// Shorthand for `Avr::new(AvrConfig::new(mcu))`.
    pub fn with_mcu(mcu: Mcu) -> Avr {
        Avr::new(AvrConfig::new(mcu))
    }

    pub fn config(&self) -> &AvrConfig {
        &self.cfg
    }
    /// Change which events stop [`Avr::run`].
    pub fn set_stop_on(&mut self, stop_on: crate::StopOn) {
        self.cfg.stop_on = stop_on;
    }
    pub fn mcu(&self) -> Mcu {
        self.v.mcu
    }
    pub fn f_cpu(&self) -> u64 {
        self.cfg.f_cpu
    }

    // ------------------------------------------------------------------ loading / flashing

    /// Programs `image` into flash (rest erased) and EEPROM (if the image has an `.eeprom`
    /// segment), then resets the core. SRAM is cleared like a power cycle; EEPROM contents not
    /// in the image are kept (as with an Arduino bootloader upload).
    pub fn flash(&mut self, image: &FirmwareImage) -> Result<(), LoadError> {
        let size = self.v.flash_bytes;
        let mut bytes = vec![0xFFu8; size];
        for s in &image.flash {
            let end = s.addr as usize + s.data.len();
            if end > size {
                return Err(LoadError::TooLarge {
                    size: end,
                    max: size,
                });
            }
            bytes[s.addr as usize..end].copy_from_slice(&s.data);
        }
        for s in &image.eeprom {
            let end = s.addr as usize + s.data.len();
            if end > self.eeprom.len() {
                return Err(LoadError::TooLarge {
                    size: end,
                    max: self.eeprom.len(),
                });
            }
            self.eeprom[s.addr as usize..end].copy_from_slice(&s.data);
        }
        for (i, w) in self.flash.iter_mut().enumerate() {
            *w = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
        }
        self.redecode_all();
        self.data.iter_mut().for_each(|b| *b = 0);
        self.regs = [0; 32];
        self.reset(ResetCause::External);
        Ok(())
    }

    /// Parses Intel HEX text and flashes it.
    pub fn load_hex(&mut self, text: &str) -> Result<(), LoadError> {
        let img = FirmwareImage::from_hex(text)?;
        self.flash(&img)
    }

    /// Parses an AVR ELF executable and flashes it.
    pub fn load_elf(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        let img = FirmwareImage::from_elf(bytes)?;
        self.flash(&img)
    }

    /// Loads raw program words at word address 0 (tests / hand-assembled code). Does not reset.
    pub fn load_program_words(&mut self, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            if i < self.flash.len() {
                self.flash[i] = *w;
            }
        }
        self.redecode_all();
    }

    pub(crate) fn redecode_all(&mut self) {
        let n = self.flash.len();
        for i in 0..n {
            let w2 = self.flash[(i + 1) % n];
            self.ops[i] = decode(self.flash[i], w2, self.v.ext);
        }
    }

    pub(crate) fn redecode_range(&mut self, word_lo: usize, word_hi: usize) {
        let n = self.flash.len();
        let lo = word_lo.saturating_sub(1);
        for i in lo..word_hi.min(n) {
            let w2 = self.flash[(i + 1) % n];
            self.ops[i] = decode(self.flash[i], w2, self.v.ext);
        }
    }

    // ------------------------------------------------------------------ reset / power

    /// Resets the MCU (like the reset pin / watchdog / brown-out would). Registers r0–r31 and
    /// SRAM keep their contents; I/O registers return to their reset values; EEPROM is kept.
    pub fn reset(&mut self, cause: ResetCause) {
        self.reset_internal(cause, false);
    }

    pub(crate) fn reset_internal(&mut self, cause: ResetCause, power_on: bool) {
        let now = self.cycle;
        // Peripherals first (they may release pin overrides).
        for i in 0..self.timers.len() {
            self.timers[i].reset(self.v);
        }
        for i in 0..self.usarts.len() {
            self.usarts[i].reset();
        }
        self.adc.reset();
        self.acomp.reset();
        self.spi.reset();
        self.twi.reset();
        self.ee.reset();
        self.exint.reset();
        let wdrf_forces = cause == ResetCause::Watchdog;
        self.wdt.reset(now, wdrf_forces);
        let mcusr = if power_on || cause == ResetCause::PowerOn {
            ResetCause::PowerOn.mcusr_bit()
        } else {
            self.sys.mcusr | cause.mcusr_bit()
        };
        self.sys.reset(now);
        self.sys.mcusr = mcusr;
        for a in 0x20..self.sram_start {
            self.data[a] = 0;
        }
        self.sreg = 0;
        self.sp = self.v.ramend as u16;
        self.pc = 0;
        self.rampz = 0;
        self.eind = 0;
        self.irq_skip_one = false;
        self.pending = 0;
        self.dbg.shadow.clear();
        self.dbg.step = None;
        self.dbg.bp_skip = None;
        self.state = CpuState::Running;
        self.fault_reported_sp = false;
        for p in 0..self.ports.len() {
            self.ports[p].reset();
            self.port_refresh(p, now);
        }
        if self.cfg.bootloader == Bootloader::ArduinoHandoff {
            self.regs[2] = self.sys.mcusr;
            self.sys.mcusr = 0;
            self.wdt.disable();
        }
        self.last_reset = cause;
        self.reset_count += 1;
        if self.cfg.stop_on.reset && !power_on {
            self.request_stop(StopReason::Reset { cause });
        }
        self.recompute_next_event();
    }

    /// Drives the RESET pin. While low the core is held in reset (pins tri-stated); releasing it
    /// starts the firmware from 0 with EXTRF set.
    pub fn set_reset_pin(&mut self, low: bool) {
        if low == self.reset_pin_low {
            return;
        }
        self.reset_pin_low = low;
        if low {
            self.enter_reset_hold(ResetCause::External, None);
        } else if let CpuState::InReset {
            cause: ResetCause::External,
            ..
        } = self.state
        {
            self.leave_reset_hold(ResetCause::External);
        }
    }

    pub(crate) fn enter_reset_hold(&mut self, cause: ResetCause, release: Option<u64>) {
        let now = self.cycle;
        self.reset_internal(cause, false);
        self.state = CpuState::InReset { release, cause };
        for p in 0..self.ports.len() {
            self.port_refresh(p, now);
        }
    }

    pub(crate) fn leave_reset_hold(&mut self, cause: ResetCause) {
        let now = self.cycle;
        let mcusr_keep = self.sys.mcusr;
        self.state = CpuState::Running;
        self.reset_internal(cause, false);
        if self.cfg.bootloader == Bootloader::None {
            self.sys.mcusr = mcusr_keep | cause.mcusr_bit();
        }
        for p in 0..self.ports.len() {
            self.port_refresh(p, now);
        }
    }

    /// Sets the supply voltage. Below the brown-out level the core is held in reset; when Vcc
    /// rises above level + 50 mV hysteresis it restarts after
    /// [`AvrConfig::reset_delay_cycles`] with BORF set. Below 1.2 V it is a power-on reset.
    pub fn set_vcc(&mut self, vcc: f64) {
        let old = self.vcc;
        self.vcc = vcc;
        if self.aref_tracks_vcc() {
            self.aref = vcc;
        }
        const POR: f64 = 1.2;
        let bod = self.cfg.bod_level;
        let low_trip = bod.map(|l| l - 0.025).unwrap_or(POR);
        let high_trip = bod.map(|l| l + 0.025).unwrap_or(POR + 0.1);
        match self.state {
            CpuState::InReset { cause, .. }
                if matches!(cause, ResetCause::BrownOut | ResetCause::PowerOn) =>
            {
                if vcc >= high_trip {
                    let at = self.cycle + self.cfg.reset_delay_cycles;
                    self.state = CpuState::InReset {
                        release: Some(at),
                        cause,
                    };
                    self.schedule(at);
                } else {
                    self.state = CpuState::InReset {
                        release: None,
                        cause,
                    };
                }
            }
            _ => {
                if vcc < POR && old >= POR {
                    self.enter_reset_hold(ResetCause::PowerOn, None);
                } else if vcc < low_trip && bod.is_some() {
                    self.enter_reset_hold(ResetCause::BrownOut, None);
                }
            }
        }
        self.acomp_eval(self.cycle);
    }

    fn aref_tracks_vcc(&self) -> bool {
        self.sys.aref_tracks_vcc
    }

    pub fn vcc(&self) -> f64 {
        self.vcc
    }

    /// Voltage on the AREF pin (default: follows Vcc until set).
    pub fn set_aref(&mut self, v: f64) {
        self.sys.aref_tracks_vcc = false;
        self.aref = v;
    }

    /// Die temperature for the 328P's internal temperature sensor (ADC8).
    pub fn set_temperature(&mut self, celsius: f64) {
        self.temperature_c = celsius;
    }

    /// The cause of the most recent reset.
    pub fn last_reset_cause(&self) -> ResetCause {
        self.last_reset
    }
    /// Number of resets since construction (power-on counts as one).
    pub fn reset_count(&self) -> u64 {
        self.reset_count
    }
    /// True while held in reset (reset pin, brown-out).
    pub fn in_reset(&self) -> bool {
        matches!(self.state, CpuState::InReset { .. })
    }
    /// Current sleep mode, if sleeping.
    pub fn sleep_mode(&self) -> Option<SleepMode> {
        match self.state {
            CpuState::Sleeping(m) => Some(m),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ time

    /// Elapsed oscillator cycles since construction.
    pub fn cycle(&self) -> u64 {
        self.cycle
    }
    /// Elapsed simulated time (exact at 16 MHz).
    pub fn time(&self) -> SimTime {
        SimTime::from_cycles(self.cycle, self.cfg.f_cpu)
    }
    /// Converts a simulated time to an oscillator cycle number (rounding down).
    pub fn cycle_at(&self, t: SimTime) -> u64 {
        t.to_cycles(self.cfg.f_cpu)
    }
    /// Instructions retired since construction.
    pub fn instructions(&self) -> u64 {
        self.instret
    }

    // ------------------------------------------------------------------ run loop

    /// Runs until `until_cycle` or an early stop. See [`StopReason`].
    pub fn run(&mut self, until_cycle: u64, io: &mut dyn AvrIo) -> StopReason {
        if let Some(r) = self.adc_waiting_reason() {
            return r;
        }
        loop {
            if self.cycle >= self.next_event {
                self.service_events(io);
            }
            if let Some(r) = self.stop.take() {
                return self.finish_run(r);
            }
            if self.cycle >= until_cycle {
                return self.finish_run(StopReason::ReachedCycle);
            }
            if self.dbg.pause_requested {
                self.dbg.pause_requested = false;
                return self.finish_run(StopReason::Paused);
            }
            match self.state {
                CpuState::Running => {}
                CpuState::Sleeping(_) => {
                    if self.pending != 0 && self.sreg & FLAG_I != 0 {
                        self.wake_up();
                        continue;
                    }
                    self.cycle = self.cycle.max(self.next_event.min(until_cycle));
                    continue;
                }
                CpuState::Halted { until } => {
                    if self.cycle >= until {
                        self.state = CpuState::Running;
                        continue;
                    }
                    self.cycle = until.min(self.next_event).min(until_cycle).max(self.cycle);
                    continue;
                }
                CpuState::InReset { release, cause } => {
                    if let Some(at) = release
                        && self.cycle >= at
                    {
                        self.leave_reset_hold(cause);
                        continue;
                    }
                    let t = release
                        .unwrap_or(NEVER)
                        .min(self.next_event)
                        .min(until_cycle);
                    self.cycle = self.cycle.max(t);
                    continue;
                }
            }
            if self.irq_skip_one {
                self.irq_skip_one = false;
                self.loop_limit = NEVER;
                if !self.exec_checked() {
                    continue;
                }
                continue;
            }
            if self.pending != 0 && self.sreg & FLAG_I != 0 {
                self.dispatch_irq();
                if self.dbg.step.is_some() && self.step_filter_hit() {
                    self.request_stop(StopReason::StepDone);
                }
                continue;
            }
            self.loop_limit = self.next_event.min(until_cycle);
            if self.dbg.active() || self.sys.clk_shift != 0 {
                while self.cycle < self.loop_limit {
                    if !self.exec_checked() {
                        break;
                    }
                }
            } else {
                while self.cycle < self.loop_limit {
                    self.step();
                }
            }
        }
    }

    /// Executes one instruction with debugger checks. Returns false if execution stopped.
    fn exec_checked(&mut self) -> bool {
        let pc = self.pc;
        if self.dbg.bp_count > 0 && self.dbg.bp[pc as usize] {
            if self.dbg.bp_skip == Some(pc) {
                self.dbg.bp_skip = None;
            } else {
                self.dbg.bp_skip = Some(pc);
                self.request_stop(StopReason::Breakpoint { pc: pc * 2 });
                return false;
            }
        }
        let c0 = self.cycle;
        self.step();
        if self.sys.clk_shift != 0 {
            let d = self.cycle - c0;
            self.cycle += d * ((1u64 << self.sys.clk_shift) - 1);
        }
        self.dbg.bp_skip = None;
        if self.dbg.step.is_some() && self.step_filter_hit() {
            self.request_stop(StopReason::StepDone);
            return false;
        }
        self.stop.is_none()
    }

    fn finish_run(&mut self, r: StopReason) -> StopReason {
        // Bring lazily-updated peripherals up to date so register peeks are exact.
        let now = self.cycle;
        for i in 0..self.timers.len() {
            self.timer_sync(i, now);
        }
        if self.cfg.stop_on.pin_change || !self.pin_events.is_empty() {
            self.pin_events.sort_by_key(|e| e.cycle);
        }
        r
    }

    /// Executes exactly one instruction (or one interrupt entry), regardless of breakpoints.
    /// A sleeping / halted core is advanced until it executes something, at most one simulated
    /// second.
    pub fn step_instruction(&mut self, io: &mut dyn AvrIo) -> StopReason {
        let saved = self.dbg.step.take();
        let skip = self.dbg.bp_skip;
        self.dbg.bp_skip = Some(self.pc);
        self.dbg.step = Some(StepFilter::Instructions(1));
        let base = self.cycle;
        let mut r = self.run(base.saturating_add(self.cfg.f_cpu), io);
        if r == StopReason::ReachedCycle || self.cycle == base {
            r = StopReason::StepDone;
        }
        self.dbg.step = saved;
        if self.dbg.bp_skip.is_some() {
            self.dbg.bp_skip = skip;
        }
        r
    }

    /// Makes the next (or current) [`Avr::run`] return [`StopReason::Paused`] at the next
    /// instruction boundary.
    pub fn request_pause(&mut self) {
        self.dbg.pause_requested = true;
        self.loop_limit = 0;
    }

    pub(crate) fn request_stop(&mut self, r: StopReason) {
        match &self.stop {
            Some(cur) if stop_priority(cur) >= stop_priority(&r) => {}
            _ => self.stop = Some(r),
        }
        self.loop_limit = 0;
    }

    /// Makes sure the run loop looks at events no later than `at`.
    #[inline]
    pub(crate) fn schedule(&mut self, at: u64) {
        if at < self.next_event {
            self.next_event = at;
        }
        if at < self.loop_limit {
            self.loop_limit = at;
        }
    }

    pub(crate) fn recompute_next_event(&mut self) {
        let mut t = NEVER;
        for tm in &self.timers {
            t = t.min(tm.next_event);
        }
        for u in &self.usarts {
            t = t.min(u.next_event);
        }
        t = t
            .min(self.adc.next_event)
            .min(self.spi.next_event)
            .min(self.twi.next_event)
            .min(self.ee.next_event)
            .min(self.wdt.next_event)
            .min(self.sys.spm_busy_until)
            .min(self.sched_inputs.front().map(|x| x.0).unwrap_or(NEVER));
        if let CpuState::InReset {
            release: Some(at), ..
        } = self.state
        {
            t = t.min(at);
        }
        if let CpuState::Halted { until } = self.state {
            t = t.min(until);
        }
        self.next_event = t;
        if t < self.loop_limit {
            self.loop_limit = t;
        }
    }

    /// Processes every peripheral event due at or before the current cycle, in time order.
    fn service_events(&mut self, io: &mut dyn AvrIo) {
        let now = self.cycle;
        loop {
            #[derive(Clone, Copy)]
            enum Src {
                None,
                Timer(usize),
                Usart(usize),
                Adc,
                Spi,
                Twi,
                Ee,
                Wdt,
                Spm,
                Input,
            }
            let mut best = (NEVER, Src::None);
            for (i, tm) in self.timers.iter().enumerate() {
                if tm.next_event < best.0 {
                    best = (tm.next_event, Src::Timer(i));
                }
            }
            for (i, u) in self.usarts.iter().enumerate() {
                if u.next_event < best.0 {
                    best = (u.next_event, Src::Usart(i));
                }
            }
            if self.adc.next_event < best.0 {
                best = (self.adc.next_event, Src::Adc);
            }
            if self.spi.next_event < best.0 {
                best = (self.spi.next_event, Src::Spi);
            }
            if self.twi.next_event < best.0 {
                best = (self.twi.next_event, Src::Twi);
            }
            if self.ee.next_event < best.0 {
                best = (self.ee.next_event, Src::Ee);
            }
            if self.wdt.next_event < best.0 {
                best = (self.wdt.next_event, Src::Wdt);
            }
            if self.sys.spm_busy_until < best.0 {
                best = (self.sys.spm_busy_until, Src::Spm);
            }
            if let Some(&(t, _, _)) = self.sched_inputs.front()
                && t < best.0
            {
                best = (t, Src::Input);
            }
            let (t, src) = best;
            if t > now {
                break;
            }
            match src {
                Src::None => break,
                Src::Timer(i) => self.timer_sync(i, t),
                Src::Usart(i) => self.usart_event(i, t),
                Src::Adc => self.adc_event(t, io),
                Src::Spi => self.spi_event(t, io),
                Src::Twi => self.twi_event(t),
                Src::Ee => self.ee_event(t),
                Src::Wdt => self.wdt_event(t),
                Src::Spm => self.spm_done(t),
                Src::Input => {
                    if let Some((at, pin, lvl)) = self.sched_inputs.pop_front() {
                        self.apply_input(pin, lvl, at);
                    }
                }
            }
            if matches!(self.stop, Some(StopReason::AdcSampleNeeded { .. })) {
                break;
            }
        }
        self.recompute_next_event();
    }

    // ------------------------------------------------------------------ interrupts

    #[inline]
    pub(crate) fn set_irq(&mut self, vec: u8, on: bool) {
        if vec == NO_VEC {
            return;
        }
        let bit = 1u64 << vec;
        if on {
            if self.pending & bit == 0 {
                self.pending |= bit;
                if self.sreg & FLAG_I != 0 || matches!(self.state, CpuState::Sleeping(_)) {
                    self.loop_limit = 0;
                }
            }
        } else {
            self.pending &= !bit;
        }
    }

    pub(crate) fn vector_of(&self, irq: Irq) -> u8 {
        self.v.vector_of(irq).unwrap_or(NO_VEC)
    }

    fn dispatch_irq(&mut self) {
        let vec = self.pending.trailing_zeros() as u8;
        self.ack_irq(vec);
        let ret = self.pc;
        let sp_before = self.sp;
        self.push_pc(ret);
        self.sreg &= !FLAG_I;
        let base = if self.sys.mcucr & 0x02 != 0 {
            self.v.nrww_start as u32 / 2
        } else {
            0
        };
        self.pc = (base + vec as u32 * 2) & self.pc_mask;
        self.cycle += if self.v.pc22 { 5 } else { 4 };
        self.instret += 1;
        if self.cfg.shadow_stack {
            self.dbg.push_frame(CallFrame {
                kind: crate::CallKind::Interrupt { vector: vec },
                call_pc: ret * 2,
                target: self.pc * 2,
                return_pc: ret * 2,
                sp: sp_before.wrapping_sub(if self.v.pc22 { 3 } else { 2 }),
            });
        }
    }

    /// Clears the flag of an edge-type source when its vector is executed.
    fn ack_irq(&mut self, vec: u8) {
        let Some(irq) = self.v.vectors.get(vec as usize).copied() else {
            return;
        };
        match irq {
            Irq::TimerOvf(n) => self.timer_clear_flags(n as usize, 0x01),
            Irq::TimerCompA(n) => self.timer_clear_flags(n as usize, 0x02),
            Irq::TimerCompB(n) => self.timer_clear_flags(n as usize, 0x04),
            Irq::TimerCompC(n) => self.timer_clear_flags(n as usize, 0x08),
            Irq::TimerCapt(n) => self.timer_clear_flags(n as usize, 0x20),
            Irq::Int(n) => self.exint_ack(n),
            Irq::Pcint(g) => self.pcint_ack(g),
            Irq::Wdt => self.wdt_ack(),
            Irq::Spi => self.spi_ack(),
            Irq::UsartTx(n) => self.usart_ack_txc(n as usize),
            Irq::Adc => self.adc_ack(),
            Irq::AnaComp => self.acomp_ack(),
            Irq::UsartRx(_)
            | Irq::UsartUdre(_)
            | Irq::EeReady
            | Irq::Twi
            | Irq::SpmReady
            | Irq::Reset => {}
        }
    }

    // ------------------------------------------------------------------ sleep

    pub(crate) fn enter_sleep(&mut self) {
        let mode = match (self.sys.smcr >> 1) & 7 {
            0 => SleepMode::Idle,
            1 => SleepMode::AdcNoiseReduction,
            2 => SleepMode::PowerDown,
            3 => SleepMode::PowerSave,
            6 => SleepMode::Standby,
            7 => SleepMode::ExtendedStandby,
            _ => SleepMode::Idle,
        };
        self.state = CpuState::Sleeping(mode);
        self.loop_limit = 0;
        self.apply_clock_gating(Some(mode));
        if mode == SleepMode::AdcNoiseReduction {
            self.adc_noise_reduction_start();
        }
        if self.cfg.stop_on.sleep {
            self.request_stop(StopReason::Sleep { mode });
        }
    }

    fn wake_up(&mut self) {
        let CpuState::Sleeping(mode) = self.state else {
            return;
        };
        let startup = match mode {
            SleepMode::PowerDown | SleepMode::PowerSave => self.cfg.startup_cycles,
            SleepMode::Standby | SleepMode::ExtendedStandby => 6,
            _ => 0,
        };
        self.apply_clock_gating(None);
        let until = self.cycle + startup + 4;
        self.state = CpuState::Halted { until };
        self.schedule(until);
    }

    /// Stops/starts peripheral clocks for a sleep mode (`None` = awake).
    fn apply_clock_gating(&mut self, mode: Option<SleepMode>) {
        let now = self.cycle;
        let io_stopped = matches!(
            mode,
            Some(
                SleepMode::AdcNoiseReduction
                    | SleepMode::PowerDown
                    | SleepMode::PowerSave
                    | SleepMode::Standby
                    | SleepMode::ExtendedStandby
            )
        );
        let adc_stopped = io_stopped && mode != Some(SleepMode::AdcNoiseReduction);
        let async_t2_runs = matches!(
            mode,
            None | Some(
                SleepMode::Idle
                    | SleepMode::AdcNoiseReduction
                    | SleepMode::PowerSave
                    | SleepMode::ExtendedStandby
            )
        ) && self.sys.assr & 0x20 != 0;
        for i in 0..self.timers.len() {
            let gated = if self.timers[i].d.async_timer && async_t2_runs {
                false
            } else {
                io_stopped
            };
            self.timer_set_sleep_gate(i, gated, now);
        }
        for i in 0..self.usarts.len() {
            self.usart_set_gate(i, io_stopped, now);
        }
        self.spi_set_gate(io_stopped, now);
        self.adc_set_gate(adc_stopped, now);
        self.recompute_next_event();
    }

    // ------------------------------------------------------------------ CPU state access

    /// Program counter as a **byte** address.
    pub fn pc(&self) -> u32 {
        self.pc * 2
    }
    /// Sets the program counter (byte address, must be even).
    pub fn set_pc(&mut self, byte_addr: u32) {
        self.pc = (byte_addr / 2) & self.pc_mask;
        self.dbg.bp_skip = None;
    }
    pub fn sp(&self) -> u16 {
        self.sp
    }
    pub fn set_sp(&mut self, sp: u16) {
        self.sp = sp;
    }
    pub fn sreg(&self) -> u8 {
        self.sreg
    }
    pub fn set_sreg(&mut self, v: u8) {
        self.sreg = v;
    }
    pub fn reg(&self, r: usize) -> u8 {
        self.regs[r & 31]
    }
    pub fn set_reg(&mut self, r: usize, v: u8) {
        self.regs[r & 31] = v;
    }
    pub fn regs(&self) -> &[u8; 32] {
        &self.regs
    }
    /// 16-bit register pair `r+1:r` (X = 26, Y = 28, Z = 30).
    pub fn reg_pair(&self, r: usize) -> u16 {
        u16::from_le_bytes([self.regs[r & 31], self.regs[(r + 1) & 31]])
    }
    pub fn rampz(&self) -> u8 {
        self.rampz
    }
    pub fn eind(&self) -> u8 {
        self.eind
    }

    /// Reads data space without side effects (registers, I/O registers as stored, SRAM).
    pub fn peek(&self, addr: u16) -> u8 {
        let a = addr as usize;
        if a < 32 {
            self.regs[a]
        } else if a < self.sram_start {
            self.io_peek(a)
        } else {
            self.data.get(a).copied().unwrap_or(0)
        }
    }
    /// Writes data space directly (debugger). I/O writes have their normal side effects.
    pub fn poke(&mut self, addr: u16, v: u8) {
        let a = addr as usize;
        if a < 32 {
            self.regs[a] = v;
        } else if a < self.sram_start {
            self.io_write(a, v);
        } else if a < self.data.len() {
            self.data[a] = v;
        }
    }
    /// SRAM contents (data addresses `sram_start..=ramend`).
    pub fn sram(&self) -> &[u8] {
        &self.data[self.sram_start..]
    }
    /// Reads `len` bytes of data space starting at `addr` (no side effects).
    pub fn read_data_block(&self, addr: u16, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| self.peek(addr.wrapping_add(i as u16)))
            .collect()
    }
    /// Flash as bytes (little-endian words).
    pub fn flash_bytes(&self) -> Vec<u8> {
        self.flash.iter().flat_map(|w| w.to_le_bytes()).collect()
    }
    pub fn flash_byte(&self, byte_addr: u32) -> u8 {
        let w = self.flash[((byte_addr / 2) as usize) % self.flash.len()];
        if byte_addr & 1 == 0 {
            w as u8
        } else {
            (w >> 8) as u8
        }
    }
    pub fn flash_word(&self, byte_addr: u32) -> u16 {
        self.flash[((byte_addr / 2) as usize) % self.flash.len()]
    }
    /// Decoded instruction at a byte address (for disassembly views).
    pub fn instruction_at(&self, byte_addr: u32) -> Op {
        self.ops[((byte_addr / 2) as usize) % self.ops.len()]
    }
    pub fn eeprom(&self) -> &[u8] {
        &self.eeprom
    }
    pub fn eeprom_mut(&mut self) -> &mut [u8] {
        &mut self.eeprom
    }

    /// Named I/O registers with their current values (for the debugger's I/O view).
    pub fn io_registers(&self) -> Vec<(&'static str, u16, u8)> {
        crate::io::io_register_names(self.v)
            .into_iter()
            .map(|(name, addr)| (name, addr, self.peek(addr)))
            .collect()
    }

    // ------------------------------------------------------------------ pins

    /// Drained list of pin drive changes since the last call, in cycle order.
    pub fn take_pin_events(&mut self) -> Vec<PinEvent> {
        self.pin_events.sort_by_key(|e| e.cycle);
        std::mem::take(&mut self.pin_events)
    }

    /// Pins that exist on this part.
    pub fn pins(&self) -> Vec<Pin> {
        self.v
            .ports
            .iter()
            .flat_map(|&p| (0..8).map(move |b| Pin { port: p, bit: b }))
            .collect()
    }

    /// Logical state of `pin`.
    pub fn pin_info(&self, pin: Pin) -> PinInfo {
        self.pin_info_impl(pin)
    }

    /// Sets the external logic level on `pin` (the resolved line level as seen by the input
    /// buffer, including the MCU's own drive if the co-simulator models it). `None` disconnects:
    /// the pin then reads its own output / pull-up / low. Applied at the current cycle.
    pub fn set_pin_input(&mut self, pin: Pin, level: Option<bool>) {
        let now = self.cycle;
        self.apply_input(pin, level, now);
        self.recompute_next_event();
    }

    /// Schedules an external level change at an exact future cycle (≥ now); the core stops
    /// right there to apply it. Used for bit-exact waveforms (UART RX, test stimuli).
    pub fn schedule_pin_input(&mut self, pin: Pin, level: Option<bool>, at_cycle: u64) {
        let at = at_cycle.max(self.cycle);
        let pos = self.sched_inputs.partition_point(|x| x.0 <= at);
        self.sched_inputs.insert(pos, (at, pin, level));
        self.schedule(at);
    }

    /// Analog voltage on `pin` for the analog comparator and as ADC fallback (see
    /// [`AvrIo::adc_sample`]). Does not change the digital level.
    pub fn set_pin_voltage(&mut self, pin: Pin, volts: f64) {
        let i = pin.index();
        if i < self.analog.len() {
            self.analog[i] = volts;
        }
        let now = self.cycle;
        self.acomp_eval(now);
    }

    /// Supplies the voltage requested by [`StopReason::AdcSampleNeeded`].
    pub fn supply_adc_sample(&mut self, volts: f64) {
        self.adc_supply(volts);
    }

    // ------------------------------------------------------------------ USART fast paths

    /// Bytes transmitted by USART `n` since the last call (fast-path serial monitor).
    pub fn take_usart_tx(&mut self, n: usize) -> Vec<UsartByte> {
        self.usarts
            .get_mut(n)
            .map(|u| std::mem::take(&mut u.tx_log))
            .unwrap_or_default()
    }
    /// Feeds bytes into USART `n`'s receiver with correct frame timing (as if a perfectly
    /// matched transmitter sent them back-to-back starting now), without touching the RX pin.
    pub fn usart_inject(&mut self, n: usize, bytes: &[u8]) {
        if n < self.usarts.len() {
            let now = self.cycle;
            self.usart_inject_bytes(n, bytes, now);
            self.recompute_next_event();
        }
    }
    /// Enables/disables bit-level TX pin waveforms for USART `n` (default on). With it off the
    /// TX pin idles high and bytes are only available through [`Avr::take_usart_tx`] — much
    /// cheaper when nothing is wired to the pin.
    pub fn set_usart_waveform(&mut self, n: usize, on: bool) {
        if let Some(u) = self.usarts.get_mut(n) {
            u.waveform = on;
        }
    }
    /// Current baud rate of USART `n` from UBRR/U2X (0 if out of range).
    pub fn usart_baud(&self, n: usize) -> f64 {
        self.usarts
            .get(n)
            .map(|u| self.cfg.f_cpu as f64 / u.bit_cycles() as f64)
            .unwrap_or(0.0)
    }
    /// Enables/disables SPI pin waveforms (default on). With it off, master transfers use
    /// [`AvrIo::spi_exchange`].
    pub fn set_spi_waveform(&mut self, on: bool) {
        self.spi.waveform = on;
    }

    // ------------------------------------------------------------------ debug API

    /// Sets a breakpoint at a byte address.
    pub fn set_breakpoint(&mut self, byte_addr: u32) {
        let w = ((byte_addr / 2) & self.pc_mask) as usize;
        if !self.dbg.bp[w] {
            self.dbg.bp[w] = true;
            self.dbg.bp_count += 1;
        }
    }
    pub fn clear_breakpoint(&mut self, byte_addr: u32) {
        let w = ((byte_addr / 2) & self.pc_mask) as usize;
        if self.dbg.bp[w] {
            self.dbg.bp[w] = false;
            self.dbg.bp_count -= 1;
        }
    }
    pub fn clear_all_breakpoints(&mut self) {
        self.dbg.bp.iter_mut().for_each(|b| *b = false);
        self.dbg.bp_count = 0;
    }
    pub fn has_breakpoint(&self, byte_addr: u32) -> bool {
        self.dbg.bp[((byte_addr / 2) & self.pc_mask) as usize]
    }
    /// Byte addresses of all breakpoints.
    pub fn breakpoints(&self) -> Vec<u32> {
        self.dbg
            .bp
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(i, _)| i as u32 * 2)
            .collect()
    }
    pub fn add_watchpoint(&mut self, w: Watchpoint) {
        self.dbg.watches.push(w);
    }
    pub fn clear_watchpoints(&mut self) {
        self.dbg.watches.clear();
    }
    /// Arms a step condition checked after every instruction; [`Avr::run`] returns
    /// [`StopReason::StepDone`] when it is met.
    pub fn set_step_filter(&mut self, f: Option<StepFilter>) {
        self.dbg.step = f;
        self.loop_limit = 0;
    }
    pub fn step_filter(&self) -> Option<&StepFilter> {
        self.dbg.step.as_ref()
    }
    /// Shadow call stack, outermost first (exact; maintained by CALL/RET/interrupts).
    pub fn call_stack(&self) -> &[CallFrame] {
        &self.dbg.shadow
    }

    // ------------------------------------------------------------------ helpers

    #[inline]
    pub(crate) fn next_rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(crate) fn gaussian(&mut self) -> f64 {
        let u1 = ((self.next_rand() >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        let u2 = (self.next_rand() >> 11) as f64 / (1u64 << 53) as f64;
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}
