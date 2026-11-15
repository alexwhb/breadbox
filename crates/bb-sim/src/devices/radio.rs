//! UART radio modules on the bench — ESP-01 (ESP8266 ESP-AT), HM-10 (BLE), HC-05 (SPP) and
//! REYAX RYLR896 (LoRa, from the LoRa pack) — bridging their pins to the real modems in the
//! bench's [`crate::RadioService`] (`bb_radio::RadioSpace`):
//!
//! * RX: Schmitt watch on the RX pin, decoded with exact bit timing (`bb_radio::uart`, following
//!   the module's current baud — ESP-01 boot log at 74 880 baud, `AT+UART_CUR`); each byte
//!   reaches the modem at its stop-bit sample time.
//! * TX: the modem's frames become Thevenin level changes on the TX pin at their exact times.
//! * Power: VCC (and ESP-01 EN/CH_PD, RST, HC-05 KEY) are watched; the module draws its supply
//!   current. STATE pins (HM-10, HC-05) go high while the phone is connected.
//!
//! Without a radio service on the bench (custom `BenchOptions` without it) the module falls back
//! to the small AT responder [`super::AtStub`].

use super::uart::AtFlavor;
use super::{AtStub, DeviceRegistry, ok, pin_of, prop_f64};
use crate::radio::{ModuleKind, RadioService, TOKEN_RADIO_TX};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, PinDrive, SimTime, UserInput, VisualState};
use bb_radio::uart::UartDecoder;
use bb_radio::{DeviceId, ModulePin};
use std::collections::VecDeque;

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("esp01", |s| Box::new(RadioModule::new(s, Flavor::Esp01)));
    r.register_fn("hm10", |s| Box::new(RadioModule::new(s, Flavor::Hm10)));
    r.register_fn("hc05", |s| Box::new(RadioModule::new(s, Flavor::Hc05)));
    r.register_fn("rylr896", |s| {
        Box::new(RadioModule::new(s, Flavor::Rylr896))
    });
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Flavor {
    Esp01,
    Hm10,
    Hc05,
    Rylr896,
}

const T_RX: u64 = 0x5241_0001;
const T_TX: u64 = 0x5241_0002;
const T_STATE: u64 = 0x5241_0003;

/// A UART radio module bridged to the radio space.
pub struct RadioModule {
    flavor: Flavor,
    part: String,
    baud: u32,
    rx: usize,
    tx: usize,
    vcc: usize,
    en: usize,
    rst: usize,
    key: usize,
    state: usize,
    v_high: f64,
    supply_a: f64,
    id: Option<DeviceId>,
    decoder: Option<UartDecoder>,
    pending: VecDeque<(SimTime, bool)>,
    powered: bool,
    in_reset: bool,
    status: String,
    link: bool,
    bytes_in: u64,
    bytes_out: u64,
    stub: Option<AtStub>,
}

impl RadioModule {
    fn new(s: &DeviceSpec, flavor: Flavor) -> RadioModule {
        let (rx, tx) = match flavor {
            Flavor::Esp01 => ("RX", "TX"),
            _ => ("RXD", "TXD"),
        };
        let first = |names: &[&str]| {
            names
                .iter()
                .map(|n| pin_of(s, n))
                .find(|p| ok(*p))
                .unwrap_or(usize::MAX)
        };
        let default_baud = match flavor {
            Flavor::Esp01 => 115_200.0,
            Flavor::Rylr896 => 115_200.0,
            _ => 9600.0,
        };
        RadioModule {
            flavor,
            part: s.part.clone(),
            baud: prop_f64(s, "baud", default_baud) as u32,
            rx: pin_of(s, rx),
            tx: pin_of(s, tx),
            vcc: first(&["VCC", "VDD"]),
            en: if flavor == Flavor::Esp01 {
                pin_of(s, "EN")
            } else {
                usize::MAX
            },
            rst: first(&["RST", "NRST"]),
            key: if flavor == Flavor::Hc05 {
                first(&["EN", "KEY"])
            } else {
                usize::MAX
            },
            state: pin_of(s, "STATE"),
            v_high: 3.3,
            supply_a: match flavor {
                Flavor::Esp01 => 0.07,
                Flavor::Hm10 => 0.009,
                Flavor::Hc05 => 0.03,
                Flavor::Rylr896 => 0.017,
            },
            id: None,
            decoder: None,
            pending: VecDeque::new(),
            powered: false,
            in_reset: false,
            status: "off".into(),
            link: false,
            bytes_in: 0,
            bytes_out: 0,
            stub: match flavor {
                Flavor::Esp01 => Some(AtStub::new(s, AtFlavor::Esp)),
                Flavor::Hm10 => Some(AtStub::new(s, AtFlavor::Hm10)),
                Flavor::Hc05 => Some(AtStub::new(s, AtFlavor::Hc05)),
                Flavor::Rylr896 => None,
            },
        }
    }

    fn kind(&self) -> ModuleKind {
        match self.flavor {
            Flavor::Esp01 => ModuleKind::Esp01 { baud: self.baud },
            Flavor::Hm10 => ModuleKind::Hm10,
            Flavor::Hc05 => ModuleKind::Hc05,
            Flavor::Rylr896 => ModuleKind::Rylr896,
        }
    }

    fn svc(ctx: &mut dyn DeviceCtx) -> Option<&mut RadioService> {
        ctx.service("radio")?.downcast_mut::<RadioService>()
    }

    fn supply_ok(&self, ctx: &dyn DeviceCtx) -> bool {
        let vcc = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 2.5;
        let en = !ok(self.en) || ctx.pin_voltage(self.en) > 1.5;
        vcc && en
    }

    fn reset_low(&self, ctx: &dyn DeviceCtx) -> bool {
        ok(self.rst) && ctx.pin_voltage(self.rst) < 0.8
    }

    /// Re-evaluates supply / enable / reset and tells the modem.
    fn update_power(&mut self, ctx: &mut dyn DeviceCtx) {
        let Some(id) = self.id else { return };
        let on = self.supply_ok(ctx);
        let rst = self.reset_low(ctx);
        let now = ctx.now();
        if ok(self.vcc) {
            ctx.set_drive(
                self.vcc,
                PinDrive::Current {
                    i: if on { -self.supply_a } else { 0.0 },
                },
            );
        }
        if self.flavor == Flavor::Hc05 && ok(self.key) {
            let key = ctx.pin_voltage(self.key) > 1.5;
            if let Some(s) = Self::svc(ctx) {
                s.set_pin(id, ModulePin::Key, key, now);
            }
        }
        if on != self.powered {
            self.powered = on;
            if let Some(s) = Self::svc(ctx) {
                s.power(id, on, now);
            }
            if !on {
                self.pending.clear();
            }
        }
        if on && rst != self.in_reset {
            self.in_reset = rst;
            if let Some(s) = Self::svc(ctx) {
                s.set_pin(id, ModulePin::Reset, !rst, now);
            }
        }
        self.drive_tx_idle(ctx);
        self.pull_frames(ctx);
    }

    fn drive_tx_idle(&mut self, ctx: &mut dyn DeviceCtx) {
        if !ok(self.tx) || !self.pending.is_empty() {
            return;
        }
        let d = if self.powered {
            PinDrive::Thevenin {
                v: self.v_high,
                r: 50.0,
            }
        } else {
            PinDrive::HighZ
        };
        ctx.set_drive(self.tx, d);
    }

    /// Takes the modem's frames and queues their level changes on TX.
    fn pull_frames(&mut self, ctx: &mut dyn DeviceCtx) {
        let Some(id) = self.id else { return };
        let now = ctx.now();
        let (frames, status, link) = match Self::svc(ctx) {
            Some(s) => (s.take_frames(id), s.status(id), s.link_up(id)),
            None => return,
        };
        self.status = status;
        if link != self.link {
            self.link = link;
            ctx.wake_at(now, T_STATE);
        }
        if frames.is_empty() || !self.powered {
            return;
        }
        let was_empty = self.pending.is_empty();
        let mut last = self.pending.back().map(|p| p.0).unwrap_or(SimTime::ZERO);
        for f in frames {
            self.bytes_out += 1;
            let shift = last
                .saturating_sub(f.start)
                .max(now.saturating_sub(f.start));
            for (t, level) in f.transitions() {
                let t = t + shift;
                last = last.max(t);
                self.pending.push_back((t, level));
            }
        }
        if was_empty && let Some(&(t, _)) = self.pending.front() {
            ctx.wake_at(t, T_TX);
        }
    }

    fn tx_due(&mut self, ctx: &mut dyn DeviceCtx) {
        let now = ctx.now();
        while let Some(&(t, level)) = self.pending.front() {
            if t > now {
                break;
            }
            self.pending.pop_front();
            if ok(self.tx) {
                ctx.set_drive(
                    self.tx,
                    PinDrive::Thevenin {
                        v: if level { self.v_high } else { 0.0 },
                        r: 50.0,
                    },
                );
            }
        }
        if let Some(&(t, _)) = self.pending.front() {
            ctx.wake_at(t, T_TX);
        }
    }

    fn rx_due(&mut self, ctx: &mut dyn DeviceCtx) {
        let (Some(id), Some(dec)) = (self.id, self.decoder.as_mut()) else {
            return;
        };
        let now = ctx.now();
        let bytes = dec.poll(now);
        if let Some(s) = ctx
            .service("radio")
            .and_then(|a| a.downcast_mut::<RadioService>())
        {
            // a break / glitch (stop bit low) is dropped, like a real UART flags and discards it
            for b in bytes.iter().filter(|b| b.ok()) {
                if self.powered {
                    s.uart_rx(id, b.data as u8, b.time);
                }
            }
            if let Some(cfg) = s.uart_config(id) {
                let dec = self.decoder.as_mut().expect("decoder");
                if cfg != dec.config() && !dec.busy() {
                    dec.set_config(cfg);
                }
            }
        }
        self.bytes_in += bytes.len() as u64;
        if let Some(t) = self.decoder.as_ref().and_then(|d| d.next_decision()) {
            ctx.wake_at(t, T_RX);
        }
        self.pull_frames(ctx);
    }
}

impl Device for RadioModule {
    fn kind(&self) -> &str {
        match self.flavor {
            Flavor::Esp01 => "esp01",
            Flavor::Hm10 => "hm10",
            Flavor::Hc05 => "hc05",
            Flavor::Rylr896 => "rylr896",
        }
    }

    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        let kind = self.kind();
        let part = self.part.clone();
        let attached = Self::svc(ctx).map(|s| {
            let id = s.attach(&part, kind);
            (id, s.uart_config(id))
        });
        let Some((id, cfg)) = attached else {
            // no radio space on this bench: small AT responder
            if let Some(stub) = self.stub.as_mut() {
                stub.on_start(ctx);
            }
            return;
        };
        self.stub = None;
        self.id = Some(id);
        self.decoder = Some(UartDecoder::new(cfg.unwrap_or_default()));
        // ESP-01S / RYLR896: 10 kΩ pull-ups on EN (CH_PD) and RST, so unconnected = enabled
        for p in [self.en, self.rst] {
            if ok(p) {
                ctx.set_drive(
                    p,
                    PinDrive::Pull {
                        v: self.v_high,
                        r: 10_000.0,
                    },
                );
            }
        }
        for p in [self.vcc, self.en, self.rst, self.key] {
            if ok(p) {
                ctx.watch(p, 0.8, 2.0);
            }
        }
        if ok(self.rx) {
            ctx.watch(self.rx, 0.8, 2.0);
        }
        // after a rebuild the modem keeps running: resume where it was
        self.powered = false;
        self.in_reset = false;
        self.update_power(ctx);
    }

    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        if let Some(stub) = self.stub.as_mut() {
            stub.on_edge(ctx, pin, high);
            return;
        }
        if pin == self.rx {
            if let Some(dec) = self.decoder.as_mut() {
                dec.edge(ctx.now(), high);
                if let Some(t) = dec.next_decision() {
                    ctx.wake_at(t, T_RX);
                }
            }
            return;
        }
        self.update_power(ctx);
    }

    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        if let Some(stub) = self.stub.as_mut() {
            stub.on_wake(ctx, token);
            return;
        }
        match token {
            T_RX => self.rx_due(ctx),
            T_TX => self.tx_due(ctx),
            TOKEN_RADIO_TX => self.pull_frames(ctx),
            T_STATE if ok(self.state) => {
                ctx.set_drive(
                    self.state,
                    PinDrive::Thevenin {
                        v: if self.link && self.powered {
                            self.v_high
                        } else {
                            0.0
                        },
                        r: 100.0,
                    },
                );
            }
            _ => {}
        }
    }

    fn on_input(&mut self, ctx: &mut dyn DeviceCtx, input: &UserInput) {
        if let Some(stub) = self.stub.as_mut() {
            stub.on_input(ctx, input);
        }
    }

    fn visual(&self) -> VisualState {
        if let Some(stub) = &self.stub {
            return stub.visual();
        }
        Look::Readout {
            text: if self.powered {
                self.status.clone()
            } else {
                "off".into()
            },
        }
        .into()
    }

    fn readings(&self) -> Vec<(String, f64)> {
        if let Some(stub) = &self.stub {
            return stub.readings();
        }
        vec![
            ("powered".into(), self.powered as u8 as f64),
            ("bytes_in".into(), self.bytes_in as f64),
            ("frames_out".into(), self.bytes_out as f64),
            ("link".into(), self.link as u8 as f64),
        ]
    }
}
