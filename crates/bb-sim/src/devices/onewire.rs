//! Timing-protocol sensors: DHT11/DHT22 (single-wire, bit-accurate), DS18B20 (1-Wire with ROM
//! search) and the HC-SR04 ultrasonic ranger.

use super::{DeviceRegistry, ok, pin_of, prop_f64};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, PinDrive, Pos, SimTime, VisualState};
use std::collections::VecDeque;

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("dht22", |s| Box::new(Dht::new(s, true)));
    r.register_fn("dht11", |s| Box::new(Dht::new(s, false)));
    r.register_fn("ds18b20", |s| Box::new(Ds18b20::new(s)));
    r.register_fn("hc_sr04", |s| Box::new(HcSr04::new(s)));
}

const LOW: PinDrive = PinDrive::Thevenin { v: 0.0, r: 50.0 };

/// Transmits a precomputed waveform: (time, drive low?) transitions.
#[derive(Default)]
struct Waveform {
    steps: VecDeque<(SimTime, bool)>,
}

impl Waveform {
    fn arm(&self, ctx: &mut dyn DeviceCtx, token: u64) {
        if let Some(&(t, _)) = self.steps.front() {
            ctx.wake_at(t, token);
        }
    }
    /// Applies due steps; returns true when finished.
    fn fire(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, token: u64) -> bool {
        while let Some(&(t, low)) = self.steps.front() {
            if t > ctx.now() {
                break;
            }
            self.steps.pop_front();
            ctx.set_drive(pin, if low { LOW } else { PinDrive::HighZ });
        }
        self.arm(ctx, token);
        self.steps.is_empty()
    }
}

// ------------------------------------------------------------------------------------ DHT

/// DHT22 (AM2302) / DHT11: host start pulse → 20–40 µs → 80 µs low, 80 µs high → 40 bits
/// (50 µs low + 26 µs high for 0 / 70 µs for 1) → 50 µs low → released. Values come from the
/// world's air temperature and humidity at the sensor (with ±0.1 °C / ±0.5 %RH noise).
struct Dht {
    dht22: bool,
    data: usize,
    vcc: usize,
    pos: Pos,
    fell: Option<SimTime>,
    busy_until: SimTime,
    wave: Waveform,
    last_t: f64,
    last_rh: f64,
    reads: u64,
}

const TOKEN_WAVE: u64 = 1;

impl Dht {
    fn new(s: &DeviceSpec, dht22: bool) -> Dht {
        Dht {
            dht22,
            data: pin_of(s, "DATA"),
            vcc: pin_of(s, "VCC"),
            pos: s.pos.unwrap_or_default(),
            fell: None,
            busy_until: SimTime::ZERO,
            wave: Waveform::default(),
            last_t: f64::NAN,
            last_rh: f64::NAN,
            reads: 0,
        }
    }

    fn frame(&mut self, ctx: &mut dyn DeviceCtx) -> [u8; 5] {
        let mut t = ctx.world().temperature_at(self.pos);
        let mut rh = ctx.world().humidity();
        if ctx.world().noise_enabled() {
            t += ctx.rng().gauss() * 0.05;
            rh += ctx.rng().gauss() * 0.25;
        }
        rh = rh.clamp(0.0, 99.9);
        self.last_t = t;
        self.last_rh = rh;
        let mut b = [0u8; 5];
        if self.dht22 {
            let h = (rh * 10.0).round() as u16;
            let tt = (t.abs() * 10.0).round() as u16 & 0x7FFF;
            let tt = if t < 0.0 { tt | 0x8000 } else { tt };
            b[0] = (h >> 8) as u8;
            b[1] = h as u8;
            b[2] = (tt >> 8) as u8;
            b[3] = tt as u8;
        } else {
            let t = t.clamp(0.0, 50.0);
            b[0] = rh.round() as u8;
            b[1] = 0;
            b[2] = t.floor() as u8;
            b[3] = ((t - t.floor()) * 10.0).floor() as u8;
        }
        b[4] = b[0].wrapping_add(b[1]).wrapping_add(b[2]).wrapping_add(b[3]);
        b
    }

    fn respond(&mut self, ctx: &mut dyn DeviceCtx) {
        let bytes = self.frame(ctx);
        let us = |n: u64| SimTime::from_us(n);
        let mut t = ctx.now() + us(30);
        let mut steps = VecDeque::new();
        steps.push_back((t, true));
        t += us(80);
        steps.push_back((t, false));
        t += us(80);
        for byte in bytes {
            for k in (0..8).rev() {
                steps.push_back((t, true));
                t += us(50);
                steps.push_back((t, false));
                t += if byte >> k & 1 == 1 { us(70) } else { us(26) };
            }
        }
        steps.push_back((t, true));
        t += us(50);
        steps.push_back((t, false));
        self.busy_until = t;
        self.wave.steps = steps;
        self.wave.arm(ctx, TOKEN_WAVE);
        self.reads += 1;
    }
}

impl Device for Dht {
    fn kind(&self) -> &str {
        if self.dht22 { "dht22" } else { "dht11" }
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.data) {
            ctx.watch(self.data, 1.0, 2.0);
        }
        if ok(self.vcc) {
            ctx.set_drive(self.vcc, PinDrive::Current { i: -1.5e-3 });
        }
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        if ctx.now() < self.busy_until {
            return; // our own answer
        }
        if !high {
            self.fell = Some(ctx.now());
            return;
        }
        let Some(f) = self.fell.take() else { return };
        let low = (ctx.now() - f).as_secs_f64();
        let min = if self.dht22 { 0.5e-3 } else { 15e-3 };
        let powered = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 3.0;
        if low >= min && low < 0.1 && powered {
            self.respond(ctx);
        }
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        if token == TOKEN_WAVE && ok(self.data) {
            self.wave.fire(ctx, self.data, TOKEN_WAVE);
        }
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: if self.last_t.is_nan() {
                "—".into()
            } else {
                format!("{:.1} °C · {:.1} %RH", self.last_t, self.last_rh)
            },
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("temp_c".into(), self.last_t),
            ("rh".into(), self.last_rh),
            ("reads".into(), self.reads as f64),
        ]
    }
}

// ------------------------------------------------------------------------------------ DS18B20

#[derive(Debug, Clone, PartialEq)]
enum Ow {
    /// Waiting for a reset pulse.
    Idle,
    /// Receiving a ROM command.
    RomCmd,
    /// MATCH ROM: receiving 64 bits.
    Match,
    /// SEARCH ROM: bit index and phase (0 = send bit, 1 = send complement, 2 = read direction).
    Search { bit: u32, phase: u8 },
    /// Receiving a function command.
    FuncCmd,
    /// Sending bytes.
    Send,
    /// WRITE SCRATCHPAD: receiving 3 bytes.
    WriteScratch,
    /// Read slots answer 1 when idle / 0 while converting.
    Status,
    /// Not addressed until the next reset.
    Off,
}

/// Dallas/Maxim CRC-8 (x⁸ + x⁵ + x⁴ + 1).
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        let mut x = b;
        for _ in 0..8 {
            let mix = (crc ^ x) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            x >>= 1;
        }
    }
    crc
}

struct Ds18b20 {
    dq: usize,
    vdd: usize,
    pos: Pos,
    rom: [u8; 8],
    st: Ow,
    fell: Option<SimTime>,
    own_low: bool,
    rx: u64,
    rx_n: u32,
    tx: Vec<u8>,
    tx_bit: usize,
    scratch: [u8; 9],
    conv_until: SimTime,
    temp: f64,
    conversions: u64,
}

const T_PRESENCE: u64 = 1;
const T_RELEASE: u64 = 2;
const T_SAMPLE: u64 = 3;

impl Ds18b20 {
    fn new(s: &DeviceSpec) -> Ds18b20 {
        let serial = prop_f64(s, "serial", 1.0) as u64;
        let mut rom = [0u8; 8];
        rom[0] = 0x28;
        for (i, slot) in rom.iter_mut().enumerate().take(7).skip(1) {
            *slot = (serial >> ((i - 1) * 8)) as u8;
        }
        rom[7] = crc8(&rom[..7]);
        let mut scratch = [0x50, 0x05, 0x4B, 0x46, 0x7F, 0xFF, 0x0C, 0x10, 0];
        scratch[8] = crc8(&scratch[..8]);
        Ds18b20 {
            dq: pin_of(s, "DQ"),
            vdd: pin_of(s, "VDD"),
            pos: s.pos.unwrap_or_default(),
            rom,
            st: Ow::Idle,
            fell: None,
            own_low: false,
            rx: 0,
            rx_n: 0,
            tx: Vec::new(),
            tx_bit: 0,
            scratch,
            conv_until: SimTime::ZERO,
            temp: f64::NAN,
            conversions: 0,
        }
    }

    fn rom_bit(&self, i: u32) -> bool {
        self.rom[(i / 8) as usize] >> (i % 8) & 1 == 1
    }

    /// The next bit the device sends in a read slot (None = it does not drive the slot).
    fn send_bit(&mut self, now: SimTime) -> Option<bool> {
        match self.st.clone() {
            Ow::Send => {
                let byte = *self.tx.get(self.tx_bit / 8)?;
                let b = byte >> (self.tx_bit % 8) & 1 == 1;
                self.tx_bit += 1;
                if self.tx_bit >= self.tx.len() * 8 {
                    self.st = Ow::Status;
                }
                Some(b)
            }
            Ow::Search { bit, phase } if phase < 2 => {
                let b = self.rom_bit(bit);
                self.st = Ow::Search {
                    bit,
                    phase: phase + 1,
                };
                Some(if phase == 0 { b } else { !b })
            }
            Ow::Status => Some(now >= self.conv_until),
            _ => None,
        }
    }

    fn convert(&mut self, ctx: &mut dyn DeviceCtx) {
        let mut t = ctx.world().temperature_at(self.pos);
        if ctx.world().noise_enabled() {
            t += ctx.rng().gauss() * 0.03;
        }
        self.temp = t;
        let res = ((self.scratch[4] >> 5) & 3) as u32; // 0..3 → 9..12 bit
        let raw = (t * 16.0).round() as i32;
        let mask = !((1i32 << (3 - res)) - 1);
        let raw = (raw & mask) as i16;
        self.scratch[0] = raw as u8;
        self.scratch[1] = (raw >> 8) as u8;
        self.scratch[8] = crc8(&self.scratch[..8]);
        let ms = [94u64, 188, 375, 750][res as usize];
        self.conv_until = ctx.now() + SimTime::from_ms(ms);
        self.conversions += 1;
    }

    /// A complete bit received from the master.
    fn got_bit(&mut self, ctx: &mut dyn DeviceCtx, bit: bool) {
        match self.st.clone() {
            Ow::Search { bit: i, phase: 2 } => {
                if bit != self.rom_bit(i) {
                    self.st = Ow::Off;
                } else if i == 63 {
                    self.st = Ow::FuncCmd;
                    self.rx = 0;
                    self.rx_n = 0;
                } else {
                    self.st = Ow::Search {
                        bit: i + 1,
                        phase: 0,
                    };
                }
                return;
            }
            Ow::RomCmd | Ow::Match | Ow::FuncCmd | Ow::WriteScratch => {}
            _ => return,
        }
        self.rx |= (bit as u64) << self.rx_n;
        self.rx_n += 1;
        match self.st {
            Ow::RomCmd if self.rx_n == 8 => {
                let cmd = self.rx as u8;
                self.rx = 0;
                self.rx_n = 0;
                self.st = match cmd {
                    0x33 => {
                        self.tx = self.rom.to_vec();
                        self.tx_bit = 0;
                        Ow::Send
                    }
                    0x55 => Ow::Match,
                    0xCC => Ow::FuncCmd,
                    0xF0 => Ow::Search { bit: 0, phase: 0 },
                    _ => Ow::Off,
                };
            }
            Ow::Match if self.rx_n == 64 => {
                let ok = self.rx.to_le_bytes() == self.rom;
                self.rx = 0;
                self.rx_n = 0;
                self.st = if ok { Ow::FuncCmd } else { Ow::Off };
            }
            Ow::FuncCmd if self.rx_n == 8 => {
                let cmd = self.rx as u8;
                self.rx = 0;
                self.rx_n = 0;
                self.st = match cmd {
                    0x44 => {
                        self.convert(ctx);
                        Ow::Status
                    }
                    0xBE => {
                        self.tx = self.scratch.to_vec();
                        self.tx_bit = 0;
                        Ow::Send
                    }
                    0x4E => Ow::WriteScratch,
                    0xB4 => {
                        self.tx = vec![0xFF];
                        self.tx_bit = 0;
                        Ow::Send
                    }
                    0x48 | 0xB8 => Ow::Status,
                    _ => Ow::Off,
                };
            }
            Ow::WriteScratch if self.rx_n == 24 => {
                self.scratch[2] = self.rx as u8;
                self.scratch[3] = (self.rx >> 8) as u8;
                self.scratch[4] = ((self.rx >> 16) as u8 & 0x60) | 0x1F;
                self.scratch[8] = crc8(&self.scratch[..8]);
                self.rx = 0;
                self.rx_n = 0;
                self.st = Ow::Status;
            }
            _ => {}
        }
    }
}

impl Device for Ds18b20 {
    fn kind(&self) -> &str {
        "ds18b20"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.dq) {
            ctx.watch(self.dq, 1.0, 2.0);
        }
        if ok(self.vdd) {
            ctx.set_drive(self.vdd, PinDrive::Current { i: -1e-3 });
        }
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        if self.own_low {
            return;
        }
        let now = ctx.now();
        if !high {
            self.fell = Some(now);
            // a slot starts
            match self.send_bit(now) {
                Some(false) => {
                    self.own_low = true;
                    ctx.set_drive(self.dq, LOW);
                    ctx.wake_at(now + SimTime::from_us(30), T_RELEASE);
                }
                Some(true) => {}
                None => {
                    if matches!(
                        self.st,
                        Ow::RomCmd | Ow::Match | Ow::FuncCmd | Ow::WriteScratch | Ow::Search { phase: 2, .. }
                    ) {
                        ctx.wake_at(now + SimTime::from_us(30), T_SAMPLE);
                    }
                }
            }
            return;
        }
        if let Some(f) = self.fell.take()
            && (now - f).as_secs_f64() >= 400e-6
        {
            // reset pulse → presence after 30 µs
            ctx.cancel_wake(T_SAMPLE);
            self.st = Ow::RomCmd;
            self.rx = 0;
            self.rx_n = 0;
            ctx.wake_at(now + SimTime::from_us(30), T_PRESENCE);
        }
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        let now = ctx.now();
        match token {
            T_PRESENCE => {
                self.own_low = true;
                ctx.set_drive(self.dq, LOW);
                ctx.wake_at(now + SimTime::from_us(120), T_RELEASE);
            }
            T_RELEASE => {
                ctx.set_drive(self.dq, PinDrive::HighZ);
                self.own_low = false;
                self.fell = None;
            }
            T_SAMPLE => {
                let bit = ctx.pin_voltage(self.dq) > 1.5;
                self.got_bit(ctx, bit);
            }
            _ => {}
        }
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: if self.temp.is_nan() {
                "85.0 °C".into()
            } else {
                format!("{:.2} °C", self.temp)
            },
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("temp_c".into(), self.temp), ("conversions".into(), self.conversions as f64)]
    }
}

// ------------------------------------------------------------------------------------ HC-SR04

struct HcSr04 {
    trig: usize,
    echo: usize,
    vcc: usize,
    pos: Pos,
    dir: f64,
    rose: Option<SimTime>,
    busy: bool,
    last_m: f64,
}

const T_ECHO_START: u64 = 1;
const T_ECHO_END: u64 = 2;

impl HcSr04 {
    fn new(s: &DeviceSpec) -> HcSr04 {
        HcSr04 {
            trig: pin_of(s, "TRIG"),
            echo: pin_of(s, "ECHO"),
            vcc: pin_of(s, "VCC"),
            pos: s.pos.unwrap_or_default(),
            dir: prop_f64(s, "direction", -90.0),
            rose: None,
            busy: false,
            last_m: f64::NAN,
        }
    }
    fn echo(&self, ctx: &mut dyn DeviceCtx, high: bool) {
        if ok(self.echo) {
            let vcc = if ok(self.vcc) { ctx.pin_voltage(self.vcc) } else { 5.0 };
            ctx.set_drive(
                self.echo,
                PinDrive::Thevenin {
                    v: if high { vcc } else { 0.0 },
                    r: 50.0,
                },
            );
        }
    }
}

impl Device for HcSr04 {
    fn kind(&self) -> &str {
        "hc_sr04"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.trig) {
            ctx.set_drive(self.trig, PinDrive::Pull { v: 0.0, r: 10e3 });
            ctx.watch(self.trig, 0.8, 2.0);
        }
        if ok(self.vcc) {
            ctx.set_drive(self.vcc, PinDrive::Current { i: -15e-3 });
        }
        self.echo(ctx, false);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        if high {
            self.rose = Some(ctx.now());
        } else if let Some(r) = self.rose.take()
            && !self.busy
            && (ctx.now() - r).as_secs_f64() >= 9e-6
        {
            self.busy = true;
            ctx.wake_at(ctx.now() + SimTime::from_us(250), T_ECHO_START);
        }
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        match token {
            T_ECHO_START => {
                self.echo(ctx, true);
                let d = ctx.world().distance(self.pos, self.dir, 15.0, 4.0);
                self.last_m = d.unwrap_or(f64::NAN);
                let width = match d {
                    Some(m) if m >= 0.02 => 2.0 * m / 343.0,
                    Some(_) => 2.0 * 0.02 / 343.0,
                    None => 0.038,
                };
                ctx.wake_at(ctx.now() + SimTime::from_secs_f64(width), T_ECHO_END);
            }
            T_ECHO_END => {
                self.echo(ctx, false);
                self.busy = false;
            }
            _ => {}
        }
    }
    fn set_param(&mut self, _ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        if key == "direction" {
            self.dir = value;
            true
        } else {
            false
        }
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: if self.last_m.is_nan() {
                "no echo".into()
            } else {
                format!("{:.1} cm", self.last_m * 100.0)
            },
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("distance_m".into(), self.last_m)]
    }
}
