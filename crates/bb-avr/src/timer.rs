//! Timer/Counters 0–5: normal, CTC, fast PWM, phase-correct and phase/frequency-correct modes,
//! double-buffered OCR, OCnx pin outputs, input capture (+ noise canceler), external clock,
//! shared prescalers (PSRSYNC / PSRASY), PRR and sleep clock gating.
//!
//! Timers are lazy: the counter only moves when it is read/written or when the next *event
//! tick* is due (a compare match, TOP, BOTTOM or MAX). Between events the counter is linear, so
//! the core can jump straight to the next event.
//!
//! Timing convention (datasheet timing diagrams): a compare match / TOP / BOTTOM event happens
//! on the timer tick at which TCNT *leaves* the matching value. Fast PWM non-inverting output
//! is therefore high for OCR+1 ticks of TOP+1, phase-correct for 2·OCR of 2·TOP.

use crate::PinOwner;
use crate::avr::{Avr, NEVER, NO_VEC};
use crate::io::TReg;
use crate::variant::{Irq, TimerDesc, Variant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slope {
    Normal,
    Ctc,
    Fast,
    PhaseCorrect,
    PhaseFreq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopSrc {
    Fixed(u16),
    OcrA,
    Icr,
}

pub(crate) const TOV: u8 = 0x01;
pub(crate) const OCFA: u8 = 0x02;
pub(crate) const ICF: u8 = 0x20;

#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub d: &'static TimerDesc,
    pub tccra: u8,
    pub tccrb: u8,
    pub tcnt: u16,
    pub ocr: [u16; 3],
    pub ocr_buf: [u16; 3],
    pub icr: u16,
    pub tifr: u8,
    pub timsk: u8,
    pub temp: u8,
    pub down: bool,
    pub oc: [bool; 3],
    pub block_compare: bool,
    pub slope: Slope,
    pub top_src: TopSrc,
    pub div: u64,
    pub next_tick: u64,
    pub next_event: u64,
    pub sleep_gated: bool,
    pub prr_gated: bool,
    pub pending_capture: u64,
    /// Vectors: ovf, compa, compb, compc, capt.
    pub vec: [u8; 5],
}

impl Timer {
    pub fn new(d: &'static TimerDesc, v: &Variant) -> Timer {
        let vo = |irq| v.vector_of(irq).unwrap_or(NO_VEC);
        let n = d.n;
        Timer {
            d,
            tccra: 0,
            tccrb: 0,
            tcnt: 0,
            ocr: [0; 3],
            ocr_buf: [0; 3],
            icr: 0,
            tifr: 0,
            timsk: 0,
            temp: 0,
            down: false,
            oc: [false; 3],
            block_compare: false,
            slope: Slope::Normal,
            top_src: TopSrc::Fixed(if d.wide { 0xFFFF } else { 0xFF }),
            div: 0,
            next_tick: NEVER,
            next_event: NEVER,
            sleep_gated: false,
            prr_gated: false,
            pending_capture: NEVER,
            vec: [
                vo(Irq::TimerOvf(n)),
                vo(Irq::TimerCompA(n)),
                vo(Irq::TimerCompB(n)),
                vo(Irq::TimerCompC(n)),
                vo(Irq::TimerCapt(n)),
            ],
        }
    }

    pub fn reset(&mut self, v: &Variant) {
        let d = self.d;
        *self = Timer::new(d, v);
    }

    #[inline]
    fn max(&self) -> u16 {
        if self.d.wide { 0xFFFF } else { 0xFF }
    }

    fn wgm(&self) -> u8 {
        if self.d.wide {
            ((self.tccrb >> 1) & 0x0C) | (self.tccra & 3)
        } else {
            ((self.tccrb >> 1) & 0x04) | (self.tccra & 3)
        }
    }

    pub fn update_mode(&mut self) {
        use Slope::*;
        use TopSrc::*;
        let (s, t) = if self.d.wide {
            match self.wgm() {
                0 | 13 => (Normal, Fixed(0xFFFF)),
                1 => (PhaseCorrect, Fixed(0xFF)),
                2 => (PhaseCorrect, Fixed(0x1FF)),
                3 => (PhaseCorrect, Fixed(0x3FF)),
                4 => (Ctc, OcrA),
                5 => (Fast, Fixed(0xFF)),
                6 => (Fast, Fixed(0x1FF)),
                7 => (Fast, Fixed(0x3FF)),
                8 => (PhaseFreq, Icr),
                9 => (PhaseFreq, OcrA),
                10 => (PhaseCorrect, Icr),
                11 => (PhaseCorrect, OcrA),
                12 => (Ctc, Icr),
                14 => (Fast, Icr),
                _ => (Fast, OcrA),
            }
        } else {
            match self.wgm() {
                1 => (PhaseCorrect, Fixed(0xFF)),
                2 => (Ctc, OcrA),
                3 => (Fast, Fixed(0xFF)),
                5 => (PhaseCorrect, OcrA),
                7 => (Fast, OcrA),
                _ => (Normal, Fixed(0xFF)),
            }
        };
        if !matches!(s, PhaseCorrect | PhaseFreq) {
            self.down = false;
        }
        if !self.pwm_mode_for(s) {
            self.ocr = self.ocr_buf;
        }
        self.slope = s;
        self.top_src = t;
    }

    fn pwm_mode_for(&self, s: Slope) -> bool {
        matches!(s, Slope::Fast | Slope::PhaseCorrect | Slope::PhaseFreq)
    }

    #[inline]
    pub fn pwm(&self) -> bool {
        self.pwm_mode_for(self.slope)
    }

    #[inline]
    pub fn top(&self) -> u16 {
        match self.top_src {
            TopSrc::Fixed(v) => v,
            TopSrc::OcrA => self.ocr[0],
            TopSrc::Icr => self.icr,
        }
    }

    fn com(&self, ch: usize) -> u8 {
        (self.tccra >> (6 - 2 * ch)) & 3
    }

    /// COMnx=01 toggles channel A in some PWM modes.
    fn toggle_allowed(&self, ch: usize) -> bool {
        match self.slope {
            Slope::Normal | Slope::Ctc => true,
            Slope::Fast => {
                ch == 0
                    && if self.d.wide {
                        self.tccrb & 0x10 != 0
                    } else {
                        self.tccrb & 0x08 != 0
                    }
            }
            Slope::PhaseCorrect | Slope::PhaseFreq => ch == 0 && self.top_src == TopSrc::OcrA,
        }
    }

    pub fn oc_connected(&self, ch: usize) -> bool {
        match self.com(ch) {
            0 => false,
            1 => self.toggle_allowed(ch),
            _ => true,
        }
    }

    /// Output action on a compare match; returns true if OCnx changed.
    fn compare_output(&mut self, ch: usize, up: bool) -> bool {
        let com = self.com(ch);
        let cur = self.oc[ch];
        let new = match (self.slope, com) {
            (_, 0) => return false,
            (_, 1) => {
                if self.toggle_allowed(ch) {
                    !cur
                } else {
                    return false;
                }
            }
            (Slope::Normal | Slope::Ctc | Slope::Fast, 2) => false,
            (Slope::Normal | Slope::Ctc | Slope::Fast, _) => true,
            (_, 2) => !up,
            (_, _) => up,
        };
        self.oc[ch] = new;
        new != cur
    }

    /// Ticks (≥1) until the next tick on which an event occurs.
    pub fn ticks_to_event(&self) -> u64 {
        if self.block_compare {
            return 1;
        }
        let c = self.tcnt as u64;
        let top = self.top() as u64;
        let max = self.max() as u64;
        let n = self.d.channels;
        let mut best;
        match self.slope {
            Slope::Normal | Slope::Ctc | Slope::Fast => {
                let wrap = if c <= top { top } else { max };
                best = wrap - c + 1;
                for &o in &self.ocr[..n] {
                    let o = o as u64;
                    if o >= c && o <= wrap {
                        best = best.min(o - c + 1);
                    }
                }
            }
            Slope::PhaseCorrect | Slope::PhaseFreq => {
                if !self.down {
                    if c >= top {
                        return 1;
                    }
                    best = top - c + 1;
                    for &o in &self.ocr[..n] {
                        let o = o as u64;
                        if o >= c && o <= top {
                            best = best.min(o - c + 1);
                        }
                    }
                } else {
                    best = c + 1;
                    for &o in &self.ocr[..n] {
                        let o = o as u64;
                        if o <= c {
                            best = best.min(c - o + 1);
                        }
                    }
                }
            }
        }
        best.max(1)
    }

    /// Moves `n` ticks along the counting direction (caller guarantees no event in between).
    #[inline]
    pub fn linear(&mut self, n: u64) {
        if self.down {
            self.tcnt = self.tcnt.wrapping_sub(n as u16);
        } else {
            self.tcnt = self.tcnt.wrapping_add(n as u16);
        }
    }

    /// One timer tick with full event processing. Returns (flags set, OC changed mask).
    pub fn event_tick(&mut self) -> (u8, u8) {
        let old = self.tcnt;
        let top = self.top();
        let max = self.max();
        let n = self.d.channels;
        let blocked = std::mem::take(&mut self.block_compare);
        let mut flags = 0u8;
        let mut occh = 0u8;
        match self.slope {
            Slope::Normal | Slope::Ctc | Slope::Fast => {
                if !blocked {
                    for ch in 0..n {
                        if old == self.ocr[ch] {
                            flags |= OCFA << ch;
                            if self.compare_output(ch, true) {
                                occh |= 1 << ch;
                            }
                        }
                    }
                }
                if self.slope != Slope::Normal && old == top {
                    self.tcnt = 0;
                    if self.top_src == TopSrc::Icr {
                        flags |= ICF;
                    }
                    if self.slope == Slope::Fast {
                        flags |= TOV;
                        self.ocr = self.ocr_buf;
                        for ch in 0..n {
                            let set = match self.com(ch) {
                                2 => true,
                                3 => false,
                                _ => continue,
                            };
                            if self.oc[ch] != set {
                                self.oc[ch] = set;
                                occh |= 1 << ch;
                            }
                        }
                    } else if old == max {
                        flags |= TOV;
                    }
                } else if old == max {
                    self.tcnt = 0;
                    flags |= TOV;
                } else {
                    self.tcnt = old + 1;
                }
            }
            Slope::PhaseCorrect | Slope::PhaseFreq => {
                let up = !self.down;
                if !blocked {
                    for ch in 0..n {
                        if old == self.ocr[ch] {
                            // The direction of this move decides set vs clear.
                            let moving_up = if up { old < top } else { old == 0 };
                            flags |= OCFA << ch;
                            if self.compare_output(ch, moving_up) {
                                occh |= 1 << ch;
                            }
                        }
                    }
                }
                if top == 0 {
                    self.tcnt = 0;
                    flags |= TOV;
                } else if up {
                    if old >= top {
                        self.down = true;
                        self.tcnt = old - 1;
                        if self.top_src == TopSrc::Icr {
                            flags |= ICF;
                        }
                        if self.slope == Slope::PhaseCorrect {
                            self.ocr = self.ocr_buf;
                        }
                    } else {
                        self.tcnt = old + 1;
                    }
                } else if old == 0 {
                    self.down = false;
                    self.tcnt = 1;
                    flags |= TOV;
                    if self.slope == Slope::PhaseFreq {
                        self.ocr = self.ocr_buf;
                    }
                } else {
                    self.tcnt = old - 1;
                }
            }
        }
        (flags, occh)
    }
}

impl Avr {
    /// (Re)computes the tick period and the next tick from CS bits, gating and prescaler phase.
    pub(crate) fn timer_clock_setup(&mut self, i: usize, now: u64) {
        let shift = self.sys.clk_shift;
        let f_cpu = self.cfg.f_cpu;
        let async_on = self.sys.assr & 0x20 != 0;
        let tsm_halt = self.sys.gtccr & 0x80 != 0;
        let t = &mut self.timers[i];
        let cs = (t.tccrb & 7) as usize;
        let halted_by_tsm = tsm_halt
            && if t.d.async_timer {
                self.sys.gtccr & 0x02 != 0
            } else {
                self.sys.gtccr & 0x01 != 0
            };
        let pre: u64 = if t.d.async_timer {
            [0, 1, 8, 32, 64, 128, 256, 1024][cs]
        } else {
            [0, 1, 8, 64, 256, 1024, 0, 0][cs]
        };
        if pre == 0 || t.sleep_gated || t.prr_gated || halted_by_tsm {
            t.div = 0;
            t.next_tick = NEVER;
        } else {
            let (div, epoch) = if t.d.async_timer {
                let div = if async_on {
                    ((f_cpu as f64 * pre as f64 / 32_768.0).round() as u64).max(1)
                } else {
                    pre << shift
                };
                (div, self.sys.psr_async_epoch)
            } else {
                (pre << shift, self.sys.psr_sync_epoch)
            };
            t.div = div;
            let base = now.max(epoch);
            t.next_tick = epoch + ((base - epoch) / div + 1) * div;
        }
        self.timer_update_next_event(i);
    }

    pub(crate) fn timer_update_next_event(&mut self, i: usize) {
        let t = &mut self.timers[i];
        let tick_ev = if t.next_tick == NEVER {
            NEVER
        } else {
            t.next_tick
                .saturating_add((t.ticks_to_event() - 1).saturating_mul(t.div))
        };
        t.next_event = tick_ev.min(t.pending_capture);
        let ne = t.next_event;
        self.schedule(ne);
    }

    /// Brings timer `i` up to cycle `now`, processing every event tick on the way.
    pub(crate) fn timer_sync(&mut self, i: usize, now: u64) {
        let pc = self.timers[i].pending_capture;
        if pc <= now {
            self.timer_sync_ticks(i, pc);
            self.timers[i].pending_capture = NEVER;
            self.timer_capture(i, pc);
        }
        self.timer_sync_ticks(i, now);
        self.timer_update_next_event(i);
    }

    fn timer_sync_ticks(&mut self, i: usize, now: u64) {
        loop {
            let t = &mut self.timers[i];
            if t.next_tick > now {
                break;
            }
            let k = t.ticks_to_event();
            let ev = t.next_tick + (k - 1) * t.div;
            if ev <= now {
                t.linear(k - 1);
                let (flags, occh) = t.event_tick();
                t.next_tick = ev + t.div;
                self.timer_apply(i, flags, occh, ev);
            } else {
                let n = (now - t.next_tick) / t.div + 1;
                t.linear(n);
                t.next_tick += n * t.div;
                break;
            }
        }
    }

    fn timer_apply(&mut self, i: usize, flags: u8, occh: u8, at: u64) {
        let t = &mut self.timers[i];
        let newly = flags & !t.tifr;
        t.tifr |= flags;
        if newly != 0 {
            self.timer_update_irq(i);
            // ADC auto-trigger sources (ADTS): rising edge of the flag.
            match self.timers[i].d.n {
                0 => {
                    if newly & OCFA != 0 {
                        self.adc_auto_trigger(3, at);
                    }
                    if newly & TOV != 0 {
                        self.adc_auto_trigger(4, at);
                    }
                }
                1 => {
                    if newly & 0x04 != 0 {
                        self.adc_auto_trigger(5, at);
                    }
                    if newly & TOV != 0 {
                        self.adc_auto_trigger(6, at);
                    }
                    if newly & ICF != 0 {
                        self.adc_auto_trigger(7, at);
                    }
                }
                _ => {}
            }
        }
        if occh != 0 {
            self.timer_update_oc_pins(i, at);
        }
    }

    pub(crate) fn timer_update_irq(&mut self, i: usize) {
        let t = &self.timers[i];
        let act = t.tifr & t.timsk;
        let vec = t.vec;
        self.set_irq(vec[0], act & TOV != 0);
        self.set_irq(vec[1], act & 0x02 != 0);
        self.set_irq(vec[2], act & 0x04 != 0);
        self.set_irq(vec[3], act & 0x08 != 0);
        self.set_irq(vec[4], act & ICF != 0);
    }

    pub(crate) fn timer_clear_flags(&mut self, n: usize, mask: u8) {
        if let Some(t) = self.timers.get_mut(n) {
            t.tifr &= !mask;
            self.timer_update_irq(n);
        }
    }

    pub(crate) fn timer_update_oc_pins(&mut self, i: usize, at: u64) {
        let n = self.timers[i].d.n;
        for ch in 0..self.timers[i].d.channels {
            let Some(pin) = self.timers[i].d.oc[ch] else {
                continue;
            };
            let owner = PinOwner::OutputCompare {
                timer: n,
                channel: ch as u8,
            };
            if self.timers[i].oc_connected(ch) {
                let lvl = self.timers[i].oc[ch];
                if self.ports[pin.port as usize].owner[pin.bit as usize] == owner {
                    self.set_override_level(pin, lvl, at);
                } else {
                    self.set_pin_override(pin, owner, None, Some(lvl), at);
                }
            } else {
                self.clear_pin_override(pin, owner, at);
            }
        }
    }

    fn timer_capture(&mut self, i: usize, at: u64) {
        self.timer_sync_ticks(i, at);
        let t = &mut self.timers[i];
        t.icr = t.tcnt;
        self.timer_apply(i, ICF, 0, at);
    }

    /// Input capture trigger from the ICPn pin (or the analog comparator for timer 1).
    pub(crate) fn timer_capture_trigger(&mut self, i: usize, rising: bool, at: u64) {
        let Some(t) = self.timers.get(i) else { return };
        if t.top_src == TopSrc::Icr {
            return;
        }
        let ices = t.tccrb & 0x40 != 0;
        if ices != rising {
            return;
        }
        if t.tccrb & 0x80 != 0 {
            let when = at + (4u64 << self.sys.clk_shift);
            self.timers[i].pending_capture = when;
            self.schedule(when);
            self.timer_update_next_event(i);
        } else {
            self.timer_capture(i, at);
            self.timer_update_next_event(i);
        }
    }

    pub(crate) fn timer_icp_edge(&mut self, n: usize, level: bool, at: u64) {
        // Timer 1's capture source can be switched to the analog comparator (ACIC).
        if n == 1 && self.acomp.acsr & 0x04 != 0 {
            return;
        }
        self.timer_capture_trigger(n, level, at);
    }

    pub(crate) fn timer_ext_clock_edge(&mut self, n: usize, level: bool, at: u64) {
        let Some(t) = self.timers.get(n) else { return };
        if t.d.async_timer {
            return;
        }
        let cs = t.tccrb & 7;
        let want_rising = match cs {
            6 => false,
            7 => true,
            _ => return,
        };
        if level != want_rising || t.sleep_gated || t.prr_gated {
            return;
        }
        let t = &mut self.timers[n];
        if t.ticks_to_event() == 1 {
            let (flags, occh) = t.event_tick();
            self.timer_apply(n, flags, occh, at);
        } else {
            t.linear(1);
        }
        self.timer_update_next_event(n);
    }

    pub(crate) fn timer_set_sleep_gate(&mut self, i: usize, gated: bool, now: u64) {
        if self.timers[i].sleep_gated == gated {
            return;
        }
        self.timer_sync(i, now);
        self.timers[i].sleep_gated = gated;
        self.timer_clock_setup(i, now);
    }

    pub(crate) fn timer_set_prr_gate(&mut self, i: usize, gated: bool, now: u64) {
        if self.timers[i].prr_gated == gated {
            return;
        }
        self.timer_sync(i, now);
        self.timers[i].prr_gated = gated;
        self.timer_clock_setup(i, now);
    }

    pub(crate) fn timer_peek(&self, n: usize, r: TReg) -> u8 {
        let Some(t) = self.timers.get(n) else {
            return 0;
        };
        match r {
            TReg::A => t.tccra,
            TReg::B => t.tccrb & if t.d.wide { 0xDF } else { 0x0F },
            TReg::C => 0,
            TReg::CntL | TReg::Cnt8 => t.tcnt as u8,
            TReg::CntH => (t.tcnt >> 8) as u8,
            TReg::IcrL => t.icr as u8,
            TReg::IcrH => (t.icr >> 8) as u8,
            TReg::OcrL(c) | TReg::Ocr8(c) => t.ocr_buf[c as usize % 3] as u8,
            TReg::OcrH(c) => (t.ocr_buf[c as usize % 3] >> 8) as u8,
            TReg::Tifr => t.tifr,
            TReg::Timsk => t.timsk,
        }
    }

    pub(crate) fn timer_read(&mut self, n: usize, r: TReg) -> u8 {
        if n >= self.timers.len() {
            return 0;
        }
        let now = self.cycle;
        match r {
            TReg::Cnt8 | TReg::Tifr => {
                self.timer_sync(n, now);
                self.timer_peek(n, r)
            }
            TReg::CntL => {
                self.timer_sync(n, now);
                let t = &mut self.timers[n];
                t.temp = (t.tcnt >> 8) as u8;
                t.tcnt as u8
            }
            TReg::IcrL => {
                let t = &mut self.timers[n];
                t.temp = (t.icr >> 8) as u8;
                t.icr as u8
            }
            TReg::CntH | TReg::IcrH => self.timers[n].temp,
            _ => self.timer_peek(n, r),
        }
    }

    pub(crate) fn timer_write(&mut self, n: usize, r: TReg, v: u8) {
        if n >= self.timers.len() {
            return;
        }
        let now = self.cycle;
        self.timer_sync(n, now);
        match r {
            TReg::A => {
                self.timers[n].tccra = v;
                self.timers[n].update_mode();
                self.timer_update_oc_pins(n, now);
            }
            TReg::B => {
                let wide = self.timers[n].d.wide;
                let keep = if wide { 0xDF } else { 0x0F };
                self.timers[n].tccrb = v & keep;
                self.timers[n].update_mode();
                if !wide {
                    self.timer_force_compare(n, (v >> 6) & 3, now);
                }
                self.timer_clock_setup(n, now);
                self.timer_update_oc_pins(n, now);
            }
            TReg::C => self.timer_force_compare(n, (v >> 5) & 7, now),
            TReg::Cnt8 => {
                let t = &mut self.timers[n];
                t.tcnt = v as u16;
                t.block_compare = true;
            }
            TReg::CntH | TReg::IcrH | TReg::OcrH(_) => self.timers[n].temp = v,
            TReg::CntL => {
                let t = &mut self.timers[n];
                t.tcnt = ((t.temp as u16) << 8) | v as u16;
                t.block_compare = true;
            }
            TReg::IcrL => {
                let t = &mut self.timers[n];
                t.icr = ((t.temp as u16) << 8) | v as u16;
            }
            TReg::OcrL(c) | TReg::Ocr8(c) => {
                let t = &mut self.timers[n];
                let c = c as usize % 3;
                let val = if matches!(r, TReg::OcrL(_)) {
                    ((t.temp as u16) << 8) | v as u16
                } else {
                    v as u16
                };
                t.ocr_buf[c] = val;
                if !t.pwm() {
                    t.ocr[c] = val;
                }
            }
            TReg::Tifr => {
                self.timers[n].tifr &= !v;
                self.timer_update_irq(n);
            }
            TReg::Timsk => {
                self.timers[n].timsk = v & 0x2F;
                self.timer_update_irq(n);
            }
        }
        self.timer_update_next_event(n);
    }

    /// FOCnx strobes (non-PWM modes only): output action without flag or counter clear.
    fn timer_force_compare(&mut self, n: usize, mask: u8, now: u64) {
        if mask == 0 || self.timers[n].pwm() {
            return;
        }
        let chans = self.timers[n].d.channels;
        let mut changed = 0;
        for ch in 0..chans {
            // FOC bits are ordered A (MSB) … ; mask bit (chans-1-ch) belongs to channel ch.
            let bit = if self.timers[n].d.wide {
                2 - ch
            } else {
                1 - ch
            };
            if mask & (1 << bit) != 0 && self.timers[n].compare_output(ch, true) {
                changed |= 1 << ch;
            }
        }
        if changed != 0 {
            self.timer_update_oc_pins(n, now);
        }
    }

    /// GTCCR: TSM, PSRASY (timer 2 prescaler reset), PSRSYNC (shared prescaler reset).
    pub(crate) fn gtccr_write(&mut self, v: u8) {
        let now = self.cycle;
        for i in 0..self.timers.len() {
            self.timer_sync(i, now);
        }
        let tsm = v & 0x80 != 0;
        if v & 0x01 != 0 {
            self.sys.psr_sync_epoch = now;
        }
        if v & 0x02 != 0 {
            self.sys.psr_async_epoch = now;
        }
        // Without TSM the reset bits clear immediately.
        self.sys.gtccr = if tsm { v & 0x83 } else { 0 };
        for i in 0..self.timers.len() {
            self.timer_clock_setup(i, now);
        }
    }
}
