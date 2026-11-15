//! Bit-level I²C slave ([`I2cSlave`]) and the I²C devices built on it: BME280 and SSD1306.
//! (The PCF8574 LCD backpack lives with the HD44780 in `lcd.rs`.)

use super::{DeviceRegistry, ok, pin_of, prop_text};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, PinDrive, Pos, VisualState};

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("bme280", |s| Box::new(Bme280Dev::new(s)));
    r.register_fn("ssd1306", |s| Box::new(Ssd1306Dev::new(s)));
}

/// Register-level behaviour behind an I²C address.
pub trait I2cRegs {
    /// 7-bit address.
    fn address(&self) -> u8;
    /// A transaction to this device starts (after a matching address byte).
    fn start(&mut self, _read: bool, _ctx: &mut dyn DeviceCtx) {}
    /// A byte written by the master (`index` counts bytes since the address).
    fn write(&mut self, byte: u8, index: usize, ctx: &mut dyn DeviceCtx);
    /// The next byte to send to the master.
    fn read(&mut self, ctx: &mut dyn DeviceCtx) -> u8;
    fn stop(&mut self, _ctx: &mut dyn DeviceCtx) {}
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ph {
    Idle,
    Addr,
    AckAddr {
        read: bool,
    },
    Write,
    AckWrite,
    Read,
    MasterAck,
    /// Not addressed: wait for STOP / repeated START.
    Ignore,
}

/// Bit-level I²C slave state machine driven by SCL/SDA edges (open-drain SDA: drive low or
/// release).
pub struct I2cSlave<R: I2cRegs> {
    pub regs: R,
    pub sda: usize,
    pub scl: usize,
    ph: Ph,
    scl_high: bool,
    sda_high: bool,
    bits: u8,
    byte: u8,
    index: usize,
    out: u8,
    master_acked: bool,
    pub transactions: u64,
}

impl<R: I2cRegs> I2cSlave<R> {
    pub fn new(regs: R, sda: usize, scl: usize) -> Self {
        I2cSlave {
            regs,
            sda,
            scl,
            ph: Ph::Idle,
            scl_high: true,
            sda_high: true,
            bits: 0,
            byte: 0,
            index: 0,
            out: 0,
            master_acked: false,
            transactions: 0,
        }
    }

    pub fn start(&mut self, ctx: &mut dyn DeviceCtx, vcc: f64) {
        if ok(self.sda) && ok(self.scl) {
            ctx.watch(self.scl, 0.3 * vcc, 0.7 * vcc);
            ctx.watch(self.sda, 0.3 * vcc, 0.7 * vcc);
            self.scl_high = ctx.pin_voltage(self.scl) > 0.5 * vcc;
            self.sda_high = ctx.pin_voltage(self.sda) > 0.5 * vcc;
        }
    }

    fn sda_low(&self, ctx: &mut dyn DeviceCtx, low: bool) {
        let d = if low {
            PinDrive::Thevenin { v: 0.0, r: 30.0 }
        } else {
            PinDrive::HighZ
        };
        ctx.set_drive(self.sda, d);
    }

    /// Feed every edge of the device here; returns true if it was an I²C pin.
    pub fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) -> bool {
        if pin == self.sda {
            self.sda_high = high;
            if self.scl_high {
                if !high {
                    // START / repeated START
                    self.ph = Ph::Addr;
                    self.bits = 0;
                    self.byte = 0;
                    self.index = 0;
                } else {
                    // STOP
                    if !matches!(self.ph, Ph::Idle | Ph::Ignore) {
                        self.regs.stop(ctx);
                    }
                    self.ph = Ph::Idle;
                    self.sda_low(ctx, false);
                }
            }
            return true;
        }
        if pin != self.scl {
            return false;
        }
        self.scl_high = high;
        if high {
            // sample on the rising edge
            match self.ph {
                Ph::Addr | Ph::Write => {
                    self.byte = (self.byte << 1) | self.sda_high as u8;
                    self.bits += 1;
                }
                Ph::MasterAck => self.master_acked = !self.sda_high,
                _ => {}
            }
            return true;
        }
        // falling edge: change our SDA output
        match self.ph {
            Ph::Addr if self.bits == 8 => {
                let addr = self.byte >> 1;
                let read = self.byte & 1 == 1;
                if addr == self.regs.address() {
                    self.transactions += 1;
                    self.regs.start(read, ctx);
                    self.sda_low(ctx, true);
                    self.ph = Ph::AckAddr { read };
                } else {
                    self.ph = Ph::Ignore;
                }
            }
            Ph::AckAddr { read } => {
                if read {
                    self.out = self.regs.read(ctx);
                    self.sda_low(ctx, self.out & 0x80 == 0);
                    self.bits = 1;
                    self.ph = Ph::Read;
                } else {
                    self.sda_low(ctx, false);
                    self.bits = 0;
                    self.byte = 0;
                    self.ph = Ph::Write;
                }
            }
            Ph::Write if self.bits == 8 => {
                let b = self.byte;
                let i = self.index;
                self.index += 1;
                self.regs.write(b, i, ctx);
                self.sda_low(ctx, true);
                self.ph = Ph::AckWrite;
            }
            Ph::AckWrite => {
                self.sda_low(ctx, false);
                self.bits = 0;
                self.byte = 0;
                self.ph = Ph::Write;
            }
            Ph::Read => {
                if self.bits < 8 {
                    let bit = self.out >> (7 - self.bits) & 1;
                    self.sda_low(ctx, bit == 0);
                    self.bits += 1;
                } else {
                    self.sda_low(ctx, false);
                    self.ph = Ph::MasterAck;
                }
            }
            Ph::MasterAck => {
                if self.master_acked {
                    self.out = self.regs.read(ctx);
                    self.sda_low(ctx, self.out & 0x80 == 0);
                    self.bits = 1;
                    self.ph = Ph::Read;
                } else {
                    self.sda_low(ctx, false);
                    self.ph = Ph::Ignore;
                }
            }
            _ => {}
        }
        true
    }
}

impl<R: I2cRegs> I2cSlave<R> {
    /// One transaction-level transfer from an API-level master (ESP32 / Pico `Wire`): a write
    /// phase (if any bytes), then a read phase of `read` bytes. `None` = address NACK.
    pub fn transfer(
        &mut self,
        ctx: &mut dyn DeviceCtx,
        addr: u8,
        write: &[u8],
        read: usize,
    ) -> Option<Vec<u8>> {
        if addr != self.regs.address() {
            return None;
        }
        self.transactions += 1;
        if !write.is_empty() || read == 0 {
            self.regs.start(false, ctx);
            for (i, b) in write.iter().enumerate() {
                self.regs.write(*b, i, ctx);
            }
            if read == 0 {
                self.regs.stop(ctx);
                return Some(Vec::new());
            }
        }
        self.regs.start(true, ctx);
        let out = (0..read).map(|_| self.regs.read(ctx)).collect();
        self.regs.stop(ctx);
        Some(out)
    }
}

fn parse_addr(s: &str, default: u8) -> u8 {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u8::from_str_radix(t, 16).unwrap_or(default)
}

// ------------------------------------------------------------------------------------ BME280

/// Bosch BME280 with real calibration constants: raw ADC values are found by inverting the
/// datasheet's compensation formulas, so the Adafruit library reads back the world's values.
pub struct Bme280 {
    addr: u8,
    regs: [u8; 256],
    ptr: u8,
    pub temp: f64,
    pub rh: f64,
    pub pressure: f64,
    pos: Pos,
}

const T1: f64 = 27504.0;
const T2: f64 = 26435.0;
const T3: f64 = -1000.0;
const P: [f64; 9] = [
    36477.0, -10685.0, 3024.0, 2855.0, 140.0, -7.0, 15500.0, -14600.0, 6000.0,
];
const H1: f64 = 75.0;
const H2: f64 = 362.0;
const H3: f64 = 0.0;
const H4: f64 = 313.0;
const H5: f64 = 50.0;
const H6: f64 = 30.0;

/// Datasheet integer compensation (temperature, °C×100) and t_fine.
fn comp_t(adc: i32) -> (i32, i32) {
    let (t1, t2, t3) = (T1 as i32, T2 as i32, T3 as i32);
    let var1 = (((adc >> 3) - (t1 << 1)) * t2) >> 11;
    let var2 = (((((adc >> 4) - t1) * ((adc >> 4) - t1)) >> 12) * t3) >> 14;
    let t_fine = var1 + var2;
    ((t_fine * 5 + 128) >> 8, t_fine)
}

/// Pressure in Pa × 256 (64-bit version).
fn comp_p(adc: i32, t_fine: i32) -> i64 {
    let p: Vec<i64> = P.iter().map(|x| *x as i64).collect();
    let mut var1 = t_fine as i64 - 128000;
    let mut var2 = var1 * var1 * p[5];
    var2 += (var1 * p[4]) << 17;
    var2 += p[3] << 35;
    var1 = ((var1 * var1 * p[2]) >> 8) + ((var1 * p[1]) << 12);
    var1 = (((1i64 << 47) + var1) * p[0]) >> 33;
    if var1 == 0 {
        return 0;
    }
    let mut pp = 1048576 - adc as i64;
    pp = (((pp << 31) - var2) * 3125) / var1;
    var1 = (p[8] * (pp >> 13) * (pp >> 13)) >> 25;
    var2 = (p[7] * pp) >> 19;
    ((pp + var1 + var2) >> 8) + (p[6] << 4)
}

/// Humidity in %RH × 1024.
fn comp_h(adc: i32, t_fine: i32) -> i32 {
    let (h1, h2, h3, h4, h5, h6) = (
        H1 as i32, H2 as i32, H3 as i32, H4 as i32, H5 as i32, H6 as i32,
    );
    let mut v = t_fine - 76800;
    v = ((((adc << 14) - (h4 << 20) - (h5 * v)) + 16384) >> 15)
        * (((((((v * h6) >> 10) * (((v * h3) >> 11) + 32768)) >> 10) + 2097152) * h2 + 8192) >> 14);
    v -= ((((v >> 15) * (v >> 15)) >> 7) * h1) >> 4;
    v = v.clamp(0, 419430400);
    v >> 12
}

fn search(lo: i32, hi: i32, increasing: bool, want: f64, f: impl Fn(i32) -> f64) -> i32 {
    let (mut a, mut b) = (lo, hi);
    while b - a > 1 {
        let m = (a + b) / 2;
        let y = f(m);
        if (y < want) == increasing {
            a = m;
        } else {
            b = m;
        }
    }
    if (f(a) - want).abs() <= (f(b) - want).abs() {
        a
    } else {
        b
    }
}

impl Bme280 {
    pub fn new(addr: u8, pos: Pos) -> Bme280 {
        let mut regs = [0u8; 256];
        regs[0xD0] = 0x60;
        fn put16(regs: &mut [u8; 256], a: usize, v: f64) {
            let x = v as i32 as u16;
            regs[a] = x as u8;
            regs[a + 1] = (x >> 8) as u8;
        }
        put16(&mut regs, 0x88, T1);
        put16(&mut regs, 0x8A, T2);
        put16(&mut regs, 0x8C, T3);
        for (i, v) in P.iter().enumerate() {
            put16(&mut regs, 0x8E + 2 * i, *v);
        }
        regs[0xA1] = H1 as u8;
        put16(&mut regs, 0xE1, H2);
        regs[0xE3] = H3 as u8;
        let h4 = H4 as i32;
        let h5 = H5 as i32;
        regs[0xE4] = (h4 >> 4) as u8;
        regs[0xE5] = ((h4 & 0x0F) | ((h5 & 0x0F) << 4)) as u8;
        regs[0xE6] = (h5 >> 4) as u8;
        regs[0xE7] = H6 as i8 as u8;
        Bme280 {
            addr,
            regs,
            ptr: 0,
            temp: 21.0,
            rh: 45.0,
            pressure: 101325.0,
            pos,
        }
    }

    /// Fills the data registers for temperature (°C), humidity (%RH), pressure (Pa).
    pub fn measure(&mut self, t: f64, rh: f64, p: f64) {
        self.temp = t;
        self.rh = rh;
        self.pressure = p;
        let adc_t = search(0, 1 << 20, true, t * 100.0, |a| comp_t(a).0 as f64);
        let t_fine = comp_t(adc_t).1;
        let adc_p = search(0, 1 << 20, false, p * 256.0, |a| comp_p(a, t_fine) as f64);
        let adc_h = search(0, 1 << 16, true, rh * 1024.0, |a| comp_h(a, t_fine) as f64);
        let r = &mut self.regs;
        r[0xF7] = (adc_p >> 12) as u8;
        r[0xF8] = (adc_p >> 4) as u8;
        r[0xF9] = ((adc_p & 0xF) << 4) as u8;
        r[0xFA] = (adc_t >> 12) as u8;
        r[0xFB] = (adc_t >> 4) as u8;
        r[0xFC] = ((adc_t & 0xF) << 4) as u8;
        r[0xFD] = (adc_h >> 8) as u8;
        r[0xFE] = adc_h as u8;
    }

    fn sample(&mut self, ctx: &mut dyn DeviceCtx) {
        let w = ctx.world();
        let (mut t, mut rh, p) = (w.temperature_at(self.pos), w.humidity(), w.pressure());
        if ctx.world().noise_enabled() {
            t += ctx.rng().gauss() * 0.01;
            rh += ctx.rng().gauss() * 0.05;
        }
        self.measure(t, rh, p);
    }
}

impl I2cRegs for Bme280 {
    fn address(&self) -> u8 {
        self.addr
    }
    fn write(&mut self, byte: u8, index: usize, ctx: &mut dyn DeviceCtx) {
        if index.is_multiple_of(2) {
            self.ptr = byte;
        } else {
            match self.ptr {
                0xE0 if byte == 0xB6 => {
                    for a in [0xF2, 0xF4, 0xF5] {
                        self.regs[a] = 0;
                    }
                }
                0xF4 => {
                    self.regs[0xF4] = byte;
                    if byte & 3 != 0 {
                        self.sample(ctx);
                    }
                }
                a @ (0xF2 | 0xF5) => self.regs[a as usize] = byte,
                _ => {}
            }
            self.ptr = self.ptr.wrapping_add(1);
        }
    }
    fn read(&mut self, ctx: &mut dyn DeviceCtx) -> u8 {
        if self.ptr == 0xF7 && self.regs[0xF4] & 3 == 3 {
            // normal mode: fresh values for every burst read
            self.sample(ctx);
        }
        let v = if self.ptr == 0xF3 {
            0
        } else {
            self.regs[self.ptr as usize]
        };
        self.ptr = self.ptr.wrapping_add(1);
        v
    }
}

struct Bme280Dev {
    i2c: I2cSlave<Bme280>,
}

impl Bme280Dev {
    fn new(s: &DeviceSpec) -> Bme280Dev {
        let addr = parse_addr(&prop_text(s, "address", "0x76"), 0x76);
        Bme280Dev {
            i2c: I2cSlave::new(
                Bme280::new(addr, s.pos.unwrap_or_default()),
                pin_of(s, "SDA"),
                pin_of(s, "SCL"),
            ),
        }
    }
}

impl Device for Bme280Dev {
    fn kind(&self) -> &str {
        "bme280"
    }
    fn i2c_transfer(
        &mut self,
        ctx: &mut dyn DeviceCtx,
        addr: u8,
        write: &[u8],
        read: usize,
    ) -> Option<Vec<u8>> {
        self.i2c.transfer(ctx, addr, write, read)
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.i2c.start(ctx, 3.3);
        self.i2c.regs.sample(ctx);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        self.i2c.on_edge(ctx, pin, high);
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: format!(
                "{:.1} °C · {:.0} %RH · {:.1} hPa",
                self.i2c.regs.temp,
                self.i2c.regs.rh,
                self.i2c.regs.pressure / 100.0
            ),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("temp_c".into(), self.i2c.regs.temp),
            ("transactions".into(), self.i2c.transactions as f64),
        ]
    }
}

// ------------------------------------------------------------------------------------ SSD1306

/// SSD1306 128×64 OLED controller: command parser + GDDRAM with horizontal / vertical / page
/// addressing.
pub struct Ssd1306 {
    addr: u8,
    pub ram: [[u8; 128]; 8],
    pub on: bool,
    pub inverted: bool,
    pub contrast: u8,
    pub seg_remap: bool,
    pub com_flip: bool,
    mode: u8,
    col: (u8, u8),
    page: (u8, u8),
    c: u8,
    p: u8,
    data_mode: bool,
    continuation: bool,
    pending: Vec<u8>,
    need: usize,
}

impl Ssd1306 {
    pub fn new(addr: u8) -> Ssd1306 {
        Ssd1306 {
            addr,
            ram: [[0; 128]; 8],
            on: false,
            inverted: false,
            contrast: 0x7F,
            seg_remap: false,
            com_flip: false,
            mode: 2,
            col: (0, 127),
            page: (0, 7),
            c: 0,
            p: 0,
            data_mode: false,
            continuation: false,
            pending: Vec::new(),
            need: 0,
        }
    }

    fn command(&mut self, b: u8) {
        if self.need > 0 {
            self.pending.push(b);
            self.need -= 1;
            if self.need == 0 {
                let a = std::mem::take(&mut self.pending);
                match a[0] {
                    0x81 => self.contrast = a[1],
                    0x20 => self.mode = a[1] & 3,
                    0x21 => {
                        self.col = (a[1] & 127, a[2] & 127);
                        self.c = self.col.0;
                    }
                    0x22 => {
                        self.page = (a[1] & 7, a[2] & 7);
                        self.p = self.page.0;
                    }
                    _ => {}
                }
            }
            return;
        }
        let args = match b {
            0x81 | 0x20 | 0xD5 | 0xA8 | 0xD3 | 0x8D | 0xDA | 0xD9 | 0xDB => 1,
            0x21 | 0x22 => 2,
            0x26 | 0x27 => 6,
            0x29 | 0x2A => 5,
            0xA3 => 2,
            _ => 0,
        };
        if args > 0 {
            self.pending = vec![b];
            self.need = args;
            return;
        }
        match b {
            0xAE => self.on = false,
            0xAF => self.on = true,
            0xA6 => self.inverted = false,
            0xA7 => self.inverted = true,
            0xA0 => self.seg_remap = false,
            0xA1 => self.seg_remap = true,
            0xC0 => self.com_flip = false,
            0xC8 => self.com_flip = true,
            0xB0..=0xB7 => self.p = b & 7,
            0x00..=0x0F => self.c = (self.c & 0xF0) | (b & 0x0F),
            0x10..=0x1F => self.c = (self.c & 0x0F) | ((b & 0x0F) << 4),
            _ => {}
        }
    }

    fn data(&mut self, b: u8) {
        self.ram[(self.p & 7) as usize][(self.c & 127) as usize] = b;
        match self.mode {
            0 => {
                if self.c >= self.col.1 {
                    self.c = self.col.0;
                    self.p = if self.p >= self.page.1 {
                        self.page.0
                    } else {
                        self.p + 1
                    };
                } else {
                    self.c += 1;
                }
            }
            1 => {
                if self.p >= self.page.1 {
                    self.p = self.page.0;
                    self.c = if self.c >= self.col.1 {
                        self.col.0
                    } else {
                        self.c + 1
                    };
                } else {
                    self.p += 1;
                }
            }
            _ => self.c = (self.c + 1) & 127,
        }
    }

    /// Pixels as drawn on the glass (row-major, 0/1).
    pub fn pixels(&self) -> Vec<u8> {
        let mut out = vec![0u8; 128 * 64];
        // Adafruit's init (A1 + C8) shows RAM upright; the reset defaults rotate it 180°
        for row in 0..64 {
            let r = if self.com_flip { row } else { 63 - row };
            for x in 0..128 {
                let col = if self.seg_remap { x } else { 127 - x };
                let bit = self.ram[r / 8][col] >> (r % 8) & 1;
                out[row * 128 + x] = bit ^ self.inverted as u8;
            }
        }
        out
    }

    /// Number of lit pixels (tests).
    pub fn lit(&self) -> usize {
        self.ram
            .iter()
            .flatten()
            .map(|b| b.count_ones() as usize)
            .sum()
    }
}

impl I2cRegs for Ssd1306 {
    fn address(&self) -> u8 {
        self.addr
    }
    fn start(&mut self, _read: bool, _ctx: &mut dyn DeviceCtx) {
        self.continuation = true;
        self.data_mode = false;
    }
    fn write(&mut self, byte: u8, _index: usize, _ctx: &mut dyn DeviceCtx) {
        if self.continuation {
            // control byte: Co (bit 7), D/C# (bit 6)
            self.data_mode = byte & 0x40 != 0;
            self.continuation = byte & 0x80 != 0;
            return;
        }
        if self.data_mode {
            self.data(byte);
        } else {
            self.command(byte);
        }
    }
    fn read(&mut self, _ctx: &mut dyn DeviceCtx) -> u8 {
        if self.on { 0x03 } else { 0x43 }
    }
}

struct Ssd1306Dev {
    i2c: I2cSlave<Ssd1306>,
}

impl Ssd1306Dev {
    fn new(s: &DeviceSpec) -> Ssd1306Dev {
        let addr = parse_addr(&prop_text(s, "address", "0x3C"), 0x3C);
        Ssd1306Dev {
            i2c: I2cSlave::new(Ssd1306::new(addr), pin_of(s, "SDA"), pin_of(s, "SCL")),
        }
    }
}

impl Device for Ssd1306Dev {
    fn kind(&self) -> &str {
        "ssd1306"
    }
    fn i2c_transfer(
        &mut self,
        ctx: &mut dyn DeviceCtx,
        addr: u8,
        write: &[u8],
        read: usize,
    ) -> Option<Vec<u8>> {
        self.i2c.transfer(ctx, addr, write, read)
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.i2c.start(ctx, 3.3);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        self.i2c.on_edge(ctx, pin, high);
    }
    fn visual(&self) -> VisualState {
        let r = &self.i2c.regs;
        Look::Pixels {
            width: 128,
            height: 64,
            data: r.pixels(),
            on: r.on,
            inverted: r.inverted,
            contrast: r.contrast,
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("lit_pixels".into(), self.i2c.regs.lit() as f64),
            ("transactions".into(), self.i2c.transactions as f64),
        ]
    }
}
