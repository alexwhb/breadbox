//! Radio services for host-emulated boards (ESP32 under `bb-firmware`'s `HostBoard`, `MCU-6`).
//!
//! The board's child process asks for Wi-Fi / socket / BLE operations through opaque
//! *service requests* (channel + bytes). `bb-firmware` only transports them; the co-simulator
//! hands each one to [`crate::space::RadioSpace::host_service`] and returns the reply. The
//! byte format is defined here (and mirrored by `crates/bb-firmware/hostcore/bb_host.h`), so
//! neither crate depends on the other. Every operation is non-blocking; Arduino's blocking
//! semantics (`WiFiClient::connect` waiting for the handshake, `delay` while DNS resolves, …)
//! are implemented in the C++ core by polling with simulated time passing.
//!
//! Encoding: little-endian integers, `str`/`bytes` = u16 length + data, IPv4 = u32 in network
//! order packed big-endian into the LE u32 (i.e. `a.b.c.d` → bytes `a b c d`).

use crate::ble::{BleSpace, PeripheralEvent, PeripheralId, Uuid};
use crate::net::{AnnotationKind, JoinError, NetEvent, Network, SockId, StationId, TcpState};
use bb_core::SimTime;
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};

/// Service channels.
pub const CH_NET: u8 = 1;
pub const CH_BLE: u8 = 2;

/// Network opcodes.
pub mod net_op {
    pub const WIFI_BEGIN: u8 = 0x01;
    pub const WIFI_STATUS: u8 = 0x02;
    pub const WIFI_DISCONNECT: u8 = 0x03;
    pub const WIFI_SCAN: u8 = 0x04;
    pub const WIFI_MAC: u8 = 0x05;
    pub const WIFI_CONFIG: u8 = 0x06;
    pub const WIFI_HOSTNAME: u8 = 0x07;
    pub const DNS_START: u8 = 0x10;
    pub const DNS_RESULT: u8 = 0x11;
    pub const TCP_CONNECT: u8 = 0x20;
    pub const TCP_STATUS: u8 = 0x21;
    pub const TCP_SEND: u8 = 0x22;
    pub const TCP_RECV: u8 = 0x23;
    pub const TCP_CLOSE: u8 = 0x24;
    pub const TCP_LISTEN: u8 = 0x25;
    pub const TCP_ACCEPT: u8 = 0x26;
    pub const TCP_PEER: u8 = 0x27;
    pub const UDP_BIND: u8 = 0x30;
    pub const UDP_SENDTO: u8 = 0x31;
    pub const UDP_RECV: u8 = 0x32;
    pub const UDP_CLOSE: u8 = 0x33;
}

/// BLE opcodes.
pub mod ble_op {
    pub const INIT: u8 = 0x01;
    pub const ADD_SERVICE: u8 = 0x02;
    pub const ADD_CHAR: u8 = 0x03;
    pub const SET_VALUE: u8 = 0x04;
    pub const GET_VALUE: u8 = 0x05;
    pub const NOTIFY: u8 = 0x06;
    pub const ADVERTISE: u8 = 0x07;
    pub const POLL: u8 = 0x08;
    pub const CONNECTED: u8 = 0x09;
    pub const DISCONNECT: u8 = 0x0a;
}

/// Arduino `wl_status_t`.
pub mod wl {
    pub const IDLE: u8 = 0;
    pub const NO_SSID_AVAIL: u8 = 1;
    pub const CONNECTED: u8 = 3;
    pub const CONNECT_FAILED: u8 = 4;
    pub const DISCONNECTED: u8 = 6;
}

/// Little-endian reader for requests.
pub struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, i: 0 }
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
    pub fn i32(&mut self) -> i32 {
        self.u32() as i32
    }
    pub fn ip(&mut self) -> Ipv4Addr {
        Ipv4Addr::new(self.u8(), self.u8(), self.u8(), self.u8())
    }
    pub fn bytes(&mut self) -> Vec<u8> {
        let n = self.u16() as usize;
        let end = (self.i + n).min(self.b.len());
        let v = self.b[self.i.min(end)..end].to_vec();
        self.i = end;
        v
    }
    pub fn str(&mut self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }
}

/// Little-endian writer for replies.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.u32(v as u32)
    }
    pub fn ip(&mut self, ip: Ipv4Addr) -> &mut Self {
        self.0.extend_from_slice(&ip.octets());
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        let n = b.len().min(u16::MAX as usize);
        self.u16(n as u16);
        self.0.extend_from_slice(&b[..n]);
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    pub fn done(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

#[derive(Debug, Clone)]
enum HSock {
    Tcp { sock: SockId, failed: bool, closed: bool, rx: VecDeque<u8> },
    Listener { sock: SockId, pending: VecDeque<(SockId, SocketAddrV4)> },
    Udp { sock: SockId, rx: VecDeque<(SocketAddrV4, Vec<u8>)> },
}

/// Wi-Fi + BLE radio of a host-emulated board.
#[derive(Debug, Clone)]
pub struct HostRadio {
    pub label: String,
    pub station: StationId,
    pub peripheral: PeripheralId,
    socks: HashMap<i32, HSock>,
    by_sock: HashMap<SockId, i32>,
    next_handle: i32,
    dns: HashMap<i32, Option<Option<Ipv4Addr>>>,
    status: u8,
    ssid: String,
    ble_events: VecDeque<PeripheralEvent>,
    ble_services: Vec<usize>,
    notes: Vec<(SimTime, AnnotationKind, String)>,
}

impl HostRadio {
    pub fn new(label: &str, station: StationId, peripheral: PeripheralId) -> Self {
        HostRadio {
            label: label.into(),
            station,
            peripheral,
            socks: HashMap::new(),
            by_sock: HashMap::new(),
            next_handle: 1,
            dns: HashMap::new(),
            status: wl::IDLE,
            ssid: String::new(),
            ble_events: VecDeque::new(),
            ble_services: vec![],
            notes: vec![],
        }
    }

    pub fn next_wake(&self) -> Option<SimTime> {
        None
    }

    pub fn take_notes(&mut self) -> Vec<(SimTime, AnnotationKind, String)> {
        std::mem::take(&mut self.notes)
    }

    /// Arduino `WiFi.status()`.
    pub fn wifi_status(&self) -> u8 {
        self.status
    }

    fn handle(&mut self, s: HSock) -> i32 {
        let h = self.next_handle;
        self.next_handle += 1;
        let sock = match &s {
            HSock::Tcp { sock, .. } | HSock::Listener { sock, .. } | HSock::Udp { sock, .. } => *sock,
        };
        self.by_sock.insert(sock, h);
        self.socks.insert(h, s);
        h
    }

    /// Consumes network + BLE events for this board.
    pub fn poll(&mut self, net: &mut Network, ble: &mut BleSpace, now: SimTime) {
        for e in net.take_events(self.station) {
            match e {
                NetEvent::WifiConnected { .. } => {}
                NetEvent::GotIp { ip, .. } => {
                    self.status = wl::CONNECTED;
                    let rssi = net.station_info(self.station).and_then(|i| i.link).map(|l| l.rssi_dbm.round() as i32).unwrap_or(0);
                    self.notes.push((now, AnnotationKind::Wifi, format!("WiFi.begin(\"{}\") → connected rssi={rssi} dBm ip={ip}", self.ssid)));
                }
                NetEvent::WifiJoinFailed { reason, ssid } => {
                    self.status = if reason == JoinError::NoAp { wl::NO_SSID_AVAIL } else { wl::CONNECT_FAILED };
                    self.notes.push((now, AnnotationKind::Wifi, format!("WiFi.begin(\"{ssid}\") → failed ({})", reason.text())));
                }
                NetEvent::WifiDisconnected { .. } => self.status = wl::DISCONNECTED,
                NetEvent::TcpConnected { .. } => {}
                NetEvent::TcpConnectFailed { sock, .. } => {
                    if let Some(HSock::Tcp { failed, .. }) = self.by_sock.get(&sock).and_then(|h| self.socks.get_mut(h)) {
                        *failed = true;
                    }
                }
                NetEvent::TcpData { sock, data } => {
                    if let Some(HSock::Tcp { rx, .. }) = self.by_sock.get(&sock).and_then(|h| self.socks.get_mut(h)) {
                        rx.extend(data);
                    }
                }
                NetEvent::TcpClosed { sock, .. } => {
                    if let Some(HSock::Tcp { closed, .. }) = self.by_sock.get(&sock).and_then(|h| self.socks.get_mut(h)) {
                        *closed = true;
                    }
                }
                NetEvent::TcpAccepted { listener, sock, remote } => {
                    if let Some(HSock::Listener { pending, .. }) = self.by_sock.get(&listener).and_then(|h| self.socks.get_mut(h)) {
                        pending.push_back((sock, remote));
                    } else {
                        net.tcp_abort(sock, now);
                    }
                }
                NetEvent::UdpData { sock, from, data } => {
                    if let Some(HSock::Udp { rx, .. }) = self.by_sock.get(&sock).and_then(|h| self.socks.get_mut(h)) {
                        rx.push_back((from, data));
                    }
                }
                NetEvent::DnsResolved { query, ip, .. } => {
                    self.dns.insert(query as i32, Some(ip));
                }

                NetEvent::TcpAcked { .. } | NetEvent::PingReply { .. } | NetEvent::PingTimeout { .. } => {}
            }
        }
        let evs = ble.take_peripheral_events(self.peripheral);
        for e in &evs {
            match e {
                PeripheralEvent::Connected => self.notes.push((now, AnnotationKind::Ble, format!("phone connected to {} (ESP32 BLE)", self.label))),
                PeripheralEvent::Disconnected => {
                    self.notes.push((now, AnnotationKind::Ble, format!("phone disconnected from {}", self.label)));
                    // BLEServer keeps the old link; sketches restart advertising themselves.
                }
                _ => {}
            }
        }
        self.ble_events.extend(evs);
    }

    /// Serves one request (see the module docs for the encoding).
    pub fn serve(&mut self, net: &mut Network, ble: &mut BleSpace, channel: u8, req: &[u8], now: SimTime) -> Vec<u8> {
        let mut r = Reader::new(req);
        let op = r.u8();
        let mut w = Writer::default();
        match channel {
            CH_NET => self.serve_net(net, op, &mut r, &mut w, now),
            CH_BLE => self.serve_ble(ble, op, &mut r, &mut w, now),
            _ => {
                w.i32(-1);
            }
        }
        w.done()
    }

    fn serve_net(&mut self, net: &mut Network, op: u8, r: &mut Reader, w: &mut Writer, now: SimTime) {
        use net_op::*;
        let st = self.station;
        match op {
            WIFI_BEGIN => {
                let ssid = r.str();
                let pass = r.str();
                self.ssid = ssid.clone();
                self.status = wl::DISCONNECTED;
                let p = (!pass.is_empty()).then_some(pass.as_str());
                let ok = net.join(st, &ssid, p, now).is_ok();
                w.i32(if ok { 0 } else { -1 });
            }
            WIFI_STATUS => {
                let info = net.station_info(st);
                let ip = net.ip_of(st).unwrap_or(Ipv4Addr::UNSPECIFIED);
                let has_ip = net.ip_of(st).is_some();
                w.u8(self.status);
                w.ip(ip);
                w.ip(if has_ip { net.gateway() } else { Ipv4Addr::UNSPECIFIED });
                w.ip(if has_ip { net.config.netmask } else { Ipv4Addr::UNSPECIFIED });
                w.ip(if has_ip { net.gateway() } else { Ipv4Addr::UNSPECIFIED });
                let link = info.as_ref().and_then(|i| i.link);
                w.i32(link.map(|l| l.rssi_dbm.round() as i32).unwrap_or(0));
                w.u8(info.as_ref().and_then(|i| i.channel).unwrap_or(0));
                let bssid = net.access_points().iter().find(|a| info.as_ref().and_then(|i| i.ssid.as_ref()) == Some(&a.ssid)).map(|a| a.bssid).unwrap_or_default();
                w.0.extend_from_slice(&bssid);
                w.str(info.and_then(|i| i.ssid).as_deref().unwrap_or(""));
            }
            WIFI_DISCONNECT => {
                let _ = net.leave(st, now);
                self.status = wl::DISCONNECTED;
                w.i32(0);
            }
            WIFI_SCAN => {
                let aps = net.scan(st);
                w.u8(aps.len().min(255) as u8);
                for a in aps.iter().take(255) {
                    w.str(&a.ssid).i32(a.rssi).u8(a.channel).u8(a.secure as u8);
                    w.0.extend_from_slice(&a.bssid);
                }
            }
            WIFI_MAC => {
                let mac = net.station_info(st).map(|i| i.mac).unwrap_or_default();
                w.0.extend_from_slice(&mac);
            }
            WIFI_CONFIG => {
                let (ip, gw, mask) = (r.ip(), r.ip(), r.ip());
                if ip.is_unspecified() {
                    net.set_static_ip(st, None);
                } else {
                    net.set_static_ip(st, Some((ip, gw, mask)));
                }
                w.i32(0);
            }
            WIFI_HOSTNAME => {
                let h = r.str();
                net.set_hostname(st, &h);
                w.i32(0);
            }
            DNS_START => {
                let name = r.str();
                match net.dns_resolve(st, &name, now) {
                    Ok(q) => {
                        // Answered by a later poll (literal addresses on the very next one).
                        self.dns.entry(q as i32).or_insert(None);
                        w.i32(q as i32);
                    }
                    Err(_) => {
                        w.i32(-1);
                    }
                }
            }
            DNS_RESULT => {
                let q = r.i32();
                match self.dns.get(&q) {
                    None => {
                        w.u8(2).ip(Ipv4Addr::UNSPECIFIED);
                    }
                    Some(None) => {
                        w.u8(0).ip(Ipv4Addr::UNSPECIFIED);
                    }
                    Some(Some(ip)) => {
                        let ip = *ip;
                        self.dns.remove(&q);
                        match ip {
                            Some(ip) => w.u8(1).ip(ip),
                            None => w.u8(2).ip(Ipv4Addr::UNSPECIFIED),
                        };
                    }
                }
            }
            TCP_CONNECT => {
                let ip = r.ip();
                let port = r.u16();
                match net.tcp_connect(st, SocketAddrV4::new(ip, port), now) {
                    Ok(sock) => {
                        let h = self.handle(HSock::Tcp { sock, failed: false, closed: false, rx: VecDeque::new() });
                        w.i32(h);
                    }
                    Err(_) => {
                        w.i32(-1);
                    }
                }
            }
            TCP_STATUS => {
                let h = r.i32();
                match self.socks.get(&h) {
                    Some(HSock::Tcp { sock, failed, closed, rx }) => {
                        let state = if *failed {
                            3
                        } else if *closed {
                            2
                        } else {
                            match net.tcp_state(*sock) {
                                TcpState::Connecting => 0,
                                TcpState::Established => 1,
                                TcpState::Closed => 2,
                            }
                        };
                        w.u8(state).u32(rx.len() as u32);
                    }
                    _ => {
                        w.u8(3).u32(0);
                    }
                }
            }
            TCP_SEND => {
                let h = r.i32();
                let data = r.bytes();
                let n = match self.socks.get(&h) {
                    Some(HSock::Tcp { sock, .. }) => net.tcp_send(*sock, &data, now).map(|n| n as i32).unwrap_or(-1),
                    _ => -1,
                };
                w.i32(n);
            }
            TCP_RECV => {
                let h = r.i32();
                let max = r.u32() as usize;
                let data: Vec<u8> = match self.socks.get_mut(&h) {
                    Some(HSock::Tcp { rx, .. }) => {
                        let n = max.min(rx.len()).min(60_000);
                        rx.drain(..n).collect()
                    }
                    _ => vec![],
                };
                w.bytes(&data);
            }
            TCP_CLOSE | UDP_CLOSE => {
                let h = r.i32();
                if let Some(s) = self.socks.remove(&h) {
                    let sock = match s {
                        HSock::Tcp { sock, .. } | HSock::Listener { sock, .. } | HSock::Udp { sock, .. } => sock,
                    };
                    self.by_sock.remove(&sock);
                    net.close(sock, now);
                }
                w.i32(0);
            }
            TCP_LISTEN => {
                let port = r.u16();
                match net.tcp_listen(st, port) {
                    Ok(sock) => {
                        let h = self.handle(HSock::Listener { sock, pending: VecDeque::new() });
                        w.i32(h);
                    }
                    Err(_) => {
                        w.i32(-1);
                    }
                }
            }
            TCP_ACCEPT => {
                let h = r.i32();
                let next = match self.socks.get_mut(&h) {
                    Some(HSock::Listener { pending, .. }) => pending.pop_front(),
                    _ => None,
                };
                match next {
                    Some((sock, remote)) => {
                        let nh = self.handle(HSock::Tcp { sock, failed: false, closed: false, rx: VecDeque::new() });
                        w.i32(nh).ip(*remote.ip()).u16(remote.port());
                    }
                    None => {
                        w.i32(-1).ip(Ipv4Addr::UNSPECIFIED).u16(0);
                    }
                }
            }
            TCP_PEER => {
                let h = r.i32();
                let addrs = match self.socks.get(&h) {
                    Some(HSock::Tcp { sock, .. }) => net.tcp_addrs(*sock),
                    _ => None,
                };
                let (l, rem) = addrs.unwrap_or((SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)));
                w.ip(*rem.ip()).u16(rem.port()).ip(*l.ip()).u16(l.port());
            }
            UDP_BIND => {
                let port = r.u16();
                match net.udp_bind(st, port) {
                    Ok(sock) => {
                        let h = self.handle(HSock::Udp { sock, rx: VecDeque::new() });
                        w.i32(h);
                    }
                    Err(_) => {
                        w.i32(-1);
                    }
                }
            }
            UDP_SENDTO => {
                let h = r.i32();
                let ip = r.ip();
                let port = r.u16();
                let data = r.bytes();
                let ok = match self.socks.get(&h) {
                    Some(HSock::Udp { sock, .. }) => net.udp_send(*sock, SocketAddrV4::new(ip, port), &data, now).is_ok(),
                    _ => false,
                };
                w.i32(if ok { data.len() as i32 } else { -1 });
            }
            UDP_RECV => {
                let h = r.i32();
                let next = match self.socks.get_mut(&h) {
                    Some(HSock::Udp { rx, .. }) => rx.pop_front(),
                    _ => None,
                };
                match next {
                    Some((from, data)) => {
                        w.u8(1).ip(*from.ip()).u16(from.port()).bytes(&data);
                    }
                    None => {
                        w.u8(0).ip(Ipv4Addr::UNSPECIFIED).u16(0).bytes(&[]);
                    }
                }
            }
            _ => {
                w.i32(-1);
            }
        }
    }

    fn serve_ble(&mut self, ble: &mut BleSpace, op: u8, r: &mut Reader, w: &mut Writer, now: SimTime) {
        use ble_op::*;
        let p = self.peripheral;
        match op {
            INIT => {
                let name = r.str();
                ble.set_name(p, &name);
                ble.set_powered(p, true, now);
                ble.set_tx_power(p, 3.0);
                w.i32(0);
            }
            ADD_SERVICE => {
                let uuid = Uuid::parse(&r.str()).unwrap_or(Uuid::U16(0));
                let idx = ble.gatt_mut(p).map(|g| g.add_service(uuid)).unwrap_or(0);
                self.ble_services.push(idx);
                w.i32(idx as i32);
            }
            ADD_CHAR => {
                let svc = r.i32().max(0) as usize;
                let uuid = Uuid::parse(&r.str()).unwrap_or(Uuid::U16(0));
                let props = r.u8();
                let h = ble.gatt_mut(p).map(|g| g.add_characteristic(svc, uuid, props, b"")).unwrap_or(0);
                w.i32(h as i32);
            }
            SET_VALUE => {
                let h = r.u16();
                let v = r.bytes();
                ble.set_value(p, h, &v);
                w.i32(0);
            }
            GET_VALUE => {
                let h = r.u16();
                let v = ble.gatt(p).and_then(|g| g.char(h)).map(|c| c.value.clone()).unwrap_or_default();
                w.bytes(&v);
            }
            NOTIFY => {
                let h = r.u16();
                let v = r.bytes();
                let sent = ble.notify(p, h, &v, now);
                if sent {
                    self.notes.push((now, AnnotationKind::Ble, format!("{} → phone notify {} B", self.label, v.len())));
                }
                w.u8(sent as u8);
            }
            ADVERTISE => {
                let on = r.u8() != 0;
                ble.set_advertising(p, on);
                w.i32(0);
            }
            POLL => {
                let evs: Vec<PeripheralEvent> = self.ble_events.drain(..).collect();
                w.u8(evs.len().min(255) as u8);
                for e in evs.into_iter().take(255) {
                    match e {
                        PeripheralEvent::Connected => {
                            w.u8(1);
                        }
                        PeripheralEvent::Disconnected => {
                            w.u8(2);
                        }
                        PeripheralEvent::Write { handle, data } => {
                            w.u8(3).u16(handle).bytes(&data);
                        }
                        PeripheralEvent::Read { handle } => {
                            w.u8(4).u16(handle);
                        }
                        PeripheralEvent::Subscribed { handle, on } => {
                            w.u8(5).u16(handle).u8(on as u8);
                        }
                        PeripheralEvent::SppData(_) => {
                            w.u8(0);
                        }
                    }
                }
            }
            CONNECTED => {
                w.u8(ble.is_connected(p) as u8);
            }
            DISCONNECT => {
                ble.peripheral_disconnect(p, now);
                w.i32(0);
            }
            _ => {
                w.i32(-1);
            }
        }
    }
}
