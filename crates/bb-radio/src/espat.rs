//! ESP-01 / ESP8266 running Espressif's ESP-AT firmware (`RF-5`), as a state machine over UART
//! bytes and [`SimTime`], backed by the virtual [`Network`].
//!
//! Behaviour follows ESP-AT 2.2 for ESP8266 (with the v1.7 strings most Arduino libraries
//! parse): power-on ROM boot log at **74 880 baud** (garbage at any other speed) then `ready`
//! at the configured baud; command echo (`ATE0/1`); `busy p...` while a command is running;
//! unsolicited `WIFI CONNECTED` / `WIFI GOT IP` / `WIFI DISCONNECT`; `+IPD,n:` delivery;
//! `AT+CIPSEND` with the `>` prompt, `Recv n bytes` and `SEND OK` once the peer ACKs; SNTP; the
//! ESP-AT MQTT client (`AT+MQTT*`, `+MQTTSUBRECV`); `AT+UART_CUR/DEF`. Flash-persisted settings
//! (default UART, saved AP, auto-connect) survive resets and are part of the saved bench state.

use crate::at::{self, AtCommand, AtKind};
use crate::mqtt::{ClientEvent, ClientState, MqttClient, Will};
use crate::net::{AnnotationKind, CloseReason, NetEvent, Network, SockId, StationId, TcpState};
use crate::uart::{UartConfig, UartEncoder, UartFrame};
use bb_core::SimTime;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddrV4};

/// Processing latency of unsolicited output (events from the network, timers).
const EVT: SimTime = SimTime::from_us(150);

/// The ROM bootloader's power-on log (sent at 74 880 baud).
pub const BOOT_ROM_LOG: &str = "\r\n ets Jan  8 2013,rst cause:1, boot mode:(3,6)\r\n\r\nload 0x40100000, len 2592, room 16 \r\ntail 0\r\nchksum 0xf3\r\nload 0x3ffe8000, len 764, room 8 \r\ntail 4\r\nchksum 0x92\r\nload 0x3ffe82fc, len 676, room 4 \r\ntail 0\r\nchksum 0x22\r\ncsum 0x22\r\n\r\n2nd boot version : 1.7(5d6f877)\r\n  SPI Speed      : 40MHz\r\n  SPI Mode       : DOUT\r\n  SPI Flash Size & Map: 8Mbit(512KB+512KB)\r\njump to run user1 @ 1000\r\n\r\n";

/// Persisted (flash) settings of the module — saved with the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EspFlash {
    /// `AT+UART_DEF` (the garden-node bench uses 9600).
    pub uart: UartConfig,
    pub cwmode: u8,
    /// AP stored by `AT+CWJAP` (SYSSTORE=1) for auto-connect.
    pub saved_ap: Option<(String, String)>,
    pub autoconn: bool,
}

impl Default for EspFlash {
    fn default() -> Self {
        EspFlash { uart: UartConfig::new(115_200), cwmode: 1, saved_ap: None, autoconn: true }
    }
}

/// Modem options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EspAtConfig {
    pub flash: EspFlash,
    /// Time from power-on to `ready`.
    pub boot_time_ms: u64,
    /// Typical command processing latency.
    pub cmd_latency_us: u64,
}

impl Default for EspAtConfig {
    fn default() -> Self {
        EspAtConfig { flash: EspFlash::default(), boot_time_ms: 360, cmd_latency_us: 1_200 }
    }
}

impl EspAtConfig {
    /// Factory module reconfigured to `baud` (as with `AT+UART_DEF`).
    pub fn with_baud(baud: u32) -> Self {
        let mut c = EspAtConfig::default();
        c.flash.uart = UartConfig::new(baud);
        c
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LinkKind {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq)]
struct Link {
    kind: LinkKind,
    sock: SockId,
    remote: SocketAddrV4,
    local_port: u16,
    server: bool,
    /// Bytes waiting for the peer's ACK before `SEND OK`.
    awaiting_ack: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum Busy {
    Join { ssid: String },
    Start { link: usize, kind: LinkKind, port: u16, local_port: u16, host: String, query: Option<u32>, sock: Option<SockId> },
    /// `>` prompt shown, collecting `len` bytes.
    SendData { link: usize, len: usize, buf: Vec<u8> },
    /// Data handed to TCP, `SEND OK` pending.
    SendWait { link: usize },
    Ping { host: String, query: Option<u32>, ping: Option<u32> },
    Domain { query: u32 },
    Scan,
    MqttConn { query: Option<u32> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Timer {
    Ready,
    ScanDone,
    JoinTimeout(u64),
    SntpPoll,
    SendOk(usize),
}

#[derive(Debug, Clone, Default)]
struct Sntp {
    enabled: bool,
    tz_hours: i32,
    servers: Vec<String>,
    sock: Option<SockId>,
    query: Option<u32>,
    base: Option<(SimTime, f64)>,
}

#[derive(Debug, Clone)]
struct MqttLink {
    client: MqttClient,
    scheme: u8,
    host: String,
    port: u16,
    reconnect: bool,
    sock: Option<SockId>,
    subs: Vec<(String, u8)>,
}

/// High-level state for the radio card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EspStatus {
    pub powered: bool,
    pub ready: bool,
    pub baud: u32,
    pub echo: bool,
    pub links: usize,
    pub mqtt_connected: bool,
    pub sntp_synced: bool,
}

/// An ESP-01 module.
#[derive(Debug, Clone)]
pub struct EspAt {
    pub label: String,
    pub station: StationId,
    pub config: EspAtConfig,
    powered: bool,
    ready: bool,
    uart: UartConfig,
    enc: UartEncoder,
    out: Vec<UartFrame>,
    line: Vec<u8>,
    echo: bool,
    mux: bool,
    busy: Option<Busy>,
    links: [Option<Link>; 5],
    server: Option<(SockId, u16)>,
    timers: Vec<(SimTime, Timer)>,
    join_token: u64,
    sntp: Sntp,
    mqtt: Option<MqttLink>,
    mqtt_cfg: Option<MqttClient>,
    notes: Vec<(SimTime, AnnotationKind, String)>,
    /// Bytes received while booting or garbled (diagnostics).
    pub ignored_bytes: u64,
}

impl EspAt {
    pub fn new(label: &str, station: StationId, config: EspAtConfig) -> Self {
        let uart = config.flash.uart;
        EspAt {
            label: label.into(),
            station,
            powered: false,
            ready: false,
            uart,
            enc: UartEncoder::new(uart),
            out: vec![],
            line: vec![],
            echo: true,
            mux: false,
            busy: None,
            links: Default::default(),
            server: None,
            timers: vec![],
            join_token: 0,
            sntp: Sntp::default(),
            mqtt: None,
            mqtt_cfg: None,
            notes: vec![],
            ignored_bytes: 0,
            config,
        }
    }

    // ================================================================ co-sim interface

    /// The UART format the module currently receives and transmits with.
    pub fn uart_config(&self) -> UartConfig {
        self.uart
    }

    /// Frames to put on the module's TX pin (each knows its own baud: the boot log is 74 880).
    pub fn take_tx(&mut self) -> Vec<UartFrame> {
        std::mem::take(&mut self.out)
    }

    /// Serial-monitor annotations (`wifi: AT+CWJAP="SIM-AP" → OK rssi=-54 dBm ip=10.0.0.42`).
    pub fn take_notes(&mut self) -> Vec<(SimTime, AnnotationKind, String)> {
        std::mem::take(&mut self.notes)
    }

    pub fn status(&self, net: &Network) -> EspStatus {
        EspStatus {
            powered: self.powered,
            ready: self.ready,
            baud: self.uart.baud,
            echo: self.echo,
            links: self.links.iter().flatten().filter(|l| net.tcp_state(l.sock) != TcpState::Closed || l.kind == LinkKind::Udp).count(),
            mqtt_connected: self.mqtt.as_ref().is_some_and(|m| m.client.state() == ClientState::Connected),
            sntp_synced: self.sntp.base.is_some(),
        }
    }

    /// Earliest internal deadline.
    pub fn next_wake(&self) -> Option<SimTime> {
        let t = self.timers.iter().map(|t| t.0).min();
        let p = self.mqtt.as_ref().and_then(|m| m.client.ping_due());
        match (t, p) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Supply on (or EN/CH_PD high): the module boots.
    pub fn power_on(&mut self, net: &mut Network, now: SimTime) {
        self.boot(net, now);
    }

    /// Supply off / EN low: everything stops; links and the Wi-Fi association drop.
    pub fn power_off(&mut self, net: &mut Network, now: SimTime) {
        self.powered = false;
        self.ready = false;
        self.drop_all(net, now);
        let _ = net.leave(self.station, now);
        net.take_events(self.station);
        self.timers.clear();
        self.busy = None;
    }

    /// Pulse on the RST pin.
    pub fn reset(&mut self, net: &mut Network, now: SimTime) {
        self.power_off(net, now);
        self.boot(net, now);
    }

    fn boot(&mut self, net: &mut Network, now: SimTime) {
        self.powered = true;
        self.ready = false;
        self.drop_all(net, now);
        let _ = net.leave(self.station, now);
        net.take_events(self.station);
        self.uart = self.config.flash.uart;
        self.echo = true;
        self.mux = false;
        self.busy = None;
        self.line.clear();
        self.timers.clear();
        self.sntp = Sntp::default();
        self.mqtt = None;
        self.mqtt_cfg = None;
        // ROM log at 74880 baud, then the SDK switches to the AT UART speed.
        self.enc.set_config(UartConfig::new(74_880));
        let start = now + SimTime::from_us(800);
        self.out.extend(self.enc.push_bytes(BOOT_ROM_LOG.as_bytes(), start));
        self.enc.set_config(self.uart);
        self.timers.push((now + SimTime::from_ms(self.config.boot_time_ms), Timer::Ready));
    }

    fn drop_all(&mut self, net: &mut Network, now: SimTime) {
        for l in self.links.iter_mut() {
            if let Some(l) = l.take() {
                net.close(l.sock, now);
            }
        }
        if let Some((s, _)) = self.server.take() {
            net.close(s, now);
        }
        if let Some(sock) = self.mqtt.as_mut().and_then(|m| m.sock.take()) {
            net.close(sock, now);
        }
        if let Some(sock) = self.sntp.sock.take() {
            net.close(sock, now);
        }
    }

    fn emit_at(&mut self, at: SimTime, text: &[u8]) {
        if !self.powered {
            return;
        }
        self.out.extend(self.enc.push_bytes(text, at));
    }

    fn reply(&mut self, now: SimTime, text: &[u8]) {
        let at = now + SimTime::from_us(self.config.cmd_latency_us);
        self.emit_at(at, text);
    }

    fn note(&mut self, now: SimTime, kind: AnnotationKind, text: String) {
        self.notes.push((now, kind, text));
    }

    fn timer(&mut self, at: SimTime, t: Timer) {
        self.timers.push((at, t));
    }

    /// A byte arrived on the module's RX pin at `now` (decoded at [`EspAt::uart_config`]).
    pub fn rx_byte(&mut self, net: &mut Network, byte: u8, now: SimTime) {
        self.poll(net, now);
        if !self.powered || !self.ready {
            self.ignored_bytes += 1;
            return;
        }
        if let Some(Busy::SendData { link, len, buf }) = &mut self.busy {
            buf.push(byte);
            if buf.len() >= *len {
                let (link, data) = (*link, std::mem::take(buf));
                self.finish_send(net, link, data, now);
            }
            return;
        }
        if self.echo && !self.is_busy() && byte != b'\n' {
            self.emit_at(now + SimTime::from_us(60), &[byte]);
        }
        self.line.push(byte);
        if self.line.len() > 1024 {
            self.line.clear();
        }
        if byte == b'\n' {
            let raw = std::mem::take(&mut self.line);
            let text = String::from_utf8_lossy(&raw).into_owned();
            let text = text.trim_end_matches(['\r', '\n']).to_string();
            if text.trim().is_empty() {
                return;
            }
            if self.is_busy() {
                let msg: &[u8] = if matches!(self.busy, Some(Busy::SendWait { .. })) { b"busy s...\r\n" } else { b"busy p...\r\n" };
                self.reply(now, msg);
                return;
            }
            if self.echo {
                // ESP-AT echoes the terminator as CR CR LF.
                self.emit_at(now + SimTime::from_us(80), b"\r\n");
            }
            self.command(net, &text, now);
        }
    }

    fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Handles due timers and network events. Call at every network step / wake time.
    pub fn poll(&mut self, net: &mut Network, now: SimTime) {
        loop {
            let due = self.timers.iter().enumerate().filter(|(_, t)| t.0 <= now).min_by_key(|(_, t)| t.0).map(|(i, _)| i);
            let Some(i) = due else { break };
            let (at, t) = self.timers.remove(i);
            let _ = at;
            self.on_timer(net, t, now);
        }
        if let Some(m) = self.mqtt.as_mut()
            && let Some(sock) = m.sock
            && let Some(bytes) = m.client.poll_ping(now)
        {
            let _ = net.tcp_send(sock, &bytes, now);
        }
        for e in net.take_events(self.station) {
            if self.powered {
                self.on_net(net, e, now);
            }
        }
    }

    fn on_timer(&mut self, net: &mut Network, t: Timer, now: SimTime) {
        match t {
            Timer::Ready => {
                self.ready = true;
                self.emit_at(now + EVT, b"\r\nready\r\n");
                if self.config.flash.autoconn
                    && let Some((ssid, pass)) = self.config.flash.saved_ap.clone()
                {
                    let _ = net.join(self.station, &ssid, Some(&pass), now);
                }
            }
            Timer::ScanDone => {
                if self.busy == Some(Busy::Scan) {
                    self.busy = None;
                    let mut text = String::new();
                    for ap in net.scan(self.station) {
                        let ecn = if ap.secure { 3 } else { 0 };
                        text.push_str(&format!("+CWLAP:({},\"{}\",{},\"{}\",{},-1,-1)\r\n", ecn, ap.ssid, ap.rssi, crate::wire::mac_string(ap.bssid), ap.channel));
                    }
                    text.push_str("\r\nOK\r\n");
                    self.emit_at(now + EVT, text.as_bytes());
                }
            }
            Timer::JoinTimeout(tok) => {
                if tok == self.join_token && matches!(self.busy, Some(Busy::Join { .. })) {
                    self.busy = None;
                    self.emit_at(now + EVT, b"+CWJAP:1\r\n\r\nFAIL\r\n");
                }
            }
            Timer::SntpPoll => self.sntp_request(net, now),
            Timer::SendOk(link) => {
                if self.busy == Some(Busy::SendWait { link }) {
                    self.busy = None;
                    self.emit_at(now + EVT, b"\r\nSEND OK\r\n");
                }
            }
        }
    }

    // ================================================================ commands

    fn command(&mut self, net: &mut Network, line: &str, now: SimTime) {
        let Some(cmd) = at::parse(line) else {
            self.reply(now, b"\r\nERROR\r\n");
            return;
        };
        let ok = |m: &mut EspAt| m.reply(now, b"\r\nOK\r\n");
        let err = |m: &mut EspAt| m.reply(now, b"\r\nERROR\r\n");
        let st = self.station;
        match (cmd.name.as_str(), &cmd.kind) {
            ("", AtKind::Exec) => ok(self),
            ("E0", _) => {
                self.echo = false;
                ok(self)
            }
            ("E1", _) => {
                self.echo = true;
                ok(self)
            }
            ("+RST", AtKind::Exec) => {
                self.reply(now, b"\r\nOK\r\n");
                // Reboot after the OK has left the UART.
                let at = self.enc.idle_at().max(now) + SimTime::from_ms(5);
                self.drop_all(net, now);
                let _ = net.leave(st, now);
                net.take_events(st);
                self.ready = false;
                self.boot_at(at);
            }
            ("+RESTORE", AtKind::Exec) => {
                self.config.flash = EspFlash::default();
                self.reply(now, b"\r\nOK\r\n");
                let at = self.enc.idle_at().max(now) + SimTime::from_ms(5);
                self.drop_all(net, now);
                let _ = net.leave(st, now);
                net.take_events(st);
                self.ready = false;
                self.boot_at(at);
            }
            ("+GMR", AtKind::Exec) => self.reply(
                now,
                b"AT version:2.2.0.0(breadbox-sim - ESP8266 - Nov 14 2026 00:00:00)\r\nSDK version:v3.4-22-g967752e2\r\ncompile time(sim):Nov 14 2026 00:00:00\r\nBin version:2.2.0(ESP8266_1MB)\r\n\r\nOK\r\n",
            ),
            ("+CWMODE" | "+CWMODE_CUR" | "+CWMODE_DEF", AtKind::Query) => {
                let t = format!("{}:{}\r\n\r\nOK\r\n", cmd.name, self.config.flash.cwmode);
                self.reply(now, t.as_bytes())
            }
            ("+CWMODE" | "+CWMODE_CUR" | "+CWMODE_DEF", AtKind::Set(_)) => match cmd.arg_int(0) {
                Some(m @ 0..=3) => {
                    self.config.flash.cwmode = m as u8;
                    ok(self)
                }
                _ => err(self),
            },
            ("+CWAUTOCONN", AtKind::Set(_)) => {
                self.config.flash.autoconn = cmd.arg_int(0) == Some(1);
                ok(self)
            }
            ("+SYSSTORE" | "+CWDHCP" | "+CWDHCP_CUR" | "+SLEEP" | "+CIPDINFO" | "+CWHOSTNAME" | "+CIPRECVMODE", AtKind::Set(_)) => {
                if cmd.name == "+CWHOSTNAME"
                    && let Some(h) = cmd.arg_str(0)
                {
                    net.set_hostname(st, h);
                }
                if cmd.name == "+CIPRECVMODE" && cmd.arg_int(0) != Some(0) {
                    return err(self);
                }
                ok(self)
            }
            ("+CWJAP" | "+CWJAP_CUR" | "+CWJAP_DEF", AtKind::Set(_)) => {
                let (Some(ssid), pass) = (cmd.arg_str(0).map(|x| x.to_string()), cmd.arg_str(1).map(|x| x.to_string())) else {
                    return err(self);
                };
                if self.config.flash.cwmode == 2 || self.config.flash.cwmode == 0 {
                    return err(self);
                }
                if net.is_connected(st) {
                    self.emit_at(now + EVT, b"WIFI DISCONNECT\r\n");
                }
                if cmd.name != "+CWJAP_CUR" {
                    self.config.flash.saved_ap = Some((ssid.clone(), pass.clone().unwrap_or_default()));
                }
                self.drop_links(net, now);
                let _ = net.join(st, &ssid, pass.as_deref(), now);
                self.join_token += 1;
                self.busy = Some(Busy::Join { ssid });
                let tok = self.join_token;
                self.timer(now + SimTime::from_secs(15), Timer::JoinTimeout(tok));
            }
            ("+CWJAP" | "+CWJAP_CUR" | "+CWJAP_DEF", AtKind::Query) => {
                let t = match net.station_info(st) {
                    Some(i) if i.state == crate::net::WifiStateKind::Connected => {
                        let ap = net.access_points().iter().find(|a| Some(&a.ssid) == i.ssid.as_ref()).cloned();
                        let rssi = i.link.map(|l| l.rssi_dbm.round() as i32).unwrap_or(0);
                        format!(
                            "{}:\"{}\",\"{}\",{},{}\r\n\r\nOK\r\n",
                            cmd.name,
                            i.ssid.unwrap_or_default(),
                            ap.map(|a| crate::wire::mac_string(a.bssid)).unwrap_or_default(),
                            i.channel.unwrap_or(0),
                            rssi
                        )
                    }
                    _ => "No AP\r\n\r\nOK\r\n".to_string(),
                };
                self.reply(now, t.as_bytes())
            }
            ("+CWQAP", AtKind::Exec) => {
                let was = net.is_connected(st);
                self.drop_links(net, now);
                let _ = net.leave(st, now);
                net.take_events(st);
                self.reply(now, if was { b"\r\nOK\r\nWIFI DISCONNECT\r\n" } else { b"\r\nOK\r\n" });
            }
            ("+CWLAP", AtKind::Exec) => {
                self.busy = Some(Busy::Scan);
                self.timer(now + SimTime::from_ms(1_800), Timer::ScanDone);
            }
            ("+CIFSR", AtKind::Exec) => {
                let ip = net.ip_of(st).unwrap_or(Ipv4Addr::UNSPECIFIED);
                let mac = net.station_info(st).map(|i| crate::wire::mac_string(i.mac)).unwrap_or_default();
                let t = format!("+CIFSR:STAIP,\"{ip}\"\r\n+CIFSR:STAMAC,\"{mac}\"\r\n\r\nOK\r\n");
                self.reply(now, t.as_bytes())
            }
            ("+CIPSTA" | "+CIPSTA_CUR" | "+CIPSTA_DEF", AtKind::Query) => {
                let ip = net.ip_of(st).unwrap_or(Ipv4Addr::UNSPECIFIED);
                let (gw, mask) = if net.ip_of(st).is_some() { (net.gateway(), net.config.netmask) } else { (Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED) };
                let n = &cmd.name;
                let t = format!("{n}:ip:\"{ip}\"\r\n{n}:gateway:\"{gw}\"\r\n{n}:netmask:\"{mask}\"\r\n\r\nOK\r\n");
                self.reply(now, t.as_bytes())
            }
            ("+CIPSTA" | "+CIPSTA_CUR" | "+CIPSTA_DEF", AtKind::Set(_)) => {
                let ip = cmd.arg_str(0).and_then(|x| x.parse::<Ipv4Addr>().ok());
                let gw = cmd.arg_str(1).and_then(|x| x.parse::<Ipv4Addr>().ok()).unwrap_or(net.gateway());
                let mask = cmd.arg_str(2).and_then(|x| x.parse::<Ipv4Addr>().ok()).unwrap_or(Ipv4Addr::new(255, 255, 255, 0));
                match ip {
                    Some(ip) => {
                        net.set_static_ip(st, Some((ip, gw, mask)));
                        ok(self)
                    }
                    None => err(self),
                }
            }
            ("+CIPSTAMAC" | "+CIPSTAMAC_CUR", AtKind::Query) => {
                let mac = net.station_info(st).map(|i| crate::wire::mac_string(i.mac)).unwrap_or_default();
                let t = format!("{}:\"{mac}\"\r\n\r\nOK\r\n", cmd.name);
                self.reply(now, t.as_bytes())
            }
            ("+CIPMUX", AtKind::Query) => {
                let t = format!("+CIPMUX:{}\r\n\r\nOK\r\n", self.mux as u8);
                self.reply(now, t.as_bytes())
            }
            ("+CIPMUX", AtKind::Set(_)) => {
                let m = cmd.arg_int(0);
                if self.links.iter().any(|l| l.is_some()) || self.server.is_some() {
                    self.reply(now, b"link is builded\r\n\r\nERROR\r\n");
                } else if let Some(m @ 0..=1) = m {
                    self.mux = m == 1;
                    ok(self)
                } else {
                    err(self)
                }
            }
            ("+CIPMODE", AtKind::Set(_)) => {
                if cmd.arg_int(0) == Some(0) {
                    ok(self)
                } else {
                    err(self)
                }
            }
            ("+CIPSTART", AtKind::Set(_)) => self.cipstart(net, &cmd, now),
            ("+CIPSEND", AtKind::Set(_)) => self.cipsend(net, &cmd, now),
            ("+CIPCLOSE", _) => {
                let id = if self.mux { cmd.arg_int(0).unwrap_or(-1) } else { 0 };
                let ids: Vec<usize> = if id == 5 { (0..5).collect() } else if (0..5).contains(&id) { vec![id as usize] } else { vec![] };
                let mut any = false;
                let mut text = String::new();
                for i in ids {
                    if let Some(l) = self.links[i].take() {
                        net.close(l.sock, now);
                        any = true;
                        text.push_str(&if self.mux { format!("{i},CLOSED\r\n") } else { "CLOSED\r\n".to_string() });
                    }
                }
                if any {
                    text.push_str("\r\nOK\r\n");
                    self.reply(now, text.as_bytes())
                } else {
                    self.reply(now, b"link is not valid\r\n\r\nERROR\r\n")
                }
            }
            ("+CIPSTATUS", AtKind::Exec) => {
                let code = if !net.is_connected(st) {
                    5
                } else if self.links.iter().flatten().any(|_| true) {
                    3
                } else if net.ip_of(st).is_some() {
                    2
                } else {
                    4
                };
                let mut t = format!("STATUS:{code}\r\n");
                for (i, l) in self.links.iter().enumerate() {
                    if let Some(l) = l {
                        let kind = if l.kind == LinkKind::Tcp { "TCP" } else { "UDP" };
                        t.push_str(&format!("+CIPSTATUS:{},\"{}\",\"{}\",{},{},{}\r\n", i, kind, l.remote.ip(), l.remote.port(), l.local_port, l.server as u8));
                    }
                }
                t.push_str("\r\nOK\r\n");
                self.reply(now, t.as_bytes())
            }
            ("+CIPSERVER", AtKind::Set(_)) => {
                if !self.mux {
                    return err(self);
                }
                match cmd.arg_int(0) {
                    Some(1) => {
                        let port = cmd.arg_int(1).unwrap_or(333) as u16;
                        match net.tcp_listen(st, port) {
                            Ok(sock) => {
                                self.server = Some((sock, port));
                                ok(self)
                            }
                            Err(_) => self.reply(now, b"no change\r\n\r\nOK\r\n"),
                        }
                    }
                    Some(0) => {
                        if let Some((s, _)) = self.server.take() {
                            net.close(s, now);
                        }
                        ok(self)
                    }
                    _ => err(self),
                }
            }
            ("+CIPDOMAIN", AtKind::Set(_)) => match cmd.arg_str(0).map(|x| x.to_string()) {
                Some(name) => match net.dns_resolve(st, &name, now) {
                    Ok(q) => self.busy = Some(Busy::Domain { query: q }),
                    Err(_) => self.reply(now, b"DNS Fail\r\n\r\nERROR\r\n"),
                },
                None => err(self),
            },
            ("+PING", AtKind::Set(_)) => match cmd.arg_str(0).map(|x| x.to_string()) {
                Some(host) => {
                    if net.ip_of(st).is_none() {
                        return self.reply(now, b"+PING:TIMEOUT\r\n\r\nERROR\r\n");
                    }
                    match net.dns_resolve(st, &host, now) {
                        Ok(q) => self.busy = Some(Busy::Ping { host, query: Some(q), ping: None }),
                        Err(_) => err(self),
                    }
                }
                None => err(self),
            },
            ("+UART" | "+UART_CUR" | "+UART_DEF", AtKind::Set(_)) => {
                let baud = cmd.arg_int(0).unwrap_or(0);
                let bits = cmd.arg_int(1).unwrap_or(8);
                let stop = cmd.arg_int(2).unwrap_or(1);
                let parity = cmd.arg_int(3).unwrap_or(0);
                if !(80..=5_000_000).contains(&baud) || !(5..=8).contains(&bits) || !(1..=3).contains(&stop) || !(0..=2).contains(&parity) {
                    return err(self);
                }
                let p = match parity {
                    1 => crate::uart::Parity::Odd,
                    2 => crate::uart::Parity::Even,
                    _ => crate::uart::Parity::None,
                };
                let cfg = UartConfig::new(baud as u32).with_format(bits as u8, p, if stop == 3 { 2 } else { stop as u8 });
                self.reply(now, b"\r\nOK\r\n");
                if cmd.name != "+UART_CUR" {
                    self.config.flash.uart = cfg;
                }
                self.uart = cfg;
                self.enc.set_config(cfg);
            }
            ("+UART" | "+UART_CUR" | "+UART_DEF", AtKind::Query) => {
                let u = if cmd.name == "+UART_DEF" { self.config.flash.uart } else { self.uart };
                let p = match u.parity {
                    crate::uart::Parity::None => 0,
                    crate::uart::Parity::Odd => 1,
                    crate::uart::Parity::Even => 2,
                };
                let t = format!("{}:{},{},{},{},0\r\n\r\nOK\r\n", cmd.name, u.baud, u.data_bits, u.stop_bits, p);
                self.reply(now, t.as_bytes())
            }
            ("+CIPSNTPCFG", AtKind::Set(_)) => {
                self.sntp.enabled = cmd.arg_int(0) == Some(1);
                self.sntp.tz_hours = cmd.arg_int(1).unwrap_or(0) as i32;
                self.sntp.servers = (2..5).filter_map(|i| cmd.arg_str(i).map(|x| x.to_string())).collect();
                if self.sntp.servers.is_empty() {
                    self.sntp.servers = vec!["pool.ntp.org".into()];
                }
                self.sntp.base = None;
                ok(self);
                if self.sntp.enabled {
                    self.timer(now + SimTime::from_ms(50), Timer::SntpPoll);
                }
            }
            ("+CIPSNTPCFG", AtKind::Query) => {
                let srv: Vec<String> = self.sntp.servers.iter().map(|x| format!("\"{x}\"")).collect();
                let t = format!("+CIPSNTPCFG:{},{},{}\r\n\r\nOK\r\n", self.sntp.enabled as u8, self.sntp.tz_hours, srv.join(","));
                self.reply(now, t.as_bytes())
            }
            ("+CIPSNTPTIME", AtKind::Query) => {
                let unix = match self.sntp.base {
                    Some((t0, u0)) => u0 + (now - t0).as_secs_f64() + self.sntp.tz_hours as f64 * 3600.0,
                    None => 0.0,
                };
                let t = format!("+CIPSNTPTIME:{}\r\nOK\r\n", esp_time(unix as i64));
                self.reply(now, t.as_bytes())
            }
            ("+MQTTUSERCFG", AtKind::Set(_)) => {
                let id = cmd.arg_str(2).unwrap_or("esp").to_string();
                let mut c = MqttClient::new(&id);
                c.username = cmd.arg_str(3).filter(|u| !u.is_empty()).map(|u| u.to_string());
                c.password = cmd.arg_str(4).filter(|p| !p.is_empty()).map(|p| p.as_bytes().to_vec());
                if let Some(prev) = &self.mqtt_cfg {
                    c.keep_alive = prev.keep_alive;
                    c.will = prev.will.clone();
                    c.clean_session = prev.clean_session;
                }
                self.mqtt_cfg = Some(c);
                ok(self)
            }
            ("+MQTTCONNCFG", AtKind::Set(_)) => {
                let c = self.mqtt_cfg.get_or_insert_with(|| MqttClient::new("esp"));
                c.keep_alive = cmd.arg_int(1).unwrap_or(120).clamp(0, 7200) as u16;
                c.clean_session = cmd.arg_int(2).unwrap_or(0) == 0;
                if let Some(t) = cmd.arg_str(3).filter(|t| !t.is_empty()) {
                    c.will = Some(Will {
                        topic: t.into(),
                        payload: cmd.arg_str(4).unwrap_or("").as_bytes().to_vec(),
                        qos: cmd.arg_int(5).unwrap_or(0).clamp(0, 1) as u8,
                        retain: cmd.arg_int(6).unwrap_or(0) == 1,
                    });
                }
                ok(self)
            }
            ("+MQTTCONN", AtKind::Set(_)) => self.mqtt_conn(net, &cmd, now),
            ("+MQTTCONN", AtKind::Query) => {
                let t = match &self.mqtt {
                    Some(m) => {
                        let state = match m.client.state() {
                            ClientState::Connected => 4,
                            ClientState::Connecting => 3,
                            _ => 1,
                        };
                        format!("+MQTTCONN:0,{},{},\"{}\",\"{}\",\"\",{}\r\n\r\nOK\r\n", state, m.scheme, m.host, m.port, m.reconnect as u8)
                    }
                    None => "\r\nOK\r\n".into(),
                };
                self.reply(now, t.as_bytes())
            }
            ("+MQTTPUB", AtKind::Set(_)) => {
                let (Some(topic), Some(data)) = (cmd.arg_str(1).map(|x| x.to_string()), cmd.arg_str(2).map(|x| x.to_string())) else {
                    return err(self);
                };
                let qos = cmd.arg_int(3).unwrap_or(0).clamp(0, 1) as u8;
                let retain = cmd.arg_int(4).unwrap_or(0) == 1;
                let Some(m) = self.mqtt.as_mut().filter(|m| m.client.state() == ClientState::Connected) else {
                    return err(self);
                };
                let bytes = m.client.publish(&topic, data.as_bytes(), qos, retain, now);
                match m.sock.map(|s| net.tcp_send(s, &bytes, now)) {
                    Some(Ok(_)) => ok(self),
                    _ => err(self),
                }
            }
            ("+MQTTSUB", AtKind::Set(_)) => {
                let Some(topic) = cmd.arg_str(1).map(|x| x.to_string()) else { return err(self) };
                let qos = cmd.arg_int(2).unwrap_or(0).clamp(0, 1) as u8;
                let Some(m) = self.mqtt.as_mut().filter(|m| m.client.state() == ClientState::Connected) else {
                    return err(self);
                };
                if m.subs.iter().any(|(t, _)| *t == topic) {
                    return self.reply(now, b"ALREADY SUBSCRIBE\r\n\r\nOK\r\n");
                }
                let (_, bytes) = m.client.subscribe(&topic, qos, now);
                m.subs.push((topic, qos));
                match m.sock.map(|s| net.tcp_send(s, &bytes, now)) {
                    Some(Ok(_)) => ok(self),
                    _ => err(self),
                }
            }
            ("+MQTTSUB", AtKind::Query) => {
                let mut t = String::new();
                if let Some(m) = &self.mqtt {
                    for (topic, q) in &m.subs {
                        t.push_str(&format!("+MQTTSUB:0,4,\"{topic}\",{q}\r\n"));
                    }
                }
                t.push_str("\r\nOK\r\n");
                self.reply(now, t.as_bytes())
            }
            ("+MQTTUNSUB", AtKind::Set(_)) => {
                let Some(topic) = cmd.arg_str(1).map(|x| x.to_string()) else { return err(self) };
                let Some(m) = self.mqtt.as_mut() else { return err(self) };
                if !m.subs.iter().any(|(t, _)| *t == topic) {
                    return self.reply(now, b"NO UNSUBSCRIBE\r\n\r\nOK\r\n");
                }
                m.subs.retain(|(t, _)| *t != topic);
                let (_, bytes) = m.client.unsubscribe(&topic, now);
                if let Some(s) = m.sock {
                    let _ = net.tcp_send(s, &bytes, now);
                }
                ok(self)
            }
            ("+MQTTCLEAN", AtKind::Set(_)) => {
                if let Some(mut m) = self.mqtt.take() {
                    if let Some(s) = m.sock {
                        let bytes = m.client.disconnect();
                        let _ = net.tcp_send(s, &bytes, now);
                        net.close(s, now);
                    }
                    ok(self)
                } else {
                    err(self)
                }
            }
            _ => err(self),
        }
    }

    fn boot_at(&mut self, at: SimTime) {
        self.powered = true;
        self.uart = self.config.flash.uart;
        self.echo = true;
        self.mux = false;
        self.busy = None;
        self.line.clear();
        self.timers.clear();
        self.sntp = Sntp::default();
        self.mqtt = None;
        self.enc.set_config(UartConfig::new(74_880));
        self.out.extend(self.enc.push_bytes(BOOT_ROM_LOG.as_bytes(), at));
        self.enc.set_config(self.uart);
        self.timer(at + SimTime::from_ms(self.config.boot_time_ms), Timer::Ready);
        self.ready = false;
    }

    fn drop_links(&mut self, net: &mut Network, now: SimTime) {
        for l in self.links.iter_mut() {
            if let Some(l) = l.take() {
                net.close(l.sock, now);
            }
        }
    }

    fn link_prefix(&self, i: usize) -> String {
        if self.mux { format!("{i},") } else { String::new() }
    }

    fn cipstart(&mut self, net: &mut Network, cmd: &AtCommand, now: SimTime) {
        let a = cmd.args();
        let off = if self.mux { 1 } else { 0 };
        let link = if self.mux { cmd.arg_int(0).unwrap_or(-1) } else { 0 };
        if !(0..5).contains(&link) || a.len() < 3 + off {
            return self.reply(now, b"\r\nERROR\r\n");
        }
        let link = link as usize;
        if self.links[link].is_some() {
            return self.reply(now, b"ALREADY CONNECTED\r\n\r\nERROR\r\n");
        }
        let kind = match cmd.arg_str(off).map(|k| k.to_ascii_uppercase()) {
            Some(k) if k == "TCP" => LinkKind::Tcp,
            Some(k) if k == "UDP" => LinkKind::Udp,
            _ => return self.reply(now, b"\r\nERROR\r\n"),
        };
        let host = cmd.arg_str(off + 1).unwrap_or("").to_string();
        let port = cmd.arg_int(off + 2).unwrap_or(0) as u16;
        let local_port = if kind == LinkKind::Udp { cmd.arg_int(off + 3).unwrap_or(0) as u16 } else { 0 };
        if net.ip_of(self.station).is_none() {
            return self.reply(now, b"no ip\r\n\r\nERROR\r\n");
        }
        match net.dns_resolve(self.station, &host, now) {
            Ok(q) => self.busy = Some(Busy::Start { link, kind, port, local_port, host, query: Some(q), sock: None }),
            Err(_) => self.reply(now, b"DNS Fail\r\n\r\nERROR\r\n"),
        }
    }

    fn cipsend(&mut self, net: &mut Network, cmd: &AtCommand, now: SimTime) {
        let (link, len) = if self.mux { (cmd.arg_int(0).unwrap_or(-1), cmd.arg_int(1).unwrap_or(0)) } else { (0, cmd.arg_int(0).unwrap_or(0)) };
        if !(0..5).contains(&link) || !(1..=2048).contains(&len) {
            return self.reply(now, b"\r\nERROR\r\n");
        }
        let link = link as usize;
        let valid = self.links[link].as_ref().is_some_and(|l| l.kind == LinkKind::Udp || net.tcp_state(l.sock) == TcpState::Established);
        if !valid {
            return self.reply(now, b"link is not valid\r\n\r\nERROR\r\n");
        }
        self.reply(now, b"\r\nOK\r\n> ");
        self.busy = Some(Busy::SendData { link, len: len as usize, buf: Vec::with_capacity(len as usize) });
    }

    fn finish_send(&mut self, net: &mut Network, link: usize, data: Vec<u8>, now: SimTime) {
        let t = format!("\r\nRecv {} bytes\r\n", data.len());
        self.reply(now, t.as_bytes());
        let Some(l) = self.links[link].as_mut() else {
            self.busy = None;
            return self.reply(now, b"\r\nSEND FAIL\r\n");
        };
        let res = match l.kind {
            LinkKind::Tcp => net.tcp_send(l.sock, &data, now).map(|_| ()),
            LinkKind::Udp => net.udp_send(l.sock, l.remote, &data, now),
        };
        match (res, l.kind) {
            (Ok(()), LinkKind::Tcp) => {
                l.awaiting_ack += data.len();
                self.busy = Some(Busy::SendWait { link });
            }
            (Ok(()), LinkKind::Udp) => {
                self.busy = Some(Busy::SendWait { link });
                self.timer(now + SimTime::from_ms(2), Timer::SendOk(link));
            }
            (Err(_), _) => {
                self.busy = None;
                self.reply(now, b"\r\nSEND FAIL\r\n");
            }
        }
    }

    fn mqtt_conn(&mut self, net: &mut Network, cmd: &AtCommand, now: SimTime) {
        let (Some(host), Some(port)) = (cmd.arg_str(1).map(|x| x.to_string()), cmd.arg_int(2)) else {
            return self.reply(now, b"\r\nERROR\r\n");
        };
        let Some(client) = self.mqtt_cfg.clone() else { return self.reply(now, b"\r\nERROR\r\n") };
        if self.mqtt.as_ref().is_some_and(|m| m.client.state() == ClientState::Connected) {
            return self.reply(now, b"\r\nERROR\r\n");
        }
        let reconnect = cmd.arg_int(3).unwrap_or(0) == 1;
        self.mqtt = Some(MqttLink { client, scheme: 1, host: host.clone(), port: port as u16, reconnect, sock: None, subs: vec![] });
        match net.dns_resolve(self.station, &host, now) {
            Ok(q) => self.busy = Some(Busy::MqttConn { query: Some(q) }),
            Err(_) => self.reply(now, b"\r\nERROR\r\n"),
        }
    }

    fn sntp_request(&mut self, net: &mut Network, now: SimTime) {
        if !self.sntp.enabled {
            return;
        }
        if net.ip_of(self.station).is_none() {
            self.timer(now + SimTime::from_secs(1), Timer::SntpPoll);
            return;
        }
        if let Some(server) = self.sntp.servers.first().cloned()
            && let Ok(q) = net.dns_resolve(self.station, &server, now)
        {
            self.sntp.query = Some(q);
        }
        // Re-sync hourly.
        self.timer(now + SimTime::from_secs(3600), Timer::SntpPoll);
    }

    // ================================================================ network events

    fn link_of(&self, sock: SockId) -> Option<usize> {
        self.links.iter().position(|l| l.as_ref().is_some_and(|l| l.sock == sock))
    }

    fn on_net(&mut self, net: &mut Network, e: NetEvent, now: SimTime) {
        let st = self.station;
        match e {
            NetEvent::WifiConnected { .. } => self.emit_at(now + EVT, b"WIFI CONNECTED\r\n"),
            NetEvent::GotIp { ip, .. } => {
                self.emit_at(now + EVT, b"WIFI GOT IP\r\n");
                if let Some(Busy::Join { ssid }) = self.busy.clone() {
                    self.busy = None;
                    self.emit_at(now + EVT, b"\r\nOK\r\n");
                    let rssi = net.station_info(st).and_then(|i| i.link).map(|l| l.rssi_dbm.round() as i32).unwrap_or(0);
                    self.note(now, AnnotationKind::Wifi, format!("AT+CWJAP=\"{ssid}\" → OK rssi={rssi} dBm ip={ip}"));
                }
                if self.sntp.enabled {
                    self.timer(now + SimTime::from_ms(20), Timer::SntpPoll);
                }
            }
            NetEvent::WifiJoinFailed { ssid, reason } => {
                if let Some(Busy::Join { .. }) = self.busy {
                    self.busy = None;
                    let t = format!("+CWJAP:{}\r\n\r\nFAIL\r\n", reason.esp_code());
                    self.emit_at(now + EVT, t.as_bytes());
                    self.note(now, AnnotationKind::Wifi, format!("AT+CWJAP=\"{ssid}\" → FAIL ({})", reason.text()));
                }
            }
            NetEvent::WifiDisconnected { .. } => {
                self.emit_at(now + EVT, b"WIFI DISCONNECT\r\n");
                // Links die with the association (TcpClosed events follow).
            }
            NetEvent::DnsResolved { query, name, ip } => self.on_dns(net, query, &name, ip, now),
            NetEvent::TcpConnected { sock } => {
                if let Some(Busy::Start { link, sock: Some(s), port, host, .. }) = self.busy.clone()
                    && s == sock
                {
                    self.busy = None;
                    let remote = net.tcp_addrs(sock).map(|a| a.1).unwrap_or(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
                    let local_port = net.tcp_addrs(sock).map(|a| a.0.port()).unwrap_or(0);
                    self.links[link] = Some(Link { kind: LinkKind::Tcp, sock, remote, local_port, server: false, awaiting_ack: 0 });
                    let t = format!("{}CONNECT\r\n\r\nOK\r\n", self.link_prefix(link));
                    self.emit_at(now + EVT, t.as_bytes());
                    self.note(now, AnnotationKind::Net, format!("AT+CIPSTART TCP {host}:{port} → CONNECT"));
                    return;
                }
                if let Some(m) = self.mqtt.as_mut()
                    && m.sock == Some(sock)
                {
                    let bytes = m.client.connect(now);
                    let _ = net.tcp_send(sock, &bytes, now);
                }
            }
            NetEvent::TcpConnectFailed { sock, .. } => {
                if let Some(Busy::Start { sock: Some(s), .. }) = self.busy
                    && s == sock
                {
                    self.busy = None;
                    self.emit_at(now + EVT, b"ERROR\r\nCLOSED\r\n");
                    net.close(sock, now);
                    return;
                }
                if self.mqtt.as_ref().is_some_and(|m| m.sock == Some(sock)) {
                    self.mqtt_failed(net, now);
                }
            }
            NetEvent::TcpData { sock, data } => {
                if let Some(i) = self.link_of(sock) {
                    let mut t = format!("\r\n+IPD,{}{}:", self.link_prefix(i), data.len()).into_bytes();
                    t.extend_from_slice(&data);
                    self.emit_at(now + EVT, &t);
                    return;
                }
                if let Some(m) = self.mqtt.as_mut()
                    && m.sock == Some(sock)
                {
                    let (events, reply) = m.client.input(&data);
                    if !reply.is_empty() {
                        let _ = net.tcp_send(sock, &reply, now);
                    }
                    for ev in events {
                        self.on_mqtt(ev, now);
                    }
                }
            }
            NetEvent::TcpAcked { sock, bytes } => {
                if let Some(i) = self.link_of(sock)
                    && let Some(l) = self.links[i].as_mut()
                {
                    l.awaiting_ack = l.awaiting_ack.saturating_sub(bytes);
                    if l.awaiting_ack == 0 && self.busy == Some(Busy::SendWait { link: i }) {
                        self.busy = None;
                        self.emit_at(now + EVT, b"\r\nSEND OK\r\n");
                    }
                }
            }
            NetEvent::TcpClosed { sock, reason } => {
                if let Some(i) = self.link_of(sock) {
                    self.links[i] = None;
                    net.close(sock, now);
                    if self.busy == Some(Busy::SendWait { link: i }) {
                        self.busy = None;
                        self.emit_at(now + EVT, b"\r\nSEND FAIL\r\n");
                    }
                    if matches!(self.busy, Some(Busy::SendData { link, .. }) if link == i) {
                        self.busy = None;
                    }
                    let t = format!("{}CLOSED\r\n", self.link_prefix(i));
                    self.emit_at(now + EVT, t.as_bytes());
                    let why = match reason {
                        CloseReason::PeerClosed => "closed by peer",
                        CloseReason::Reset => "reset",
                        CloseReason::Timeout => "timeout",
                        CloseReason::LinkLost => "Wi-Fi lost",
                        _ => "closed",
                    };
                    self.note(now, AnnotationKind::Net, format!("link {i} CLOSED ({why})"));
                    return;
                }
                if self.mqtt.as_ref().is_some_and(|m| m.sock == Some(sock)) {
                    net.close(sock, now);
                    self.mqtt_failed(net, now);
                }
            }
            NetEvent::TcpAccepted { sock, remote, .. } => {
                match self.links.iter().position(|l| l.is_none()) {
                    Some(i) => {
                        let local_port = net.tcp_addrs(sock).map(|a| a.0.port()).unwrap_or(0);
                        self.links[i] = Some(Link { kind: LinkKind::Tcp, sock, remote, local_port, server: true, awaiting_ack: 0 });
                        let t = format!("{i},CONNECT\r\n");
                        self.emit_at(now + EVT, t.as_bytes());
                    }
                    None => net.tcp_abort(sock, now),
                }
            }
            NetEvent::UdpData { sock, from, data } => {
                if self.sntp.sock == Some(sock) {
                    if let Some(u) = crate::wire::ntp_reply_time(&data) {
                        self.sntp.base = Some((now, u));
                    }
                    return;
                }
                if let Some(i) = self.link_of(sock) {
                    if let Some(l) = self.links[i].as_mut() {
                        l.remote = from;
                    }
                    let mut t = format!("\r\n+IPD,{}{}:", self.link_prefix(i), data.len()).into_bytes();
                    t.extend_from_slice(&data);
                    self.emit_at(now + EVT, &t);
                }
            }
            NetEvent::PingReply { ping, rtt } => {
                if let Some(Busy::Ping { ping: Some(p), .. }) = self.busy
                    && p == ping
                {
                    self.busy = None;
                    let t = format!("+PING:{}\r\n\r\nOK\r\n", rtt.as_millis().max(1));
                    self.emit_at(now + EVT, t.as_bytes());
                }
            }
            NetEvent::PingTimeout { ping } => {
                if let Some(Busy::Ping { ping: Some(p), .. }) = self.busy
                    && p == ping
                {
                    self.busy = None;
                    self.emit_at(now + EVT, b"+PING:TIMEOUT\r\n\r\nERROR\r\n");
                }
            }
        }
    }

    fn on_dns(&mut self, net: &mut Network, query: u32, name: &str, ip: Option<Ipv4Addr>, now: SimTime) {
        let st = self.station;
        if self.sntp.query == Some(query) {
            self.sntp.query = None;
            if let Some(ip) = ip {
                if self.sntp.sock.is_none() {
                    self.sntp.sock = net.udp_bind(st, 0).ok();
                }
                if let Some(s) = self.sntp.sock {
                    let _ = net.udp_send(s, SocketAddrV4::new(ip, 123), &crate::wire::ntp_request(0.0), now);
                }
            }
            return;
        }
        match self.busy.clone() {
            Some(Busy::Start { link, kind, port, local_port, host, query: Some(q), .. }) if q == query => {
                let Some(ip) = ip else {
                    self.busy = None;
                    return self.emit_at(now + EVT, b"DNS Fail\r\n\r\nERROR\r\n");
                };
                let dst = SocketAddrV4::new(ip, port);
                match kind {
                    LinkKind::Tcp => match net.tcp_connect(st, dst, now) {
                        Ok(sock) => self.busy = Some(Busy::Start { link, kind, port, local_port, host, query: None, sock: Some(sock) }),
                        Err(_) => {
                            self.busy = None;
                            self.emit_at(now + EVT, b"ERROR\r\nCLOSED\r\n");
                        }
                    },
                    LinkKind::Udp => match net.udp_bind(st, local_port) {
                        Ok(sock) => {
                            self.busy = None;
                            let lp = net.udp_local_port(sock).unwrap_or(local_port);
                            self.links[link] = Some(Link { kind, sock, remote: dst, local_port: lp, server: false, awaiting_ack: 0 });
                            let t = format!("{}CONNECT\r\n\r\nOK\r\n", self.link_prefix(link));
                            self.emit_at(now + EVT, t.as_bytes());
                        }
                        Err(_) => {
                            self.busy = None;
                            self.emit_at(now + EVT, b"\r\nERROR\r\n");
                        }
                    },
                }
            }
            Some(Busy::Ping { host, query: Some(q), .. }) if q == query => match ip {
                Some(ip) => match net.ping(st, ip, 32, SimTime::from_secs(2), now) {
                    Ok(p) => self.busy = Some(Busy::Ping { host, query: None, ping: Some(p) }),
                    Err(_) => {
                        self.busy = None;
                        self.emit_at(now + EVT, b"+PING:TIMEOUT\r\n\r\nERROR\r\n");
                    }
                },
                None => {
                    self.busy = None;
                    self.emit_at(now + EVT, b"+PING:TIMEOUT\r\n\r\nERROR\r\n");
                }
            },
            Some(Busy::Domain { query: q }) if q == query => {
                self.busy = None;
                match ip {
                    Some(ip) => {
                        let t = format!("+CIPDOMAIN:\"{ip}\"\r\n\r\nOK\r\n");
                        self.emit_at(now + EVT, t.as_bytes());
                    }
                    None => self.emit_at(now + EVT, b"DNS Fail\r\n\r\nERROR\r\n"),
                }
                let _ = name;
            }
            Some(Busy::MqttConn { query: Some(q) }) if q == query => {
                let Some(ip) = ip else {
                    self.busy = None;
                    self.mqtt = None;
                    return self.emit_at(now + EVT, b"\r\nERROR\r\n");
                };
                let port = self.mqtt.as_ref().map(|m| m.port).unwrap_or(1883);
                match net.tcp_connect(st, SocketAddrV4::new(ip, port), now) {
                    Ok(sock) => {
                        if let Some(m) = self.mqtt.as_mut() {
                            m.sock = Some(sock);
                        }
                        self.busy = Some(Busy::MqttConn { query: None });
                    }
                    Err(_) => {
                        self.busy = None;
                        self.mqtt = None;
                        self.emit_at(now + EVT, b"\r\nERROR\r\n");
                    }
                }
            }
            _ => {}
        }
    }

    fn on_mqtt(&mut self, ev: ClientEvent, now: SimTime) {
        match ev {
            ClientEvent::Connected { .. } => {
                let (host, port, rc) = self.mqtt.as_ref().map(|m| (m.host.clone(), m.port, m.reconnect as u8)).unwrap_or_default();
                let t = format!("+MQTTCONNECTED:0,1,\"{host}\",\"{port}\",\"\",{rc}\r\n");
                self.emit_at(now + EVT, t.as_bytes());
                if self.busy == Some(Busy::MqttConn { query: None }) {
                    self.busy = None;
                    self.emit_at(now + EVT, b"\r\nOK\r\n");
                }
            }
            ClientEvent::Refused(_) => {
                if self.busy == Some(Busy::MqttConn { query: None }) {
                    self.busy = None;
                    self.emit_at(now + EVT, b"\r\nERROR\r\n");
                }
            }
            ClientEvent::Message { topic, payload, .. } => {
                let mut t = format!("+MQTTSUBRECV:0,\"{topic}\",{},", payload.len()).into_bytes();
                t.extend_from_slice(&payload);
                t.extend_from_slice(b"\r\n");
                self.emit_at(now + EVT, &t);
            }
            _ => {}
        }
    }

    fn mqtt_failed(&mut self, net: &mut Network, now: SimTime) {
        let was_connected = self.mqtt.as_ref().is_some_and(|m| m.client.state() == ClientState::Connected);
        if self.busy == Some(Busy::MqttConn { query: None }) {
            self.busy = None;
            self.emit_at(now + EVT, b"\r\nERROR\r\n");
        }
        if let Some(m) = self.mqtt.as_mut() {
            m.client.transport_closed();
            m.sock = None;
        }
        if was_connected {
            self.emit_at(now + EVT, b"+MQTTDISCONNECTED:0\r\n");
        }
        let _ = net;
    }
}

/// `Thu Aug 04 14:48:05 2016` (ESP-AT's zero-padded asctime).
fn esp_time(unix: i64) -> String {
    const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MO: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, m, d, h, mi, s, wd) = crate::wire::civil_from_unix(unix);
    format!("{} {} {:02} {:02}:{:02}:{:02} {}", WD[wd as usize], MO[m as usize - 1], d, h, mi, s, y)
}
