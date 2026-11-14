//! "Bridge to my real LAN broker" (`RF-3`): mirrors MQTT traffic between the built-in broker
//! and a real broker (host:port) so real dashboards see simulated devices and can command them.
//!
//! All blocking socket I/O runs on a dedicated thread; the simulation side only touches two
//! channels in [`LanBridge::pump`] (never blocks, never waits for the network). The I/O thread
//! reconnects with back-off. Loops are prevented on both sides: a message the bridge injected
//! into one broker is not forwarded back when that broker echoes it to the bridge's own
//! subscription.

use crate::mqtt::codec::{self, Connect, Packet, Publish, StreamDecoder};
use crate::mqtt::{ConnKey, Message};
use crate::net::Network;
use bb_core::SimTime;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    /// Topic filters mirrored in both directions.
    pub topics: Vec<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keep_alive_s: u16,
}

impl BridgeConfig {
    pub fn new(host: &str, port: u16) -> Self {
        BridgeConfig { host: host.into(), port, client_id: "breadbox-bridge".into(), topics: vec!["#".into()], username: None, password: None, keep_alive_s: 30 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum BridgeStatus {
    Connecting,
    Connected,
    /// Lost or refused; the thread retries.
    Disconnected(String),
    Stopped,
}

enum ToIo {
    Publish(Message),
    Stop,
}

enum FromIo {
    Status(BridgeStatus),
    Message(Message),
}

/// Counters for the UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct BridgeStats {
    pub to_lan: u64,
    pub from_lan: u64,
}

pub struct LanBridge {
    pub config: BridgeConfig,
    to_io: Sender<ToIo>,
    from_io: Receiver<FromIo>,
    thread: Option<JoinHandle<()>>,
    status: BridgeStatus,
    sim_client: Option<ConnKey>,
    /// Messages we injected into the sim broker (dropped when they come back to us).
    injected: VecDeque<(String, Vec<u8>)>,
    pub stats: BridgeStats,
}

impl std::fmt::Debug for LanBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanBridge").field("config", &self.config).field("status", &self.status).finish()
    }
}

impl LanBridge {
    /// Starts the I/O thread (returns immediately).
    pub fn start(config: BridgeConfig) -> LanBridge {
        let (to_io, rx_io) = mpsc::channel();
        let (tx_sim, from_io) = mpsc::channel();
        let cfg = config.clone();
        let thread = std::thread::Builder::new().name("bb-lan-bridge".into()).spawn(move || io_thread(cfg, rx_io, tx_sim)).ok();
        LanBridge { config, to_io, from_io, thread, status: BridgeStatus::Connecting, sim_client: None, injected: VecDeque::new(), stats: BridgeStats::default() }
    }

    pub fn status(&self) -> &BridgeStatus {
        &self.status
    }

    /// Exchanges messages with the I/O thread. Non-blocking; call every sim step (or a few
    /// times per simulated second).
    pub fn pump(&mut self, net: &mut Network, now: SimTime) {
        let key = match self.sim_client {
            Some(k) => k,
            None => {
                let k = net.broker_mut().virtual_connect(&format!("{}-sim", self.config.client_id), now);
                for f in self.config.topics.clone() {
                    let out = net.broker_mut().virtual_subscribe(k, &f, 0, now);
                    net.apply_broker(out);
                }
                self.sim_client = Some(k);
                k
            }
        };
        // sim → LAN
        for r in net.broker_mut().virtual_take(key) {
            let m = r.message;
            if let Some(i) = self.injected.iter().position(|(t, p)| *t == m.topic && *p == m.payload) {
                self.injected.remove(i);
                continue;
            }
            if self.status == BridgeStatus::Connected {
                self.stats.to_lan += 1;
                let _ = self.to_io.send(ToIo::Publish(m));
            }
        }
        // LAN → sim
        loop {
            match self.from_io.try_recv() {
                Ok(FromIo::Status(s)) => self.status = s,
                Ok(FromIo::Message(m)) => {
                    self.stats.from_lan += 1;
                    self.injected.push_back((m.topic.clone(), m.payload.clone()));
                    if self.injected.len() > 1000 {
                        self.injected.pop_front();
                    }
                    let out = net.broker_mut().virtual_publish(key, m, now);
                    net.apply_broker(out);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = BridgeStatus::Stopped;
                    break;
                }
            }
        }
    }

    /// Stops the I/O thread and detaches from the sim broker.
    pub fn stop(mut self, net: &mut Network, now: SimTime) {
        let _ = self.to_io.send(ToIo::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(k) = self.sim_client.take() {
            let out = net.broker_mut().virtual_disconnect(k, now);
            net.apply_broker(out);
        }
    }
}

impl Drop for LanBridge {
    fn drop(&mut self) {
        let _ = self.to_io.send(ToIo::Stop);
    }
}

fn io_thread(cfg: BridgeConfig, rx: Receiver<ToIo>, tx: Sender<FromIo>) {
    let mut backoff = Duration::from_millis(200);
    loop {
        let _ = tx.send(FromIo::Status(BridgeStatus::Connecting));
        match session(&cfg, &rx, &tx) {
            Ok(()) => {
                let _ = tx.send(FromIo::Status(BridgeStatus::Stopped));
                return;
            }
            Err(e) => {
                if tx.send(FromIo::Status(BridgeStatus::Disconnected(e))).is_err() {
                    return;
                }
            }
        }
        // Wait before retrying, but stay responsive to Stop.
        let until = Instant::now() + backoff;
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(ToIo::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = tx.send(FromIo::Status(BridgeStatus::Stopped));
                    return;
                }
                _ => {}
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

/// One connected session; `Ok` = asked to stop, `Err` = connection problem.
fn session(cfg: &BridgeConfig, rx: &Receiver<ToIo>, tx: &Sender<FromIo>) -> Result<(), String> {
    let addr = (cfg.host.as_str(), cfg.port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    s.set_nodelay(true).ok();
    s.set_read_timeout(Some(Duration::from_millis(20))).map_err(|e| e.to_string())?;
    let mut c = Connect::new(&cfg.client_id, cfg.keep_alive_s);
    c.username = cfg.username.clone();
    c.password = cfg.password.as_ref().map(|p| p.as_bytes().to_vec());
    s.write_all(&codec::encode(&Packet::Connect(c))).map_err(|e| e.to_string())?;
    let mut dec = StreamDecoder::default();
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(3);
    // CONNACK
    loop {
        if Instant::now() > deadline {
            return Err("no CONNACK".into());
        }
        match s.read(&mut buf) {
            Ok(0) => return Err("closed by broker".into()),
            Ok(n) => dec.push(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
        match dec.next_packet().map_err(|e| e.to_string())? {
            Some(Packet::ConnAck { code: 0, .. }) => break,
            Some(Packet::ConnAck { code, .. }) => return Err(format!("refused (CONNACK {code})")),
            Some(_) => return Err("unexpected packet".into()),
            None => {}
        }
    }
    let topics: Vec<(String, u8)> = cfg.topics.iter().map(|t| (t.clone(), 0)).collect();
    s.write_all(&codec::encode(&Packet::Subscribe { pid: 1, topics })).map_err(|e| e.to_string())?;
    let _ = tx.send(FromIo::Status(BridgeStatus::Connected));
    let mut sent: VecDeque<(String, Vec<u8>)> = VecDeque::new();
    let mut last_tx = Instant::now();
    loop {
        // Outgoing.
        loop {
            match rx.try_recv() {
                Ok(ToIo::Publish(m)) => {
                    let p = Publish { dup: false, qos: 0, retain: m.retain, topic: m.topic.clone(), pid: None, payload: m.payload.clone() };
                    s.write_all(&codec::encode(&Packet::Publish(p))).map_err(|e| e.to_string())?;
                    sent.push_back((m.topic, m.payload));
                    if sent.len() > 1000 {
                        sent.pop_front();
                    }
                    last_tx = Instant::now();
                }
                Ok(ToIo::Stop) => {
                    let _ = s.write_all(&codec::encode(&Packet::Disconnect));
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if cfg.keep_alive_s > 0 && last_tx.elapsed() > Duration::from_secs(cfg.keep_alive_s as u64 / 2) {
            s.write_all(&codec::encode(&Packet::PingReq)).map_err(|e| e.to_string())?;
            last_tx = Instant::now();
        }
        // Incoming.
        match s.read(&mut buf) {
            Ok(0) => return Err("closed by broker".into()),
            Ok(n) => dec.push(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
        while let Some(p) = dec.next_packet().map_err(|e| e.to_string())? {
            if let Packet::Publish(p) = p {
                if let Some(i) = sent.iter().position(|(t, d)| *t == p.topic && *d == p.payload) {
                    sent.remove(i); // our own message echoed back
                    continue;
                }
                let m = Message { topic: p.topic, payload: p.payload, qos: 0, retain: p.retain };
                if tx.send(FromIo::Message(m)).is_err() {
                    return Ok(());
                }
            }
        }
    }
}
