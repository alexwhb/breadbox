//! HD44780 character LCD controller ([`Hd44780`]) with the parallel 4/8-bit bus device and the
//! PCF8574 I²C backpack.

use super::i2c::{I2cRegs, I2cSlave};
use super::{DeviceRegistry, ok, pin_of, prop_text};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, PinDrive, SimTime, VisualState};

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("hd44780", |s| Box::new(ParallelLcd::new(s)));
    r.register_fn("hd44780_i2c", |s| Box::new(I2cLcd::new(s)));
}

/// The controller: DDRAM, CGRAM, address counter, entry mode, display control, shifting,
/// 4-bit nibble assembly (power-on state is 8-bit, as on the real chip).
#[derive(Debug, Clone)]
pub struct Hd44780 {
    pub ddram: [u8; 128],
    pub cgram: [u8; 64],
    pub ac: u8,
    cg: bool,
    pub eight_bit: bool,
    pub two_lines: bool,
    pub display_on: bool,
    pub cursor_on: bool,
    pub blink_on: bool,
    increment: bool,
    shift_display: bool,
    pub shift: i32,
    nibble: Option<u8>,
    read_nibble: Option<u8>,
    pub busy_until: SimTime,
    pub commands: u64,
    pub cols: u8,
}

impl Default for Hd44780 {
    fn default() -> Self {
        Hd44780 {
            ddram: [0x20; 128],
            cgram: [0; 64],
            ac: 0,
            cg: false,
            eight_bit: true,
            two_lines: false,
            display_on: false,
            cursor_on: false,
            blink_on: false,
            increment: true,
            shift_display: false,
            shift: 0,
            nibble: None,
            read_nibble: None,
            busy_until: SimTime::ZERO,
            commands: 0,
            cols: 16,
        }
    }
}

impl Hd44780 {
    /// A write cycle (falling edge of E): `bus` is D7…D0 as seen on the pins.
    pub fn write(&mut self, now: SimTime, rs: bool, bus: u8) {
        if self.eight_bit {
            self.execute(now, rs, bus);
        } else {
            match self.nibble.take() {
                None => self.nibble = Some(bus & 0xF0),
                Some(hi) => self.execute(now, rs, hi | (bus >> 4)),
            }
        }
    }

    /// A read cycle: the byte the controller puts on the bus (busy flag + AC, or data).
    pub fn read(&mut self, now: SimTime, rs: bool) -> u8 {
        let full = if rs {
            if self.cg {
                self.cgram[(self.ac & 63) as usize]
            } else {
                self.ddram[(self.ac & 127) as usize]
            }
        } else {
            ((now < self.busy_until) as u8) << 7 | (self.ac & 0x7F)
        };
        if self.eight_bit {
            if rs {
                self.advance();
            }
            full
        } else {
            match self.read_nibble.take() {
                None => {
                    self.read_nibble = Some(full);
                    full & 0xF0
                }
                Some(b) => {
                    if rs {
                        self.advance();
                    }
                    (b & 0x0F) << 4
                }
            }
        }
    }

    fn advance(&mut self) {
        if self.increment {
            self.ac = self.ac.wrapping_add(1);
        } else {
            self.ac = self.ac.wrapping_sub(1);
        }
        self.ac &= if self.cg { 63 } else { 127 };
    }

    fn execute(&mut self, now: SimTime, rs: bool, b: u8) {
        self.commands += 1;
        let mut busy = SimTime::from_us(37);
        if rs {
            if self.cg {
                self.cgram[(self.ac & 63) as usize] = b & 0x1F;
            } else {
                self.ddram[(self.ac & 127) as usize] = b;
            }
            self.advance();
            if self.shift_display && !self.cg {
                self.shift += if self.increment { 1 } else { -1 };
            }
        } else if b & 0x80 != 0 {
            self.cg = false;
            self.ac = b & 0x7F;
        } else if b & 0x40 != 0 {
            self.cg = true;
            self.ac = b & 0x3F;
        } else if b & 0x20 != 0 {
            self.eight_bit = b & 0x10 != 0;
            self.two_lines = b & 0x08 != 0;
            self.nibble = None;
        } else if b & 0x10 != 0 {
            let right = b & 0x04 != 0;
            if b & 0x08 != 0 {
                self.shift += if right { 1 } else { -1 };
            } else if right {
                self.ac = self.ac.wrapping_add(1) & 127;
            } else {
                self.ac = self.ac.wrapping_sub(1) & 127;
            }
        } else if b & 0x08 != 0 {
            self.display_on = b & 0x04 != 0;
            self.cursor_on = b & 0x02 != 0;
            self.blink_on = b & 0x01 != 0;
        } else if b & 0x04 != 0 {
            self.increment = b & 0x02 != 0;
            self.shift_display = b & 0x01 != 0;
        } else if b & 0x02 != 0 {
            self.ac = 0;
            self.cg = false;
            self.shift = 0;
            busy = SimTime::from_us(1520);
        } else if b & 0x01 != 0 {
            self.ddram = [0x20; 128];
            self.ac = 0;
            self.cg = false;
            self.shift = 0;
            self.increment = true;
            busy = SimTime::from_us(1520);
        }
        self.busy_until = now + busy;
    }

    fn line_base(line: usize) -> usize {
        if line == 0 { 0x00 } else { 0x40 }
    }

    /// DDRAM codes of the visible window, per row.
    pub fn visible_codes(&self, rows: usize) -> Vec<Vec<u8>> {
        (0..rows)
            .map(|line| {
                let base = Self::line_base(line);
                (0..self.cols as i32)
                    .map(|i| {
                        let pos = (i + self.shift).rem_euclid(40) as usize;
                        self.ddram[base + pos]
                    })
                    .collect()
            })
            .collect()
    }

    /// Text as shown (A00 character ROM: `°` at 0xDF, `¥` at 0x5C, arrows at 0x7E/0x7F;
    /// custom characters 0–7 render as `█` in the text, exact codes are in `visible_codes`).
    pub fn text(&self, rows: usize) -> Vec<String> {
        self.visible_codes(rows)
            .into_iter()
            .map(|row| row.into_iter().map(rom_char).collect())
            .collect()
    }

    /// Cursor position in the visible window.
    pub fn cursor(&self) -> Option<(u8, u8)> {
        if !self.cursor_on && !self.blink_on {
            return None;
        }
        let line = if self.ac >= 0x40 { 1 } else { 0 };
        let pos = (self.ac & 0x3F) as i32 - self.shift;
        (0..self.cols as i32)
            .contains(&pos)
            .then_some((pos as u8, line))
    }
}

/// HD44780U A00 ROM code → char.
pub fn rom_char(c: u8) -> char {
    match c {
        0..=7 => '█',
        0x5C => '¥',
        0x7E => '→',
        0x7F => '←',
        0xDF => '°',
        0xA5 => '·',
        0xE4 => 'µ',
        0xF4 => 'Ω',
        0x20..=0x7D => c as char,
        _ => '□',
    }
}

fn lcd_look(hd: &Hd44780, backlight: bool, powered: bool) -> VisualState {
    let rows = 2;
    let codes = if powered {
        hd.visible_codes(rows)
    } else {
        vec![vec![0x20; 16]; rows]
    };
    let text = if powered {
        hd.text(rows)
    } else {
        vec![" ".repeat(16); rows]
    };
    Look::TextLcd {
        cols: hd.cols,
        rows: rows as u8,
        text,
        codes,
        cgram: (0..8)
            .map(|i| {
                let mut g = [0u8; 8];
                g.copy_from_slice(&hd.cgram[i * 8..i * 8 + 8]);
                g
            })
            .collect(),
        cursor: hd.cursor(),
        blink: hd.blink_on,
        display_on: hd.display_on && powered,
        backlight,
    }
    .into()
}

// ------------------------------------------------------------------------------------ parallel bus

struct ParallelLcd {
    hd: Hd44780,
    rs: usize,
    rw: usize,
    e: usize,
    d: [usize; 8],
    vdd: usize,
    reading: bool,
    backlight: bool,
    powered: bool,
}

impl ParallelLcd {
    fn new(s: &DeviceSpec) -> ParallelLcd {
        ParallelLcd {
            hd: Hd44780::default(),
            rs: pin_of(s, "RS"),
            rw: pin_of(s, "RW"),
            e: pin_of(s, "E"),
            d: std::array::from_fn(|i| pin_of(s, &format!("D{i}"))),
            vdd: pin_of(s, "VDD"),
            reading: false,
            backlight: false,
            powered: true,
        }
    }
    fn level(&self, ctx: &dyn DeviceCtx, pin: usize) -> bool {
        let vdd = if ok(self.vdd) {
            ctx.pin_voltage(self.vdd)
        } else {
            5.0
        };
        ok(pin) && ctx.pin_voltage(pin) > 0.5 * vdd.max(2.0)
    }
}

impl Device for ParallelLcd {
    fn kind(&self) -> &str {
        "hd44780"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.e) {
            ctx.watch(self.e, 1.0, 2.2);
        }
        for &p in self.d.iter().chain([self.rs, self.rw].iter()) {
            if ok(p) {
                ctx.set_drive(p, PinDrive::Pull { v: 0.0, r: 1e9 });
            }
        }
        ctx.track_element("D:bl");
        ctx.tick_every(SimTime::from_ms(20));
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        let now = ctx.now();
        let rw = self.level(ctx, self.rw);
        if high {
            if rw {
                let rs = self.level(ctx, self.rs);
                let b = self.hd.read(now, rs);
                let vdd = if ok(self.vdd) {
                    ctx.pin_voltage(self.vdd)
                } else {
                    5.0
                };
                for (i, &p) in self.d.iter().enumerate() {
                    if ok(p) {
                        let bit = b >> i & 1 == 1;
                        ctx.set_drive(
                            p,
                            PinDrive::Thevenin {
                                v: if bit { vdd } else { 0.0 },
                                r: 200.0,
                            },
                        );
                    }
                }
                self.reading = true;
            }
            return;
        }
        if self.reading {
            for &p in &self.d {
                if ok(p) {
                    ctx.set_drive(p, PinDrive::Pull { v: 0.0, r: 1e9 });
                }
            }
            self.reading = false;
            return;
        }
        if rw {
            return;
        }
        let rs = self.level(ctx, self.rs);
        let mut bus = 0u8;
        for (i, &p) in self.d.iter().enumerate() {
            if self.level(ctx, p) {
                bus |= 1 << i;
            }
        }
        self.hd.write(now, rs, bus);
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        self.backlight = ctx.element_current("D:bl").unwrap_or(0.0) > 1e-3;
        self.powered = !ok(self.vdd) || ctx.pin_voltage(self.vdd) > 2.7;
    }
    fn visual(&self) -> VisualState {
        lcd_look(&self.hd, self.backlight, self.powered)
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("commands".into(), self.hd.commands as f64)]
    }
}

// ------------------------------------------------------------------------------------ PCF8574 backpack

/// PCF8574 port expander wired the LiquidCrystal_I2C way: P0 RS, P1 RW, P2 E, P3 backlight,
/// P4–P7 → D4–D7.
pub struct Pcf8574Lcd {
    addr: u8,
    port: u8,
    pub hd: Hd44780,
    now: SimTime,
}

impl I2cRegs for Pcf8574Lcd {
    fn address(&self) -> u8 {
        self.addr
    }
    fn write(&mut self, byte: u8, _index: usize, ctx: &mut dyn DeviceCtx) {
        self.now = ctx.now();
        let e_was = self.port & 0x04 != 0;
        let e_now = byte & 0x04 != 0;
        if e_was && !e_now && byte & 0x02 == 0 {
            let rs = byte & 0x01 != 0;
            self.hd.write(self.now, rs, byte & 0xF0);
        }
        self.port = byte;
    }
    fn read(&mut self, _ctx: &mut dyn DeviceCtx) -> u8 {
        self.port
    }
}

struct I2cLcd {
    i2c: I2cSlave<Pcf8574Lcd>,
    vcc: usize,
    powered: bool,
}

impl I2cLcd {
    fn new(s: &DeviceSpec) -> I2cLcd {
        let a = prop_text(s, "address", "0x27");
        let addr = u8::from_str_radix(a.trim_start_matches("0x").trim_start_matches("0X"), 16)
            .unwrap_or(0x27);
        I2cLcd {
            i2c: I2cSlave::new(
                Pcf8574Lcd {
                    addr,
                    port: 0xFF,
                    hd: Hd44780::default(),
                    now: SimTime::ZERO,
                },
                pin_of(s, "SDA"),
                pin_of(s, "SCL"),
            ),
            vcc: pin_of(s, "VCC"),
            powered: true,
        }
    }
}

impl Device for I2cLcd {
    fn kind(&self) -> &str {
        "hd44780_i2c"
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
        let vcc = if ok(self.vcc) {
            ctx.pin_voltage(self.vcc)
        } else {
            5.0
        };
        self.i2c.start(ctx, vcc.max(3.0));
        ctx.tick_every(SimTime::from_ms(50));
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        self.i2c.on_edge(ctx, pin, high);
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        self.powered = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 2.7;
    }
    fn visual(&self) -> VisualState {
        let r = &self.i2c.regs;
        lcd_look(&r.hd, r.port & 0x08 != 0, self.powered)
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("commands".into(), self.i2c.regs.hd.commands as f64)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bit_init_and_text() {
        let mut hd = Hd44780::default();
        let t = SimTime::ZERO;
        // LiquidCrystal 4-bit init: 0x3 ×3, 0x2 (as single nibbles while still 8-bit)
        for n in [0x30, 0x30, 0x30, 0x20] {
            hd.write(t, false, n);
        }
        assert!(!hd.eight_bit);
        let cmd = |hd: &mut Hd44780, rs: bool, b: u8| {
            hd.write(t, rs, b & 0xF0);
            hd.write(t, rs, (b & 0x0F) << 4);
        };
        cmd(&mut hd, false, 0x28); // 2 lines
        cmd(&mut hd, false, 0x0C); // display on
        cmd(&mut hd, false, 0x01); // clear
        cmd(&mut hd, false, 0x06); // entry: increment
        for c in b"Temp: 21.5" {
            cmd(&mut hd, true, *c);
        }
        cmd(&mut hd, true, 0xDF);
        cmd(&mut hd, true, b'C');
        cmd(&mut hd, false, 0xC0); // line 2
        for c in b"OK" {
            cmd(&mut hd, true, *c);
        }
        let text = hd.text(2);
        assert_eq!(text[0].trim_end(), "Temp: 21.5°C");
        assert_eq!(text[1].trim_end(), "OK");
        assert!(hd.display_on && hd.two_lines);
        // custom character
        cmd(&mut hd, false, 0x40);
        for row in [0x1Fu8; 8] {
            cmd(&mut hd, true, row);
        }
        assert_eq!(hd.cgram[0], 0x1F);
    }
}
