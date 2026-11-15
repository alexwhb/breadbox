//! 10-bit successive-approximation ADC and the analog comparator.
//!
//! Timing follows the datasheet: conversions start on the next ADC clock edge after ADSC is
//! written (prescaler phase counted from ADEN); sample-and-hold at 1.5 ADC clocks (13.5 for the
//! first conversion after enabling), result at 13 (25) ADC clocks; auto-triggered conversions
//! reset the prescaler at the trigger, sample at 2 and finish at 13.5 ADC clocks; free running
//! restarts immediately. The input voltage is requested from [`crate::AvrIo::adc_sample`] at the
//! exact sample-and-hold cycle.

use crate::avr::{Avr, NEVER, NO_VEC};
use crate::variant::{Irq, Variant};
use crate::{AdcErrors, AdcModel, AvrIo, Pin, StopReason};

/// One chip's static ADC errors, derived from the seed alone (no RNG state is consumed, so
/// they never change during a run and switching models keeps the same chip).
#[derive(Debug, Clone, Copy)]
pub struct AdcChip {
    seed: u64,
    /// Offset error (LSB).
    pub offset: f64,
    /// Gain error at full scale (LSB at code 1024).
    pub gain: f64,
    /// Amplitude of the INL bow (LSB).
    bow: f64,
    /// Amplitude of the per-code INL ripple (LSB).
    ripple: f64,
}

/// splitmix64: a well-mixed hash of `x`.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform in [−1, 1) from a hash.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

impl AdcChip {
    pub fn new(seed: u64, e: &AdcErrors) -> AdcChip {
        let s = mix(seed ^ 0xADC0_ADC0_ADC0_ADC0);
        AdcChip {
            seed: s,
            offset: e.offset_lsb * unit(mix(s ^ 1)),
            gain: e.gain_lsb * unit(mix(s ^ 2)),
            // bow + 2 × ripple ≤ inl_lsb: INL stays inside ±inl_lsb even measured against the
            // end-point line (whose ends carry ripple), DNL inside ±inl_lsb / 2
            bow: 0.5 * e.inl_lsb * unit(mix(s ^ 3)),
            ripple: 0.25 * e.inl_lsb,
        }
    }

    /// Integral non-linearity of the transition into code position `k` (1…1023), in LSB:
    /// a bow over the range plus a fixed per-code ripple.
    pub fn inl(&self, k: i64) -> f64 {
        let bow = self.bow * (std::f64::consts::PI * k as f64 / 1024.0).sin();
        bow + self.ripple * unit(mix(self.seed ^ (k as u64).wrapping_mul(0x1000_0000_01B3)))
    }

    /// Input level (LSB, after offset / gain) at which code `c` begins; `lo` is the bottom
    /// code (0, or −512 for differential channels), so positions run 0…1023.
    pub fn transition(&self, c: i64, lo: i64) -> f64 {
        c as f64 + self.inl(c - lo)
    }

    /// Code for an ideal input level `x` (LSB) plus `noise` (LSB), in `lo..=hi`.
    pub fn convert(&self, x: f64, noise: f64, lo: i32, hi: i32) -> i32 {
        let (lo, hi) = (lo as i64, hi as i64);
        let y = x + self.offset + self.gain * (x / 1024.0) + noise;
        if !y.is_finite() {
            return lo as i32;
        }
        // positions count from the bottom code (differential channels start at −512)
        let mut c = (y.floor() as i64).clamp(lo, hi);
        while c < hi && self.transition(c + 1, lo) <= y {
            c += 1;
        }
        while c > lo && self.transition(c, lo) > y {
            c -= 1;
        }
        c as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AdcSel {
    Single(Pin),
    Diff { p: Pin, n: Pin, gain: f64 },
    Fixed(f64),
    Temp,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Conv {
    sh: u64,
    end: u64,
    sel: AdcSel,
    refv: f64,
    sampled: bool,
    vp: Option<f64>,
    vn: Option<f64>,
    code: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct Adc {
    pub admux: u8,
    pub adcsra: u8,
    pub adcsrb: u8,
    pub didr: [u8; 4],
    pub adif: bool,
    pub data: u16,
    pub locked: bool,
    pub epoch: u64,
    pub first: bool,
    pub conv: Option<Conv>,
    pub need: Option<(Pin, u64)>,
    pub supplied: Option<f64>,
    pub next_event: u64,
    pub vec: u8,
    pub gated: bool,
    pub gated_at: u64,
    pub ext_mux: bool,
}

impl Adc {
    pub fn new(v: &Variant) -> Adc {
        Adc {
            admux: 0,
            adcsra: 0,
            adcsrb: 0,
            didr: [0; 4],
            adif: false,
            data: 0,
            locked: false,
            epoch: 0,
            first: true,
            conv: None,
            need: None,
            supplied: None,
            next_event: NEVER,
            vec: v.vector_of(Irq::Adc).unwrap_or(NO_VEC),
            gated: false,
            gated_at: 0,
            ext_mux: v.ext,
        }
    }
    pub fn reset(&mut self) {
        let (vec, ext) = (self.vec, self.ext_mux);
        *self = Adc {
            vec,
            ext_mux: ext,
            ..Adc::new(&crate::variant::M328P)
        };
    }
    fn enabled(&self) -> bool {
        self.adcsra & 0x80 != 0
    }
    fn free_running(&self) -> bool {
        self.adcsra & 0x20 != 0 && self.adcsrb & 7 == 0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AComp {
    pub acsr: u8,
    pub aco: bool,
    pub aci: bool,
    pub vec: u8,
}

impl AComp {
    pub fn new(v: &Variant) -> AComp {
        AComp {
            acsr: 0,
            aco: false,
            aci: false,
            vec: v.vector_of(Irq::AnaComp).unwrap_or(NO_VEC),
        }
    }
    pub fn reset(&mut self) {
        self.acsr = 0;
        self.aco = false;
        self.aci = false;
    }
}

/// Internal temperature sensor voltage (ATmega328P datasheet table 24-2).
fn temp_sensor_volts(t: f64) -> f64 {
    if t <= 25.0 {
        0.314 + (t - 25.0) * (0.314 - 0.242) / 70.0
    } else {
        0.314 + (t - 25.0) * (0.380 - 0.314) / 60.0
    }
}

impl Avr {
    fn adc_div(&self) -> u64 {
        let ps = self.adc.adcsra & 7;
        ([2u64, 2, 4, 8, 16, 32, 64, 128][ps as usize]) << self.sys.clk_shift
    }

    fn adc_ref(&self) -> f64 {
        match (self.adc.admux >> 6, self.adc.ext_mux) {
            (0, _) => self.aref,
            (1, _) => self.vcc,
            (2, false) | (3, false) | (2, true) => 1.1,
            _ => 2.56,
        }
    }

    fn adc_select(&self) -> AdcSel {
        let pins = self.v.adc_pins;
        if !self.adc.ext_mux {
            let m = self.adc.admux & 0x0F;
            return match m {
                0..=7 => AdcSel::Single(pins[m as usize]),
                8 if self.v.has_temp_sensor => AdcSel::Temp,
                14 => AdcSel::Fixed(1.1),
                _ => AdcSel::Fixed(0.0),
            };
        }
        let mux = (self.adc.admux & 0x1F) | if self.adc.adcsrb & 0x08 != 0 { 0x20 } else { 0 };
        let hi = if mux & 0x20 != 0 { 8 } else { 0 };
        let m = mux & 0x1F;
        let diff = |p: usize, n: usize, gain: f64| AdcSel::Diff {
            p: pins[p + hi],
            n: pins[n + hi],
            gain,
        };
        match m {
            0..=7 => AdcSel::Single(pins[m as usize + hi]),
            0x08 => diff(0, 0, 10.0),
            0x09 => diff(1, 0, 10.0),
            0x0A => diff(0, 0, 200.0),
            0x0B => diff(1, 0, 200.0),
            0x0C => diff(2, 2, 10.0),
            0x0D => diff(3, 2, 10.0),
            0x0E => diff(2, 2, 200.0),
            0x0F => diff(3, 2, 200.0),
            0x10..=0x17 => diff((m - 0x10) as usize, 1, 1.0),
            0x18..=0x1D => diff((m - 0x18) as usize, 2, 1.0),
            0x1E if hi == 0 => AdcSel::Fixed(1.1),
            _ => AdcSel::Fixed(0.0),
        }
    }

    fn adc_start(&mut self, start: u64, sh_off2: u64, end_off2: u64) {
        // Offsets are in half ADC clocks.
        let div = self.adc_div();
        let (sh_off2, end_off2) = if self.adc.first {
            (27, 50)
        } else {
            (sh_off2, end_off2)
        };
        self.adc.first = false;
        let sel = self.adc_select();
        let refv = self.adc_ref();
        self.adc.conv = Some(Conv {
            sh: start + sh_off2 * div / 2,
            end: start + end_off2 * div / 2,
            sel,
            refv,
            sampled: false,
            vp: None,
            vn: None,
            code: 0,
        });
        self.adc_update_next();
    }

    fn adc_start_manual(&mut self, now: u64) {
        if !self.adc.enabled() || self.adc.conv.is_some() {
            return;
        }
        let div = self.adc_div();
        let epoch = self.adc.epoch.min(now);
        let start = epoch + ((now - epoch) / div + 1) * div;
        self.adc_start(start, 3, 26);
    }

    /// ADTS trigger source `src` had a rising flag edge at `at`.
    pub(crate) fn adc_auto_trigger(&mut self, src: u8, at: u64) {
        let a = &self.adc;
        if !a.enabled()
            || a.adcsra & 0x20 == 0
            || a.adcsrb & 7 != src
            || a.conv.is_some()
            || a.gated
        {
            return;
        }
        self.adc.epoch = at;
        self.adc_start(at, 4, 27);
    }

    pub(crate) fn adc_noise_reduction_start(&mut self) {
        let now = self.cycle;
        self.adc_start_manual(now);
    }

    fn adc_update_next(&mut self) {
        let t = match (&self.adc.conv, self.adc.gated) {
            (Some(c), false) => {
                if c.sampled {
                    c.end
                } else {
                    c.sh
                }
            }
            _ => NEVER,
        };
        self.adc.next_event = t;
        self.schedule(t);
    }

    fn adc_update_irq(&mut self) {
        let on = self.adc.adif && self.adc.adcsra & 0x08 != 0;
        let v = self.adc.vec;
        self.set_irq(v, on);
    }

    pub(crate) fn adc_waiting_reason(&self) -> Option<StopReason> {
        match (self.adc.need, self.adc.supplied) {
            (Some((pin, cycle)), None) => Some(StopReason::AdcSampleNeeded { pin, cycle }),
            _ => None,
        }
    }

    pub(crate) fn adc_supply(&mut self, v: f64) {
        if self.adc.need.is_some() {
            self.adc.supplied = Some(v);
        }
    }

    /// Voltage on a pin for sampling: AvrIo → supplied → pushed analog value → stop.
    fn adc_pin_voltage(&mut self, pin: Pin, sh: u64, io: &mut dyn AvrIo) -> Option<f64> {
        if let Some((np, _)) = self.adc.need
            && np == pin
            && let Some(v) = self.adc.supplied.take()
        {
            self.adc.need = None;
            return Some(v);
        }
        if let Some(v) = io.adc_sample(pin, sh) {
            return Some(v);
        }
        let a = self.analog[pin.index()];
        if !a.is_nan() {
            return Some(a);
        }
        self.adc.need = Some((pin, sh));
        self.adc.supplied = None;
        self.request_stop(StopReason::AdcSampleNeeded { pin, cycle: sh });
        None
    }

    pub(crate) fn adc_event(&mut self, t: u64, io: &mut dyn AvrIo) {
        let Some(mut c) = self.adc.conv else {
            self.adc_update_next();
            return;
        };
        if !c.sampled && t >= c.sh {
            let code: i32 = match c.sel {
                AdcSel::Single(p) => {
                    let Some(v) = self.adc_pin_voltage(p, c.sh, io) else {
                        self.adc.conv = Some(c);
                        return;
                    };
                    c.vp = Some(v);
                    self.adc_quantize(v, c.refv, 1024.0, 0, 1023)
                }
                AdcSel::Diff { p, n, gain } => {
                    if c.vp.is_none() {
                        let Some(v) = self.adc_pin_voltage(p, c.sh, io) else {
                            self.adc.conv = Some(c);
                            return;
                        };
                        c.vp = Some(v);
                    }
                    if c.vn.is_none() {
                        self.adc.conv = Some(c);
                        let Some(v) = self.adc_pin_voltage(n, c.sh, io) else {
                            return;
                        };
                        c.vn = Some(v);
                    }
                    let d = (c.vp.unwrap_or(0.0) - c.vn.unwrap_or(0.0)) * gain;
                    self.adc_quantize(d, c.refv, 512.0, -512, 511) & 0x3FF
                }
                AdcSel::Fixed(v) => self.adc_quantize(v, c.refv, 1024.0, 0, 1023),
                AdcSel::Temp => {
                    let v = temp_sensor_volts(self.temperature_c);
                    self.adc_quantize(v, c.refv, 1024.0, 0, 1023)
                }
            };
            c.code = code as u16;
            c.sampled = true;
            self.adc.need = None;
            self.adc.supplied = None;
        }
        if c.sampled && t >= c.end {
            if !self.adc.locked {
                self.adc.data = c.code;
            }
            let newly = !self.adc.adif;
            self.adc.adif = true;
            self.adc.conv = None;
            self.adc_update_irq();
            if self.adc.free_running() && self.adc.enabled() {
                self.adc_start(c.end, 3, 26);
            }
            let _ = newly;
        } else {
            self.adc.conv = Some(c);
        }
        self.adc_update_next();
    }

    /// Sampled voltage → code through the configured [`crate::AdcModel`] (`WORLD-6`).
    fn adc_quantize(&mut self, v: f64, refv: f64, full: f64, lo: i32, hi: i32) -> i32 {
        let mut x = if refv > 0.0 { v * full / refv } else { 0.0 };
        if self.cfg.adc_noise_lsb > 0.0 {
            x += self.gaussian() * self.cfg.adc_noise_lsb;
        }
        match self.cfg.adc_model {
            AdcModel::Ideal => (x.floor() as i64).clamp(lo as i64, hi as i64) as i32,
            AdcModel::Realistic(e) => {
                let noise = if e.noise_lsb > 0.0 {
                    (self.gaussian() * e.noise_lsb).clamp(-e.noise_max_lsb, e.noise_max_lsb)
                } else {
                    0.0
                };
                AdcChip::new(self.cfg.seed, &e).convert(x, noise, lo, hi)
            }
        }
    }

    pub(crate) fn adc_ack(&mut self) {
        self.adc.adif = false;
        self.adc_update_irq();
    }

    fn adc_left(&self) -> bool {
        self.adc.admux & 0x20 != 0
    }

    pub(crate) fn adc_peek_l(&self) -> u8 {
        let d = self.adc.data & 0x3FF;
        if self.adc_left() {
            ((d & 3) << 6) as u8
        } else {
            d as u8
        }
    }
    pub(crate) fn adc_peek_h(&self) -> u8 {
        let d = self.adc.data & 0x3FF;
        // Differential results are 10-bit two's complement (sign in bit 9); ADCH bits 7:2
        // read as zero when right-adjusted.
        if self.adc_left() {
            (d >> 2) as u8
        } else {
            (d >> 8) as u8
        }
    }
    pub(crate) fn adc_read_l(&mut self) -> u8 {
        self.adc.locked = true;
        self.adc_peek_l()
    }
    pub(crate) fn adc_read_h(&mut self) -> u8 {
        self.adc.locked = false;
        self.adc_peek_h()
    }

    pub(crate) fn adc_peek_sra(&self) -> u8 {
        let mut v = self.adc.adcsra & 0xAF;
        if self.adc.conv.is_some()
            || (self.adc.free_running() && self.adc.enabled() && v & 0x40 != 0)
        {
            v |= 0x40;
        }
        if self.adc.conv.is_some() {
            v |= 0x40;
        }
        if self.adc.adif {
            v |= 0x10;
        }
        v
    }

    pub(crate) fn adc_write_sra(&mut self, v: u8) {
        let now = self.cycle;
        let was_en = self.adc.enabled();
        self.adc.adcsra = v & 0xAF;
        if v & 0x10 != 0 {
            self.adc.adif = false;
        }
        if !self.adc.enabled() {
            self.adc.conv = None;
            self.adc.need = None;
            self.adc.first = true;
        } else if !was_en {
            self.adc.epoch = now;
            self.adc.first = true;
        }
        if v & 0x40 != 0 {
            self.adc_start_manual(now);
        }
        self.adc_update_irq();
        self.adc_update_next();
        self.acomp_eval(now);
    }

    pub(crate) fn adc_write_srb(&mut self, v: u8) {
        self.adc.adcsrb = v & 0x4F;
        let now = self.cycle;
        self.acomp_eval(now);
    }

    pub(crate) fn adc_write_mux(&mut self, v: u8) {
        self.adc.admux = v;
        let now = self.cycle;
        self.acomp_eval(now);
    }

    pub(crate) fn adc_set_gate(&mut self, gated: bool, now: u64) {
        if self.adc.gated == gated {
            return;
        }
        if gated {
            self.adc.gated = true;
            self.adc.gated_at = now;
        } else {
            self.adc.gated = false;
            let dt = now - self.adc.gated_at;
            if let Some(c) = &mut self.adc.conv {
                c.sh += dt;
                c.end += dt;
            }
            self.adc.epoch += dt;
        }
        self.adc_update_next();
    }

    // ------------------------------------------------------------------ analog comparator

    fn analog_volts(&self, pin: Pin) -> f64 {
        let a = self.analog[pin.index()];
        if a.is_nan() {
            if self.pin_level(pin) { self.vcc } else { 0.0 }
        } else {
            a
        }
    }

    pub(crate) fn acomp_eval(&mut self, at: u64) {
        let acsr = self.acomp.acsr;
        if acsr & 0x80 != 0 {
            return;
        }
        let vpos = if acsr & 0x40 != 0 {
            1.1
        } else {
            self.analog_volts(self.v.ain0)
        };
        let acme = self.adc.adcsrb & 0x40 != 0 && !self.adc.enabled();
        let vneg = if acme {
            match self.adc_select() {
                AdcSel::Single(p) => self.analog_volts(p),
                _ => self.analog_volts(self.v.ain1),
            }
        } else {
            self.analog_volts(self.v.ain1)
        };
        let new = vpos > vneg;
        if new == self.acomp.aco {
            return;
        }
        self.acomp.aco = new;
        let fire = match acsr & 3 {
            0 => true,
            2 => !new,
            3 => new,
            _ => false,
        };
        if fire {
            let rose = !self.acomp.aci;
            self.acomp.aci = true;
            self.acomp_update_irq();
            if rose {
                self.adc_auto_trigger(1, at);
            }
        }
        if acsr & 0x04 != 0 {
            self.timer_capture_trigger(1, new, at);
        }
    }

    fn acomp_update_irq(&mut self) {
        let on = self.acomp.aci && self.acomp.acsr & 0x08 != 0;
        let v = self.acomp.vec;
        self.set_irq(v, on);
    }

    pub(crate) fn acomp_peek(&self) -> u8 {
        let mut v = self.acomp.acsr & 0xCF;
        if self.acomp.aco && self.acomp.acsr & 0x80 == 0 {
            v |= 0x20;
        }
        if self.acomp.aci {
            v |= 0x10;
        }
        v
    }

    pub(crate) fn acomp_write(&mut self, v: u8) {
        if v & 0x10 != 0 {
            self.acomp.aci = false;
        }
        self.acomp.acsr = v & 0xCF;
        self.acomp_update_irq();
        let now = self.cycle;
        self.acomp_eval(now);
    }

    pub(crate) fn acomp_ack(&mut self) {
        self.acomp.aci = false;
        self.acomp_update_irq();
    }
}
