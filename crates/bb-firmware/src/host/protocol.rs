//! Wire format between [`super::HostBoard`] and the child process built from `hostcore/`
//! (mirror of `hostcore/bb_host.h`).
//!
//! ```text
//! child → parent   [u32 len][u8 type][payload]                     len = 1 + payload
//! parent → child   [u32 len][u64 now_us][u8 n][n × irq pin][payload] (replies only)
//! ```
//! All integers little-endian. One-way messages (pin writes, serial output, PWM, IRQ setup,
//! logs) get no reply; every other message gets exactly one, possibly much later in simulated
//! time (sleeps, pin waits) — that is how the child stays in lock-step.

pub const MSG_HELLO: u8 = 0x01;
pub const MSG_TIME: u8 = 0x02;
pub const MSG_SLEEP: u8 = 0x03;
pub const MSG_PIN_MODE: u8 = 0x04;
pub const MSG_DIGITAL_WRITE: u8 = 0x05;
pub const MSG_DIGITAL_READ: u8 = 0x06;
pub const MSG_ANALOG_READ: u8 = 0x07;
pub const MSG_PWM: u8 = 0x08;
pub const MSG_ATTACH_IRQ: u8 = 0x09;
pub const MSG_DETACH_IRQ: u8 = 0x0a;
pub const MSG_SERIAL_BEGIN: u8 = 0x0b;
pub const MSG_SERIAL_WRITE: u8 = 0x0c;
pub const MSG_SERIAL_READ: u8 = 0x0d;
pub const MSG_SERIAL_AVAILABLE: u8 = 0x0e;
pub const MSG_I2C_WRITE: u8 = 0x0f;
pub const MSG_I2C_READ: u8 = 0x10;
pub const MSG_SPI_TRANSFER: u8 = 0x11;
pub const MSG_SERVICE: u8 = 0x12;
pub const MSG_WAIT_PIN: u8 = 0x13;
pub const MSG_LOG: u8 = 0x14;
pub const MSG_RESTART: u8 = 0x15;
pub const MSG_YIELD: u8 = 0x16;
pub const MSG_ASYNC: u8 = 0x17;
pub const MSG_PANIC: u8 = 0x18;

/// `pinMode` values (ESP32 core numbering, used for both boards).
pub const MODE_INPUT: u8 = 0x01;
pub const MODE_OUTPUT: u8 = 0x03;
pub const MODE_INPUT_PULLUP: u8 = 0x05;
pub const MODE_INPUT_PULLDOWN: u8 = 0x09;
pub const MODE_OUTPUT_OPEN_DRAIN: u8 = 0x13;

/// `attachInterrupt` modes.
pub const IRQ_RISING: u8 = 1;
pub const IRQ_FALLING: u8 = 2;
pub const IRQ_CHANGE: u8 = 3;
pub const IRQ_ONLOW: u8 = 4;
pub const IRQ_ONHIGH: u8 = 5;

/// Little-endian payload reader (tolerant: missing bytes read as zero).
pub struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Rd<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Rd { b, i: 0 }
    }
    pub fn u8(&mut self) -> u8 {
        let v = self.b.get(self.i).copied().unwrap_or(0);
        self.i += 1;
        v
    }
    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }
    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.u8(), self.u8(), self.u8(), self.u8()])
    }
    pub fn u64(&mut self) -> u64 {
        let lo = self.u32() as u64;
        lo | (self.u32() as u64) << 32
    }
    pub fn bytes(&mut self) -> &'a [u8] {
        let n = self.u16() as usize;
        let s = self.i.min(self.b.len());
        let e = (self.i + n).min(self.b.len());
        self.i = e;
        &self.b[s..e]
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = self.i.min(self.b.len());
        self.i = self.b.len();
        &self.b[s..]
    }
    pub fn string(&mut self) -> String {
        String::from_utf8_lossy(self.bytes()).into_owned()
    }
}

/// Reply payload builder.
#[derive(Default)]
pub struct Wr(pub Vec<u8>);

impl Wr {
    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f64(mut self, v: f64) -> Self {
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
        self
    }
    pub fn bytes(mut self, b: &[u8]) -> Self {
        let n = b.len().min(u16::MAX as usize);
        self.0.extend_from_slice(&(n as u16).to_le_bytes());
        self.0.extend_from_slice(&b[..n]);
        self
    }
}

/// Frames a reply.
pub fn reply_frame(now_us: u64, irqs: &[u8], payload: &[u8]) -> Vec<u8> {
    let n = irqs.len().min(255);
    let len = 8 + 1 + n + payload.len();
    let mut out = Vec::with_capacity(4 + len);
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(&now_us.to_le_bytes());
    out.push(n as u8);
    out.extend_from_slice(&irqs[..n]);
    out.extend_from_slice(payload);
    out
}
