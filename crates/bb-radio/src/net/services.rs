//! Gateway services (DHCP server, DNS, NTP, HTTP test server, echo, MQTT broker glue) and the
//! station-side DHCP + DNS clients.

use super::*;
use std::collections::HashSet;
use wire::{DhcpMessage, DhcpType, DnsMessage};

// ===================================================================== DHCP server

#[derive(Debug, Clone)]
struct Lease {
    ip: Ipv4Addr,
    expires: SimTime,
}

#[derive(Debug, Clone)]
pub(crate) struct DhcpServer {
    leases: HashMap<[u8; 6], Lease>,
}

impl DhcpServer {
    pub fn new(_cfg: &NetConfig) -> Self {
        DhcpServer { leases: HashMap::new() }
    }

    /// The address for `mac`: its existing lease, else the lowest free pool address.
    fn allocate(&mut self, mac: [u8; 6], cfg: &NetConfig, now: SimTime, statics: &HashSet<Ipv4Addr>) -> Option<Ipv4Addr> {
        if let Some(l) = self.leases.get(&mac) {
            return Some(l.ip);
        }
        let base = cfg.gateway.octets();
        let taken: HashSet<Ipv4Addr> = self.leases.iter().filter(|(_, l)| l.expires > now).map(|(_, l)| l.ip).collect();
        for last in cfg.pool_first..=cfg.pool_last {
            let ip = Ipv4Addr::new(base[0], base[1], base[2], last);
            if ip != cfg.gateway && !taken.contains(&ip) && !statics.contains(&ip) {
                // Expired leases of other clients give their address up.
                self.leases.retain(|_, l| l.ip != ip);
                return Some(ip);
            }
        }
        None
    }

    /// Current leases: (MAC, address, expiry).
    pub fn leases(&self) -> Vec<([u8; 6], Ipv4Addr, SimTime)> {
        let mut v: Vec<_> = self.leases.iter().map(|(m, l)| (*m, l.ip, l.expires)).collect();
        v.sort_by_key(|x| x.1);
        v
    }
}

// ===================================================================== DHCP client

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DhcpState {
    #[default]
    Idle,
    Selecting,
    Requesting,
    Bound,
    Renewing,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DhcpClient {
    state: DhcpState,
    xid: u32,
    attempt: u32,
    offered: Option<Ipv4Addr>,
    server: Option<Ipv4Addr>,
    dgen: u64,
}

// ===================================================================== HTTP

/// A parsed HTTP/1.x request.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Parses one complete request from the front of `buf`: `Some((request, consumed))`.
    pub fn parse(buf: &[u8]) -> Option<(HttpRequest, usize)> {
        let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&buf[..end]);
        let mut lines = head.split("\r\n");
        let mut rl = lines.next()?.split_whitespace();
        let method = rl.next()?.to_string();
        let target = rl.next()?.to_string();
        let version = rl.next().unwrap_or("HTTP/1.0").to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        let len: usize = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let total = end + 4 + len;
        if buf.len() < total {
            return None;
        }
        let (path, q) = target.split_once('?').unwrap_or((&target, ""));
        let query = q
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (url_decode(k), url_decode(v))
            })
            .collect();
        Some((
            HttpRequest { method, path: path.to_string(), query, version, headers, body: buf[end + 4..total].to_vec() },
            total,
        ))
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An HTTP response.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn json(status: u16, v: serde_json::Value) -> Self {
        HttpResponse { status, content_type: "application/json".into(), body: serde_json::to_vec_pretty(&v).unwrap_or_default() }
    }
    fn text(status: u16, s: &str) -> Self {
        HttpResponse { status, content_type: "text/plain; charset=utf-8".into(), body: s.as_bytes().to_vec() }
    }

    pub fn encode(&self, keep_alive: bool) -> Vec<u8> {
        let mut h = format!(
            "HTTP/1.1 {} {}\r\nServer: breadbox-sim\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
            self.status,
            reason(self.status),
            self.content_type,
            self.body.len(),
            if keep_alive { "keep-alive" } else { "close" }
        )
        .into_bytes();
        h.extend_from_slice(&self.body);
        h
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        418 => "I'm a teapot",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// The HTTP test server's routes (httpbin-style): `/`, `/get`, `/post`, `/put`, `/echo`,
/// `/status/<code>`, `/bytes/<n>`, `/time`, `/json`.
pub fn http_handle(req: &HttpRequest, client: Ipv4Addr, unix_now: f64) -> HttpResponse {
    let headers: serde_json::Map<String, serde_json::Value> =
        req.headers.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
    let args: serde_json::Map<String, serde_json::Value> =
        req.query.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
    let url = format!("http://{}{}", req.header("host").unwrap_or("10.0.0.1"), req.path);
    let seg: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    match (req.method.as_str(), seg.as_slice()) {
        ("GET", [""]) => HttpResponse {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: b"<html><body><h1>Breadbox test server</h1><p>Try /get, /post, /status/418, /time, /json.</p></body></html>".to_vec(),
        },
        ("GET", ["get"]) => HttpResponse::json(200, serde_json::json!({"args": args, "headers": headers, "origin": client.to_string(), "url": url})),
        (m @ ("POST" | "PUT" | "PATCH" | "DELETE"), [p]) if p.eq_ignore_ascii_case(m) => {
            let data = String::from_utf8_lossy(&req.body).into_owned();
            let json: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
            HttpResponse::json(200, serde_json::json!({"args": args, "data": data, "json": json, "headers": headers, "origin": client.to_string(), "url": url}))
        }
        (_, ["echo"]) => HttpResponse { status: 200, content_type: req.header("content-type").unwrap_or("application/octet-stream").to_string(), body: req.body.clone() },
        (_, ["status", code]) => {
            let c: u16 = code.parse().unwrap_or(400);
            HttpResponse::text(c.clamp(100, 599), reason(c))
        }
        ("GET", ["bytes", n]) => {
            let n: usize = n.parse::<usize>().unwrap_or(0).min(100_000);
            HttpResponse { status: 200, content_type: "application/octet-stream".into(), body: (0..n).map(|i| (i % 251) as u8).collect() }
        }
        ("GET", ["time"]) => HttpResponse::json(200, serde_json::json!({"unix": unix_now.floor() as i64, "utc": wire::asctime(unix_now as i64)})),
        ("GET", ["json"]) => HttpResponse::json(200, serde_json::json!({"slideshow": {"title": "Breadbox", "author": "virtual RF space"}})),
        _ => HttpResponse::text(404, "Not Found"),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HttpSession {
    buf: Vec<u8>,
}

// ===================================================================== Network glue

/// Names the gateway answers for (besides `<hostname>.sim` stations and configured hosts).
const GATEWAY_NAMES: &[&str] = &[
    "broker.sim", "mqtt.sim", "gateway.sim", "router.sim", "ap.sim", "http.sim", "echo.sim", "test.sim", "httpbin.sim", "api.sim",
    "ntp.sim", "time.sim", "www.sim", "dns.sim",
    // Public names that sketches copied from tutorials use; there is no internet in the sim.
    "test.mosquitto.org", "broker.hivemq.com", "broker.emqx.io", "mqtt.eclipseprojects.io", "broker.mqttdashboard.com",
    "pool.ntp.org", "time.google.com", "time.nist.gov", "time.windows.com", "time.apple.com", "httpbin.org", "example.com",
];

impl Network {
    pub(super) fn gateway_listening(&self, port: u16) -> bool {
        matches!(port, PORT_MQTT | PORT_HTTP | PORT_ECHO)
    }

    pub(super) fn service_open(&mut self, c: ConnId, port: u16) {
        match port {
            PORT_MQTT => self.broker.open(c, self.now),
            PORT_HTTP => {
                self.http.insert(c, HttpSession::default());
            }
            _ => {}
        }
    }

    pub(super) fn service_data(&mut self, c: ConnId, port: u16, data: Vec<u8>) {
        match port {
            PORT_MQTT => {
                let out = self.broker.input(c, &data, self.now);
                self.apply_broker(out);
            }
            PORT_HTTP => {
                let client = self.conns.get(&c).map(|k| *k.ends[0].addr.ip()).unwrap_or(Ipv4Addr::UNSPECIFIED);
                let unix = self.unix_time(self.now);
                let Some(sess) = self.http.get_mut(&c) else { return };
                sess.buf.extend_from_slice(&data);
                let mut out = Vec::new();
                let mut close = false;
                while let Some((req, n)) = HttpRequest::parse(&sess.buf) {
                    sess.buf.drain(..n);
                    let keep = req.version == "HTTP/1.1" && !req.header("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"))
                        || req.header("connection").is_some_and(|v| v.eq_ignore_ascii_case("keep-alive"));
                    out.extend(http_handle(&req, client, unix).encode(keep));
                    if !keep {
                        close = true;
                        break;
                    }
                }
                if !out.is_empty() {
                    self.tcp_write_end(c, 1, &out);
                }
                if close {
                    self.tcp_close_end(c, 1);
                }
            }
            PORT_ECHO => self.tcp_write_end(c, 1, &data),
            _ => {}
        }
    }

    pub(super) fn service_closed(&mut self, c: ConnId, port: u16) {
        match port {
            PORT_MQTT => {
                let out = self.broker.closed(c, self.now);
                self.apply_broker(out);
            }
            PORT_HTTP => {
                self.http.remove(&c);
            }
            _ => {}
        }
    }

    pub(super) fn gateway_udp(&mut self, p: &Packet, data: Vec<u8>) {
        let reply = |net: &mut Network, payload: Vec<u8>| {
            let r = Packet { src: p.dst, dst: p.src, body: Body::Udp { data: payload }, from: Host::Gateway, to: Some(p.from) };
            net.transmit(r);
        };
        match p.dst.port() {
            PORT_DHCP_S => self.dhcp_server_receive(p, &data),
            PORT_DNS => {
                let Some(q) = DnsMessage::decode(&data) else { return };
                if q.response {
                    return;
                }
                let mut r = q.clone();
                r.response = true;
                match self.dns_lookup(&q.name) {
                    Some(ip) if q.qtype == 1 => r.answers.push((ip, 300)),
                    Some(_) => {}
                    None => r.rcode = 3,
                }
                let src = SocketAddrV4::new(self.config.gateway, PORT_DNS);
                let pk = Packet { src, dst: p.src, body: Body::Udp { data: r.encode() }, from: Host::Gateway, to: Some(p.from) };
                self.transmit(pk);
            }
            PORT_NTP => {
                if let Some(r) = wire::ntp_reply(&data, self.unix_time(self.now)) {
                    reply(self, r);
                }
            }
            PORT_ECHO => reply(self, data),
            _ => {}
        }
    }

    /// DNS resolution as the gateway's server sees it.
    pub fn dns_lookup(&self, name: &str) -> Option<Ipv4Addr> {
        let n = name.trim_end_matches('.').to_ascii_lowercase();
        if let Ok(ip) = n.parse::<Ipv4Addr>() {
            return Some(ip);
        }
        if let Some((_, ip)) = self.config.hosts.iter().find(|(h, _)| h.eq_ignore_ascii_case(&n)) {
            return Some(*ip);
        }
        let suffix = format!(".{}", self.config.domain);
        if let Some(host) = n.strip_suffix(&suffix) {
            for (i, s) in self.stations.iter().enumerate() {
                if (s.hostname.eq_ignore_ascii_case(host) || s.label.eq_ignore_ascii_case(host))
                    && let Some(ip) = self.ip_of(StationId(i as u32))
                {
                    return Some(ip);
                }
            }
        }
        if GATEWAY_NAMES.contains(&n.as_str()) || n.ends_with(".pool.ntp.org") {
            return Some(self.config.gateway);
        }
        None
    }

    // ----------------------------------------------------------------- DHCP server

    fn dhcp_server_receive(&mut self, p: &Packet, data: &[u8]) {
        let Some(m) = DhcpMessage::decode(data) else { return };
        if m.op != 1 {
            return;
        }
        let Host::Station(sta) = p.from else { return };
        let statics: HashSet<Ipv4Addr> = self.stations.iter().filter_map(|s| s.static_ip.map(|c| c.ip)).collect();
        let now = self.now;
        let cfg = self.config.clone();
        let mut reply = DhcpMessage::request(DhcpType::Offer, m.xid, m.chaddr);
        reply.op = 2;
        reply.server_id = Some(cfg.gateway);
        reply.subnet_mask = Some(cfg.netmask);
        reply.router = Some(cfg.gateway);
        reply.dns = Some(cfg.gateway);
        reply.domain = Some(cfg.domain.clone());
        reply.siaddr = cfg.gateway;
        match m.msg_type {
            DhcpType::Discover => {
                let Some(ip) = self.dhcp_server.allocate(m.chaddr, &cfg, now, &statics) else { return };
                // Hold the offer briefly so a parallel DISCOVER doesn't get the same address.
                self.dhcp_server.leases.entry(m.chaddr).or_insert(Lease { ip, expires: now + SimTime::from_secs(60) });
                reply.yiaddr = ip;
                reply.lease_secs = Some(cfg.lease_secs);
            }
            DhcpType::Request => {
                let wanted = m.requested_ip.unwrap_or(m.ciaddr);
                if m.server_id.is_some_and(|s| s != cfg.gateway) {
                    return; // chose another server
                }
                let ok = self.dhcp_server.leases.get(&m.chaddr).is_some_and(|l| l.ip == wanted)
                    || (self.dhcp_server.allocate(m.chaddr, &cfg, now, &statics) == Some(wanted));
                if ok {
                    self.dhcp_server.leases.insert(m.chaddr, Lease { ip: wanted, expires: now + SimTime::from_secs(cfg.lease_secs as u64) });
                    reply.msg_type = DhcpType::Ack;
                    reply.yiaddr = wanted;
                    reply.lease_secs = Some(cfg.lease_secs);
                } else {
                    reply.msg_type = DhcpType::Nak;
                }
            }
            DhcpType::Release => {
                self.dhcp_server.leases.remove(&m.chaddr);
                return;
            }
            _ => return,
        }
        let pk = Packet {
            src: SocketAddrV4::new(cfg.gateway, PORT_DHCP_S),
            dst: SocketAddrV4::new(Ipv4Addr::BROADCAST, PORT_DHCP_C),
            body: Body::Udp { data: reply.encode() },
            from: Host::Gateway,
            to: Some(Host::Station(sta)),
        };
        self.transmit(pk);
    }

    pub fn dhcp_leases(&self) -> Vec<([u8; 6], Ipv4Addr, SimTime)> {
        self.dhcp_server.leases()
    }

    // ----------------------------------------------------------------- DHCP client

    pub(super) fn dhcp_start(&mut self, id: StationId) {
        let xid = self.rng.next_u32();
        let Ok(s) = self.sta_mut(id) else { return };
        s.dhcp = DhcpClient { state: DhcpState::Selecting, xid, attempt: 0, offered: None, server: None, dgen: s.dhcp.dgen + 1 };
        self.dhcp_send(id);
    }

    fn dhcp_send(&mut self, id: StationId) {
        let Ok(s) = self.sta(id) else { return };
        let d = s.dhcp.clone();
        let (mac, host, epoch, cur_ip) = (s.mac, s.hostname.clone(), s.epoch, s.ip.map(|c| c.ip));
        let (ty, src_ip, dst_ip) = match d.state {
            DhcpState::Selecting => (DhcpType::Discover, Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST),
            DhcpState::Requesting => (DhcpType::Request, Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST),
            DhcpState::Renewing => (DhcpType::Request, cur_ip.unwrap_or(Ipv4Addr::UNSPECIFIED), d.server.unwrap_or(self.config.gateway)),
            _ => return,
        };
        let mut m = DhcpMessage::request(ty, d.xid, mac);
        m.hostname = Some(host);
        match d.state {
            DhcpState::Requesting => {
                m.requested_ip = d.offered;
                m.server_id = d.server;
            }
            DhcpState::Renewing => m.ciaddr = cur_ip.unwrap_or(Ipv4Addr::UNSPECIFIED),
            _ => {}
        }
        let p = Packet {
            src: SocketAddrV4::new(src_ip, PORT_DHCP_C),
            dst: SocketAddrV4::new(dst_ip, PORT_DHCP_S),
            body: Body::Udp { data: m.encode() },
            from: Host::Station(id),
            to: Some(Host::Gateway),
        };
        self.transmit(p);
        // Retransmission: 1 s, 2 s, 4 s, 8 s … (±10 %), renewals every 60 s.
        let backoff = if d.state == DhcpState::Renewing { 60.0 } else { (1u64 << d.attempt.min(4)) as f64 };
        let jitter = self.rng.range_f64(0.9, 1.1);
        let at = self.now + SimTime::from_secs_f64(backoff * jitter);
        self.schedule(at, Ev::Dhcp { sta: id, epoch, dgen: d.dgen });
    }

    pub(super) fn dhcp_timer(&mut self, id: StationId, epoch: u64, dgen: u64) {
        let Ok(s) = self.sta_mut(id) else { return };
        if s.epoch != epoch || s.dhcp.dgen != dgen || !matches!(s.state, WifiState::Connected { .. }) {
            return;
        }
        s.dhcp.dgen += 1;
        match s.dhcp.state {
            DhcpState::Selecting => {
                s.dhcp.attempt += 1;
                self.dhcp_send(id);
            }
            DhcpState::Requesting => {
                s.dhcp.attempt += 1;
                if s.dhcp.attempt > 3 {
                    self.dhcp_start(id);
                } else {
                    self.dhcp_send(id);
                }
            }
            DhcpState::Bound => {
                // T1: renew with the server that granted the lease.
                s.dhcp.state = DhcpState::Renewing;
                s.dhcp.attempt = 0;
                self.dhcp_send(id);
            }
            DhcpState::Renewing => {
                s.dhcp.attempt += 1;
                if s.dhcp.attempt > 4 {
                    s.dhcp.state = DhcpState::Selecting;
                    s.dhcp.attempt = 0;
                }
                self.dhcp_send(id);
            }
            DhcpState::Idle => {}
        }
    }

    pub(super) fn dhcp_client_receive(&mut self, id: StationId, data: &[u8]) {
        let Some(m) = DhcpMessage::decode(data) else { return };
        let lease_default = self.config.lease_secs;
        let gw = self.config.gateway;
        let Ok(s) = self.sta_mut(id) else { return };
        if m.op != 2 || m.xid != s.dhcp.xid || m.chaddr != s.mac {
            return;
        }
        match (m.msg_type, s.dhcp.state) {
            (DhcpType::Offer, DhcpState::Selecting) => {
                s.dhcp.state = DhcpState::Requesting;
                s.dhcp.offered = Some(m.yiaddr);
                s.dhcp.server = m.server_id;
                s.dhcp.attempt = 0;
                s.dhcp.dgen += 1;
                self.dhcp_send(id);
            }
            (DhcpType::Ack, DhcpState::Requesting | DhcpState::Renewing) => {
                let renewing = s.dhcp.state == DhcpState::Renewing;
                let cfg = IpConfig {
                    ip: m.yiaddr,
                    gateway: m.router.unwrap_or(gw),
                    netmask: m.subnet_mask.unwrap_or(Ipv4Addr::new(255, 255, 255, 0)),
                    dns: m.dns.unwrap_or(gw),
                };
                let changed = s.ip != Some(cfg);
                s.ip = Some(cfg);
                s.dhcp.state = DhcpState::Bound;
                s.dhcp.attempt = 0;
                s.dhcp.dgen += 1;
                let (epoch, dgen) = (s.epoch, s.dhcp.dgen);
                if !renewing || changed {
                    s.events.push_back(NetEvent::GotIp { ip: cfg.ip, gateway: cfg.gateway, netmask: cfg.netmask, dns: cfg.dns });
                }
                let lease = m.lease_secs.unwrap_or(lease_default).max(10);
                let t1 = self.now + SimTime::from_secs_f64(lease as f64 / 2.0);
                self.schedule(t1, Ev::Dhcp { sta: id, epoch, dgen });
            }
            (DhcpType::Nak, _) => {
                s.ip = None;
                self.dhcp_start(id);
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------- DNS client

    /// Resolves a host name through the gateway's DNS server (real query/response packets).
    /// Outcome: `DnsResolved`. IPv4 literals resolve immediately.
    pub fn dns_resolve(&mut self, id: StationId, name: &str, now: SimTime) -> Result<u32, NetError> {
        self.sync(now);
        let qid = self.rng.next_u32() & 0x7fff_ffff;
        if let Ok(ip) = name.trim().parse::<Ipv4Addr>() {
            self.push_event(id, NetEvent::DnsResolved { query: qid, name: name.into(), ip: Some(ip) });
            return Ok(qid);
        }
        self.ip_of(id).ok_or(NetError::NoIp)?;
        let port = self.ephemeral_port(id);
        let dns_id = (self.rng.next_u32() & 0xffff) as u16;
        let s = self.sta_mut(id)?;
        s.dns.insert(dns_id, DnsQuery { query: qid, name: name.trim().into(), port, attempt: 0 });
        self.dns_send(id, dns_id);
        Ok(qid)
    }

    fn dns_send(&mut self, id: StationId, dns_id: u16) {
        let Ok(s) = self.sta(id) else { return };
        let Some(q) = s.dns.get(&dns_id).cloned() else { return };
        let Some(cfg) = s.ip else { return };
        let p = Packet {
            src: SocketAddrV4::new(cfg.ip, q.port),
            dst: SocketAddrV4::new(cfg.dns, PORT_DNS),
            body: Body::Udp { data: DnsMessage::query(dns_id, &q.name).encode() },
            from: Host::Station(id),
            to: None,
        };
        self.transmit(p);
        self.schedule(self.now + SimTime::from_secs(1), Ev::DnsRetry { sta: id, id: dns_id, attempt: q.attempt });
    }

    pub(super) fn dns_retry(&mut self, id: StationId, dns_id: u16, attempt: u32) {
        let Ok(s) = self.sta_mut(id) else { return };
        let Some(q) = s.dns.get_mut(&dns_id) else { return };
        if q.attempt != attempt {
            return;
        }
        if q.attempt >= 3 {
            let q = s.dns.remove(&dns_id).expect("present");
            s.events.push_back(NetEvent::DnsResolved { query: q.query, name: q.name, ip: None });
            return;
        }
        q.attempt += 1;
        self.dns_send(id, dns_id);
    }

    pub(super) fn dns_client_receive(&mut self, id: StationId, dns_id: u16, data: &[u8]) {
        let Some(m) = DnsMessage::decode(data) else { return };
        if !m.response || m.id != dns_id {
            return;
        }
        let Ok(s) = self.sta_mut(id) else { return };
        let Some(q) = s.dns.remove(&dns_id) else { return };
        let ip = if m.rcode == 0 { m.answers.first().map(|a| a.0) } else { None };
        s.events.push_back(NetEvent::DnsResolved { query: q.query, name: q.name, ip });
    }
}
