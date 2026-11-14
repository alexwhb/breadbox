//! REYAX RYLR896 LoRa module (SX1276 + AT firmware, default 115 200 baud). Commands end with
//! CR LF; replies `+OK`, `+ERR=<n>`, queries `+NAME=value`; received packets arrive as
//! `+RCV=<from>,<len>,<data>,<RSSI>,<SNR>`.

use super::{LoraError, LoraParams, LoraSpace, NodeId};
use crate::at::{self, AtKind};
use crate::net::AnnotationKind;
use crate::space::{DeviceId, RadioCard};
use crate::uart::{UartConfig, UartEncoder, UartFrame};
use bb_core::SimTime;

const BW_CODES: [u32; 10] = [7_800, 10_400, 15_600, 20_800, 31_250, 41_700, 62_500, 125_000, 250_000, 500_000];

#[derive(Debug, Clone)]
pub struct Rylr896 {
    pub label: String,
    pub node: NodeId,
    powered: bool,
    ready_at: Option<SimTime>,
    uart: UartConfig,
    enc: UartEncoder,
    out: Vec<UartFrame>,
    line: Vec<u8>,
    address: u16,
    network_id: u8,
    band: u32,
    params: LoraParams,
    power: u8,
    mode: u8,
    password: Option<String>,
    sending: bool,
    notes: Vec<(SimTime, AnnotationKind, String)>,
}

impl Rylr896 {
    pub fn new(label: &str, node: NodeId) -> Self {
        let uart = UartConfig::new(115_200);
        Rylr896 {
            label: label.into(),
            node,
            powered: false,
            ready_at: None,
            uart,
            enc: UartEncoder::new(uart),
            out: vec![],
            line: vec![],
            address: 0,
            network_id: 0,
            band: 915_000_000,
            params: LoraParams::default(),
            power: 15,
            mode: 0,
            password: None,
            sending: false,
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

    pub fn next_wake(&self) -> Option<SimTime> {
        self.ready_at
    }

    pub fn address(&self) -> u16 {
        self.address
    }

    fn apply(&self, lora: &mut LoraSpace) {
        lora.configure(self.node, self.band, self.params, self.power as f64, self.network_id, self.address);
        lora.set_rx(self.node, self.mode == 0);
    }

    pub fn power(&mut self, lora: &mut LoraSpace, on: bool, now: SimTime) {
        self.powered = on;
        self.sending = false;
        self.line.clear();
        lora.set_powered(self.node, false);
        if on {
            self.enc.set_config(self.uart);
            self.ready_at = Some(now + SimTime::from_ms(120));
        } else {
            self.ready_at = None;
        }
    }

    fn send(&mut self, now: SimTime, text: &str) {
        if self.powered {
            self.out.extend(self.enc.push_bytes(text.as_bytes(), now + SimTime::from_us(400)));
        }
    }

    pub fn rx_byte(&mut self, lora: &mut LoraSpace, byte: u8, now: SimTime) {
        self.poll(lora, now);
        if !self.powered || self.ready_at.is_some() {
            return;
        }
        self.line.push(byte);
        if self.line.len() > 300 {
            self.line.clear();
            return self.send(now, "+ERR=1\r\n");
        }
        if byte == b'\n' {
            let raw = std::mem::take(&mut self.line);
            let text = String::from_utf8_lossy(&raw).trim_end_matches(['\r', '\n']).to_string();
            self.command(lora, &text, now);
        }
    }

    pub fn poll(&mut self, lora: &mut LoraSpace, now: SimTime) {
        if let Some(t) = self.ready_at
            && t <= now
        {
            self.ready_at = None;
            lora.set_powered(self.node, true);
            self.apply(lora);
            self.send(now, "+READY\r\n");
        }
        for t in lora.take_tx_done(self.node) {
            if self.sending {
                self.sending = false;
                self.send(t.max(now), "+OK\r\n");
            }
        }
        for rx in lora.take_rx(self.node) {
            let data = String::from_utf8_lossy(&rx.data).into_owned();
            let line = format!("+RCV={},{},{},{},{}\r\n", rx.from, rx.data.len(), data, rx.rssi as i32, rx.snr as i32);
            self.send(now, &line);
            self.notes.push((now, AnnotationKind::Lora, format!("RX from {} \"{}\" rssi={} dBm snr={}", rx.from, data, rx.rssi as i32, rx.snr)));
        }
    }

    fn command(&mut self, lora: &mut LoraSpace, text: &str, now: SimTime) {
        let Some(cmd) = at::parse(text) else {
            return self.send(now, "+ERR=2\r\n");
        };
        let ok = "+OK\r\n";
        let reply: String = match (cmd.name.as_str(), &cmd.kind) {
            ("", AtKind::Exec) => ok.into(),
            ("+RESET", AtKind::Exec) => {
                self.send(now, "+RESET\r\n");
                let at = self.enc.idle_at().max(now);
                self.power(lora, true, at);
                return;
            }
            ("+MODE", AtKind::Query) => format!("+MODE={}\r\n", self.mode),
            ("+MODE", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(m @ 0..=1) => {
                    self.mode = m as u8;
                    self.apply(lora);
                    ok.into()
                }
                _ => "+ERR=4\r\n".into(),
            },
            ("+IPR", AtKind::Query) => format!("+IPR={}\r\n", self.uart.baud),
            ("+IPR", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(b @ (300 | 1200 | 4800 | 9600 | 19_200 | 28_800 | 38_400 | 57_600 | 115_200)) => {
                    self.send(now, ok);
                    self.uart = UartConfig::new(b as u32);
                    self.enc.set_config(self.uart);
                    return;
                }
                _ => "+ERR=4\r\n".into(),
            },
            ("+PARAMETER", AtKind::Query) => {
                let bw = BW_CODES.iter().position(|&b| b == self.params.bw_hz).unwrap_or(7);
                format!("+PARAMETER={},{},{},{}\r\n", self.params.sf, bw, self.params.cr, self.params.preamble)
            }
            ("+PARAMETER", AtKind::Set(_)) => {
                let (sf, bw, cr, pre) = (cmd.arg_int(0), cmd.arg_int(1), cmd.arg_int(2), cmd.arg_int(3));
                match (sf, bw, cr, pre) {
                    (Some(sf @ 7..=12), Some(bw @ 0..=9), Some(cr @ 1..=4), Some(pre @ 4..=24)) => {
                        self.params = LoraParams { sf: sf as u8, bw_hz: BW_CODES[bw as usize], cr: cr as u8, preamble: pre as u16, crc: true, explicit_header: true };
                        self.apply(lora);
                        ok.into()
                    }
                    _ => "+ERR=4\r\n".into(),
                }
            }
            ("+BAND", AtKind::Query) => format!("+BAND={}\r\n", self.band),
            ("+BAND", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(f @ 433_000_000..=960_000_000) => {
                    self.band = f as u32;
                    self.apply(lora);
                    ok.into()
                }
                _ => "+ERR=4\r\n".into(),
            },
            ("+ADDRESS", AtKind::Query) => format!("+ADDRESS={}\r\n", self.address),
            ("+ADDRESS", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(a @ 0..=65_535) => {
                    self.address = a as u16;
                    self.apply(lora);
                    ok.into()
                }
                _ => "+ERR=4\r\n".into(),
            },
            ("+NETWORKID", AtKind::Query) => format!("+NETWORKID={}\r\n", self.network_id),
            ("+NETWORKID", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(n @ 0..=16) => {
                    self.network_id = n as u8;
                    self.apply(lora);
                    ok.into()
                }
                _ => "+ERR=4\r\n".into(),
            },
            ("+CPIN", AtKind::Query) => match &self.password {
                Some(p) => format!("+CPIN={p}\r\n"),
                None => "+CPIN=No Password!\r\n".into(),
            },
            ("+CPIN", AtKind::Set(_)) => {
                self.password = cmd.arg_str(0).map(|x| x.to_string());
                ok.into()
            }
            ("+CRFOP", AtKind::Query) => format!("+CRFOP={}\r\n", self.power),
            ("+CRFOP", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(p @ 0..=15) => {
                    self.power = p as u8;
                    self.apply(lora);
                    ok.into()
                }
                _ => "+ERR=4\r\n".into(),
            },
            ("+UID", AtKind::Query) => format!("+UID=0000000000000000000{:05}\r\n", self.node.0),
            ("+VER", AtKind::Query) => "+VER=RYLR89C_V1.2.7\r\n".into(),
            ("+SEND", AtKind::Set(_)) => {
                // Data may contain commas: address,length,<rest>.
                let body = text.split_once('=').map(|x| x.1).unwrap_or("");
                let mut it = body.splitn(3, ',');
                let (Some(a), Some(l), Some(data)) = (it.next(), it.next(), it.next()) else { return self.send(now, "+ERR=4\r\n") };
                let (Ok(addr), Ok(len)) = (a.trim().parse::<u16>(), l.trim().parse::<usize>()) else { return self.send(now, "+ERR=4\r\n") };
                if len > 240 {
                    return self.send(now, "+ERR=13\r\n");
                }
                if len != data.len() {
                    return self.send(now, "+ERR=5\r\n");
                }
                if self.mode != 0 {
                    return self.send(now, "+ERR=4\r\n");
                }
                match lora.transmit(self.node, addr, data.as_bytes(), now) {
                    Ok(end) => {
                        self.sending = true;
                        let air = (end - now).as_secs_f64() * 1e3;
                        self.notes.push((now, AnnotationKind::Lora, format!("TX to {addr} \"{data}\" (SF{}, {air:.0} ms on air)", self.params.sf)));
                        return;
                    }
                    Err(LoraError::DutyCycle(w)) => {
                        self.notes.push((now, AnnotationKind::Lora, format!("TX refused: duty cycle, wait {w:.1} s")));
                        "+ERR=10\r\n".into()
                    }
                    Err(LoraError::TooLong(_)) => "+ERR=13\r\n".into(),
                    Err(_) => "+ERR=10\r\n".into(),
                }
            }
            _ => "+ERR=4\r\n".into(),
        };
        self.send(now, &reply);
    }

    pub fn card(&self, device: DeviceId, lora: &LoraSpace) -> RadioCard {
        let status = if !self.powered { "off" } else if self.sending { "transmitting" } else { "listening" };
        let mut rows = vec![
            ("Address".into(), format!("{} · network {}", self.address, self.network_id)),
            ("Band".into(), format!("{:.1} MHz", self.band as f64 / 1e6)),
            ("Modulation".into(), format!("SF{} · {} kHz · 4/{}", self.params.sf, self.params.bw_hz as f64 / 1e3, self.params.cr + 4)),
        ];
        if let Some(pos) = lora.node_pos(self.node) {
            let g = &lora.gateway;
            let rssi = lora.rssi_between(pos, g.pos, self.power as f64, self.band) + g.gain_db;
            rows.push(("Gateway".into(), format!("{:.0} dBm at {:.0} m", rssi, pos.dist(g.pos))));
        }
        RadioCard { device, label: self.label.clone(), title: "RYLR896 LoRa".into(), status: status.into(), rows }
    }
}
