//! HC-05 classic Bluetooth SPP module. With KEY/EN high at power-up it boots into AT mode at
//! 38 400 baud (`AT+NAME?` → `+NAME:HC-05` / `OK`); otherwise it is a transparent serial link at
//! its data baud (default 9600) to the phone's Bluetooth serial terminal once paired (PIN 1234).

use super::{BleSpace, PeripheralEvent, PeripheralId, printable};
use crate::at::{self, AtKind};
use crate::net::AnnotationKind;
use crate::space::{DeviceId, RadioCard};
use crate::uart::{UartConfig, UartEncoder, UartFrame};
use bb_core::SimTime;

#[derive(Debug, Clone)]
pub struct Hc05 {
    pub label: String,
    pub device: PeripheralId,
    powered: bool,
    key: bool,
    at_mode: bool,
    ready_at: Option<SimTime>,
    data_uart: UartConfig,
    uart: UartConfig,
    enc: UartEncoder,
    out: Vec<UartFrame>,
    line: Vec<u8>,
    data: Vec<u8>,
    data_deadline: Option<SimTime>,
    name: String,
    pin: String,
    role: u8,
    connected: bool,
    notes: Vec<(SimTime, AnnotationKind, String)>,
}

impl Hc05 {
    pub fn new(label: &str, device: PeripheralId) -> Self {
        let uart = UartConfig::new(9600);
        Hc05 {
            label: label.into(),
            device,
            powered: false,
            key: false,
            at_mode: false,
            ready_at: None,
            data_uart: uart,
            uart,
            enc: UartEncoder::new(uart),
            out: vec![],
            line: vec![],
            data: vec![],
            data_deadline: None,
            name: "HC-05".into(),
            pin: "1234".into(),
            role: 0,
            connected: false,
            notes: vec![],
        }
    }

    pub fn uart_config(&self) -> UartConfig {
        self.uart
    }

    pub fn take_tx(&mut self) -> Vec<UartFrame> {
        std::mem::take(&mut self.out)
    }

    pub fn take_notes(&mut self) -> Vec<(SimTime, AnnotationKind, String)> {
        std::mem::take(&mut self.notes)
    }

    /// STATE pin (high while a phone is connected).
    pub fn state_pin(&self) -> bool {
        self.connected
    }

    pub fn in_at_mode(&self) -> bool {
        self.at_mode
    }

    pub fn next_wake(&self) -> Option<SimTime> {
        [self.ready_at, self.data_deadline]
            .into_iter()
            .flatten()
            .min()
    }

    /// KEY/EN pin level (sampled at power-up).
    pub fn set_key(&mut self, high: bool) {
        self.key = high;
    }

    pub fn power(&mut self, ble: &mut BleSpace, on: bool, now: SimTime) {
        self.powered = on;
        self.connected = false;
        self.line.clear();
        self.data.clear();
        ble.set_powered(self.device, false, now);
        ble.take_peripheral_events(self.device);
        if on {
            self.at_mode = self.key;
            self.uart = if self.at_mode {
                UartConfig::new(38_400)
            } else {
                self.data_uart
            };
            self.enc.set_config(self.uart);
            self.ready_at = Some(now + SimTime::from_ms(500));
        } else {
            self.ready_at = None;
        }
    }

    fn send(&mut self, now: SimTime, text: &[u8]) {
        if self.powered {
            self.out
                .extend(self.enc.push_bytes(text, now + SimTime::from_us(600)));
        }
    }

    pub fn rx_byte(&mut self, ble: &mut BleSpace, byte: u8, now: SimTime) {
        self.poll(ble, now);
        if !self.powered || self.ready_at.is_some() {
            return;
        }
        if self.at_mode {
            self.line.push(byte);
            if self.line.len() > 128 {
                self.line.clear();
            }
            if byte == b'\n' {
                let raw = std::mem::take(&mut self.line);
                let text = String::from_utf8_lossy(&raw).trim().to_string();
                self.command(ble, &text, now);
            }
            return;
        }
        if self.connected {
            self.data.push(byte);
            if self.data.len() >= 64 {
                self.flush(ble, now);
            } else {
                self.data_deadline = Some(now + SimTime::from_ms(5));
            }
        }
    }

    fn flush(&mut self, ble: &mut BleSpace, now: SimTime) {
        self.data_deadline = None;
        let d = std::mem::take(&mut self.data);
        if !d.is_empty() {
            ble.spp_send(self.device, &d, now);
            self.notes.push((
                now,
                AnnotationKind::Ble,
                format!(
                    "{} → phone SPP \"{}\" ({} B)",
                    self.label,
                    printable(&d),
                    d.len()
                ),
            ));
        }
    }

    pub fn poll(&mut self, ble: &mut BleSpace, now: SimTime) {
        if let Some(t) = self.ready_at
            && t <= now
        {
            self.ready_at = None;
            ble.set_name(self.device, &self.name);
            ble.set_pin(self.device, &self.pin);
            ble.set_tx_power(self.device, 4.0);
            ble.set_powered(self.device, true, now);
        }
        if self.data_deadline.is_some_and(|t| t <= now) {
            self.flush(ble, now);
        }
        for e in ble.take_peripheral_events(self.device) {
            match e {
                PeripheralEvent::Connected => {
                    self.connected = true;
                    self.notes.push((
                        now,
                        AnnotationKind::Ble,
                        format!("phone paired + connected to {} (HC-05 SPP)", self.label),
                    ));
                }
                PeripheralEvent::Disconnected => {
                    self.connected = false;
                    self.notes.push((
                        now,
                        AnnotationKind::Ble,
                        format!("phone disconnected from {}", self.label),
                    ));
                }
                PeripheralEvent::SppData(d) if !self.at_mode => self.send(now, &d),
                _ => {}
            }
        }
    }

    fn command(&mut self, ble: &mut BleSpace, text: &str, now: SimTime) {
        let Some(cmd) = at::parse(text) else {
            return self.send(now, b"ERROR:(0)\r\n");
        };
        let ok = b"OK\r\n";
        let reply: Vec<u8> = match (cmd.name.as_str(), &cmd.kind) {
            ("", AtKind::Exec) => ok.to_vec(),
            ("+NAME", AtKind::Query) => format!("+NAME:{}\r\nOK\r\n", self.name).into_bytes(),
            ("+NAME", AtKind::Set(_)) => {
                self.name = text
                    .split_once('=')
                    .map(|x| x.1.trim().trim_matches('"').to_string())
                    .unwrap_or_default();
                ble.set_name(self.device, &self.name);
                ok.to_vec()
            }
            ("+UART", AtKind::Query) => {
                format!("+UART:{},0,0\r\nOK\r\n", self.data_uart.baud).into_bytes()
            }
            ("+UART", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(b) if (1200..=1_382_400).contains(&b) => {
                    let stop = if cmd.arg_int(1) == Some(1) { 2 } else { 1 };
                    let parity = match cmd.arg_int(2) {
                        Some(1) => crate::uart::Parity::Odd,
                        Some(2) => crate::uart::Parity::Even,
                        _ => crate::uart::Parity::None,
                    };
                    self.data_uart = UartConfig::new(b as u32).with_format(8, parity, stop);
                    ok.to_vec()
                }
                _ => b"ERROR:(1D)\r\n".to_vec(),
            },
            ("+PSWD" | "+PIN", AtKind::Query) => {
                format!("{}:\"{}\"\r\nOK\r\n", cmd.name, self.pin).into_bytes()
            }
            ("+PSWD" | "+PIN", AtKind::Set(_)) => {
                self.pin = cmd.arg_str(0).unwrap_or("1234").to_string();
                ble.set_pin(self.device, &self.pin);
                ok.to_vec()
            }
            ("+ROLE", AtKind::Query) => format!("+ROLE:{}\r\nOK\r\n", self.role).into_bytes(),
            ("+ROLE", AtKind::Set(_)) => {
                self.role = cmd.arg_int(0).unwrap_or(0).clamp(0, 2) as u8;
                ok.to_vec()
            }
            ("+ADDR", AtKind::Query) => {
                let a = ble.address(self.device).unwrap_or_default();
                format!(
                    "+ADDR:{:02x}{:02x}:{:02x}:{:02x}{:02x}{:02x}\r\nOK\r\n",
                    a[0], a[1], a[2], a[3], a[4], a[5]
                )
                .into_bytes()
            }
            ("+VERSION", AtKind::Query) => b"+VERSION:3.0-20170601\r\nOK\r\n".to_vec(),
            ("+STATE", AtKind::Query) => {
                let s = if self.connected {
                    "CONNECTED"
                } else {
                    "PAIRABLE"
                };
                format!("+STATE:{s}\r\nOK\r\n").into_bytes()
            }
            ("+CMODE", AtKind::Query) => b"+CMODE:0\r\nOK\r\n".to_vec(),
            ("+CMODE" | "+INIT" | "+INQ" | "+CLASS" | "+IAC", _) => ok.to_vec(),
            ("+ORGL", AtKind::Exec) => {
                self.name = "HC-05".into();
                self.pin = "1234".into();
                self.role = 0;
                self.data_uart = UartConfig::new(9600);
                ok.to_vec()
            }
            ("+RESET", AtKind::Exec) => {
                self.send(now, ok);
                let at = self.enc.idle_at().max(now);
                // Reboots; KEY is sampled again (still high → stays in AT mode).
                self.power(ble, true, at);
                return;
            }
            _ => b"ERROR:(0)\r\n".to_vec(),
        };
        self.send(now, &reply);
    }

    pub fn card(&self, device: DeviceId, ble: &BleSpace) -> RadioCard {
        let status = if !self.powered {
            "off"
        } else if self.at_mode {
            "AT mode"
        } else if self.connected {
            "connected"
        } else {
            "pairable"
        };
        let mut rows = vec![
            ("Name".into(), self.name.clone()),
            ("PIN".into(), self.pin.clone()),
        ];
        if let Some(r) = ble.rssi(self.device) {
            rows.push(("RSSI at phone".into(), format!("{} dBm", r.round() as i32)));
        }
        rows.push(("UART".into(), format!("{} baud", self.uart.baud)));
        RadioCard {
            device,
            label: self.label.clone(),
            title: "HC-05 Bluetooth SPP".into(),
            status: status.into(),
            rows,
        }
    }
}
