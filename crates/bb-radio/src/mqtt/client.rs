//! A small MQTT 3.1.1 client state machine over any byte transport (used by the ESP-AT
//! modem's `AT+MQTT*` commands and the LAN bridge). It produces bytes to send and consumes
//! bytes received; timing (keep-alive pings) is driven by the caller's clock.

use super::codec::{self, Connect, MqttError, Packet, Publish, StreamDecoder, Will};
use bb_core::SimTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Idle,
    Connecting,
    Connected,
    Closed,
}

/// Events surfaced to the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Connected {
        session_present: bool,
    },
    Refused(u8),
    Message {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    },
    SubAck {
        pid: u16,
        codes: Vec<u8>,
    },
    UnsubAck(u16),
    PubAck(u16),
    PingResp,
    ProtocolError(String),
}

#[derive(Debug, Clone)]
pub struct MqttClient {
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
    pub keep_alive: u16,
    pub clean_session: bool,
    pub will: Option<Will>,
    state: ClientState,
    decoder: StreamDecoder,
    next_pid: u16,
    last_tx: SimTime,
    ping_outstanding: bool,
}

impl MqttClient {
    pub fn new(client_id: &str) -> Self {
        MqttClient {
            client_id: client_id.into(),
            username: None,
            password: None,
            keep_alive: 120,
            clean_session: true,
            will: None,
            state: ClientState::Idle,
            decoder: StreamDecoder::default(),
            next_pid: 1,
            last_tx: SimTime::ZERO,
            ping_outstanding: false,
        }
    }

    pub fn state(&self) -> ClientState {
        self.state
    }

    /// CONNECT bytes (call once the transport is up).
    pub fn connect(&mut self, now: SimTime) -> Vec<u8> {
        self.state = ClientState::Connecting;
        self.decoder = StreamDecoder::default();
        self.last_tx = now;
        self.ping_outstanding = false;
        codec::encode(&Packet::Connect(Connect {
            protocol_name: "MQTT".into(),
            protocol_level: 4,
            client_id: self.client_id.clone(),
            clean_session: self.clean_session,
            keep_alive: self.keep_alive,
            username: self.username.clone(),
            password: self.password.clone(),
            will: self.will.clone(),
        }))
    }

    fn pid(&mut self) -> u16 {
        let p = self.next_pid;
        self.next_pid = self.next_pid.wrapping_add(1).max(1);
        p
    }

    pub fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        qos: u8,
        retain: bool,
        now: SimTime,
    ) -> Vec<u8> {
        self.last_tx = now;
        let pid = (qos > 0).then(|| self.pid());
        codec::encode(&Packet::Publish(Publish {
            dup: false,
            qos: qos.min(1),
            retain,
            topic: topic.into(),
            pid,
            payload: payload.to_vec(),
        }))
    }

    /// Returns (packet id, bytes).
    pub fn subscribe(&mut self, filter: &str, qos: u8, now: SimTime) -> (u16, Vec<u8>) {
        self.last_tx = now;
        let pid = self.pid();
        (
            pid,
            codec::encode(&Packet::Subscribe {
                pid,
                topics: vec![(filter.into(), qos.min(1))],
            }),
        )
    }

    pub fn unsubscribe(&mut self, filter: &str, now: SimTime) -> (u16, Vec<u8>) {
        self.last_tx = now;
        let pid = self.pid();
        (
            pid,
            codec::encode(&Packet::Unsubscribe {
                pid,
                topics: vec![filter.into()],
            }),
        )
    }

    pub fn disconnect(&mut self) -> Vec<u8> {
        self.state = ClientState::Closed;
        codec::encode(&Packet::Disconnect)
    }

    /// The transport closed underneath us.
    pub fn transport_closed(&mut self) {
        self.state = ClientState::Closed;
    }

    /// When the next PINGREQ is due (keep-alive), if connected.
    pub fn ping_due(&self) -> Option<SimTime> {
        (self.state == ClientState::Connected && self.keep_alive > 0)
            .then(|| self.last_tx + SimTime::from_secs(self.keep_alive as u64))
    }

    /// PINGREQ bytes if the keep-alive interval elapsed.
    pub fn poll_ping(&mut self, now: SimTime) -> Option<Vec<u8>> {
        let due = self.ping_due()?;
        if now < due {
            return None;
        }
        self.last_tx = now;
        self.ping_outstanding = true;
        Some(codec::encode(&Packet::PingReq))
    }

    /// Consumes received bytes; returns events and any bytes to send back (PUBACKs).
    pub fn input(&mut self, data: &[u8]) -> (Vec<ClientEvent>, Vec<u8>) {
        self.decoder.push(data);
        let mut ev = vec![];
        let mut reply = vec![];
        loop {
            match self.decoder.next_packet() {
                Ok(Some(p)) => match p {
                    Packet::ConnAck {
                        session_present,
                        code,
                    } => {
                        if code == 0 {
                            self.state = ClientState::Connected;
                            ev.push(ClientEvent::Connected { session_present });
                        } else {
                            self.state = ClientState::Closed;
                            ev.push(ClientEvent::Refused(code));
                        }
                    }
                    Packet::Publish(p) => {
                        if p.qos == 1
                            && let Some(pid) = p.pid
                        {
                            reply.extend(codec::encode(&Packet::PubAck(pid)));
                        }
                        if p.qos == 2
                            && let Some(pid) = p.pid
                        {
                            reply.extend(codec::encode(&Packet::PubRec(pid)));
                        }
                        ev.push(ClientEvent::Message {
                            topic: p.topic,
                            payload: p.payload,
                            qos: p.qos,
                            retain: p.retain,
                        });
                    }
                    Packet::PubRel(pid) => reply.extend(codec::encode(&Packet::PubComp(pid))),
                    Packet::SubAck { pid, codes } => ev.push(ClientEvent::SubAck { pid, codes }),
                    Packet::UnsubAck(pid) => ev.push(ClientEvent::UnsubAck(pid)),
                    Packet::PubAck(pid) => ev.push(ClientEvent::PubAck(pid)),
                    Packet::PingResp => {
                        self.ping_outstanding = false;
                        ev.push(ClientEvent::PingResp);
                    }
                    other => ev.push(ClientEvent::ProtocolError(format!(
                        "unexpected {}",
                        other.name()
                    ))),
                },
                Ok(None) => break,
                Err(e) => {
                    ev.push(ClientEvent::ProtocolError(describe(&e)));
                    self.state = ClientState::Closed;
                    break;
                }
            }
        }
        (ev, reply)
    }
}

fn describe(e: &MqttError) -> String {
    e.to_string()
}
