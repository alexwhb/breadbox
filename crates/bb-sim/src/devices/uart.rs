//! UART modules. [`UartPort`] turns a device's RX/TX pins into bytes using
//! `bb_radio::uart` (exact bit timing both ways) — the integration point for the radio
//! modules (ESP-01 ESP-AT modem, HM-10, HC-05, LoRa). The built-in factories here are small AT
//! responders so benches with a radio run before the full `bb-radio` glue replaces them via
//! [`super::DeviceRegistry::register`].

use super::{DeviceRegistry, ok, pin_of, prop_f64};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, DeviceEvent, Look, PinDrive, SimTime, VisualState};
use bb_radio::uart::{UartConfig, UartDecoder, UartEncoder};
use std::collections::VecDeque;

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("esp01", |s| Box::new(AtStub::new(s, AtFlavor::Esp)));
    r.register_fn("hm10", |s| Box::new(AtStub::new(s, AtFlavor::Hm10)));
    r.register_fn("hc05", |s| Box::new(AtStub::new(s, AtFlavor::Hc05)));
}

/// Bytes in and out of a pair of UART pins with exact bit timing.
pub struct UartPort {
    pub rx: UartDecoder,
    pub tx: UartEncoder,
    pub rx_pin: usize,
    pub tx_pin: usize,
    /// High level of the TX pin (module supply).
    pub v_high: f64,
    pending: VecDeque<(SimTime, bool)>,
    pub token_tx: u64,
    pub token_rx: u64,
    received: Vec<u8>,
}

impl UartPort {
    pub fn new(baud: u32, rx_pin: usize, tx_pin: usize, token_base: u64) -> UartPort {
        let cfg = UartConfig::new(baud);
        UartPort {
            rx: UartDecoder::new(cfg),
            tx: UartEncoder::new(cfg),
            rx_pin,
            tx_pin,
            v_high: 3.3,
            pending: VecDeque::new(),
            token_tx: token_base,
            token_rx: token_base + 1,
            received: Vec::new(),
        }
    }

    pub fn start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.tx_pin) {
            ctx.set_drive(
                self.tx_pin,
                PinDrive::Thevenin {
                    v: self.v_high,
                    r: 50.0,
                },
            );
        }
        if ok(self.rx_pin) {
            ctx.watch(self.rx_pin, 0.8, 2.0);
        }
    }

    /// Feed device edges here; returns true for the RX pin.
    pub fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) -> bool {
        if pin != self.rx_pin {
            return false;
        }
        self.rx.edge(ctx.now(), high);
        if let Some(t) = self.rx.next_decision() {
            ctx.wake_at(t, self.token_rx);
        }
        true
    }

    /// Queues bytes for transmission now (after anything already queued).
    pub fn send(&mut self, ctx: &mut dyn DeviceCtx, bytes: &[u8]) {
        let was_empty = self.pending.is_empty();
        for tr in self.tx.encode(bytes, ctx.now()) {
            self.pending.push_back(tr);
        }
        if was_empty && let Some(&(t, _)) = self.pending.front() {
            ctx.wake_at(t, self.token_tx);
        }
    }

    /// Handles this port's wake tokens; returns received bytes (if any) for RX wakes.
    pub fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) -> Option<Vec<u8>> {
        if token == self.token_tx {
            let now = ctx.now();
            while let Some(&(t, level)) = self.pending.front() {
                if t > now {
                    break;
                }
                self.pending.pop_front();
                if ok(self.tx_pin) {
                    ctx.set_drive(
                        self.tx_pin,
                        PinDrive::Thevenin {
                            v: if level { self.v_high } else { 0.0 },
                            r: 50.0,
                        },
                    );
                }
            }
            if let Some(&(t, _)) = self.pending.front() {
                ctx.wake_at(t, self.token_tx);
            }
            return None;
        }
        if token == self.token_rx {
            for b in self.rx.poll(ctx.now()) {
                if b.ok() {
                    self.received.push(b.data as u8);
                }
            }
            if let Some(t) = self.rx.next_decision() {
                ctx.wake_at(t, self.token_rx);
            }
            return Some(std::mem::take(&mut self.received));
        }
        None
    }

    /// True while bytes are still being transmitted.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum AtFlavor {
    Esp,
    Hm10,
    Hc05,
}

/// A small AT-command responder (stand-in until the full `bb-radio` modems are wired in):
/// ESP-01 answers `AT`, `ATE0/1`, `AT+CWMODE`, `AT+CWJAP` (joins `SIM-AP`), `AT+CIPSTART`,
/// `AT+CIPSEND` (with the `>` prompt; MQTT CONNECT gets a CONNACK, PUBLISH is annotated),
/// `AT+CIPCLOSE`; HM-10 / HC-05 answer `AT` and pass data through.
pub struct AtStub {
    flavor: AtFlavor,
    port: UartPort,
    vcc: usize,
    en: usize,
    echo: bool,
    line: Vec<u8>,
    send_left: usize,
    payload: Vec<u8>,
    booted: bool,
    joined: bool,
    linked: bool,
    pub commands: Vec<String>,
    queue: VecDeque<(SimTime, Vec<u8>)>,
}

const T_BOOT: u64 = 10;
const T_REPLY: u64 = 11;
/// HM-10 commands have no line ending: a pause after the last byte ends one.
const T_GAP: u64 = 12;

impl AtStub {
    pub fn new(s: &DeviceSpec, flavor: AtFlavor) -> AtStub {
        let (rx, tx) = match flavor {
            AtFlavor::Esp => ("RX", "TX"),
            _ => ("RXD", "TXD"),
        };
        let baud = prop_f64(
            s,
            "baud",
            if flavor == AtFlavor::Esp {
                115200.0
            } else {
                9600.0
            },
        ) as u32;
        AtStub {
            flavor,
            port: UartPort::new(baud, pin_of(s, rx), pin_of(s, tx), 1),
            vcc: pin_of(s, "VCC"),
            en: pin_of(s, "EN"),
            echo: true,
            line: Vec::new(),
            send_left: 0,
            payload: Vec::new(),
            booted: false,
            joined: false,
            linked: false,
            commands: Vec::new(),
            queue: VecDeque::new(),
        }
    }

    fn reply_after(&mut self, ctx: &mut dyn DeviceCtx, delay: SimTime, text: &[u8]) {
        let at = ctx.now() + delay;
        let t = self.queue.back().map(|q| q.0.max(at)).unwrap_or(at);
        self.queue.push_back((t, text.to_vec()));
        ctx.wake_at(self.queue.front().map(|q| q.0).unwrap_or(t), T_REPLY);
    }

    fn command(&mut self, ctx: &mut dyn DeviceCtx, cmd: &str) {
        self.commands.push(cmd.to_string());
        let ms = SimTime::from_ms;
        let up = cmd.to_ascii_uppercase();
        if self.flavor != AtFlavor::Esp {
            let r = match up.as_str() {
                "AT" => "OK".to_string(),
                "AT+NAME?" => "OK+NAME:HMSoft".into(),
                "AT+ADDR?" => "OK+ADDR:A4C138F0B0B5".into(),
                _ if self.flavor == AtFlavor::Hm10 && up.starts_with("AT+") => {
                    // HM-10 set commands echo the value: AT+NAMEfoo → OK+Set:foo
                    let rest = &cmd[3..];
                    let split = rest
                        .find(|c: char| !c.is_ascii_uppercase())
                        .unwrap_or(rest.len());
                    format!("OK+Set:{}", &rest[split..])
                }
                _ => "OK".into(),
            };
            self.reply_after(ctx, ms(5), r.as_bytes());
            return;
        }
        if up == "AT"
            || up.starts_with("AT+CWMODE")
            || up.starts_with("AT+CIPMUX")
            || up.starts_with("AT+UART")
        {
            self.reply_after(ctx, ms(2), b"\r\nOK\r\n");
        } else if up == "ATE0" {
            self.echo = false;
            self.reply_after(ctx, ms(2), b"\r\nOK\r\n");
        } else if up == "ATE1" {
            self.echo = true;
            self.reply_after(ctx, ms(2), b"\r\nOK\r\n");
        } else if up.starts_with("AT+CWJAP=") {
            let ssid = cmd.split('"').nth(1).unwrap_or("").to_string();
            if ssid == "SIM-AP" {
                self.joined = true;
                self.reply_after(ctx, ms(900), b"WIFI CONNECTED\r\n");
                self.reply_after(ctx, ms(600), b"WIFI GOT IP\r\n\r\nOK\r\n");
                ctx.emit(DeviceEvent::Annotation {
                    channel: "wifi".into(),
                    text: format!("AT+CWJAP=\"{ssid}\" → OK  rssi=-54 dBm  ip=10.0.0.42"),
                });
            } else {
                self.reply_after(ctx, ms(3000), b"+CWJAP:3\r\n\r\nFAIL\r\n");
            }
        } else if up.starts_with("AT+CIPSTART") {
            if self.joined {
                self.linked = true;
                self.reply_after(ctx, ms(40), b"CONNECT\r\n\r\nOK\r\n");
            } else {
                self.reply_after(ctx, ms(5), b"\r\nERROR\r\n");
            }
        } else if let Some(n) = up.strip_prefix("AT+CIPSEND=") {
            let n: usize = n.trim().parse().unwrap_or(0);
            if self.linked && n > 0 {
                self.send_left = n;
                self.payload.clear();
                self.reply_after(ctx, ms(2), b"\r\nOK\r\n> ");
            } else {
                self.reply_after(ctx, ms(2), b"\r\nERROR\r\n");
            }
        } else if up.starts_with("AT+CIPCLOSE") {
            self.linked = false;
            self.reply_after(ctx, ms(5), b"CLOSED\r\n\r\nOK\r\n");
        } else if up == "AT+RST" {
            self.joined = false;
            self.linked = false;
            self.reply_after(ctx, ms(2), b"\r\nOK\r\n");
            self.reply_after(ctx, ms(400), b"\r\nready\r\n");
        } else {
            self.reply_after(ctx, ms(2), b"\r\nERROR\r\n");
        }
    }

    fn payload_done(&mut self, ctx: &mut dyn DeviceCtx) {
        let n = self.payload.len();
        let msg = format!("\r\nRecv {n} bytes\r\n\r\nSEND OK\r\n");
        self.reply_after(ctx, SimTime::from_ms(20), msg.as_bytes());
        let p = std::mem::take(&mut self.payload);
        match p.first().map(|b| b >> 4) {
            Some(1) => {
                // MQTT CONNECT → CONNACK
                self.reply_after(ctx, SimTime::from_ms(10), b"\r\n+IPD,4:\x20\x02\x00\x00");
                ctx.emit(DeviceEvent::Annotation {
                    channel: "mqtt".into(),
                    text: "CONNECT broker.sim:1883 → CONNACK accepted".into(),
                });
            }
            Some(3) => {
                // PUBLISH: fixed header, remaining length, topic length, topic, payload
                let mut i = 1;
                let mut _len = 0usize;
                let mut mul = 1;
                while i < p.len() {
                    _len += (p[i] & 0x7F) as usize * mul;
                    mul *= 128;
                    i += 1;
                    if p[i - 1] & 0x80 == 0 {
                        break;
                    }
                }
                if i + 2 <= p.len() {
                    let tl = ((p[i] as usize) << 8) | p[i + 1] as usize;
                    let topic = String::from_utf8_lossy(p.get(i + 2..i + 2 + tl).unwrap_or(&[]))
                        .to_string();
                    let body =
                        String::from_utf8_lossy(p.get(i + 2 + tl..).unwrap_or(&[])).to_string();
                    ctx.emit(DeviceEvent::Annotation {
                        channel: "mqtt".into(),
                        text: format!("PUB {topic} {body}  ({n} B)"),
                    });
                }
            }
            _ => {}
        }
    }
}

impl Device for AtStub {
    fn kind(&self) -> &str {
        "uart_module"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.port.start(ctx);
        if ok(self.vcc) {
            ctx.set_drive(self.vcc, PinDrive::Current { i: -0.07 });
        }
        let boot = if self.flavor == AtFlavor::Esp {
            360
        } else {
            50
        };
        ctx.wake_at(ctx.now() + SimTime::from_ms(boot), T_BOOT);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        self.port.on_edge(ctx, pin, high);
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        match token {
            T_BOOT => {
                let enabled = !ok(self.en) || ctx.pin_voltage(self.en) > 1.5;
                let powered = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 2.5;
                if enabled && powered {
                    self.booted = true;
                    if self.flavor == AtFlavor::Esp {
                        self.port.send(ctx, b"\r\nready\r\n");
                    }
                } else {
                    ctx.wake_at(ctx.now() + SimTime::from_ms(100), T_BOOT);
                }
            }
            T_GAP => {
                let l = String::from_utf8_lossy(&self.line).trim().to_string();
                self.line.clear();
                if !l.is_empty() {
                    self.command(ctx, &l);
                }
            }
            T_REPLY => {
                let now = ctx.now();
                while let Some((t, _)) = self.queue.front() {
                    if *t > now {
                        break;
                    }
                    let (_, bytes) = self.queue.pop_front().expect("front");
                    self.port.send(ctx, &bytes);
                }
                if let Some(&(t, _)) = self.queue.front() {
                    ctx.wake_at(t, T_REPLY);
                }
            }
            _ => {
                if let Some(bytes) = self.port.on_wake(ctx, token) {
                    if !self.booted {
                        return;
                    }
                    for b in bytes {
                        if self.send_left > 0 {
                            self.payload.push(b);
                            self.send_left -= 1;
                            if self.send_left == 0 {
                                self.payload_done(ctx);
                            }
                            continue;
                        }
                        if self.echo && self.flavor == AtFlavor::Esp {
                            self.port.send(ctx, &[b]);
                        }
                        match b {
                            b'\n' => {
                                let l = String::from_utf8_lossy(&self.line).trim().to_string();
                                self.line.clear();
                                if !l.is_empty() {
                                    self.command(ctx, &l);
                                }
                            }
                            b'\r' => {}
                            _ => {
                                if self.line.len() < 256 {
                                    self.line.push(b);
                                }
                                if self.flavor == AtFlavor::Hm10 {
                                    ctx.cancel_wake(T_GAP);
                                    ctx.wake_at(ctx.now() + SimTime::from_ms(10), T_GAP);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: match (self.booted, self.joined, self.linked) {
                (false, _, _) => "booting".into(),
                (true, false, _) => "idle".into(),
                (true, true, false) => "Wi-Fi joined".into(),
                (true, true, true) => "connected".into(),
            },
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("commands".into(), self.commands.len() as f64)]
    }
}
