//! The virtual Wi-Fi network (`RF-1`…`RF-4`): access points, station association with WPA2
//! password check and RSSI from the floor plan, and a socket-level IPv4 stack shared by every
//! radio on the bench — DHCP (real DISCOVER/OFFER/REQUEST/ACK exchange and renewals), DNS,
//! TCP (handshake, sequence numbers, cumulative ACKs, RTO retransmission with RTT estimation,
//! out-of-order reassembly, FIN/RST), UDP, ICMP echo, plus the gateway's services: the MQTT
//! broker on `10.0.0.1:1883`, an HTTP test server on `:80`, NTP from sim time, DNS and an echo
//! service on port 7.
//!
//! Every frame crosses the "air" between a station and its AP with the configured latency ±
//! jitter and loss (plus extra loss near the sensitivity limit), and is recorded in the
//! [`PacketLog`]. The fault injector can drop the AP at a given time (stations disconnect, their
//! TCP connections die, the broker notices through keep-alive and publishes wills; the AP comes
//! back after `ap_down_s` and stations with auto-reconnect rejoin).
//!
//! Time only moves through [`Network::advance`]; API calls take the caller's `now` (which must
//! not go backwards). Results arrive as [`NetEvent`]s per station.

mod services;
mod tcp;

use crate::geo::{Geometry, Link, PathLoss, Pos};
use crate::log::{Dir, DropReason, LogEntry, PacketLog, Proto};
use crate::mqtt::{Broker, BrokerEvent, BrokerOut};
use crate::rng::Pcg32;
use crate::wire;
use bb_core::SimTime;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use thiserror::Error;

pub use services::{HttpRequest, HttpResponse};
use services::{DhcpClient, DhcpServer, HttpSession};
use tcp::TcpConn;
pub use tcp::TcpState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StationId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SockId(pub u32);

pub type ConnId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Host {
    Gateway,
    Station(StationId),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetError {
    #[error("no such station")]
    NoStation,
    #[error("not connected to Wi-Fi / no IP address")]
    NoIp,
    #[error("no such socket")]
    NoSocket,
    #[error("port {0} already in use")]
    PortInUse(u16),
    #[error("socket is not connected")]
    NotConnected,
    #[error("invalid argument: {0}")]
    Invalid(String),
}

/// An access point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessPoint {
    pub ssid: String,
    /// WPA2-PSK passphrase; `None` = open network.
    pub password: Option<String>,
    pub channel: u8,
    pub pos: Pos,
    /// Transmit power (EIRP, dBm).
    pub tx_dbm: f64,
    pub bssid: [u8; 6],
    /// Administratively enabled (the fault injector takes it down separately).
    pub enabled: bool,
}

impl AccessPoint {
    /// The default `SIM-AP` / `breadbox` / channel 6 at the origin.
    pub fn sim_ap() -> Self {
        AccessPoint {
            ssid: "SIM-AP".into(),
            password: Some("breadbox".into()),
            channel: 6,
            pos: Pos::new(0.0, 0.0),
            tx_dbm: 16.0,
            bssid: [0x02, 0x00, 0x5e, 0x10, 0x00, 0x01],
            enabled: true,
        }
    }
}

/// Network-wide settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetConfig {
    pub gateway: Ipv4Addr,
    pub netmask: Ipv4Addr,
    /// DHCP pool, last octet range (default 42–254, so the first lease is `10.0.0.42`).
    pub pool_first: u8,
    pub pool_last: u8,
    pub lease_secs: u32,
    pub domain: String,
    /// Wall-clock Unix time at sim t = 0 (NTP, HTTP `/time`).
    pub epoch_unix: i64,
    pub seed: u64,
    /// Extra DNS names → addresses.
    pub hosts: Vec<(String, Ipv4Addr)>,
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            gateway: Ipv4Addr::new(10, 0, 0, 1),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            pool_first: 42,
            pool_last: 254,
            lease_secs: 3600,
            domain: "sim".into(),
            epoch_unix: 1_767_225_600, // 2026-01-01T00:00:00Z
            seed: 0x00b4_ead6_0b0c,
            hosts: vec![],
        }
    }
}

/// The network-conditions panel (`RF-3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetConditions {
    /// One-way air latency (ms).
    pub latency_ms: f64,
    /// Uniform jitter ± (ms).
    pub jitter_ms: f64,
    /// Random frame loss (%).
    pub loss_pct: f64,
    /// Take every AP down at this sim time (seconds)…
    pub drop_ap_at_s: Option<f64>,
    /// …for this long.
    pub ap_down_s: f64,
}

impl Default for NetConditions {
    fn default() -> Self {
        NetConditions { latency_ms: 3.0, jitter_ms: 1.0, loss_pct: 0.0, drop_ap_at_s: None, ap_down_s: 10.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinError {
    /// ESP-AT `+CWJAP:1`.
    Timeout,
    /// `+CWJAP:2`.
    WrongPassword,
    /// `+CWJAP:3`.
    NoAp,
}

impl JoinError {
    pub fn esp_code(self) -> u8 {
        match self {
            JoinError::Timeout => 1,
            JoinError::WrongPassword => 2,
            JoinError::NoAp => 3,
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            JoinError::Timeout => "timeout",
            JoinError::WrongPassword => "wrong password",
            JoinError::NoAp => "AP not found",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisconnectReason {
    ApDown,
    Left,
    OutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    /// The peer sent FIN.
    PeerClosed,
    /// RST received.
    Reset,
    /// Connection refused (nothing listening).
    Refused,
    /// Retransmissions exhausted.
    Timeout,
    /// The Wi-Fi link went away.
    LinkLost,
    /// Closed locally.
    Local,
}

/// Events delivered to a station's owner (modem, host-board adapter).
#[derive(Debug, Clone, PartialEq)]
pub enum NetEvent {
    WifiConnected { ssid: String, bssid: [u8; 6], channel: u8, rssi: i32 },
    WifiJoinFailed { ssid: String, reason: JoinError },
    GotIp { ip: Ipv4Addr, gateway: Ipv4Addr, netmask: Ipv4Addr, dns: Ipv4Addr },
    WifiDisconnected { reason: DisconnectReason },
    TcpConnected { sock: SockId },
    TcpConnectFailed { sock: SockId, reason: CloseReason },
    TcpData { sock: SockId, data: Vec<u8> },
    /// Bytes acknowledged by the peer.
    TcpAcked { sock: SockId, bytes: usize },
    TcpClosed { sock: SockId, reason: CloseReason },
    TcpAccepted { listener: SockId, sock: SockId, remote: SocketAddrV4 },
    UdpData { sock: SockId, from: SocketAddrV4, data: Vec<u8> },
    DnsResolved { query: u32, name: String, ip: Option<Ipv4Addr> },
    PingReply { ping: u32, rtt: SimTime },
    PingTimeout { ping: u32 },
}

/// Serial-monitor annotation produced by the network (`RF-8`).
#[derive(Debug, Clone, PartialEq)]
pub struct NetAnnotation {
    pub time: SimTime,
    pub station: StationId,
    pub kind: AnnotationKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnnotationKind {
    Wifi,
    Mqtt,
    Net,
    Ble,
    Lora,
}

impl AnnotationKind {
    pub fn prefix(self) -> &'static str {
        match self {
            AnnotationKind::Wifi => "wifi",
            AnnotationKind::Mqtt => "mqtt",
            AnnotationKind::Net => "net",
            AnnotationKind::Ble => "ble",
            AnnotationKind::Lora => "lora",
        }
    }
}

/// Snapshot of a station for the radio card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationInfo {
    pub label: String,
    pub mac: [u8; 6],
    pub hostname: String,
    pub pos: Pos,
    pub state: WifiStateKind,
    pub ssid: Option<String>,
    pub channel: Option<u8>,
    pub link: Option<Link>,
    pub ip: Option<Ipv4Addr>,
    pub dhcp: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WifiStateKind {
    Idle,
    Joining,
    Connected,
}

/// One AP from a scan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanResult {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i32,
    pub secure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IpConfig {
    pub ip: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub dns: Ipv4Addr,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum WifiState {
    Idle,
    Joining { ap: usize },
    Connected { ap: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct Station {
    pub label: String,
    pub mac: [u8; 6],
    pub hostname: String,
    pub pos: Pos,
    state: WifiState,
    pub ip: Option<IpConfig>,
    pub static_ip: Option<IpConfig>,
    pub dhcp: DhcpClient,
    join: Option<(String, Option<String>)>,
    pub auto_reconnect: bool,
    events: VecDeque<NetEvent>,
    epoch: u64,
    next_port: u16,
    dns: HashMap<u16, DnsQuery>,
    pings: HashMap<u16, (u32, SimTime)>,
}

#[derive(Debug, Clone)]
struct DnsQuery {
    query: u32,
    name: String,
    port: u16,
    attempt: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SockKind {
    Tcp { conn: Option<ConnId>, end: usize },
    Listener { port: u16 },
    Udp { port: u16 },
}

#[derive(Debug, Clone)]
pub(crate) struct Socket {
    pub owner: StationId,
    pub kind: SockKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Body {
    Tcp { conn: ConnId, from_end: usize, flags: u8, seq: u32, ack: u32, data: Vec<u8> },
    Udp { data: Vec<u8> },
    Icmp { reply: bool, id: u16, seq: u16, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Packet {
    pub src: SocketAddrV4,
    pub dst: SocketAddrV4,
    pub body: Body,
    pub from: Host,
    /// Explicit destination (DHCP replies to stations without an address).
    pub to: Option<Host>,
}

#[derive(Debug, Clone)]
enum Ev {
    Deliver(Box<Packet>, Host),
    AssocDone { sta: StationId, epoch: u64, ap: usize },
    JoinFailed { sta: StationId, epoch: u64, ssid: String, reason: JoinError },
    Dhcp { sta: StationId, epoch: u64, dgen: u64 },
    TcpRto { conn: ConnId, end: usize, epoch: u64 },
    ApDown { cgen: u64 },
    ApUp { cgen: u64 },
    Reconnect { sta: StationId, epoch: u64 },
    DnsRetry { sta: StationId, id: u16, attempt: u32 },
    PingTimeout { sta: StationId, id: u16 },
}

#[derive(Debug)]
struct Scheduled {
    at: SimTime,
    seq: u64,
    ev: Ev,
}

impl PartialEq for Scheduled {
    fn eq(&self, o: &Self) -> bool {
        self.at == o.at && self.seq == o.seq
    }
}
impl Eq for Scheduled {}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Scheduled {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap by (time, insertion order).
        o.at.cmp(&self.at).then(o.seq.cmp(&self.seq))
    }
}

/// The virtual network.
#[derive(Debug)]
pub struct Network {
    pub config: NetConfig,
    conditions: NetConditions,
    cond_gen: u64,
    ap_down: bool,
    pub geometry: Geometry,
    pub path_loss: PathLoss,
    aps: Vec<AccessPoint>,
    pub(crate) stations: Vec<Station>,
    pub(crate) sockets: HashMap<SockId, Socket>,
    next_sock: u32,
    pub(crate) conns: HashMap<ConnId, TcpConn>,
    next_conn: ConnId,
    queue: BinaryHeap<Scheduled>,
    seq: u64,
    pub(crate) now: SimTime,
    rng: Pcg32,
    ip_ident: u16,
    pub(crate) log: PacketLog,
    annotations: Vec<NetAnnotation>,
    pub(crate) broker: Broker,
    broker_events: Vec<(SimTime, BrokerEvent)>,
    pub(crate) dhcp_server: DhcpServer,
    pub(crate) http: HashMap<ConnId, HttpSession>,
    /// Pure-ACK rows in the packet log (off by default).
    pub log_acks: bool,
    pub(crate) mss: usize,
}

impl Default for Network {
    fn default() -> Self {
        Network::new(NetConfig::default())
    }
}

pub(crate) const PORT_MQTT: u16 = 1883;
pub(crate) const PORT_HTTP: u16 = 80;
pub(crate) const PORT_ECHO: u16 = 7;
pub(crate) const PORT_DNS: u16 = 53;
pub(crate) const PORT_NTP: u16 = 123;
pub(crate) const PORT_DHCP_S: u16 = 67;
pub(crate) const PORT_DHCP_C: u16 = 68;
/// Sensitivity of an ESP8266-class station (dBm): weaker APs are invisible.
pub const WIFI_SENSITIVITY_DBM: f64 = -90.0;

impl Network {
    pub fn new(config: NetConfig) -> Self {
        let rng = Pcg32::new(config.seed);
        let dhcp_server = DhcpServer::new(&config);
        Network {
            config,
            conditions: NetConditions::default(),
            cond_gen: 0,
            ap_down: false,
            geometry: Geometry::default(),
            path_loss: PathLoss::WIFI_INDOOR,
            aps: vec![AccessPoint::sim_ap()],
            stations: vec![],
            sockets: HashMap::new(),
            next_sock: 1,
            conns: HashMap::new(),
            next_conn: 1,
            queue: BinaryHeap::new(),
            seq: 0,
            now: SimTime::ZERO,
            rng,
            ip_ident: 1,
            log: PacketLog::default(),
            annotations: vec![],
            broker: Broker::default(),
            broker_events: vec![],
            dhcp_server,
            http: HashMap::new(),
            log_acks: false,
            mss: 1460,
        }
    }

    // ================================================================ configuration

    pub fn access_points(&self) -> &[AccessPoint] {
        &self.aps
    }

    pub fn access_points_mut(&mut self) -> &mut Vec<AccessPoint> {
        &mut self.aps
    }

    pub fn add_access_point(&mut self, ap: AccessPoint) -> usize {
        self.aps.push(ap);
        self.aps.len() - 1
    }

    pub fn conditions(&self) -> &NetConditions {
        &self.conditions
    }

    /// Applies new network conditions (re-arms the AP-drop fault).
    pub fn set_conditions(&mut self, c: NetConditions) {
        self.cond_gen += 1;
        if let Some(at) = c.drop_ap_at_s {
            let at = SimTime::from_secs_f64(at);
            if at >= self.now {
                self.schedule(at, Ev::ApDown { cgen: self.cond_gen });
            }
        }
        if self.ap_down && c.drop_ap_at_s.is_none() {
            // Fault removed while active: bring the AP back now.
            self.schedule(self.now, Ev::ApUp { cgen: self.cond_gen });
        }
        self.conditions = c;
    }

    /// True while the fault injector holds the APs down.
    pub fn ap_is_down(&self) -> bool {
        self.ap_down
    }

    pub fn gateway(&self) -> Ipv4Addr {
        self.config.gateway
    }

    pub fn broker(&self) -> &Broker {
        &self.broker
    }

    /// Direct broker access (virtual clients for the MQTT explorer, bridge, gateway). Transport
    /// actions returned by virtual operations must go through [`Network::apply_broker`].
    pub fn broker_mut(&mut self) -> &mut Broker {
        &mut self.broker
    }

    /// Performs the transport actions a broker call returned (deliveries to TCP clients).
    pub fn apply_broker(&mut self, out: Vec<BrokerOut>) {
        for o in out {
            match o {
                BrokerOut::Send(key, bytes) => {
                    if self.conns.contains_key(&key) {
                        self.tcp_write_end(key, 1, &bytes);
                    }
                }
                BrokerOut::Close(key) => self.tcp_close_end(key, 1),
            }
        }
        for e in self.broker.take_events() {
            self.on_broker_event(e);
        }
    }

    /// Broker events (with their time) since the last call — the MQTT explorer's feed.
    pub fn take_broker_events(&mut self) -> Vec<(SimTime, BrokerEvent)> {
        std::mem::take(&mut self.broker_events)
    }

    pub fn log(&self) -> &PacketLog {
        &self.log
    }

    pub fn log_mut(&mut self) -> &mut PacketLog {
        &mut self.log
    }

    pub fn take_annotations(&mut self) -> Vec<NetAnnotation> {
        std::mem::take(&mut self.annotations)
    }

    pub(crate) fn annotate(&mut self, station: StationId, kind: AnnotationKind, text: String) {
        self.annotations.push(NetAnnotation { time: self.now, station, kind, text });
    }

    /// Wall-clock Unix time (s) at sim time `t`.
    pub fn unix_time(&self, t: SimTime) -> f64 {
        self.config.epoch_unix as f64 + t.as_secs_f64()
    }

    pub fn now(&self) -> SimTime {
        self.now
    }

    // ================================================================ stations

    /// Adds a station (a Wi-Fi radio on the bench) at `pos`. `label` is the part reference
    /// ("U2"); the MAC is derived deterministically from the station index.
    pub fn add_station(&mut self, label: &str, pos: Pos) -> StationId {
        let id = StationId(self.stations.len() as u32);
        let n = id.0 as u8;
        self.stations.push(Station {
            label: label.into(),
            mac: [0x5c, 0xcf, 0x7f, 0xb0, 0x0b, 0x10 + n],
            hostname: format!("esp-{}", label.to_ascii_lowercase()),
            pos,
            state: WifiState::Idle,
            ip: None,
            static_ip: None,
            dhcp: DhcpClient::default(),
            join: None,
            auto_reconnect: true,
            events: VecDeque::new(),
            epoch: 0,
            next_port: 49_152 + 1_000 * n as u16 % 10_000,
            dns: HashMap::new(),
            pings: HashMap::new(),
        });
        id
    }

    pub(crate) fn sta(&self, id: StationId) -> Result<&Station, NetError> {
        self.stations.get(id.0 as usize).ok_or(NetError::NoStation)
    }

    pub(crate) fn sta_mut(&mut self, id: StationId) -> Result<&mut Station, NetError> {
        self.stations.get_mut(id.0 as usize).ok_or(NetError::NoStation)
    }

    pub fn set_station_pos(&mut self, id: StationId, pos: Pos) {
        if let Ok(s) = self.sta_mut(id) {
            s.pos = pos;
        }
    }

    pub fn set_hostname(&mut self, id: StationId, name: &str) {
        if let Ok(s) = self.sta_mut(id) {
            s.hostname = name.into();
        }
    }

    pub fn set_mac(&mut self, id: StationId, mac: [u8; 6]) {
        if let Ok(s) = self.sta_mut(id) {
            s.mac = mac;
        }
    }

    pub fn set_auto_reconnect(&mut self, id: StationId, on: bool) {
        if let Ok(s) = self.sta_mut(id) {
            s.auto_reconnect = on;
        }
    }

    /// Static IP configuration (`AT+CIPSTA=`, `WiFi.config`); `None` = DHCP.
    pub fn set_static_ip(&mut self, id: StationId, ip: Option<(Ipv4Addr, Ipv4Addr, Ipv4Addr)>) {
        let dns = self.config.gateway;
        if let Ok(s) = self.sta_mut(id) {
            s.static_ip = ip.map(|(ip, gateway, netmask)| IpConfig { ip, gateway, netmask, dns });
            if s.ip.is_some()
                && let Some(c) = s.static_ip
            {
                s.ip = Some(c);
            }
        }
    }

    pub fn station_count(&self) -> usize {
        self.stations.len()
    }

    pub fn station_label(&self, id: StationId) -> Option<&str> {
        self.sta(id).ok().map(|s| s.label.as_str())
    }

    pub fn station_by_ip(&self, ip: Ipv4Addr) -> Option<StationId> {
        self.stations
            .iter()
            .position(|s| s.ip.map(|c| c.ip) == Some(ip) && matches!(s.state, WifiState::Connected { .. }))
            .map(|i| StationId(i as u32))
    }

    /// Link budget from the AP `ap` to the station.
    pub fn link(&self, id: StationId, ap: usize) -> Option<Link> {
        let s = self.sta(id).ok()?;
        let a = self.aps.get(ap)?;
        Some(self.geometry.link(&self.path_loss, a.tx_dbm, a.pos, s.pos))
    }

    pub fn station_info(&self, id: StationId) -> Option<StationInfo> {
        let s = self.sta(id).ok()?;
        let (state, ap) = match s.state {
            WifiState::Idle => (WifiStateKind::Idle, None),
            WifiState::Joining { ap } => (WifiStateKind::Joining, Some(ap)),
            WifiState::Connected { ap } => (WifiStateKind::Connected, Some(ap)),
        };
        Some(StationInfo {
            label: s.label.clone(),
            mac: s.mac,
            hostname: s.hostname.clone(),
            pos: s.pos,
            state,
            ssid: ap.map(|a| self.aps[a].ssid.clone()),
            channel: ap.map(|a| self.aps[a].channel),
            link: ap.and_then(|a| self.link(id, a)),
            ip: s.ip.map(|c| c.ip),
            dhcp: s.static_ip.is_none(),
        })
    }

    pub fn ip_of(&self, id: StationId) -> Option<Ipv4Addr> {
        self.sta(id).ok().and_then(|s| s.ip.map(|c| c.ip))
    }

    pub fn is_connected(&self, id: StationId) -> bool {
        self.sta(id).is_ok_and(|s| matches!(s.state, WifiState::Connected { .. }))
    }

    /// Events for a station since the last call.
    pub fn take_events(&mut self, id: StationId) -> Vec<NetEvent> {
        self.sta_mut(id).map(|s| s.events.drain(..).collect()).unwrap_or_default()
    }

    pub fn has_events(&self, id: StationId) -> bool {
        self.sta(id).is_ok_and(|s| !s.events.is_empty())
    }

    pub(crate) fn push_event(&mut self, id: StationId, e: NetEvent) {
        if let Ok(s) = self.sta_mut(id) {
            s.events.push_back(e);
        }
    }

    // ================================================================ Wi-Fi

    /// APs visible from the station (strongest first).
    pub fn scan(&self, id: StationId) -> Vec<ScanResult> {
        let Ok(s) = self.sta(id) else { return vec![] };
        let mut v: Vec<ScanResult> = self
            .aps
            .iter()
            .filter(|a| a.enabled && !self.ap_down)
            .filter_map(|a| {
                let l = self.geometry.link(&self.path_loss, a.tx_dbm, a.pos, s.pos);
                (l.rssi_dbm >= WIFI_SENSITIVITY_DBM).then(|| ScanResult {
                    ssid: a.ssid.clone(),
                    bssid: a.bssid,
                    channel: a.channel,
                    rssi: l.rssi_dbm.round() as i32,
                    secure: a.password.is_some(),
                })
            })
            .collect();
        v.sort_by(|a, b| b.rssi.cmp(&a.rssi));
        v
    }

    /// Starts joining `ssid` (scan + WPA2 handshake + DHCP). Outcome: `WifiConnected` then
    /// `GotIp`, or `WifiJoinFailed`.
    pub fn join(&mut self, id: StationId, ssid: &str, password: Option<&str>, now: SimTime) -> Result<(), NetError> {
        self.sync(now);
        self.disconnect_station(id, DisconnectReason::Left, false)?;
        let s = self.sta_mut(id)?;
        s.join = Some((ssid.into(), password.map(|p| p.into())));
        s.epoch += 1;
        self.attempt_join(id, false);
        Ok(())
    }

    fn attempt_join(&mut self, id: StationId, quiet: bool) {
        let Ok(s) = self.sta(id) else { return };
        let Some((ssid, pass)) = s.join.clone() else { return };
        let epoch = s.epoch;
        let found = self.aps.iter().enumerate().find(|(_, a)| a.ssid == ssid && a.enabled && !self.ap_down).map(|(i, a)| {
            let l = self.geometry.link(&self.path_loss, a.tx_dbm, a.pos, s.pos);
            (i, a.password.clone(), l)
        });
        let label = s.label.clone();
        // Scan: ~0.6 s across channels (+ jitter), then auth/assoc.
        let scan = SimTime::from_ms(550 + self.rng.below(120) as u64);
        match found {
            Some((ap, ap_pass, link)) if link.rssi_dbm >= WIFI_SENSITIVITY_DBM => {
                if let Ok(s) = self.sta_mut(id) {
                    s.state = WifiState::Joining { ap };
                }
                let ok = match ap_pass {
                    None => true,
                    Some(p) => pass.as_deref() == Some(p.as_str()),
                };
                if !quiet {
                    self.log_mgmt(id, Dir::Tx, format!("Wi-Fi AUTH {ssid} (WPA2-PSK)"));
                }
                if ok {
                    self.schedule(self.now + scan + SimTime::from_ms(120), Ev::AssocDone { sta: id, epoch, ap });
                } else {
                    self.schedule(self.now + scan + SimTime::from_ms(1_600), Ev::JoinFailed { sta: id, epoch, ssid, reason: JoinError::WrongPassword });
                }
            }
            _ => {
                if !quiet {
                    self.log_mgmt(id, Dir::Tx, format!("Wi-Fi probe {ssid}"));
                }
                if let Ok(s) = self.sta_mut(id) {
                    s.state = WifiState::Joining { ap: usize::MAX };
                }
                let _ = label;
                self.schedule(self.now + SimTime::from_ms(2_100), Ev::JoinFailed { sta: id, epoch, ssid, reason: JoinError::NoAp });
            }
        }
    }

    /// Leaves the network (`AT+CWQAP`, `WiFi.disconnect()`); no auto-reconnect.
    pub fn leave(&mut self, id: StationId, now: SimTime) -> Result<(), NetError> {
        self.sync(now);
        let s = self.sta_mut(id)?;
        s.join = None;
        s.epoch += 1;
        self.disconnect_station(id, DisconnectReason::Left, true)
    }

    /// Drops association + IP + every TCP connection of the station.
    fn disconnect_station(&mut self, id: StationId, reason: DisconnectReason, notify: bool) -> Result<(), NetError> {
        let s = self.sta_mut(id)?;
        let was = s.state;
        s.state = WifiState::Idle;
        s.ip = None;
        s.dhcp = DhcpClient::default();
        s.dns.clear();
        if matches!(was, WifiState::Connected { .. }) {
            if notify {
                s.events.push_back(NetEvent::WifiDisconnected { reason });
            }
            self.kill_station_conns(id);
            let text = match reason {
                DisconnectReason::ApDown => "Wi-Fi DEAUTH (AP down)",
                DisconnectReason::Left => "Wi-Fi DISASSOC (leaving)",
                DisconnectReason::OutOfRange => "Wi-Fi beacon loss (out of range)",
            };
            self.log_mgmt(id, if reason == DisconnectReason::ApDown { Dir::Rx } else { Dir::Tx }, text.into());
            if notify {
                self.annotate(id, AnnotationKind::Wifi, format!("WIFI DISCONNECT ({})", match reason {
                    DisconnectReason::ApDown => "AP down",
                    DisconnectReason::Left => "left",
                    DisconnectReason::OutOfRange => "out of range",
                }));
            }
        }
        Ok(())
    }

    fn on_assoc_done(&mut self, id: StationId, ap: usize) {
        let Some(link) = self.link(id, ap) else { return };
        let a = self.aps[ap].clone();
        let Ok(s) = self.sta_mut(id) else { return };
        s.state = WifiState::Connected { ap };
        let rssi = link.rssi_dbm.round() as i32;
        s.events.push_back(NetEvent::WifiConnected { ssid: a.ssid.clone(), bssid: a.bssid, channel: a.channel, rssi });
        self.log_mgmt(id, Dir::Rx, format!("Wi-Fi ASSOC {} · ch {} · {} dBm", a.ssid, a.channel, rssi));
        let static_ip = self.sta(id).ok().and_then(|s| s.static_ip);
        match static_ip {
            Some(c) => {
                let s = self.sta_mut(id).expect("station exists");
                s.ip = Some(c);
                s.events.push_back(NetEvent::GotIp { ip: c.ip, gateway: c.gateway, netmask: c.netmask, dns: c.dns });
            }
            None => self.dhcp_start(id),
        }
    }

    // ================================================================ time

    fn schedule(&mut self, at: SimTime, ev: Ev) {
        self.seq += 1;
        self.queue.push(Scheduled { at: at.max(self.now), seq: self.seq, ev });
    }

    fn sync(&mut self, now: SimTime) {
        if now > self.now {
            self.advance(now);
        }
    }

    /// Time of the next internal event (packet arrival, timer, broker keep-alive check).
    pub fn next_event_time(&self) -> Option<SimTime> {
        let q = self.queue.peek().map(|s| s.at);
        let b = self.broker.next_deadline();
        match (q, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Processes every event due at or before `now`.
    pub fn advance(&mut self, now: SimTime) {
        loop {
            let qt = self.queue.peek().map(|s| s.at);
            let bt = self.broker.next_deadline();
            let next = match (qt, bt) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => break,
            };
            if next > now {
                break;
            }
            self.now = self.now.max(next);
            if bt.is_some_and(|b| b <= next) {
                let out = self.broker.tick(self.now);
                self.apply_broker(out);
                continue;
            }
            let Some(s) = self.queue.pop() else { continue };
            self.process(s.ev);
        }
        self.now = self.now.max(now);
    }

    fn process(&mut self, ev: Ev) {
        match ev {
            Ev::Deliver(p, to) => self.deliver(*p, to),
            Ev::AssocDone { sta, epoch, ap } => {
                if self.sta(sta).is_ok_and(|s| s.epoch == epoch) {
                    if self.ap_down || !self.aps[ap].enabled {
                        self.join_failed(sta, self.aps[ap].ssid.clone(), JoinError::NoAp);
                    } else {
                        self.on_assoc_done(sta, ap);
                    }
                }
            }
            Ev::JoinFailed { sta, epoch, ssid, reason } => {
                if self.sta(sta).is_ok_and(|s| s.epoch == epoch) {
                    self.join_failed(sta, ssid, reason);
                }
            }
            Ev::Dhcp { sta, epoch, dgen } => self.dhcp_timer(sta, epoch, dgen),
            Ev::TcpRto { conn, end, epoch } => self.tcp_rto(conn, end, epoch),
            Ev::ApDown { cgen } => {
                if cgen == self.cond_gen && !self.ap_down {
                    self.ap_down = true;
                    let label = self.aps.first().map(|a| a.ssid.clone()).unwrap_or_default();
                    self.log.push(mgmt_entry(self.now, None, Dir::Drop, format!("fault injector: AP {label} down")));
                    for i in 0..self.stations.len() {
                        let id = StationId(i as u32);
                        if self.is_connected(id) {
                            let _ = self.disconnect_station(id, DisconnectReason::ApDown, true);
                        }
                    }
                    let up = self.now + SimTime::from_secs_f64(self.conditions.ap_down_s.max(0.0));
                    self.schedule(up, Ev::ApUp { cgen });
                }
            }
            Ev::ApUp { cgen } => {
                if cgen == self.cond_gen && self.ap_down {
                    self.ap_down = false;
                    let label = self.aps.first().map(|a| a.ssid.clone()).unwrap_or_default();
                    self.log.push(mgmt_entry(self.now, None, Dir::Rx, format!("fault injector: AP {label} back up")));
                    for i in 0..self.stations.len() {
                        let id = StationId(i as u32);
                        let s = &self.stations[i];
                        if s.auto_reconnect && s.join.is_some() && s.state == WifiState::Idle {
                            let epoch = s.epoch;
                            let d = SimTime::from_ms(300 + self.rng.below(1500) as u64);
                            self.schedule(self.now + d, Ev::Reconnect { sta: id, epoch });
                        }
                    }
                }
            }
            Ev::Reconnect { sta, epoch } => {
                if self.sta(sta).is_ok_and(|s| s.epoch == epoch && s.state == WifiState::Idle) {
                    self.attempt_join(sta, false);
                }
            }
            Ev::DnsRetry { sta, id, attempt } => self.dns_retry(sta, id, attempt),
            Ev::PingTimeout { sta, id } => {
                if let Ok(s) = self.sta_mut(sta)
                    && let Some((ping, _)) = s.pings.remove(&id)
                {
                    s.events.push_back(NetEvent::PingTimeout { ping });
                }
            }
        }
    }

    fn join_failed(&mut self, id: StationId, ssid: String, reason: JoinError) {
        let auto = self.sta(id).is_ok_and(|s| s.auto_reconnect && s.join.is_some());
        let epoch = self.sta(id).map(|s| s.epoch).unwrap_or(0);
        if let Ok(s) = self.sta_mut(id) {
            s.state = WifiState::Idle;
            s.events.push_back(NetEvent::WifiJoinFailed { ssid: ssid.clone(), reason });
        }
        self.log_mgmt(id, Dir::Rx, format!("Wi-Fi join {ssid} failed: {}", reason.text()));
        // Keep trying in the background like the ESP SDK's auto-connect (the AT command has
        // already reported FAIL).
        if auto && reason == JoinError::NoAp {
            self.schedule(self.now + SimTime::from_secs(5), Ev::Reconnect { sta: id, epoch });
        }
    }

    // ================================================================ air + routing

    fn air_hop(&mut self, id: StationId) -> Result<SimTime, DropReason> {
        let s = self.sta(id).map_err(|_| DropReason::NoRoute)?;
        let WifiState::Connected { ap } = s.state else {
            return Err(if self.ap_down { DropReason::ApDown } else { DropReason::NoRoute });
        };
        if self.ap_down {
            return Err(DropReason::ApDown);
        }
        let rssi = self.link(id, ap).map(|l| l.rssi_dbm).unwrap_or(-100.0);
        let c = &self.conditions;
        let (loss, lat, jit) = (c.loss_pct, c.latency_ms, c.jitter_ms);
        if loss > 0.0 && self.rng.chance(loss / 100.0) {
            return Err(DropReason::Loss { pct: loss });
        }
        // Frames near the sensitivity limit get lost too (≈ 0 % above -80 dBm, 50 % at -88).
        let weak = ((-80.0 - rssi) / 16.0).clamp(0.0, 1.0);
        if weak > 0.0 && self.rng.chance(weak) {
            return Err(DropReason::WeakSignal);
        }
        let ms = (lat + self.rng.range_f64(-jit, jit)).max(0.2);
        Ok(SimTime::from_secs_f64(ms * 1e-3))
    }

    fn resolve_host(&self, p: &Packet) -> Option<Host> {
        if let Some(t) = p.to {
            return Some(t);
        }
        let ip = *p.dst.ip();
        if ip == self.config.gateway || ip == Ipv4Addr::BROADCAST {
            return Some(Host::Gateway);
        }
        self.station_by_ip(ip).map(Host::Station)
    }

    /// Sends a packet onto the air: logs it, applies loss/latency, schedules delivery.
    pub(crate) fn transmit(&mut self, p: Packet) {
        let to = self.resolve_host(&p);
        let mut delay = SimTime::ZERO;
        let mut drop: Option<DropReason> = None;
        if let Host::Station(s) = p.from {
            match self.air_hop(s) {
                Ok(d) => delay += d,
                Err(r) => drop = Some(r),
            }
        }
        match to {
            None => drop = drop.or(Some(DropReason::NoRoute)),
            Some(Host::Station(s)) if drop.is_none() => match self.air_hop(s) {
                Ok(d) => delay += d,
                Err(r) => drop = Some(r),
            },
            _ => {}
        }
        // Tx rows when a bench radio sends; drops are logged where they happen.
        let ack_only = is_pure_ack(&p);
        if let Host::Station(s) = p.from {
            if drop.is_some() || !ack_only || self.log_acks {
                self.log_packet(&p, Some(s), if drop.is_some() { Dir::Drop } else { Dir::Tx }, drop.clone());
            }
        } else if drop.is_some() && (!ack_only || self.log_acks) {
            let r = match to {
                Some(Host::Station(s)) => Some(s),
                _ => None,
            };
            self.log_packet(&p, r, Dir::Drop, drop.clone());
        }
        if drop.is_some() {
            return;
        }
        let to = to.expect("routable");
        self.schedule(self.now + delay, Ev::Deliver(Box::new(p), to));
    }

    fn deliver(&mut self, p: Packet, to: Host) {
        match to {
            Host::Gateway => self.gateway_receive(p),
            Host::Station(s) => {
                // The receiver may have lost its link while the frame was in flight.
                if !self.is_connected(s) {
                    if !is_pure_ack(&p) || self.log_acks {
                        let r = if self.ap_down { DropReason::ApDown } else { DropReason::NoRoute };
                        self.log_packet(&p, Some(s), Dir::Drop, Some(r));
                    }
                    return;
                }
                if !is_pure_ack(&p) || self.log_acks {
                    self.log_packet(&p, Some(s), Dir::Rx, None);
                }
                self.station_receive(s, p);
            }
        }
    }

    fn gateway_receive(&mut self, p: Packet) {
        match p.body.clone() {
            Body::Tcp { .. } => self.tcp_receive(p, Host::Gateway),
            Body::Udp { data } => self.gateway_udp(&p, data),
            Body::Icmp { reply: false, id, seq, data } => {
                let r = Packet { src: p.dst, dst: p.src, body: Body::Icmp { reply: true, id, seq, data }, from: Host::Gateway, to: Some(p.from) };
                self.transmit(r);
            }
            Body::Icmp { .. } => {}
        }
    }

    fn station_receive(&mut self, id: StationId, p: Packet) {
        match p.body.clone() {
            Body::Tcp { .. } => self.tcp_receive(p, Host::Station(id)),
            Body::Udp { data } => self.station_udp(id, &p, data),
            Body::Icmp { reply: false, id: iid, seq, data } => {
                // Stations answer pings.
                let r = Packet { src: p.dst, dst: p.src, body: Body::Icmp { reply: true, id: iid, seq, data }, from: Host::Station(id), to: Some(p.from) };
                self.transmit(r);
            }
            Body::Icmp { reply: true, id: iid, .. } => {
                let now = self.now;
                if let Ok(s) = self.sta_mut(id)
                    && let Some((ping, sent)) = s.pings.remove(&iid)
                {
                    s.events.push_back(NetEvent::PingReply { ping, rtt: now - sent });
                }
            }
        }
    }

    fn station_udp(&mut self, id: StationId, p: &Packet, data: Vec<u8>) {
        let port = p.dst.port();
        if port == PORT_DHCP_C {
            self.dhcp_client_receive(id, &data);
            return;
        }
        let dns_match = self.sta(id).ok().and_then(|s| s.dns.iter().find(|(_, q)| q.port == port).map(|(k, _)| *k));
        if let Some(dns_id) = dns_match {
            self.dns_client_receive(id, dns_id, &data);
            return;
        }
        let sock = self
            .sockets
            .iter()
            .find(|(_, s)| s.owner == id && s.kind == SockKind::Udp { port })
            .map(|(k, _)| *k);
        if let Some(sock) = sock {
            self.push_event(id, NetEvent::UdpData { sock, from: p.src, data });
        }
        // else: ICMP port unreachable (not modelled).
    }

    // ================================================================ logging

    pub(crate) fn log_mgmt(&mut self, id: StationId, dir: Dir, text: String) {
        let label = self.sta(id).ok().map(|s| s.label.clone());
        self.log.push(mgmt_entry(self.now, label, dir, text));
    }

    fn log_packet(&mut self, p: &Packet, radio: Option<StationId>, dir: Dir, dropped: Option<DropReason>) {
        let (proto, summary, bytes) = self.summarize(p);
        let frame = self.frame(p);
        let radio = radio.and_then(|r| self.sta(r).ok().map(|s| s.label.clone()));
        self.log.push(LogEntry {
            seq: 0,
            time: self.now,
            dir,
            radio,
            proto,
            summary,
            bytes,
            src: Some(p.src),
            dst: Some(p.dst),
            dropped,
            ack_only: is_pure_ack(p),
            frame: Some(frame),
        });
    }

    fn mac_of(&self, h: Host) -> [u8; 6] {
        match h {
            Host::Gateway => self.aps.first().map(|a| a.bssid).unwrap_or([2, 0, 0, 0, 0, 1]),
            Host::Station(s) => self.sta(s).map(|s| s.mac).unwrap_or([0; 6]),
        }
    }

    fn frame(&mut self, p: &Packet) -> Vec<u8> {
        self.ip_ident = self.ip_ident.wrapping_add(1);
        let (proto, l4) = match &p.body {
            Body::Tcp { flags, seq, ack, data, .. } => (wire::PROTO_TCP, wire::tcp_segment(p.src, p.dst, *seq, *ack, *flags, 64_240, data)),
            Body::Udp { data } => (wire::PROTO_UDP, wire::udp_datagram(p.src, p.dst, data)),
            Body::Icmp { reply, id, seq, data } => (wire::PROTO_ICMP, wire::icmp_echo(*reply, *id, *seq, data)),
        };
        let ip = wire::ipv4_packet(*p.src.ip(), *p.dst.ip(), proto, self.ip_ident, &l4);
        let dst_mac = match self.resolve_host(p) {
            _ if *p.dst.ip() == Ipv4Addr::BROADCAST => [0xff; 6],
            Some(h) => self.mac_of(h),
            None => [0xff; 6],
        };
        wire::ethernet_frame(dst_mac, self.mac_of(p.from), &ip)
    }

    fn summarize(&self, p: &Packet) -> (Proto, String, usize) {
        let sp = p.src.port();
        let dp = p.dst.port();
        match &p.body {
            Body::Udp { data } => {
                if sp == PORT_DHCP_C || dp == PORT_DHCP_C || sp == PORT_DHCP_S || dp == PORT_DHCP_S {
                    if let Some(m) = wire::DhcpMessage::decode(data) {
                        let s = match m.msg_type {
                            wire::DhcpType::Discover => "DHCP DISCOVER".to_string(),
                            wire::DhcpType::Offer => format!("DHCP OFFER {}", m.yiaddr),
                            wire::DhcpType::Request => format!("DHCP REQUEST {}", m.requested_ip.unwrap_or(m.ciaddr)),
                            wire::DhcpType::Ack => format!("DHCP ACK · lease {} s", m.lease_secs.unwrap_or(0)),
                            t => format!("DHCP {}", t.name()),
                        };
                        return (Proto::Dhcp, s, data.len());
                    }
                }
                if sp == PORT_DNS || dp == PORT_DNS {
                    if let Some(m) = wire::DnsMessage::decode(data) {
                        let s = if !m.response {
                            format!("DNS A? {}", m.name)
                        } else if let Some((ip, _)) = m.answers.first() {
                            format!("DNS A {} = {}", m.name, ip)
                        } else {
                            format!("DNS {} NXDOMAIN", m.name)
                        };
                        return (Proto::Dns, s, data.len());
                    }
                }
                if sp == PORT_NTP || dp == PORT_NTP {
                    let s = match wire::ntp_reply_time(data) {
                        Some(t) => format!("NTP reply {}", wire::asctime(t as i64)),
                        None => "NTP request".to_string(),
                    };
                    return (Proto::Ntp, s, data.len());
                }
                (Proto::Udp, format!("UDP {} → {} · {} B", p.src, p.dst, data.len()), data.len())
            }
            Body::Icmp { reply, seq, data, .. } => {
                (Proto::Icmp, format!("ICMP echo {} seq={}", if *reply { "reply" } else { "request" }, seq), data.len())
            }
            Body::Tcp { flags, data, .. } => {
                use wire::tcp_flags::*;
                if flags & RST != 0 {
                    return (Proto::Tcp, format!("TCP RST {}", p.dst), 0);
                }
                if flags & SYN != 0 {
                    let s = if flags & ACK != 0 { format!("TCP SYN-ACK {}", p.src) } else { format!("TCP SYN {}", p.dst) };
                    return (Proto::Tcp, s, 0);
                }
                if !data.is_empty() {
                    if sp == PORT_MQTT || dp == PORT_MQTT {
                        if let Some(s) = crate::mqtt::codec::summarize(data) {
                            return (Proto::Mqtt, s, data.len());
                        }
                        return (Proto::Mqtt, format!("MQTT data · {} B", data.len()), data.len());
                    }
                    if sp == PORT_HTTP || dp == PORT_HTTP {
                        let first = data.split(|&b| b == b'\r').next().unwrap_or(&[]);
                        let line = String::from_utf8_lossy(&first[..first.len().min(60)]).into_owned();
                        let s = if line.starts_with("HTTP/") || line.contains(" HTTP/") || line.starts_with("GET") || line.starts_with("POST") {
                            format!("HTTP {line}")
                        } else {
                            format!("HTTP data · {} B", data.len())
                        };
                        return (Proto::Http, s, data.len());
                    }
                    let s = if flags & FIN != 0 { " + FIN" } else { "" };
                    return (Proto::Tcp, format!("TCP {} → {} · {} B{}", p.src, p.dst, data.len(), s), data.len());
                }
                if flags & FIN != 0 {
                    return (Proto::Tcp, format!("TCP FIN {} → {}", p.src, p.dst), 0);
                }
                (Proto::Tcp, format!("TCP ACK {} → {}", p.src, p.dst), 0)
            }
        }
    }

    // ================================================================ broker glue

    fn on_broker_event(&mut self, e: BrokerEvent) {
        let station_of = |net: &Network, key: u64| -> Option<StationId> {
            net.conns.get(&key).and_then(|c| match c.ends[0].host {
                Host::Station(s) => Some(s),
                Host::Gateway => None,
            })
        };
        match &e {
            BrokerEvent::Published { key, message, size, .. } => {
                if let Some(s) = station_of(self, *key) {
                    let text = format!("PUB {} {} ({} B)", message.topic, String::from_utf8_lossy(&message.payload), size);
                    self.annotate(s, AnnotationKind::Mqtt, text);
                }
            }
            BrokerEvent::Connected { key, client, .. } => {
                if let Some(s) = station_of(self, *key) {
                    self.annotate(s, AnnotationKind::Mqtt, format!("CONNECT client={client} → CONNACK 0"));
                }
            }
            BrokerEvent::Rejected { key, client, code } => {
                if let Some(s) = station_of(self, *key) {
                    self.annotate(s, AnnotationKind::Mqtt, format!("CONNECT client={client} → CONNACK {code} (refused)"));
                }
            }
            BrokerEvent::Subscribed { key, filters, .. } => {
                if let Some(s) = station_of(self, *key) {
                    let f: Vec<String> = filters.iter().map(|(f, q)| format!("{f} (QoS {q})")).collect();
                    self.annotate(s, AnnotationKind::Mqtt, format!("SUB {}", f.join(", ")));
                }
            }
            _ => {}
        }
        self.broker_events.push((self.now, e));
    }

    // ================================================================ ping

    /// ICMP echo to `dst`. Result: `PingReply` or `PingTimeout` (after `timeout`).
    pub fn ping(&mut self, id: StationId, dst: Ipv4Addr, size: usize, timeout: SimTime, now: SimTime) -> Result<u32, NetError> {
        self.sync(now);
        let src = self.ip_of(id).ok_or(NetError::NoIp)?;
        let s = self.sta_mut(id)?;
        let iid = s.next_port;
        s.next_port = s.next_port.wrapping_add(1).max(49_152);
        let ping = iid as u32;
        s.pings.insert(iid, (ping, now));
        let p = Packet {
            src: SocketAddrV4::new(src, 0),
            dst: SocketAddrV4::new(dst, 0),
            body: Body::Icmp { reply: false, id: iid, seq: 1, data: vec![0x42; size.min(1400)] },
            from: Host::Station(id),
            to: None,
        };
        self.transmit(p);
        self.schedule(now + timeout, Ev::PingTimeout { sta: id, id: iid });
        Ok(ping)
    }

    pub(crate) fn ephemeral_port(&mut self, id: StationId) -> u16 {
        let s = &mut self.stations[id.0 as usize];
        let p = s.next_port;
        s.next_port = if s.next_port >= 65_000 { 49_152 } else { s.next_port + 1 };
        p
    }

    // ================================================================ UDP sockets

    /// Binds a UDP socket (`port` 0 = ephemeral).
    pub fn udp_bind(&mut self, id: StationId, port: u16) -> Result<SockId, NetError> {
        self.sta(id)?;
        let port = if port == 0 { self.ephemeral_port(id) } else { port };
        if self.sockets.values().any(|s| s.owner == id && s.kind == SockKind::Udp { port }) {
            return Err(NetError::PortInUse(port));
        }
        let sid = self.new_sock(id, SockKind::Udp { port });
        Ok(sid)
    }

    pub(crate) fn new_sock(&mut self, owner: StationId, kind: SockKind) -> SockId {
        let sid = SockId(self.next_sock);
        self.next_sock += 1;
        self.sockets.insert(sid, Socket { owner, kind });
        sid
    }

    pub fn udp_local_port(&self, sock: SockId) -> Option<u16> {
        match self.sockets.get(&sock)?.kind {
            SockKind::Udp { port } => Some(port),
            _ => None,
        }
    }

    pub fn udp_send(&mut self, sock: SockId, dst: SocketAddrV4, data: &[u8], now: SimTime) -> Result<(), NetError> {
        self.sync(now);
        let s = self.sockets.get(&sock).ok_or(NetError::NoSocket)?;
        let SockKind::Udp { port } = s.kind else { return Err(NetError::Invalid("not a UDP socket".into())) };
        let owner = s.owner;
        let ip = self.ip_of(owner).ok_or(NetError::NoIp)?;
        let p = Packet { src: SocketAddrV4::new(ip, port), dst, body: Body::Udp { data: data.to_vec() }, from: Host::Station(owner), to: None };
        self.transmit(p);
        Ok(())
    }

    /// Closes any socket (TCP: graceful FIN; listener/UDP: unbind).
    pub fn close(&mut self, sock: SockId, now: SimTime) {
        self.sync(now);
        let Some(s) = self.sockets.get(&sock).cloned() else { return };
        match s.kind {
            SockKind::Tcp { conn: Some(c), end } => {
                self.tcp_close_end(c, end);
                self.sockets.remove(&sock);
            }
            _ => {
                self.sockets.remove(&sock);
            }
        }
    }

    /// Socket owner.
    pub fn sock_owner(&self, sock: SockId) -> Option<StationId> {
        self.sockets.get(&sock).map(|s| s.owner)
    }
}

fn mgmt_entry(time: SimTime, radio: Option<String>, dir: Dir, summary: String) -> LogEntry {
    LogEntry {
        seq: 0,
        time,
        dir,
        radio,
        proto: Proto::Wifi,
        summary,
        bytes: 0,
        src: None,
        dst: None,
        dropped: None,
        ack_only: false,
        frame: None,
    }
}

fn is_pure_ack(p: &Packet) -> bool {
    matches!(&p.body, Body::Tcp { flags, data, .. } if data.is_empty() && *flags == wire::tcp_flags::ACK)
}
