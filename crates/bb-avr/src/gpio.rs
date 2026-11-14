//! GPIO ports: DDR/PORT/PIN, peripheral pin overrides, external input levels and the edge
//! fan-out to INTn / PCINT / USART RX / SPI / TWI / input capture / timer clock inputs.

use crate::avr::{Avr, CpuState};
use crate::variant::Variant;
use crate::{Pin, PinEvent, PinInfo, PinOwner, PinState, StopReason};

#[derive(Debug, Clone, Default)]
pub(crate) struct Port {
    pub present: bool,
    pub ddr: u8,
    pub port: u8,
    pub ovr_dir_mask: u8,
    pub ovr_dir_val: u8,
    pub ovr_out_mask: u8,
    pub ovr_out_val: u8,
    pub owner: [PinOwner; 8],
    pub ext_known: u8,
    pub ext_level: u8,
    pub didr: u8,
    // derived
    pub dir: u8,
    pub out: u8,
    pub pull: u8,
    pub level: u8,
}

impl Port {
    pub fn new(present: bool) -> Port {
        Port {
            present,
            ..Default::default()
        }
    }
    /// Reset: registers and overrides cleared, external connections kept.
    pub fn reset(&mut self) {
        self.ddr = 0;
        self.port = 0;
        self.ovr_dir_mask = 0;
        self.ovr_dir_val = 0;
        self.ovr_out_mask = 0;
        self.ovr_out_val = 0;
        self.owner = [PinOwner::Gpio; 8];
        self.didr = 0;
    }
}

/// Peripheral functions that listen to a pin's input level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinFunc {
    Int(u8),
    Pcint(u8, u8),
    UsartRx(u8),
    SpiSck,
    SpiSs,
    SpiMosi,
    TwiSda,
    TwiScl,
    Icp(u8),
    TClk(u8),
}

pub(crate) fn build_pin_funcs(v: &Variant) -> Vec<Vec<PinFunc>> {
    let mut f: Vec<Vec<PinFunc>> = vec![Vec::new(); 11 * 8];
    for (n, p) in v.ext_ints.iter().enumerate() {
        f[p.index()].push(PinFunc::Int(n as u8));
    }
    for (g, grp) in v.pcint.iter().enumerate() {
        for (b, p) in grp.iter().enumerate() {
            if let Some(p) = p {
                f[p.index()].push(PinFunc::Pcint(g as u8, b as u8));
            }
        }
    }
    for u in v.usarts {
        f[u.rx.index()].push(PinFunc::UsartRx(u.n));
    }
    f[v.spi_sck.index()].push(PinFunc::SpiSck);
    f[v.spi_ss.index()].push(PinFunc::SpiSs);
    f[v.spi_mosi.index()].push(PinFunc::SpiMosi);
    f[v.twi_sda.index()].push(PinFunc::TwiSda);
    f[v.twi_scl.index()].push(PinFunc::TwiScl);
    for t in v.timers {
        if let Some(p) = t.icp {
            f[p.index()].push(PinFunc::Icp(t.n));
        }
        if let Some(p) = t.tclk {
            f[p.index()].push(PinFunc::TClk(t.n));
        }
    }
    f
}

impl Avr {
    pub(crate) fn pin_reg(&self, p: usize) -> u8 {
        let s = &self.ports[p];
        s.level & !s.didr
    }

    #[inline]
    pub(crate) fn pin_level(&self, pin: Pin) -> bool {
        self.ports[pin.port as usize].level & (1 << pin.bit) != 0
    }

    pub(crate) fn gpio_write_port(&mut self, p: usize, v: u8, at: u64) {
        if self.ports[p].port != v {
            self.ports[p].port = v;
            self.port_refresh(p, at);
        }
    }

    pub(crate) fn gpio_write_ddr(&mut self, p: usize, v: u8, at: u64) {
        if self.ports[p].ddr != v {
            self.ports[p].ddr = v;
            self.port_refresh(p, at);
        }
    }

    /// Takes over `pin` for a peripheral. `dir`: Some(true) force output, Some(false) force
    /// input, None leave to DDR. `out`: Some(level) output value, None leave to PORT.
    pub(crate) fn set_pin_override(
        &mut self,
        pin: Pin,
        owner: PinOwner,
        dir: Option<bool>,
        out: Option<bool>,
        at: u64,
    ) {
        let p = pin.port as usize;
        let m = 1u8 << pin.bit;
        let s = &mut self.ports[p];
        s.owner[pin.bit as usize] = owner;
        match dir {
            Some(o) => {
                s.ovr_dir_mask |= m;
                if o {
                    s.ovr_dir_val |= m
                } else {
                    s.ovr_dir_val &= !m
                }
            }
            None => s.ovr_dir_mask &= !m,
        }
        match out {
            Some(o) => {
                s.ovr_out_mask |= m;
                if o {
                    s.ovr_out_val |= m
                } else {
                    s.ovr_out_val &= !m
                }
            }
            None => s.ovr_out_mask &= !m,
        }
        self.port_refresh(p, at);
    }

    /// Releases `pin` if `owner` currently controls it.
    pub(crate) fn clear_pin_override(&mut self, pin: Pin, owner: PinOwner, at: u64) {
        let p = pin.port as usize;
        if self.ports[p].owner[pin.bit as usize] != owner {
            return;
        }
        let m = 1u8 << pin.bit;
        let s = &mut self.ports[p];
        s.owner[pin.bit as usize] = PinOwner::Gpio;
        s.ovr_dir_mask &= !m;
        s.ovr_out_mask &= !m;
        self.port_refresh(p, at);
    }

    /// Fast update of only the output value of an already-overridden pin.
    #[inline]
    pub(crate) fn set_override_level(&mut self, pin: Pin, level: bool, at: u64) {
        let p = pin.port as usize;
        let m = 1u8 << pin.bit;
        let s = &mut self.ports[p];
        let nv = if level {
            s.ovr_out_val | m
        } else {
            s.ovr_out_val & !m
        };
        if nv != s.ovr_out_val {
            s.ovr_out_val = nv;
            if s.ovr_out_mask & m != 0 {
                self.port_refresh(p, at);
            }
        }
    }

    /// Recomputes the drive and input level of port `p`, emitting pin events and edges.
    pub(crate) fn port_refresh(&mut self, p: usize, at: u64) {
        if !self.ports[p].present {
            return;
        }
        let tristate = matches!(self.state, CpuState::InReset { .. });
        let pud = self.sys.mcucr & 0x10 != 0;
        let s = &self.ports[p];
        let (dir, out, pull) = if tristate {
            (0, 0, 0)
        } else {
            let dir = (s.ddr & !s.ovr_dir_mask) | (s.ovr_dir_val & s.ovr_dir_mask);
            let out = (s.port & !s.ovr_out_mask) | (s.ovr_out_val & s.ovr_out_mask);
            let pull = if pud { 0 } else { !dir & s.port };
            (dir, out, pull)
        };
        let changed = (dir ^ s.dir) | (dir & (out ^ s.out)) | (!dir & (pull ^ s.pull));
        let own = (dir & out) | (!dir & pull);
        let level = (s.ext_known & s.ext_level) | (!s.ext_known & own);
        let lch = level ^ s.level;
        let s = &mut self.ports[p];
        s.dir = dir;
        s.out = out;
        s.pull = pull;
        s.level = level;
        if changed != 0 {
            for b in 0..8u8 {
                if changed & (1 << b) != 0 {
                    let state = if dir & (1 << b) != 0 {
                        PinState::Output {
                            high: out & (1 << b) != 0,
                        }
                    } else {
                        PinState::Input {
                            pullup: pull & (1 << b) != 0,
                        }
                    };
                    self.pin_events.push(PinEvent {
                        cycle: at,
                        pin: Pin {
                            port: p as u8,
                            bit: b,
                        },
                        state,
                    });
                }
            }
            if self.cfg.stop_on.pin_change {
                self.request_stop(StopReason::PinChange);
            }
        }
        if lch != 0 {
            for b in 0..8u8 {
                if lch & (1 << b) != 0 {
                    self.pin_level_changed(
                        Pin {
                            port: p as u8,
                            bit: b,
                        },
                        level & (1 << b) != 0,
                        at,
                    );
                }
            }
        }
    }

    /// Applies an external input level.
    pub(crate) fn apply_input(&mut self, pin: Pin, level: Option<bool>, at: u64) {
        let p = pin.port as usize;
        if p >= self.ports.len() {
            return;
        }
        let m = 1u8 << pin.bit;
        let s = &mut self.ports[p];
        match level {
            Some(l) => {
                s.ext_known |= m;
                if l {
                    s.ext_level |= m
                } else {
                    s.ext_level &= !m
                }
            }
            None => s.ext_known &= !m,
        }
        self.port_refresh(p, at);
    }

    fn pin_level_changed(&mut self, pin: Pin, level: bool, at: u64) {
        let idx = pin.index();
        for i in 0..self.pin_funcs[idx].len() {
            match self.pin_funcs[idx][i] {
                PinFunc::Int(n) => self.exint_pin_change(n, level, at),
                PinFunc::Pcint(g, b) => self.pcint_pin_change(g, b, at),
                PinFunc::UsartRx(n) => self.usart_rx_edge(n as usize, level, at),
                PinFunc::SpiSck => self.spi_sck_edge(level, at),
                PinFunc::SpiSs => self.spi_ss_change(level, at),
                PinFunc::SpiMosi => {}
                PinFunc::TwiSda => self.twi_line_change(true, level, at),
                PinFunc::TwiScl => self.twi_line_change(false, level, at),
                PinFunc::Icp(t) => self.timer_icp_edge(t as usize, level, at),
                PinFunc::TClk(t) => self.timer_ext_clock_edge(t as usize, level, at),
            }
        }
    }

    pub(crate) fn pin_info_impl(&self, pin: Pin) -> PinInfo {
        let s = &self.ports[pin.port as usize % self.ports.len()];
        let m = 1u8 << pin.bit;
        let state = if s.dir & m != 0 {
            PinState::Output {
                high: s.out & m != 0,
            }
        } else {
            PinState::Input {
                pullup: s.pull & m != 0,
            }
        };
        PinInfo {
            ddr: s.ddr & m != 0,
            port: s.port & m != 0,
            pullup: s.pull & m != 0,
            owner: s.owner[pin.bit as usize],
            state,
            level: s.level & m != 0,
            external: (s.ext_known & m != 0).then_some(s.ext_level & m != 0),
        }
    }

    pub(crate) fn didr_write(&mut self, i: u8, v: u8) {
        self.adc.didr[i as usize & 3] = v;
        for port in self.ports.iter_mut() {
            port.didr = 0;
        }
        let vv = self.v;
        for (ch, pin) in vv.adc_pins.iter().enumerate() {
            let reg = if ch < 8 { 0 } else { 2 };
            if self.adc.didr[reg] & (1 << (ch % 8)) != 0 {
                self.ports[pin.port as usize].didr |= 1 << pin.bit;
            }
        }
        if self.adc.didr[1] & 1 != 0 {
            self.ports[vv.ain0.port as usize].didr |= 1 << vv.ain0.bit;
        }
        if self.adc.didr[1] & 2 != 0 {
            self.ports[vv.ain1.port as usize].didr |= 1 << vv.ain1.bit;
        }
    }

    /// Refreshes every port (after PUD or reset-state changes).
    pub(crate) fn refresh_all_ports(&mut self, at: u64) {
        for p in 0..self.ports.len() {
            self.port_refresh(p, at);
        }
    }
}
