//! TCP for the virtual network: three-way handshake, byte sequence numbers, cumulative ACKs,
//! retransmission on RTO (RFC 6298 estimator, Karn's rule, exponential back-off), out-of-order
//! reassembly, FIN/RST. Simplifications: no window/congestion control (IoT payloads are tiny),
//! no TIME_WAIT, and a FIN from the peer closes both directions (no half-close).
//!
//! A connection is one shared record with two ends. Each end lives and dies on its own: when
//! one side gives up (timeout, link loss, abort) only that end is marked dead; the other side
//! finds out the way a real host would — an RST in answer to its next segment, or its own
//! retransmission time-out.

use super::*;
use std::collections::BTreeMap;
use wire::tcp_flags::*;

const MIN_RTO: f64 = 0.2;
const MAX_RTO: f64 = 30.0;
const INIT_RTO: f64 = 1.0;
const MAX_SYN_RETRIES: u32 = 4;
const MAX_DATA_RETRIES: u32 = 8;

#[derive(Debug, Clone, PartialEq)]
struct Seg {
    seq: u32,
    data: Vec<u8>,
    syn: bool,
    fin: bool,
    sent_at: SimTime,
    retx: bool,
}

impl Seg {
    fn len(&self) -> u32 {
        self.data.len() as u32 + self.syn as u32 + self.fin as u32
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TcpEnd {
    pub host: Host,
    pub addr: SocketAddrV4,
    pub sock: Option<SockId>,
    snd_una: u32,
    snd_nxt: u32,
    unacked: VecDeque<Seg>,
    rcv_nxt: u32,
    ooo: BTreeMap<u32, (Vec<u8>, bool)>,
    syn_rcvd: bool,
    fin_sent: bool,
    fin_rcvd: bool,
    srtt: Option<f64>,
    rttvar: f64,
    rto: f64,
    rto_gen: u64,
    retries: u32,
    /// Handshake complete for this end.
    est: bool,
    pub alive: bool,
    /// The owner has been told the connection is over.
    notified: bool,
}

impl TcpEnd {
    fn new(host: Host, addr: SocketAddrV4, iss: u32) -> Self {
        TcpEnd {
            host,
            addr,
            sock: None,
            snd_una: iss,
            snd_nxt: iss,
            unacked: VecDeque::new(),
            rcv_nxt: 0,
            ooo: BTreeMap::new(),
            syn_rcvd: false,
            fin_sent: false,
            fin_rcvd: false,
            srtt: None,
            rttvar: 0.0,
            rto: INIT_RTO,
            rto_gen: 0,
            retries: 0,
            est: false,
            alive: true,
            notified: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TcpConn {
    pub ends: [TcpEnd; 2],
    /// Gateway service port, if end 1 is the gateway.
    pub service: Option<u16>,
    /// Station listener that accepts end 1.
    pub listener: Option<SockId>,
}

/// Public view of a TCP socket's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TcpState {
    Connecting,
    Established,
    Closed,
}

fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

enum Listener {
    Service(u16),
    Socket(SockId),
}

impl Network {
    // ================================================================ public socket API

    /// Opens a TCP connection from the station to `dst`. Outcome: `TcpConnected` or
    /// `TcpConnectFailed` (refused by RST, or SYN retries exhausted after ~31 s).
    pub fn tcp_connect(&mut self, id: StationId, dst: SocketAddrV4, now: SimTime) -> Result<SockId, NetError> {
        self.sync(now);
        let ip = self.ip_of(id).ok_or(NetError::NoIp)?;
        let port = self.ephemeral_port(id);
        let c = self.next_conn;
        self.next_conn += 1;
        let iss = self.rng.next_u32();
        let mut a = TcpEnd::new(Host::Station(id), SocketAddrV4::new(ip, port), iss);
        let sock = self.new_sock(id, SockKind::Tcp { conn: Some(c), end: 0 });
        a.sock = Some(sock);
        // Placeholder for the passive side until the SYN arrives.
        let mut b = TcpEnd::new(Host::Gateway, dst, 0);
        b.alive = false;
        b.notified = true;
        self.conns.insert(c, TcpConn { ends: [a, b], service: None, listener: None });
        self.queue_seg(c, 0, Vec::new(), true, false);
        Ok(sock)
    }

    /// Listens on a TCP port; connections arrive as `TcpAccepted`.
    pub fn tcp_listen(&mut self, id: StationId, port: u16) -> Result<SockId, NetError> {
        self.sta(id)?;
        if self.sockets.values().any(|s| s.owner == id && s.kind == SockKind::Listener { port }) {
            return Err(NetError::PortInUse(port));
        }
        Ok(self.new_sock(id, SockKind::Listener { port }))
    }

    /// Queues data on an established connection. Returns the number of bytes accepted.
    pub fn tcp_send(&mut self, sock: SockId, data: &[u8], now: SimTime) -> Result<usize, NetError> {
        self.sync(now);
        let s = self.sockets.get(&sock).ok_or(NetError::NoSocket)?;
        let SockKind::Tcp { conn: Some(c), end } = s.kind else { return Err(NetError::NotConnected) };
        let ok = self.conns.get(&c).is_some_and(|k| k.ends[end].est && !k.ends[end].fin_sent && k.ends[end].alive);
        if !ok {
            return Err(NetError::NotConnected);
        }
        self.tcp_write_end(c, end, data);
        Ok(data.len())
    }

    /// Aborts a connection with RST.
    pub fn tcp_abort(&mut self, sock: SockId, now: SimTime) {
        self.sync(now);
        let Some(s) = self.sockets.remove(&sock) else { return };
        if let SockKind::Tcp { conn: Some(c), end } = s.kind {
            self.abort_end(c, end);
        }
    }

    pub fn tcp_state(&self, sock: SockId) -> TcpState {
        match self.sockets.get(&sock).map(|s| &s.kind) {
            Some(SockKind::Tcp { conn: Some(c), end }) => match self.conns.get(c) {
                Some(k) if !k.ends[*end].alive => TcpState::Closed,
                Some(k) if k.ends[*end].est => TcpState::Established,
                Some(_) => TcpState::Connecting,
                None => TcpState::Closed,
            },
            _ => TcpState::Closed,
        }
    }

    /// (local, remote) addresses of a TCP socket.
    pub fn tcp_addrs(&self, sock: SockId) -> Option<(SocketAddrV4, SocketAddrV4)> {
        let SockKind::Tcp { conn: Some(c), end } = self.sockets.get(&sock)?.kind else { return None };
        let k = self.conns.get(&c)?;
        Some((k.ends[end].addr, k.ends[1 - end].addr))
    }

    /// Bytes sent but not yet acknowledged.
    pub fn tcp_unacked(&self, sock: SockId) -> usize {
        let Some(SockKind::Tcp { conn: Some(c), end }) = self.sockets.get(&sock).map(|s| s.kind.clone()) else { return 0 };
        self.conns.get(&c).map(|k| k.ends[end].unacked.iter().map(|s| s.data.len()).sum()).unwrap_or(0)
    }

    /// Number of live connection records (tests / diagnostics).
    pub fn tcp_connection_count(&self) -> usize {
        self.conns.len()
    }

    // ================================================================ sending

    fn queue_seg(&mut self, c: ConnId, end: usize, data: Vec<u8>, syn: bool, fin: bool) {
        let now = self.now;
        let Some(k) = self.conns.get_mut(&c) else { return };
        let e = &mut k.ends[end];
        let seg = Seg { seq: e.snd_nxt, data, syn, fin, sent_at: now, retx: false };
        e.snd_nxt = e.snd_nxt.wrapping_add(seg.len());
        let arm = e.unacked.is_empty();
        e.unacked.push_back(seg.clone());
        self.send_seg(c, end, &seg);
        if arm {
            self.arm_rto(c, end);
        }
    }

    fn send_seg(&mut self, c: ConnId, end: usize, seg: &Seg) {
        let Some(k) = self.conns.get(&c) else { return };
        let e = &k.ends[end];
        let mut flags = 0;
        if seg.syn {
            flags |= SYN;
        }
        if seg.fin {
            flags |= FIN;
        }
        if e.syn_rcvd {
            flags |= ACK;
        }
        if !seg.data.is_empty() {
            flags |= PSH;
        }
        let ack = if e.syn_rcvd { e.rcv_nxt } else { 0 };
        let p = Packet {
            src: e.addr,
            dst: k.ends[1 - end].addr,
            body: Body::Tcp { conn: c, from_end: end, flags, seq: seg.seq, ack, data: seg.data.clone() },
            from: e.host,
            to: None,
        };
        self.transmit(p);
    }

    fn send_ack(&mut self, c: ConnId, end: usize) {
        let Some(k) = self.conns.get(&c) else { return };
        let e = &k.ends[end];
        let p = Packet {
            src: e.addr,
            dst: k.ends[1 - end].addr,
            body: Body::Tcp { conn: c, from_end: end, flags: ACK, seq: e.snd_nxt, ack: e.rcv_nxt, data: vec![] },
            from: e.host,
            to: None,
        };
        self.transmit(p);
    }

    fn arm_rto(&mut self, c: ConnId, end: usize) {
        let now = self.now;
        let Some(k) = self.conns.get_mut(&c) else { return };
        let e = &mut k.ends[end];
        e.rto_gen += 1;
        let at = now + SimTime::from_secs_f64(e.rto);
        let epoch = e.rto_gen;
        self.schedule(at, Ev::TcpRto { conn: c, end, epoch });
    }

    pub(super) fn tcp_rto(&mut self, c: ConnId, end: usize, epoch: u64) {
        let now = self.now;
        let Some(k) = self.conns.get_mut(&c) else { return };
        let e = &mut k.ends[end];
        if e.rto_gen != epoch || e.unacked.is_empty() || !e.alive {
            return;
        }
        e.retries += 1;
        let syn = e.unacked.front().is_some_and(|s| s.syn);
        let limit = if syn { MAX_SYN_RETRIES } else { MAX_DATA_RETRIES };
        if e.retries > limit {
            self.kill_end(c, end, CloseReason::Timeout);
            return;
        }
        e.rto = (e.rto * 2.0).min(MAX_RTO);
        let seg = {
            let s = e.unacked.front_mut().expect("non-empty");
            s.retx = true;
            s.sent_at = now;
            s.clone()
        };
        self.send_seg(c, end, &seg);
        self.arm_rto(c, end);
    }

    /// Writes application data from `end` (split into MSS-sized segments).
    pub(crate) fn tcp_write_end(&mut self, c: ConnId, end: usize, data: &[u8]) {
        let ok = self.conns.get(&c).is_some_and(|k| k.ends[end].est && !k.ends[end].fin_sent && k.ends[end].alive);
        if !ok {
            return;
        }
        for chunk in data.chunks(self.mss) {
            self.queue_seg(c, end, chunk.to_vec(), false, false);
        }
    }

    /// Graceful close from `end` (FIN after queued data). The owner is not notified.
    pub(crate) fn tcp_close_end(&mut self, c: ConnId, end: usize) {
        let Some(k) = self.conns.get_mut(&c) else { return };
        let established = k.ends[end].est;
        let e = &mut k.ends[end];
        e.notified = true;
        let sock = e.sock.take();
        let (alive, fin_sent) = (e.alive, e.fin_sent);
        if let Some(s) = sock
            && let Some(so) = self.sockets.get_mut(&s)
        {
            so.kind = SockKind::Tcp { conn: None, end };
        }
        if !alive || fin_sent {
            self.maybe_finish(c);
            return;
        }
        if !established {
            self.abort_end(c, end);
            return;
        }
        if let Some(k) = self.conns.get_mut(&c) {
            k.ends[end].fin_sent = true;
        }
        self.queue_seg(c, end, Vec::new(), false, true);
    }

    /// Kills `end` locally and tells the peer with an RST.
    fn abort_end(&mut self, c: ConnId, end: usize) {
        let Some(k) = self.conns.get_mut(&c) else { return };
        let peer_live = k.ends[1 - end].alive;
        let peer_addr = k.ends[1 - end].addr;
        let e = &mut k.ends[end];
        e.notified = true;
        e.sock = None;
        let p = Packet {
            src: e.addr,
            dst: peer_addr,
            body: Body::Tcp { conn: c, from_end: end, flags: RST | ACK, seq: e.snd_nxt, ack: e.rcv_nxt, data: vec![] },
            from: e.host,
            to: None,
        };
        let was_alive = e.alive;
        self.kill_end(c, end, CloseReason::Local);
        if peer_live && was_alive {
            self.transmit(p);
        }
    }

    /// Marks one end dead, notifying its owner once; forgets the connection when both are dead.
    fn kill_end(&mut self, c: ConnId, i: usize, reason: CloseReason) {
        let Some(k) = self.conns.get(&c) else { return };
        let notify = !k.ends[i].notified;
        let failed = !k.ends[i].est && i == 0;
        let (host, sock, est, service) = (k.ends[i].host, k.ends[i].sock, k.ends[i].est, k.service);
        if notify {
            self.notify_closed(c, i, host, sock, est, service, reason, failed);
        }
        if let Some(k) = self.conns.get_mut(&c) {
            let e = &mut k.ends[i];
            e.alive = false;
            e.notified = true;
            e.sock = None;
            e.unacked.clear();
            e.rto_gen += 1;
            if k.ends.iter().all(|e| !e.alive) {
                self.conns.remove(&c);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn notify_closed(&mut self, c: ConnId, i: usize, host: Host, sock: Option<SockId>, est: bool, service: Option<u16>, reason: CloseReason, failed: bool) {
        match host {
            Host::Station(s) => {
                if let Some(sock) = sock {
                    if let Some(so) = self.sockets.get_mut(&sock) {
                        so.kind = SockKind::Tcp { conn: None, end: i };
                    }
                    let ev = if failed { NetEvent::TcpConnectFailed { sock, reason } } else { NetEvent::TcpClosed { sock, reason } };
                    self.push_event(s, ev);
                }
            }
            Host::Gateway => {
                if est
                    && let Some(port) = service
                {
                    self.service_closed(c, port);
                }
            }
        }
    }

    /// Removes a connection that is fully closed in both directions.
    fn maybe_finish(&mut self, c: ConnId) {
        let done = self
            .conns
            .get(&c)
            .is_some_and(|k| k.ends.iter().all(|e| !e.alive || (e.fin_sent && e.unacked.is_empty() && e.fin_rcvd)));
        if done {
            self.conns.remove(&c);
        }
    }

    /// The station lost its link: every connection end it owns dies (the peer finds out through
    /// retransmission time-outs, keep-alive, or an RST once the station is back).
    pub(super) fn kill_station_conns(&mut self, id: StationId) {
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        for c in ids {
            for i in 0..2 {
                let mine = self.conns.get(&c).is_some_and(|k| k.ends[i].host == Host::Station(id) && k.ends[i].alive);
                if mine {
                    self.kill_end(c, i, CloseReason::LinkLost);
                }
            }
        }
    }

    // ================================================================ receiving

    /// A TCP segment arrived at host `at`.
    pub(super) fn tcp_receive(&mut self, p: Packet, at: Host) {
        let Body::Tcp { conn: c, from_end, flags, seq, ack, data } = p.body.clone() else { return };
        let r = 1 - from_end;
        if !self.conns.contains_key(&c) {
            if flags & RST == 0 {
                self.rst_reply(&p, at);
            }
            return;
        }
        // Passive open (first SYN) or a retransmitted SYN.
        if flags & SYN != 0 && flags & ACK == 0 {
            if !self.conns[&c].ends[r].syn_rcvd {
                let Some(listener) = self.find_listener(at, p.dst.port()) else {
                    self.rst_reply(&p, at);
                    return;
                };
                let iss = self.rng.next_u32();
                let k = self.conns.get_mut(&c).expect("checked");
                let mut e = TcpEnd::new(at, p.dst, iss);
                e.rcv_nxt = seq.wrapping_add(1);
                e.syn_rcvd = true;
                k.ends[r] = e;
                match listener {
                    Listener::Service(port) => k.service = Some(port),
                    Listener::Socket(s) => k.listener = Some(s),
                }
                self.queue_seg(c, r, Vec::new(), true, false);
            } else if let Some(seg) = self.conns[&c].ends[r].unacked.front().cloned()
                && self.conns[&c].ends[r].alive
            {
                self.send_seg(c, r, &seg); // our SYN-ACK was lost
            }
            return;
        }
        if !self.conns[&c].ends[r].alive {
            if flags & RST == 0 {
                self.rst_reply(&p, at);
            }
            return;
        }
        if flags & RST != 0 {
            let reason = if self.conns[&c].ends[r].est { CloseReason::Reset } else { CloseReason::Refused };
            self.kill_end(c, r, reason);
            return;
        }
        if flags & ACK != 0 {
            self.process_ack(c, r, ack);
        }
        let Some(k) = self.conns.get_mut(&c) else { return };
        // Active open completes on SYN-ACK.
        if flags & SYN != 0 {
            let e = &mut k.ends[r];
            if !e.syn_rcvd {
                e.syn_rcvd = true;
                e.rcv_nxt = seq.wrapping_add(1);
                e.est = true;
                if let (Host::Station(s), Some(sock)) = (e.host, e.sock) {
                    self.push_event(s, NetEvent::TcpConnected { sock });
                }
            }
            self.send_ack(c, r);
            return;
        }
        let fin = flags & FIN != 0;
        if data.is_empty() && !fin {
            self.maybe_finish(c);
            return;
        }
        let mut delivered: Vec<u8> = Vec::new();
        let mut fin_now = false;
        {
            let e = &mut k.ends[r];
            if seq_lt(e.rcv_nxt, seq) {
                e.ooo.entry(seq).or_insert((data, fin));
            } else if !e.fin_rcvd {
                let mut take = |e: &mut TcpEnd, s: u32, d: &[u8], f: bool| {
                    let skip = e.rcv_nxt.wrapping_sub(s) as usize;
                    if skip > d.len() {
                        return;
                    }
                    delivered.extend_from_slice(&d[skip..]);
                    e.rcv_nxt = e.rcv_nxt.wrapping_add((d.len() - skip) as u32);
                    if f {
                        e.rcv_nxt = e.rcv_nxt.wrapping_add(1);
                        e.fin_rcvd = true;
                        fin_now = true;
                    }
                };
                take(e, seq, &data, fin);
                while !e.fin_rcvd {
                    let Some((&s, _)) = e.ooo.iter().next() else { break };
                    if seq_lt(e.rcv_nxt, s) {
                        break;
                    }
                    let (d, f) = e.ooo.remove(&s).expect("present");
                    take(e, s, &d, f);
                }
            }
        }
        // Acknowledge (duplicates too, the earlier ACK may have been lost), then hand data up.
        self.send_ack(c, r);
        if !delivered.is_empty() {
            self.deliver_app(c, r, delivered);
        }
        if fin_now {
            self.peer_fin(c, r);
        }
        self.maybe_finish(c);
    }

    fn process_ack(&mut self, c: ConnId, r: usize, ack: u32) {
        let now = self.now;
        let Some(k) = self.conns.get_mut(&c) else { return };
        let e = &mut k.ends[r];
        if !(seq_lt(e.snd_una, ack) && seq_le(ack, e.snd_nxt)) {
            return;
        }
        let mut acked_bytes = 0usize;
        let mut sample: Option<f64> = None;
        let mut syn_acked = false;
        while let Some(s) = e.unacked.front() {
            if !seq_le(s.seq.wrapping_add(s.len()), ack) {
                break;
            }
            let s = e.unacked.pop_front().expect("front");
            acked_bytes += s.data.len();
            syn_acked |= s.syn;
            if !s.retx {
                sample = Some((now - s.sent_at).as_secs_f64());
            }
        }
        e.snd_una = ack;
        e.retries = 0;
        if let Some(m) = sample {
            match e.srtt {
                None => {
                    e.srtt = Some(m);
                    e.rttvar = m / 2.0;
                }
                Some(srtt) => {
                    e.rttvar = 0.75 * e.rttvar + 0.25 * (srtt - m).abs();
                    e.srtt = Some(0.875 * srtt + 0.125 * m);
                }
            }
            e.rto = (e.srtt.unwrap_or(INIT_RTO) + (4.0 * e.rttvar).max(0.01)).clamp(MIN_RTO, MAX_RTO);
        }
        let has_more = !e.unacked.is_empty();
        let (host, sock) = (e.host, e.sock);
        if has_more {
            self.arm_rto(c, r);
        } else if let Some(k) = self.conns.get_mut(&c) {
            k.ends[r].rto_gen += 1; // disarm
        }
        if acked_bytes > 0
            && let (Host::Station(s), Some(sock)) = (host, sock)
        {
            self.push_event(s, NetEvent::TcpAcked { sock, bytes: acked_bytes });
        }
        // Passive open completes when our SYN is acknowledged.
        let Some(k) = self.conns.get_mut(&c) else { return };
        if syn_acked && !k.ends[r].est && r == 1 {
            k.ends[r].est = true;
            let (service, listener, remote, host) = (k.service, k.listener, k.ends[0].addr, k.ends[1].host);
            if let Some(port) = service {
                self.service_open(c, port);
            } else if let (Some(l), Host::Station(s)) = (listener, host) {
                let sock = self.new_sock(s, SockKind::Tcp { conn: Some(c), end: 1 });
                if let Some(k) = self.conns.get_mut(&c) {
                    k.ends[1].sock = Some(sock);
                }
                self.push_event(s, NetEvent::TcpAccepted { listener: l, sock, remote });
            }
        }
    }

    fn deliver_app(&mut self, c: ConnId, r: usize, data: Vec<u8>) {
        let Some(k) = self.conns.get(&c) else { return };
        match k.ends[r].host {
            Host::Station(s) => {
                if let Some(sock) = k.ends[r].sock {
                    self.push_event(s, NetEvent::TcpData { sock, data });
                }
            }
            Host::Gateway => {
                if let Some(port) = k.service {
                    self.service_data(c, port, data);
                }
            }
        }
    }

    fn peer_fin(&mut self, c: ConnId, r: usize) {
        let Some(k) = self.conns.get(&c) else { return };
        let e = &k.ends[r];
        if !e.notified {
            let (host, sock, est, service) = (e.host, e.sock, e.est, k.service);
            self.notify_closed(c, r, host, sock, est, service, CloseReason::PeerClosed, false);
        }
        self.tcp_close_end(c, r);
    }

    fn rst_reply(&mut self, p: &Packet, at: Host) {
        let Body::Tcp { conn, from_end, seq, data, flags, .. } = &p.body else { return };
        let ack = seq.wrapping_add(data.len() as u32 + (flags & (SYN | FIN) != 0) as u32);
        let r = Packet {
            src: p.dst,
            dst: p.src,
            body: Body::Tcp { conn: *conn, from_end: 1 - from_end, flags: RST | ACK, seq: 0, ack, data: vec![] },
            from: at,
            to: Some(p.from),
        };
        self.transmit(r);
    }

    fn find_listener(&self, at: Host, port: u16) -> Option<Listener> {
        match at {
            Host::Gateway => self.gateway_listening(port).then_some(Listener::Service(port)),
            Host::Station(s) => self
                .sockets
                .iter()
                .find(|(_, so)| so.owner == s && so.kind == SockKind::Listener { port })
                .map(|(k, _)| Listener::Socket(*k)),
        }
    }
}
