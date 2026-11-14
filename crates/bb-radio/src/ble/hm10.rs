//! HM-10 (CC2541, HMSoft firmware) BLE module: AT commands while disconnected (no line ending
//! needed — a command ends after a 20 ms pause, a trailing CR/LF is accepted), transparent
//! UART ↔ characteristic FFE1 (service FFE0) while a central is connected. UART bytes are
//! packed into notifications of ≤ 20 bytes; phone writes come out of the UART.

use super::{BleSpace, HM10_CHAR, HM10_SERVICE, PeripheralEvent, PeripheralId, printable, props};
use crate::net::AnnotationKind;
use crate::space::{DeviceId, RadioCard};
use crate::uart::{UartConfig, UartEncoder, UartFrame};
use bb_core::SimTime;

const BAUDS: [u32; 9] = [9600, 19_200, 38_400, 57_600, 115_200, 4800, 2400, 1200, 230_400];

#[derive(Debug, Clone)]
pub struct Hm10 {
    pub label: String,
    pub peripheral: PeripheralId,
    powered: bool,
    ready_at: Option<SimTime>,
    baud_code: usize,
    uart: UartConfig,
    enc: UartEncoder,
    out: Vec<UartFrame>,
    cmd: Vec<u8>,
    cmd_deadline: Option<SimTime>,
    data: Vec<u8>,
    data_deadline: Option<SimTime>,
    name: String,
    noti: bool,
    power_level: u8,
    handle: u16,
    connected: bool,
    notes: Vec<(SimTime, AnnotationKind, String)>,
}

impl Hm10 {
    pub fn new(label: &str, peripheral: PeripheralId) -> Self {
        let uart = UartConfig::new(9600);
        Hm10 {
            label: label.into(),
            peripheral,
            powered: false,
            ready_at: None,
            baud_code: 0,
            uart,
            enc: UartEncoder::new(uart),
            out: vec![],
            cmd: vec![],
            cmd_deadline: None,
            data: vec![],
            data_deadline: None,
            name: "HMSoft".into(),
            noti: false,
            power_level: 2,
            handle: 0,
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

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn next_wake(&self) -> Option<SimTime> {
        [self.ready_at, self.cmd_deadline, self.data_deadline].into_iter().flatten().min()
    }

    pub fn power(&mut self, ble: &mut BleSpace, on: bool, now: SimTime) {
        self.powered = on;
        self.connected = false;
        self.cmd.clear();
        self.data.clear();
        self.cmd_deadline = None;
        self.data_deadline = None;
        ble.set_powered(self.peripheral, false, now);
        ble.take_peripheral_events(self.peripheral);
        if on {
            // Settings persist across power cycles (HM-10 stores them in flash).
            self.uart = UartConfig::new(BAUDS[self.baud_code]);
            self.enc.set_config(self.uart);
            self.ready_at = Some(now + SimTime::from_ms(300));
        } else {
            self.ready_at = None;
        }
    }

    fn start_radio(&mut self, ble: &mut BleSpace, now: SimTime) {
        if let Some(g) = ble.gatt_mut(self.peripheral)
            && g.find(HM10_CHAR).is_none()
        {
            let s = g.add_service(HM10_SERVICE);
            self.handle = g.add_characteristic(s, HM10_CHAR, props::READ | props::WRITE_NO_RESPONSE | props::NOTIFY, b"");
            if let Some(c) = g.char_mut(self.handle) {
                c.description = Some("HMSoft".into());
            }
        }
        ble.set_name(self.peripheral, &self.name);
        ble.set_tx_power(self.peripheral, [-23.0, -6.0, 0.0, 6.0][self.power_level as usize % 4]);
        ble.set_powered(self.peripheral, true, now);
        ble.set_advertising(self.peripheral, true);
    }

    fn send(&mut self, now: SimTime, text: &[u8]) {
        if self.powered {
            self.out.extend(self.enc.push_bytes(text, now + SimTime::from_us(800)));
        }
    }

    /// A byte arrived on the module's RXD.
    pub fn rx_byte(&mut self, ble: &mut BleSpace, byte: u8, now: SimTime) {
        self.poll(ble, now);
        if !self.powered || self.ready_at.is_some() {
            return;
        }
        if self.connected {
            self.data.push(byte);
            if self.data.len() >= 20 {
                self.flush(ble, now);
            } else {
                self.data_deadline = Some(now + SimTime::from_ms(10));
            }
            return;
        }
        self.cmd.push(byte);
        if self.cmd.len() > 64 {
            self.cmd.clear();
        }
        self.cmd_deadline = Some(now + SimTime::from_ms(20));
        if byte == b'\n' {
            self.cmd_deadline = Some(now);
            self.poll(ble, now);
        }
    }

    fn flush(&mut self, ble: &mut BleSpace, now: SimTime) {
        self.data_deadline = None;
        if self.data.is_empty() {
            return;
        }
        let d = std::mem::take(&mut self.data);
        if ble.notify(self.peripheral, self.handle, &d, now) {
            self.notes.push((now, AnnotationKind::Ble, format!("{} → phone notify FFE1 \"{}\" ({} B)", self.label, printable(&d).trim_end_matches('·'), d.len())));
        }
    }

    pub fn poll(&mut self, ble: &mut BleSpace, now: SimTime) {
        if let Some(t) = self.ready_at
            && t <= now
        {
            self.ready_at = None;
            self.start_radio(ble, now);
        }
        if self.cmd_deadline.is_some_and(|t| t <= now) {
            self.cmd_deadline = None;
            let raw = std::mem::take(&mut self.cmd);
            let text = String::from_utf8_lossy(&raw).trim_matches(|c: char| c == '\r' || c == '\n' || c == '\0').to_string();
            if !text.is_empty() {
                self.command(ble, &text, now);
            }
        }
        if self.data_deadline.is_some_and(|t| t <= now) {
            self.flush(ble, now);
        }
        for e in ble.take_peripheral_events(self.peripheral) {
            match e {
                PeripheralEvent::Connected => {
                    self.connected = true;
                    self.cmd.clear();
                    self.cmd_deadline = None;
                    if self.noti {
                        self.send(now, b"OK+CONN");
                    }
                    self.notes.push((now, AnnotationKind::Ble, format!("phone connected to {} (HM-10 \"{}\")", self.label, self.name)));
                }
                PeripheralEvent::Disconnected => {
                    self.connected = false;
                    self.data.clear();
                    if self.noti {
                        self.send(now, b"OK+LOST");
                    }
                    self.notes.push((now, AnnotationKind::Ble, format!("phone disconnected from {}", self.label)));
                    if self.powered {
                        ble.set_advertising(self.peripheral, true);
                    }
                }
                PeripheralEvent::Write { handle, data } if handle == self.handle => {
                    self.notes.push((now, AnnotationKind::Ble, format!("phone → {} write FFE1 \"{}\" ({} B)", self.label, printable(&data), data.len())));
                    self.send(now, &data);
                }
                _ => {}
            }
        }
    }

    fn command(&mut self, ble: &mut BleSpace, text: &str, now: SimTime) {
        let up = text.to_ascii_uppercase();
        if !up.starts_with("AT") {
            return;
        }
        let rest = &text[2..];
        let rup = &up[2..];
        let reply: String = if rup.is_empty() {
            "OK".into()
        } else if rup == "+NAME?" {
            format!("OK+NAME:{}", self.name)
        } else if let Some(n) = rest.strip_prefix("+NAME").or_else(|| rest.strip_prefix("+name")) {
            let n: String = n.chars().take(12).collect();
            self.name = n.clone();
            ble.set_name(self.peripheral, &n);
            format!("OK+Set:{n}")
        } else if rup == "+ADDR?" {
            let a = ble.address(self.peripheral).unwrap_or_default();
            format!("OK+ADDR:{}", a.iter().map(|b| format!("{b:02X}")).collect::<String>())
        } else if rup == "+VERR?" || rup == "+VERS?" {
            "HMSoft V609".into()
        } else if rup == "+BAUD?" {
            format!("OK+Get:{}", self.baud_code)
        } else if let Some(c) = rup.strip_prefix("+BAUD").and_then(|x| x.parse::<usize>().ok()).filter(|c| *c < BAUDS.len()) {
            self.baud_code = c; // takes effect after AT+RESET / power cycle
            format!("OK+Set:{c}")
        } else if rup == "+ROLE?" {
            "OK+Get:0".into()
        } else if let Some(r) = rup.strip_prefix("+ROLE") {
            format!("OK+Set:{r}")
        } else if rup == "+NOTI?" {
            format!("OK+Get:{}", self.noti as u8)
        } else if let Some(n) = rup.strip_prefix("+NOTI") {
            self.noti = n == "1";
            format!("OK+Set:{}", self.noti as u8)
        } else if rup == "+UUID?" {
            "OK+Get:0xFFE0".into()
        } else if rup == "+CHAR?" {
            "OK+Get:0xFFE1".into()
        } else if rup == "+PIN?" {
            "OK+Get:000000".into()
        } else if rup == "+TYPE?" {
            "OK+Get:0".into()
        } else if rup == "+ADVI?" {
            "OK+Get:0".into()
        } else if rup == "+POWE?" {
            format!("OK+Get:{}", self.power_level)
        } else if let Some(p) = rup.strip_prefix("+POWE").and_then(|x| x.parse::<u8>().ok()).filter(|p| *p < 4) {
            self.power_level = p;
            ble.set_tx_power(self.peripheral, [-23.0, -6.0, 0.0, 6.0][p as usize]);
            format!("OK+Set:{p}")
        } else if rup == "+RESET" {
            self.send(now, b"OK+RESET");
            self.reboot(ble, now);
            return;
        } else if rup == "+RENEW" {
            self.send(now, b"OK+RENEW");
            self.name = "HMSoft".into();
            self.baud_code = 0;
            self.noti = false;
            self.power_level = 2;
            self.reboot(ble, now);
            return;
        } else {
            return; // HMSoft ignores unknown commands
        };
        self.send(now, reply.as_bytes());
    }

    fn reboot(&mut self, ble: &mut BleSpace, now: SimTime) {
        let at = self.enc.idle_at().max(now);
        ble.set_powered(self.peripheral, false, now);
        self.connected = false;
        self.uart = UartConfig::new(BAUDS[self.baud_code]);
        self.enc.set_config(self.uart);
        self.ready_at = Some(at + SimTime::from_ms(300));
    }

    pub fn card(&self, device: DeviceId, ble: &BleSpace) -> RadioCard {
        let status = if !self.powered {
            "off"
        } else if self.connected {
            "connected"
        } else {
            "advertising"
        };
        let mut rows = vec![("Name".into(), self.name.clone()), ("Service".into(), "FFE0 · FFE1 (transparent UART)".into())];
        if let Some(r) = ble.rssi(self.peripheral) {
            rows.push(("RSSI at phone".into(), format!("{} dBm", r.round() as i32)));
        }
        rows.push(("UART".into(), format!("{} baud", self.uart.baud)));
        RadioCard { device, label: self.label.clone(), title: "HM-10 BLE".into(), status: status.into(), rows }
    }
}
