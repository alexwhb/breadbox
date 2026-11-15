//! The built-in MQTT 3.1.1 broker (`RF-2`), transport-agnostic: the network feeds it the bytes
//! each TCP connection carries and sends back what it returns. Supports QoS 0/1 (QoS 2
//! publishes are accepted with the full PUBREC/PUBREL/PUBCOMP handshake and delivered at
//! QoS ≤ 1), retained messages, `+`/`#` wildcards, persistent sessions with queued QoS 1
//! messages, keep-alive timeouts (1.5 × keep-alive), last will, session takeover, and in-process
//! **virtual clients** (MQTT explorer, LAN bridge, LoRa gateway).

use super::codec::{self, Connect, MqttError, Packet, Publish, StreamDecoder, Will};
use bb_core::SimTime;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Identifies one transport connection (or virtual client) to the broker.
pub type ConnKey = u64;

/// Virtual client keys live above this bit.
pub const VIRTUAL_BIT: ConnKey = 1 << 62;

/// An application message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

impl Message {
    pub fn new(topic: &str, payload: impl Into<Vec<u8>>, qos: u8, retain: bool) -> Self {
        Message {
            topic: topic.into(),
            payload: payload.into(),
            qos,
            retain,
        }
    }
    pub fn payload_str(&self) -> String {
        String::from_utf8_lossy(&self.payload).into_owned()
    }
}

/// A message as received by a virtual client.
#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub time: SimTime,
    pub message: Message,
}

/// What the broker wants the transport to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerOut {
    Send(ConnKey, Vec<u8>),
    /// Close the transport connection (protocol error, keep-alive timeout, takeover, DISCONNECT).
    Close(ConnKey),
}

/// Broker-level events (MQTT explorer history, serial-monitor annotations, packet log).
#[derive(Debug, Clone, PartialEq)]
pub enum BrokerEvent {
    Connected {
        key: ConnKey,
        client: String,
        clean: bool,
        keep_alive: u16,
    },
    Rejected {
        key: ConnKey,
        client: String,
        code: u8,
    },
    Disconnected {
        key: ConnKey,
        client: String,
        graceful: bool,
        reason: String,
    },
    Published {
        key: ConnKey,
        client: String,
        message: Message,
        size: usize,
    },
    Subscribed {
        key: ConnKey,
        client: String,
        filters: Vec<(String, u8)>,
    },
    WillPublished {
        client: String,
        message: Message,
    },
}

/// One entry of the explorer's message history.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub time: SimTime,
    pub client: String,
    pub message: Message,
}

#[derive(Debug, Clone)]
struct Conn {
    decoder: StreamDecoder,
    client: Option<String>,
    keep_alive: u16,
    last_rx: SimTime,
}

#[derive(Debug, Clone)]
struct Session {
    conn: Option<ConnKey>,
    clean: bool,
    subs: Vec<(String, u8)>,
    /// Outgoing QoS 1 messages awaiting PUBACK, by packet id.
    inflight: BTreeMap<u16, Message>,
    /// QoS ≥ 1 messages queued while a persistent session is offline.
    queued: VecDeque<Message>,
    /// Incoming QoS 2 packet ids awaiting PUBREL.
    qos2_in: Vec<u16>,
    next_pid: u16,
    will: Option<Will>,
}

impl Session {
    fn new(clean: bool) -> Self {
        Session {
            conn: None,
            clean,
            subs: vec![],
            inflight: BTreeMap::new(),
            queued: VecDeque::new(),
            qos2_in: vec![],
            next_pid: 1,
            will: None,
        }
    }
    fn pid(&mut self) -> u16 {
        loop {
            let p = self.next_pid;
            self.next_pid = self.next_pid.wrapping_add(1).max(1);
            if !self.inflight.contains_key(&p) {
                return p;
            }
        }
    }
}

/// Broker configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct BrokerConfig {
    /// Required credentials (`None` = anonymous allowed).
    pub credentials: Option<(String, String)>,
    /// Maximum messages kept in the explorer history.
    pub history: usize,
    /// Maximum queued messages per offline session.
    pub max_queued: usize,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        BrokerConfig {
            credentials: None,
            history: 2000,
            max_queued: 1000,
        }
    }
}

/// Broker statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BrokerStats {
    pub received: u64,
    pub delivered: u64,
    pub clients: usize,
    pub retained: usize,
}

#[derive(Debug, Clone)]
pub struct Broker {
    pub config: BrokerConfig,
    conns: HashMap<ConnKey, Conn>,
    sessions: HashMap<String, Session>,
    retained: BTreeMap<String, Message>,
    out: Vec<BrokerOut>,
    events: Vec<BrokerEvent>,
    history: VecDeque<HistoryEntry>,
    inboxes: HashMap<ConnKey, Vec<Received>>,
    next_virtual: ConnKey,
    next_auto_id: u32,
    stats: BrokerStats,
    now: SimTime,
}

impl Default for Broker {
    fn default() -> Self {
        Broker::new(BrokerConfig::default())
    }
}

impl Broker {
    pub fn new(config: BrokerConfig) -> Self {
        Broker {
            config,
            conns: HashMap::new(),
            sessions: HashMap::new(),
            retained: BTreeMap::new(),
            out: vec![],
            events: vec![],
            history: VecDeque::new(),
            inboxes: HashMap::new(),
            next_virtual: VIRTUAL_BIT,
            next_auto_id: 1,
            stats: BrokerStats::default(),
            now: SimTime::ZERO,
        }
    }

    // ------------------------------------------------------------------- transport side

    /// A transport connection was opened.
    pub fn open(&mut self, key: ConnKey, now: SimTime) {
        self.now = now;
        self.conns.insert(
            key,
            Conn {
                decoder: StreamDecoder::default(),
                client: None,
                keep_alive: 0,
                last_rx: now,
            },
        );
    }

    /// Bytes arrived on a connection. Returns the transport actions to perform.
    pub fn input(&mut self, key: ConnKey, data: &[u8], now: SimTime) -> Vec<BrokerOut> {
        self.now = now;
        let Some(c) = self.conns.get_mut(&key) else {
            return self.take_out();
        };
        c.last_rx = now;
        c.decoder.push(data);
        while let Some(c) = self.conns.get_mut(&key) {
            match c.decoder.next_packet() {
                Ok(Some(p)) => self.handle(key, p),
                Ok(None) => break,
                Err(e) => {
                    self.drop_conn(key, false, &format!("{e}"));
                    self.out.push(BrokerOut::Close(key));
                    break;
                }
            }
        }
        self.take_out()
    }

    /// The transport connection closed (peer FIN/RST, link lost). Publishes the will unless the
    /// client sent DISCONNECT.
    pub fn closed(&mut self, key: ConnKey, now: SimTime) -> Vec<BrokerOut> {
        self.now = now;
        self.drop_conn(key, false, "connection lost");
        self.take_out()
    }

    /// Keep-alive enforcement; call at/after [`Broker::next_deadline`].
    pub fn tick(&mut self, now: SimTime) -> Vec<BrokerOut> {
        self.now = now;
        let expired: Vec<ConnKey> = self
            .conns
            .iter()
            .filter(|(_, c)| c.keep_alive > 0 && now >= c.last_rx + keep_alive_limit(c.keep_alive))
            .map(|(k, _)| *k)
            .collect();
        for k in expired {
            self.drop_conn(k, false, "keep-alive timeout");
            self.out.push(BrokerOut::Close(k));
        }
        self.take_out()
    }

    /// Earliest keep-alive expiry.
    pub fn next_deadline(&self) -> Option<SimTime> {
        self.conns
            .values()
            .filter(|c| c.keep_alive > 0 && c.client.is_some())
            .map(|c| c.last_rx + keep_alive_limit(c.keep_alive))
            .min()
    }

    /// Broker events since the last call.
    pub fn take_events(&mut self) -> Vec<BrokerEvent> {
        std::mem::take(&mut self.events)
    }

    fn take_out(&mut self) -> Vec<BrokerOut> {
        std::mem::take(&mut self.out)
    }

    // ------------------------------------------------------------------- virtual clients

    /// Connects an in-process client (clean session). Returns its key.
    pub fn virtual_connect(&mut self, client_id: &str, now: SimTime) -> ConnKey {
        let key = self.next_virtual;
        self.next_virtual += 1;
        self.open(key, now);
        self.inboxes.insert(key, vec![]);
        self.handle(key, Packet::Connect(Connect::new(client_id, 0)));
        self.flush_virtual();
        key
    }

    pub fn virtual_subscribe(
        &mut self,
        key: ConnKey,
        filter: &str,
        qos: u8,
        now: SimTime,
    ) -> Vec<BrokerOut> {
        self.now = now;
        self.handle(
            key,
            Packet::Subscribe {
                pid: 1,
                topics: vec![(filter.into(), qos.min(1))],
            },
        );
        self.flush_virtual();
        self.take_out()
    }

    pub fn virtual_unsubscribe(
        &mut self,
        key: ConnKey,
        filter: &str,
        now: SimTime,
    ) -> Vec<BrokerOut> {
        self.now = now;
        self.handle(
            key,
            Packet::Unsubscribe {
                pid: 1,
                topics: vec![filter.into()],
            },
        );
        self.flush_virtual();
        self.take_out()
    }

    /// Publishes as a virtual client. Returns transport actions (deliveries to TCP clients).
    pub fn virtual_publish(&mut self, key: ConnKey, msg: Message, now: SimTime) -> Vec<BrokerOut> {
        self.now = now;
        let qos = msg.qos.min(1);
        let p = Publish {
            dup: false,
            qos,
            retain: msg.retain,
            topic: msg.topic,
            pid: (qos > 0).then_some(1),
            payload: msg.payload,
        };
        self.handle(key, Packet::Publish(p));
        self.flush_virtual();
        self.take_out()
    }

    pub fn virtual_disconnect(&mut self, key: ConnKey, now: SimTime) -> Vec<BrokerOut> {
        self.now = now;
        self.handle(key, Packet::Disconnect);
        self.inboxes.remove(&key);
        self.take_out()
    }

    /// Messages waiting in a virtual client's inbox.
    pub fn virtual_pending(&self, key: ConnKey) -> usize {
        self.inboxes.get(&key).map_or(0, |v| v.len())
    }

    /// Messages delivered to a virtual client since the last call.
    pub fn virtual_take(&mut self, key: ConnKey) -> Vec<Received> {
        self.inboxes
            .get_mut(&key)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Moves packets addressed to virtual clients into their inboxes (acking QoS 1).
    fn flush_virtual(&mut self) {
        let mut i = 0;
        while i < self.out.len() {
            let BrokerOut::Send(k, _) = &self.out[i] else {
                i += 1;
                continue;
            };
            if k & VIRTUAL_BIT == 0 {
                i += 1;
                continue;
            }
            let k = *k;
            let BrokerOut::Send(_, bytes) = self.out.remove(i) else {
                unreachable!()
            };
            let mut off = 0;
            while let Ok(Some((p, n))) = codec::decode(&bytes[off..]) {
                off += n;
                if let Packet::Publish(p) = p {
                    if let Some(pid) = p.pid {
                        self.handle(k, Packet::PubAck(pid));
                    }
                    if let Some(inbox) = self.inboxes.get_mut(&k) {
                        inbox.push(Received {
                            time: self.now,
                            message: Message {
                                topic: p.topic,
                                payload: p.payload,
                                qos: p.qos,
                                retain: p.retain,
                            },
                        });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------- introspection

    pub fn retained(&self) -> impl Iterator<Item = &Message> {
        self.retained.values()
    }

    pub fn history(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.history.iter()
    }

    /// Connected client ids.
    pub fn clients(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .conns
            .values()
            .filter_map(|c| c.client.clone())
            .collect();
        v.sort();
        v
    }

    /// Client id bound to a connection.
    pub fn client_of(&self, key: ConnKey) -> Option<&str> {
        self.conns.get(&key).and_then(|c| c.client.as_deref())
    }

    /// Subscriptions of a session.
    pub fn subscriptions(&self, client: &str) -> Vec<(String, u8)> {
        self.sessions
            .get(client)
            .map(|s| s.subs.clone())
            .unwrap_or_default()
    }

    pub fn stats(&self) -> BrokerStats {
        BrokerStats {
            clients: self.conns.values().filter(|c| c.client.is_some()).count(),
            retained: self.retained.len(),
            ..self.stats
        }
    }

    // ------------------------------------------------------------------- protocol

    fn send(&mut self, key: ConnKey, p: &Packet) {
        self.out.push(BrokerOut::Send(key, codec::encode(p)));
    }

    fn handle(&mut self, key: ConnKey, p: Packet) {
        let client = self.conns.get(&key).and_then(|c| c.client.clone());
        match (client, p) {
            (None, Packet::Connect(c)) => self.on_connect(key, c),
            (None, _) => {
                self.drop_conn(key, false, "first packet was not CONNECT");
                self.out.push(BrokerOut::Close(key));
            }
            (Some(_), Packet::Connect(_)) => {
                self.drop_conn(key, false, "second CONNECT");
                self.out.push(BrokerOut::Close(key));
            }
            (Some(cid), Packet::Publish(p)) => self.on_publish(key, &cid, p),
            (Some(cid), Packet::PubAck(pid)) => {
                if let Some(s) = self.sessions.get_mut(&cid) {
                    s.inflight.remove(&pid);
                }
            }
            (Some(cid), Packet::PubRel(pid)) => {
                if let Some(s) = self.sessions.get_mut(&cid) {
                    s.qos2_in.retain(|&x| x != pid);
                }
                self.send(key, &Packet::PubComp(pid));
            }
            (Some(_), Packet::PubRec(pid)) => self.send(key, &Packet::PubRel(pid)),
            (Some(_), Packet::PubComp(_)) => {}
            (Some(cid), Packet::Subscribe { pid, topics }) => {
                self.on_subscribe(key, &cid, pid, topics)
            }
            (Some(cid), Packet::Unsubscribe { pid, topics }) => {
                if let Some(s) = self.sessions.get_mut(&cid) {
                    s.subs.retain(|(f, _)| !topics.contains(f));
                }
                self.send(key, &Packet::UnsubAck(pid));
            }
            (Some(_), Packet::PingReq) => self.send(key, &Packet::PingResp),
            (Some(cid), Packet::Disconnect) => {
                if let Some(s) = self.sessions.get_mut(&cid) {
                    s.will = None;
                }
                self.drop_conn(key, true, "DISCONNECT");
                self.out.push(BrokerOut::Close(key));
            }
            (Some(_), _) => {
                self.drop_conn(key, false, "unexpected packet from client");
                self.out.push(BrokerOut::Close(key));
            }
        }
    }

    fn on_connect(&mut self, key: ConnKey, c: Connect) {
        let refuse = |b: &mut Broker, code: u8, client: &str| {
            b.send(
                key,
                &Packet::ConnAck {
                    session_present: false,
                    code,
                },
            );
            b.events.push(BrokerEvent::Rejected {
                key,
                client: client.into(),
                code,
            });
            b.out.push(BrokerOut::Close(key));
            b.conns.remove(&key);
        };
        if !(c.protocol_name == "MQTT" && c.protocol_level == 4
            || c.protocol_name == "MQIsdp" && c.protocol_level == 3)
        {
            return refuse(self, 1, &c.client_id);
        }
        let mut client_id = c.client_id.clone();
        if client_id.is_empty() {
            if !c.clean_session {
                return refuse(self, 2, "");
            }
            client_id = format!("auto-{}", self.next_auto_id);
            self.next_auto_id += 1;
        }
        if let Some((u, pw)) = &self.config.credentials
            && (c.username.as_deref() != Some(u.as_str())
                || c.password.as_deref() != Some(pw.as_bytes()))
        {
            let code = if c.username.is_none() { 5 } else { 4 };
            return refuse(self, code, &client_id);
        }
        // Session takeover.
        if let Some(old) = self.sessions.get(&client_id).and_then(|s| s.conn)
            && old != key
        {
            self.drop_conn(old, false, "session taken over");
            self.out.push(BrokerOut::Close(old));
        }
        let present = !c.clean_session && self.sessions.get(&client_id).is_some_and(|s| !s.clean);
        if c.clean_session || !present {
            self.sessions
                .insert(client_id.clone(), Session::new(c.clean_session));
        }
        let s = self
            .sessions
            .get_mut(&client_id)
            .expect("session just ensured");
        s.conn = Some(key);
        s.clean = c.clean_session;
        s.will = c.will.clone();
        if let Some(conn) = self.conns.get_mut(&key) {
            conn.client = Some(client_id.clone());
            conn.keep_alive = c.keep_alive;
            conn.last_rx = self.now;
        }
        self.send(
            key,
            &Packet::ConnAck {
                session_present: present,
                code: 0,
            },
        );
        self.events.push(BrokerEvent::Connected {
            key,
            client: client_id.clone(),
            clean: c.clean_session,
            keep_alive: c.keep_alive,
        });
        // Resend unacknowledged and queued messages of a resumed session.
        if present {
            let (inflight, queued): (Vec<(u16, Message)>, Vec<Message>) = {
                let s = self.sessions.get_mut(&client_id).expect("session");
                (
                    s.inflight.iter().map(|(k, v)| (*k, v.clone())).collect(),
                    s.queued.drain(..).collect(),
                )
            };
            for (pid, m) in inflight {
                self.send(
                    key,
                    &Packet::Publish(Publish {
                        dup: true,
                        qos: 1,
                        retain: false,
                        topic: m.topic,
                        pid: Some(pid),
                        payload: m.payload,
                    }),
                );
            }
            for m in queued {
                self.deliver_to(&client_id, m, false);
            }
        }
    }

    fn on_publish(&mut self, key: ConnKey, client: &str, p: Publish) {
        let size = codec::encode(&Packet::Publish(p.clone())).len();
        match (p.qos, p.pid) {
            (1, Some(pid)) => self.send(key, &Packet::PubAck(pid)),
            (2, Some(pid)) => {
                self.send(key, &Packet::PubRec(pid));
                let s = self.sessions.get_mut(client).expect("connected session");
                if s.qos2_in.contains(&pid) {
                    return; // duplicate of an exactly-once message
                }
                s.qos2_in.push(pid);
            }
            _ => {}
        }
        let msg = Message {
            topic: p.topic,
            payload: p.payload,
            qos: p.qos,
            retain: p.retain,
        };
        self.stats.received += 1;
        self.events.push(BrokerEvent::Published {
            key,
            client: client.into(),
            message: msg.clone(),
            size,
        });
        self.history.push_back(HistoryEntry {
            time: self.now,
            client: client.into(),
            message: msg.clone(),
        });
        while self.history.len() > self.config.history {
            self.history.pop_front();
        }
        if msg.retain {
            if msg.payload.is_empty() {
                self.retained.remove(&msg.topic);
            } else {
                self.retained.insert(msg.topic.clone(), msg.clone());
            }
        }
        self.route(msg);
    }

    /// Delivers a message to every matching subscription (retain flag cleared per 3.3.1.3).
    fn route(&mut self, msg: Message) {
        let targets: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.subs
                    .iter()
                    .any(|(f, _)| codec::topic_matches(f, &msg.topic))
            })
            .map(|(c, _)| c.clone())
            .collect();
        for c in targets {
            self.deliver_to(&c, msg.clone(), false);
        }
        self.flush_virtual();
    }

    fn deliver_to(&mut self, client: &str, mut msg: Message, retained: bool) {
        let Some(s) = self.sessions.get_mut(client) else {
            return;
        };
        let granted = s
            .subs
            .iter()
            .filter(|(f, _)| codec::topic_matches(f, &msg.topic))
            .map(|(_, q)| *q)
            .max();
        let Some(granted) = granted else { return };
        msg.qos = msg.qos.min(granted).min(1);
        msg.retain = retained;
        let Some(conn) = s.conn else {
            if !s.clean && msg.qos > 0 && s.queued.len() < self.config.max_queued {
                s.queued.push_back(msg);
            }
            return;
        };
        let pid = if msg.qos > 0 {
            let pid = s.pid();
            s.inflight.insert(pid, msg.clone());
            Some(pid)
        } else {
            None
        };
        self.stats.delivered += 1;
        self.send(
            conn,
            &Packet::Publish(Publish {
                dup: false,
                qos: msg.qos,
                retain: msg.retain,
                topic: msg.topic,
                pid,
                payload: msg.payload,
            }),
        );
    }

    fn on_subscribe(&mut self, key: ConnKey, client: &str, pid: u16, topics: Vec<(String, u8)>) {
        let mut codes = vec![];
        {
            let s = self.sessions.get_mut(client).expect("connected session");
            for (f, q) in &topics {
                let g = (*q).min(1);
                s.subs.retain(|(x, _)| x != f);
                s.subs.push((f.clone(), g));
                codes.push(g);
            }
        }
        self.send(key, &Packet::SubAck { pid, codes });
        self.events.push(BrokerEvent::Subscribed {
            key,
            client: client.into(),
            filters: topics.clone(),
        });
        let retained: Vec<Message> = self
            .retained
            .values()
            .filter(|m| {
                topics
                    .iter()
                    .any(|(f, _)| codec::topic_matches(f, &m.topic))
            })
            .cloned()
            .collect();
        for m in retained {
            self.deliver_to(client, m, true);
        }
        self.flush_virtual();
    }

    fn drop_conn(&mut self, key: ConnKey, graceful: bool, reason: &str) {
        let Some(c) = self.conns.remove(&key) else {
            return;
        };
        let Some(client) = c.client else { return };
        let mut will = None;
        if let Some(s) = self.sessions.get_mut(&client)
            && s.conn == Some(key)
        {
            s.conn = None;
            will = s.will.take();
            if s.clean {
                self.sessions.remove(&client);
            }
        }
        self.events.push(BrokerEvent::Disconnected {
            key,
            client: client.clone(),
            graceful,
            reason: reason.into(),
        });
        if !graceful && let Some(w) = will {
            let msg = Message {
                topic: w.topic,
                payload: w.payload,
                qos: w.qos.min(1),
                retain: w.retain,
            };
            self.events.push(BrokerEvent::WillPublished {
                client: client.clone(),
                message: msg.clone(),
            });
            self.history.push_back(HistoryEntry {
                time: self.now,
                client,
                message: msg.clone(),
            });
            if msg.retain {
                self.retained.insert(msg.topic.clone(), msg.clone());
            }
            self.route(msg);
        }
    }
}

fn keep_alive_limit(ka: u16) -> SimTime {
    SimTime::from_ms(ka as u64 * 1500)
}

/// Convenience for tests and simple transports: errors as text.
pub fn describe(e: &MqttError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mqtt::codec::{decode, encode};

    fn t(s: f64) -> SimTime {
        SimTime::from_secs_f64(s)
    }

    /// Decodes every packet sent to `key`.
    fn sent_to(out: &[BrokerOut], key: ConnKey) -> Vec<Packet> {
        let mut v = vec![];
        for o in out {
            if let BrokerOut::Send(k, b) = o
                && *k == key
            {
                let mut off = 0;
                while let Ok(Some((p, n))) = decode(&b[off..]) {
                    v.push(p);
                    off += n;
                }
            }
        }
        v
    }

    fn connect(b: &mut Broker, key: ConnKey, c: Connect, now: SimTime) -> Vec<BrokerOut> {
        b.open(key, now);
        b.input(key, &encode(&Packet::Connect(c)), now)
    }

    fn publish(topic: &str, payload: &[u8], qos: u8, retain: bool, pid: Option<u16>) -> Vec<u8> {
        encode(&Packet::Publish(Publish {
            dup: false,
            qos,
            retain,
            topic: topic.into(),
            pid,
            payload: payload.to_vec(),
        }))
    }

    #[test]
    fn connect_subscribe_publish_wildcards_and_qos1() {
        let mut b = Broker::default();
        let out = connect(&mut b, 1, Connect::new("garden-node", 60), t(0.0));
        assert_eq!(
            sent_to(&out, 1),
            vec![Packet::ConnAck {
                session_present: false,
                code: 0
            }]
        );
        connect(&mut b, 2, Connect::new("dash", 0), t(0.0));
        let out = b.input(
            2,
            &encode(&Packet::Subscribe {
                pid: 9,
                topics: vec![("garden/+".into(), 1), ("other/#".into(), 0)],
            }),
            t(0.1),
        );
        assert_eq!(
            sent_to(&out, 2),
            vec![Packet::SubAck {
                pid: 9,
                codes: vec![1, 0]
            }]
        );
        // QoS 0 publish → QoS 0 delivery.
        let out = b.input(
            1,
            &publish("garden/pot1", b"{\"m\":42}", 0, false, None),
            t(1.0),
        );
        let got = sent_to(&out, 2);
        assert_eq!(got.len(), 1);
        let Packet::Publish(p) = &got[0] else {
            panic!()
        };
        assert_eq!(
            (p.topic.as_str(), p.qos, p.payload.as_slice()),
            ("garden/pot1", 0, &b"{\"m\":42}"[..])
        );
        // QoS 1 publish → PUBACK to sender, QoS 1 delivery with a packet id.
        let out = b.input(1, &publish("garden/pot1", b"x", 1, false, Some(5)), t(2.0));
        assert_eq!(sent_to(&out, 1), vec![Packet::PubAck(5)]);
        let Packet::Publish(p) = &sent_to(&out, 2)[0] else {
            panic!()
        };
        assert_eq!(p.qos, 1);
        let pid = p.pid.unwrap();
        b.input(2, &encode(&Packet::PubAck(pid)), t(2.1));
        // Not matching: garden/pot1/x vs garden/+.
        let out = b.input(1, &publish("garden/pot1/x", b"y", 0, false, None), t(3.0));
        assert!(sent_to(&out, 2).is_empty());
        // PINGREQ.
        let out = b.input(1, &encode(&Packet::PingReq), t(4.0));
        assert_eq!(sent_to(&out, 1), vec![Packet::PingResp]);
        assert_eq!(
            b.clients(),
            vec!["dash".to_string(), "garden-node".to_string()]
        );
    }

    #[test]
    fn retained_messages_and_deletion() {
        let mut b = Broker::default();
        connect(&mut b, 1, Connect::new("pub", 0), t(0.0));
        b.input(
            1,
            &publish("garden/status", b"online", 0, true, None),
            t(0.0),
        );
        let v = b.virtual_connect("explorer", t(1.0));
        b.virtual_subscribe(v, "garden/#", 0, t(1.0));
        let got = b.virtual_take(v);
        assert_eq!(got.len(), 1);
        assert!(
            got[0].message.retain,
            "retained flag set on subscription delivery"
        );
        // Live delivery has retain cleared.
        b.input(
            1,
            &publish("garden/status", b"still", 0, true, None),
            t(2.0),
        );
        let got = b.virtual_take(v);
        assert!(!got[0].message.retain);
        // Empty retained payload deletes.
        b.input(1, &publish("garden/status", b"", 0, true, None), t(3.0));
        assert_eq!(b.retained().count(), 0);
    }

    #[test]
    fn keep_alive_timeout_publishes_will() {
        let mut b = Broker::default();
        let mut c = Connect::new("node", 10);
        c.will = Some(Will {
            topic: "node/status".into(),
            payload: b"offline".to_vec(),
            qos: 1,
            retain: true,
        });
        connect(&mut b, 1, c, t(0.0));
        let v = b.virtual_connect("watch", t(0.0));
        b.virtual_subscribe(v, "node/#", 1, t(0.0));
        // 1.5 × keep-alive = 15 s.
        assert_eq!(b.next_deadline(), Some(t(15.0)));
        assert!(b.tick(t(14.9)).is_empty());
        let out = b.tick(t(15.0));
        assert!(out.contains(&BrokerOut::Close(1)));
        let got = b.virtual_take(v);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message.payload, b"offline");
        assert_eq!(b.retained().next().unwrap().topic, "node/status");
        assert!(
            b.take_events()
                .iter()
                .any(|e| matches!(e, BrokerEvent::WillPublished { .. }))
        );
    }

    #[test]
    fn graceful_disconnect_suppresses_will_and_network_loss_sends_it() {
        let mut b = Broker::default();
        let mut c = Connect::new("a", 0);
        c.will = Some(Will {
            topic: "w".into(),
            payload: b"gone".to_vec(),
            qos: 0,
            retain: false,
        });
        connect(&mut b, 1, c.clone(), t(0.0));
        let v = b.virtual_connect("watch", t(0.0));
        b.virtual_subscribe(v, "w", 0, t(0.0));
        b.input(1, &encode(&Packet::Disconnect), t(1.0));
        assert!(b.virtual_take(v).is_empty());
        connect(&mut b, 2, c, t(2.0));
        b.closed(2, t(3.0));
        assert_eq!(b.virtual_take(v).len(), 1);
    }

    #[test]
    fn persistent_session_queues_qos1_and_resends_inflight() {
        let mut b = Broker::default();
        let mut c = Connect::new("sub", 0);
        c.clean_session = false;
        connect(&mut b, 1, c.clone(), t(0.0));
        b.input(
            1,
            &encode(&Packet::Subscribe {
                pid: 1,
                topics: vec![("q/#".into(), 1)],
            }),
            t(0.0),
        );
        let p = b.virtual_connect("pub", t(0.0));
        // Delivered but never acked, then the link drops.
        let out = b.virtual_publish(p, Message::new("q/1", "one", 1, false), t(1.0));
        assert_eq!(sent_to(&out, 1).len(), 1);
        b.closed(1, t(2.0));
        // Offline: queued.
        b.virtual_publish(p, Message::new("q/2", "two", 1, false), t(3.0));
        b.virtual_publish(p, Message::new("q/3", "three-qos0", 0, false), t(3.0));
        let out = connect(&mut b, 2, c, t(4.0));
        let got = sent_to(&out, 2);
        assert_eq!(
            got[0],
            Packet::ConnAck {
                session_present: true,
                code: 0
            }
        );
        let pubs: Vec<(String, bool)> = got[1..]
            .iter()
            .map(|p| match p {
                Packet::Publish(p) => (String::from_utf8_lossy(&p.payload).into_owned(), p.dup),
                _ => panic!(),
            })
            .collect();
        assert_eq!(pubs, vec![("one".into(), true), ("two".into(), false)]);
    }

    #[test]
    fn protocol_errors_close_and_rejections() {
        let mut b = Broker::default();
        b.open(1, t(0.0));
        let out = b.input(1, &encode(&Packet::PingReq), t(0.0));
        assert!(out.contains(&BrokerOut::Close(1)));
        let mut c = Connect::new("", 0);
        c.clean_session = false;
        let out = connect(&mut b, 2, c, t(0.0));
        assert_eq!(
            sent_to(&out, 2),
            vec![Packet::ConnAck {
                session_present: false,
                code: 2
            }]
        );
        let mut c = Connect::new("x", 0);
        c.protocol_level = 5;
        let out = connect(&mut b, 3, c, t(0.0));
        assert_eq!(
            sent_to(&out, 3),
            vec![Packet::ConnAck {
                session_present: false,
                code: 1
            }]
        );
        b.config.credentials = Some(("user".into(), "pw".into()));
        let out = connect(&mut b, 4, Connect::new("y", 0), t(0.0));
        assert_eq!(
            sent_to(&out, 4),
            vec![Packet::ConnAck {
                session_present: false,
                code: 5
            }]
        );
        let mut c = Connect::new("y", 0);
        c.username = Some("user".into());
        c.password = Some(b"pw".to_vec());
        let out = connect(&mut b, 5, c, t(0.0));
        assert_eq!(
            sent_to(&out, 5),
            vec![Packet::ConnAck {
                session_present: false,
                code: 0
            }]
        );
        // Takeover closes the old connection.
        let mut c = Connect::new("y", 0);
        c.username = Some("user".into());
        c.password = Some(b"pw".to_vec());
        let out = connect(&mut b, 6, c, t(1.0));
        assert!(out.contains(&BrokerOut::Close(5)));
        // Garbage closes.
        let out = b.input(6, &[0xf0, 0x00], t(2.0));
        assert!(out.contains(&BrokerOut::Close(6)));
    }

    #[test]
    fn qos2_inbound_handshake_exactly_once() {
        let mut b = Broker::default();
        connect(&mut b, 1, Connect::new("p", 0), t(0.0));
        let v = b.virtual_connect("w", t(0.0));
        b.virtual_subscribe(v, "#", 1, t(0.0));
        let out = b.input(1, &publish("x", b"1", 2, false, Some(3)), t(1.0));
        assert_eq!(sent_to(&out, 1), vec![Packet::PubRec(3)]);
        // Duplicate before PUBREL: not delivered twice.
        b.input(1, &publish("x", b"1", 2, false, Some(3)), t(1.1));
        let out = b.input(1, &encode(&Packet::PubRel(3)), t(1.2));
        assert_eq!(sent_to(&out, 1), vec![Packet::PubComp(3)]);
        assert_eq!(b.virtual_take(v).len(), 1);
    }
}
