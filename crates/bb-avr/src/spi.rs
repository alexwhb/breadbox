//! SPI master and slave, all four modes (CPOL/CPHA), MSB/LSB first, SPI2X, WCOL, SS mode
//! fault. Master transfers drive SCK/MOSI edge by edge and sample MISO from the pin level, or —
//! with the waveform disabled — exchange whole bytes through [`crate::AvrIo::spi_exchange`].
//! Slave mode shifts on SCK edges seen on the pin while SS is low.

use crate::avr::{Avr, NEVER, NO_VEC};
use crate::variant::{Irq, Variant};
use crate::{AvrIo, PinOwner};

#[derive(Debug, Clone)]
pub(crate) struct Spi {
    pub spcr: u8,
    pub spif: bool,
    pub wcol: bool,
    pub spi2x: bool,
    pub spif_read_armed: bool,
    pub rx_buf: u8,
    pub out: u8,
    pub inb: u8,
    pub edge: u8,
    pub active: bool,
    pub start: u64,
    pub half: u64,
    pub waveform: bool,
    pub next_event: u64,
    pub vec: u8,
    pub gated: bool,
    pub gated_at: u64,
    // slave
    pub s_selected: bool,
    pub s_bits: u8,
    pub s_tx: u8,
    pub s_in: u8,
}

impl Spi {
    pub fn new(v: &Variant) -> Spi {
        Spi {
            spcr: 0,
            spif: false,
            wcol: false,
            spi2x: false,
            spif_read_armed: false,
            rx_buf: 0,
            out: 0,
            inb: 0,
            edge: 0,
            active: false,
            start: 0,
            half: 1,
            waveform: true,
            next_event: NEVER,
            vec: v.vector_of(Irq::Spi).unwrap_or(NO_VEC),
            gated: false,
            gated_at: 0,
            s_selected: false,
            s_bits: 0,
            s_tx: 0,
            s_in: 0,
        }
    }
    pub fn reset(&mut self) {
        let (vec, wf) = (self.vec, self.waveform);
        *self = Spi {
            vec,
            waveform: wf,
            ..Spi::new(&crate::variant::M328P)
        };
    }
    fn enabled(&self) -> bool {
        self.spcr & 0x40 != 0
    }
    fn master(&self) -> bool {
        self.spcr & 0x10 != 0
    }
    fn lsb_first(&self) -> bool {
        self.spcr & 0x20 != 0
    }
    fn cpol(&self) -> bool {
        self.spcr & 0x08 != 0
    }
    fn cpha(&self) -> bool {
        self.spcr & 0x04 != 0
    }
    fn out_bit(&self, byte: u8, i: u8) -> bool {
        if self.lsb_first() {
            byte >> i & 1 != 0
        } else {
            byte >> (7 - i) & 1 != 0
        }
    }
    fn in_bit(&self, acc: u8, i: u8, b: bool) -> u8 {
        if !b {
            acc
        } else if self.lsb_first() {
            acc | (1 << i)
        } else {
            acc | (1 << (7 - i))
        }
    }
}

impl Avr {
    fn spi_update_irq(&mut self) {
        let on = self.spi.spif && self.spi.spcr & 0x80 != 0;
        let v = self.spi.vec;
        self.set_irq(v, on);
    }

    fn spi_update_pins(&mut self, at: u64) {
        let (mosi, miso, sck, ss) = (
            self.v.spi_mosi,
            self.v.spi_miso,
            self.v.spi_sck,
            self.v.spi_ss,
        );
        if !self.spi.enabled() {
            for p in [mosi, miso, sck, ss] {
                self.clear_pin_override(p, PinOwner::Spi, at);
            }
            return;
        }
        if self.spi.master() {
            let idle_sck = self.spi.cpol();
            let first = if self.spi.active {
                None
            } else {
                Some(idle_sck)
            };
            if let Some(l) = first {
                self.set_pin_override(sck, PinOwner::Spi, None, Some(l), at);
            }
            let mosi_lvl = self.pin_info_impl(mosi).state == crate::PinState::Output { high: true };
            if self.ports[mosi.port as usize].owner[mosi.bit as usize] != PinOwner::Spi {
                self.set_pin_override(mosi, PinOwner::Spi, None, Some(mosi_lvl), at);
            }
            self.set_pin_override(miso, PinOwner::Spi, Some(false), None, at);
            self.clear_pin_override(ss, PinOwner::Spi, at);
        } else {
            self.set_pin_override(mosi, PinOwner::Spi, Some(false), None, at);
            self.set_pin_override(sck, PinOwner::Spi, Some(false), None, at);
            self.set_pin_override(ss, PinOwner::Spi, Some(false), None, at);
            if self.spi.s_selected {
                let b = self.spi.out_bit(self.spi.s_tx, self.spi.s_bits.min(7));
                self.set_pin_override(miso, PinOwner::Spi, None, Some(b), at);
            } else {
                self.set_pin_override(miso, PinOwner::Spi, Some(false), None, at);
            }
        }
    }

    fn spi_update_next(&mut self) {
        let s = &self.spi;
        let t = if s.active && !s.gated {
            if s.waveform {
                s.start + (s.edge as u64 + 1) * s.half
            } else {
                s.start + 16 * s.half
            }
        } else {
            NEVER
        };
        self.spi.next_event = t;
        self.schedule(t);
    }

    pub(crate) fn spi_write_cr(&mut self, v: u8) {
        let now = self.cycle;
        let old = self.spi.spcr;
        self.spi.spcr = v;
        if (old ^ v) & 0x50 != 0 || (old ^ v) & 0x08 != 0 {
            if !self.spi.enabled() {
                self.spi.active = false;
            }
            // Entering slave mode samples the current SS level.
            self.spi.s_selected =
                self.spi.enabled() && !self.spi.master() && !self.pin_level(self.v.spi_ss);
            self.spi.s_bits = 0;
            self.spi_update_pins(now);
        }
        self.spi_update_irq();
        self.spi_update_next();
    }

    pub(crate) fn spi_peek_sr(&self) -> u8 {
        (self.spi.spif as u8) << 7 | (self.spi.wcol as u8) << 6 | self.spi.spi2x as u8
    }
    pub(crate) fn spi_read_sr(&mut self) -> u8 {
        if self.spi.spif || self.spi.wcol {
            self.spi.spif_read_armed = true;
        }
        self.spi_peek_sr()
    }
    pub(crate) fn spi_write_sr(&mut self, v: u8) {
        self.spi.spi2x = v & 1 != 0;
    }

    fn spi_clear_flags_on_dr_access(&mut self) {
        if self.spi.spif_read_armed {
            self.spi.spif_read_armed = false;
            self.spi.spif = false;
            self.spi.wcol = false;
            self.spi_update_irq();
        }
    }

    pub(crate) fn spi_read_dr(&mut self) -> u8 {
        self.spi_clear_flags_on_dr_access();
        self.spi.rx_buf
    }

    pub(crate) fn spi_write_dr(&mut self, v: u8) {
        self.spi_clear_flags_on_dr_access();
        let now = self.cycle;
        if !self.spi.enabled() {
            return;
        }
        if self.spi.master() {
            if self.spi.active {
                self.spi.wcol = true;
                return;
            }
            let div: u64 = [4, 16, 64, 128][(self.spi.spcr & 3) as usize];
            let div = if self.spi.spi2x { div / 2 } else { div } << self.sys.clk_shift;
            let s = &mut self.spi;
            s.active = true;
            s.out = v;
            s.inb = 0;
            s.edge = 0;
            s.start = now;
            s.half = (div / 2).max(1);
            if s.waveform && !s.cpha() {
                let b = s.out_bit(v, 0);
                let mosi = self.v.spi_mosi;
                self.set_override_level(mosi, b, now);
            }
            self.spi_update_next();
        } else {
            if self.spi.s_selected && self.spi.s_bits > 0 {
                self.spi.wcol = true;
                return;
            }
            self.spi.s_tx = v;
            if self.spi.s_selected && !self.spi.cpha() {
                let b = self.spi.out_bit(v, 0);
                let miso = self.v.spi_miso;
                self.set_override_level(miso, b, now);
            }
        }
    }

    pub(crate) fn spi_event(&mut self, t: u64, io: &mut dyn AvrIo) {
        if !self.spi.active {
            self.spi_update_next();
            return;
        }
        if !self.spi.waveform {
            let miso = io.spi_exchange(self.spi.out, t);
            self.spi_complete(miso, t);
            return;
        }
        let (mosi, miso, sck) = (self.v.spi_mosi, self.v.spi_miso, self.v.spi_sck);
        let e = self.spi.edge;
        let leading = e % 2 == 0;
        let bit = e / 2;
        let cpol = self.spi.cpol();
        let sck_level = if leading { !cpol } else { cpol };
        self.set_override_level(sck, sck_level, t);
        let sample = leading != self.spi.cpha();
        if sample {
            let b = self.pin_level(miso);
            self.spi.inb = self.spi.in_bit(self.spi.inb, bit, b);
        } else if self.spi.cpha() {
            // CPHA=1: output this bit on the leading edge.
            let b = self.spi.out_bit(self.spi.out, bit);
            self.set_override_level(mosi, b, t);
        } else if bit < 7 {
            // CPHA=0: next bit on the trailing edge.
            let b = self.spi.out_bit(self.spi.out, bit + 1);
            self.set_override_level(mosi, b, t);
        }
        self.spi.edge += 1;
        if self.spi.edge >= 16 {
            let inb = self.spi.inb;
            self.spi_complete(inb, t);
        } else {
            self.spi_update_next();
        }
    }

    fn spi_complete(&mut self, rx: u8, _t: u64) {
        self.spi.active = false;
        self.spi.rx_buf = rx;
        self.spi.spif = true;
        self.spi_update_irq();
        self.spi_update_next();
    }

    pub(crate) fn spi_sck_edge(&mut self, level: bool, at: u64) {
        let s = &self.spi;
        if !s.enabled() || s.master() || !s.s_selected {
            return;
        }
        let leading = level != s.cpol();
        let sample = leading != s.cpha();
        let bit = s.s_bits;
        let miso = self.v.spi_miso;
        if sample {
            let b = self.pin_level(self.v.spi_mosi);
            self.spi.s_in = self.spi.in_bit(self.spi.s_in, bit, b);
            self.spi.s_bits += 1;
            if self.spi.s_bits >= 8 {
                self.spi.rx_buf = self.spi.s_in;
                self.spi.s_in = 0;
                self.spi.s_bits = 0;
                self.spi.spif = true;
                self.spi_update_irq();
                if !self.spi.cpha() {
                    let b = self.spi.out_bit(self.spi.s_tx, 0);
                    self.set_override_level(miso, b, at);
                }
            }
        } else {
            let i = if self.spi.cpha() { bit } else { bit.min(7) };
            if i < 8 {
                let b = self.spi.out_bit(self.spi.s_tx, i);
                self.set_override_level(miso, b, at);
            }
        }
    }

    pub(crate) fn spi_ss_change(&mut self, level: bool, at: u64) {
        if !self.spi.enabled() {
            return;
        }
        if self.spi.master() {
            // Mode fault: SS configured as input and driven low.
            let ss = self.v.spi_ss;
            let ss_input = self.ports[ss.port as usize].ddr & (1 << ss.bit) == 0;
            if ss_input && !level {
                self.spi.spcr &= !0x10;
                self.spi.active = false;
                self.spi.spif = true;
                self.spi.s_selected = true;
                self.spi_update_irq();
                self.spi_update_pins(at);
                self.spi_update_next();
            }
            return;
        }
        self.spi.s_selected = !level;
        self.spi.s_bits = 0;
        self.spi.s_in = 0;
        self.spi_update_pins(at);
    }

    pub(crate) fn spi_ack(&mut self) {
        self.spi.spif = false;
        self.spi_update_irq();
    }

    pub(crate) fn spi_set_gate(&mut self, gated: bool, now: u64) {
        if self.spi.gated == gated {
            return;
        }
        if gated {
            self.spi.gated = true;
            self.spi.gated_at = now;
        } else {
            self.spi.gated = false;
            self.spi.start += now - self.spi.gated_at;
        }
        self.spi_update_next();
    }
}
