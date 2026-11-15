//! MQTT 3.1.1 packet codec (OASIS standard, 2014): every control packet, remaining-length
//! varints, UTF-8 strings, and the spec's reserved-flag checks.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MqttError {
    #[error("malformed packet: {0}")]
    Malformed(&'static str),
    #[error("protocol violation: {0}")]
    Protocol(&'static str),
    #[error("packet too large ({0} bytes)")]
    TooLarge(usize),
}

/// Last will and testament.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Will {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub protocol_name: String,
    pub protocol_level: u8,
    pub client_id: String,
    pub clean_session: bool,
    pub keep_alive: u16,
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
    pub will: Option<Will>,
}

impl Connect {
    pub fn new(client_id: &str, keep_alive: u16) -> Self {
        Connect {
            protocol_name: "MQTT".into(),
            protocol_level: 4,
            client_id: client_id.into(),
            clean_session: true,
            keep_alive,
            username: None,
            password: None,
            will: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publish {
    pub dup: bool,
    pub qos: u8,
    pub retain: bool,
    pub topic: String,
    /// Packet identifier (QoS > 0 only).
    pub pid: Option<u16>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Connect(Connect),
    ConnAck { session_present: bool, code: u8 },
    Publish(Publish),
    PubAck(u16),
    PubRec(u16),
    PubRel(u16),
    PubComp(u16),
    Subscribe { pid: u16, topics: Vec<(String, u8)> },
    SubAck { pid: u16, codes: Vec<u8> },
    Unsubscribe { pid: u16, topics: Vec<String> },
    UnsubAck(u16),
    PingReq,
    PingResp,
    Disconnect,
}

impl Packet {
    pub fn name(&self) -> &'static str {
        match self {
            Packet::Connect(_) => "CONNECT",
            Packet::ConnAck { .. } => "CONNACK",
            Packet::Publish(_) => "PUBLISH",
            Packet::PubAck(_) => "PUBACK",
            Packet::PubRec(_) => "PUBREC",
            Packet::PubRel(_) => "PUBREL",
            Packet::PubComp(_) => "PUBCOMP",
            Packet::Subscribe { .. } => "SUBSCRIBE",
            Packet::SubAck { .. } => "SUBACK",
            Packet::Unsubscribe { .. } => "UNSUBSCRIBE",
            Packet::UnsubAck(_) => "UNSUBACK",
            Packet::PingReq => "PINGREQ",
            Packet::PingResp => "PINGRESP",
            Packet::Disconnect => "DISCONNECT",
        }
    }
}

/// Largest remaining length MQTT allows (256 MB); we cap lower for safety.
pub const MAX_PACKET: usize = 1 << 20;

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

fn put_remaining_len(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

fn frame(first: u8, body: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(first);
    put_remaining_len(&mut out, body.len());
    out.extend_from_slice(&body);
    out
}

/// Encodes one packet.
pub fn encode(p: &Packet) -> Vec<u8> {
    match p {
        Packet::Connect(c) => {
            let mut b = Vec::new();
            put_str(&mut b, c.protocol_name.as_bytes());
            b.push(c.protocol_level);
            let mut flags = 0u8;
            if c.clean_session {
                flags |= 0x02;
            }
            if let Some(w) = &c.will {
                flags |= 0x04 | (w.qos.min(2) << 3);
                if w.retain {
                    flags |= 0x20;
                }
            }
            if c.password.is_some() {
                flags |= 0x40;
            }
            if c.username.is_some() {
                flags |= 0x80;
            }
            b.push(flags);
            b.extend_from_slice(&c.keep_alive.to_be_bytes());
            put_str(&mut b, c.client_id.as_bytes());
            if let Some(w) = &c.will {
                put_str(&mut b, w.topic.as_bytes());
                put_str(&mut b, &w.payload);
            }
            if let Some(u) = &c.username {
                put_str(&mut b, u.as_bytes());
            }
            if let Some(pw) = &c.password {
                put_str(&mut b, pw);
            }
            frame(0x10, b)
        }
        Packet::ConnAck {
            session_present,
            code,
        } => frame(0x20, vec![*session_present as u8, *code]),
        Packet::Publish(p) => {
            let mut b = Vec::with_capacity(p.topic.len() + p.payload.len() + 4);
            put_str(&mut b, p.topic.as_bytes());
            if p.qos > 0 {
                b.extend_from_slice(&p.pid.unwrap_or(1).to_be_bytes());
            }
            b.extend_from_slice(&p.payload);
            let first = 0x30 | ((p.dup as u8) << 3) | (p.qos.min(2) << 1) | p.retain as u8;
            frame(first, b)
        }
        Packet::PubAck(id) => frame(0x40, id.to_be_bytes().to_vec()),
        Packet::PubRec(id) => frame(0x50, id.to_be_bytes().to_vec()),
        Packet::PubRel(id) => frame(0x62, id.to_be_bytes().to_vec()),
        Packet::PubComp(id) => frame(0x70, id.to_be_bytes().to_vec()),
        Packet::Subscribe { pid, topics } => {
            let mut b = pid.to_be_bytes().to_vec();
            for (t, q) in topics {
                put_str(&mut b, t.as_bytes());
                b.push(*q);
            }
            frame(0x82, b)
        }
        Packet::SubAck { pid, codes } => {
            let mut b = pid.to_be_bytes().to_vec();
            b.extend_from_slice(codes);
            frame(0x90, b)
        }
        Packet::Unsubscribe { pid, topics } => {
            let mut b = pid.to_be_bytes().to_vec();
            for t in topics {
                put_str(&mut b, t.as_bytes());
            }
            frame(0xa2, b)
        }
        Packet::UnsubAck(id) => frame(0xb0, id.to_be_bytes().to_vec()),
        Packet::PingReq => vec![0xc0, 0],
        Packet::PingResp => vec![0xd0, 0],
        Packet::Disconnect => vec![0xe0, 0],
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, MqttError> {
        let v = *self
            .b
            .get(self.i)
            .ok_or(MqttError::Malformed("truncated"))?;
        self.i += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, MqttError> {
        Ok(((self.u8()? as u16) << 8) | self.u8()? as u16)
    }
    fn bytes(&mut self) -> Result<&'a [u8], MqttError> {
        let n = self.u16()? as usize;
        let s = self
            .b
            .get(self.i..self.i + n)
            .ok_or(MqttError::Malformed("truncated string"))?;
        self.i += n;
        Ok(s)
    }
    fn string(&mut self) -> Result<String, MqttError> {
        let s = self.bytes()?;
        let s = std::str::from_utf8(s).map_err(|_| MqttError::Malformed("invalid UTF-8"))?;
        if s.contains('\0') {
            return Err(MqttError::Malformed("NUL in string"));
        }
        Ok(s.to_string())
    }
    fn rest(&mut self) -> &'a [u8] {
        let r = &self.b[self.i.min(self.b.len())..];
        self.i = self.b.len();
        r
    }
    fn done(&self) -> bool {
        self.i >= self.b.len()
    }
}

/// Parses the fixed header: `Some((first byte, remaining length, header length))`, `None` if
/// more bytes are needed.
pub fn fixed_header(buf: &[u8]) -> Result<Option<(u8, usize, usize)>, MqttError> {
    if buf.is_empty() {
        return Ok(None);
    }
    let mut len = 0usize;
    let mut mult = 1usize;
    for i in 1..=4 {
        let Some(&b) = buf.get(i) else {
            return Ok(None);
        };
        len += (b & 0x7f) as usize * mult;
        if b & 0x80 == 0 {
            if len > MAX_PACKET {
                return Err(MqttError::TooLarge(len));
            }
            return Ok(Some((buf[0], len, i + 1)));
        }
        mult *= 128;
    }
    Err(MqttError::Malformed("remaining length longer than 4 bytes"))
}

/// Decodes one packet from the front of `buf`: `Ok(Some((packet, consumed)))`, `Ok(None)` if
/// the packet is incomplete.
pub fn decode(buf: &[u8]) -> Result<Option<(Packet, usize)>, MqttError> {
    let Some((first, len, hl)) = fixed_header(buf)? else {
        return Ok(None);
    };
    if buf.len() < hl + len {
        return Ok(None);
    }
    let body = &buf[hl..hl + len];
    let p = decode_body(first, body)?;
    Ok(Some((p, hl + len)))
}

fn decode_body(first: u8, body: &[u8]) -> Result<Packet, MqttError> {
    let ty = first >> 4;
    let flags = first & 0x0f;
    let mut r = Reader { b: body, i: 0 };
    let expect_flags = |want: u8| {
        if flags != want {
            Err(MqttError::Protocol("reserved flags"))
        } else {
            Ok(())
        }
    };
    let p = match ty {
        1 => {
            expect_flags(0)?;
            let protocol_name = r.string()?;
            let protocol_level = r.u8()?;
            if !(protocol_name == "MQTT" && protocol_level == 4
                || protocol_name == "MQIsdp" && protocol_level == 3)
            {
                // Reported to the client as CONNACK 1 by the broker.
                return Ok(Packet::Connect(Connect {
                    protocol_name,
                    protocol_level,
                    client_id: String::new(),
                    clean_session: true,
                    keep_alive: 0,
                    username: None,
                    password: None,
                    will: None,
                }));
            }
            let f = r.u8()?;
            if f & 1 != 0 {
                return Err(MqttError::Protocol("CONNECT reserved flag set"));
            }
            let keep_alive = r.u16()?;
            let client_id = r.string()?;
            let will = if f & 0x04 != 0 {
                let topic = r.string()?;
                let payload = r.bytes()?.to_vec();
                let qos = (f >> 3) & 3;
                if qos == 3 {
                    return Err(MqttError::Protocol("will QoS 3"));
                }
                Some(Will {
                    topic,
                    payload,
                    qos,
                    retain: f & 0x20 != 0,
                })
            } else {
                if f & 0x38 != 0 {
                    return Err(MqttError::Protocol("will flags without will"));
                }
                None
            };
            let username = if f & 0x80 != 0 {
                Some(r.string()?)
            } else {
                None
            };
            let password = if f & 0x40 != 0 {
                Some(r.bytes()?.to_vec())
            } else {
                None
            };
            Packet::Connect(Connect {
                protocol_name,
                protocol_level,
                client_id,
                clean_session: f & 0x02 != 0,
                keep_alive,
                username,
                password,
                will,
            })
        }
        2 => {
            expect_flags(0)?;
            let sp = r.u8()?;
            let code = r.u8()?;
            Packet::ConnAck {
                session_present: sp & 1 != 0,
                code,
            }
        }
        3 => {
            let qos = (flags >> 1) & 3;
            if qos == 3 {
                return Err(MqttError::Protocol("PUBLISH QoS 3"));
            }
            let topic = r.string()?;
            if topic.contains(['+', '#']) {
                return Err(MqttError::Protocol("wildcard in PUBLISH topic"));
            }
            let pid = if qos > 0 { Some(r.u16()?) } else { None };
            Packet::Publish(Publish {
                dup: flags & 0x08 != 0,
                qos,
                retain: flags & 1 != 0,
                topic,
                pid,
                payload: r.rest().to_vec(),
            })
        }
        4 => {
            expect_flags(0)?;
            Packet::PubAck(r.u16()?)
        }
        5 => {
            expect_flags(0)?;
            Packet::PubRec(r.u16()?)
        }
        6 => {
            expect_flags(2)?;
            Packet::PubRel(r.u16()?)
        }
        7 => {
            expect_flags(0)?;
            Packet::PubComp(r.u16()?)
        }
        8 => {
            expect_flags(2)?;
            let pid = r.u16()?;
            let mut topics = Vec::new();
            while !r.done() {
                let t = r.string()?;
                let q = r.u8()?;
                if q > 2 {
                    return Err(MqttError::Protocol("SUBSCRIBE QoS > 2"));
                }
                if !valid_filter(&t) {
                    return Err(MqttError::Protocol("invalid topic filter"));
                }
                topics.push((t, q));
            }
            if topics.is_empty() {
                return Err(MqttError::Protocol("SUBSCRIBE without topics"));
            }
            Packet::Subscribe { pid, topics }
        }
        9 => {
            expect_flags(0)?;
            let pid = r.u16()?;
            Packet::SubAck {
                pid,
                codes: r.rest().to_vec(),
            }
        }
        10 => {
            expect_flags(2)?;
            let pid = r.u16()?;
            let mut topics = Vec::new();
            while !r.done() {
                topics.push(r.string()?);
            }
            if topics.is_empty() {
                return Err(MqttError::Protocol("UNSUBSCRIBE without topics"));
            }
            Packet::Unsubscribe { pid, topics }
        }
        11 => {
            expect_flags(0)?;
            Packet::UnsubAck(r.u16()?)
        }
        12 => {
            expect_flags(0)?;
            Packet::PingReq
        }
        13 => {
            expect_flags(0)?;
            Packet::PingResp
        }
        14 => {
            expect_flags(0)?;
            Packet::Disconnect
        }
        _ => return Err(MqttError::Malformed("reserved packet type")),
    };
    Ok(p)
}

/// A topic filter is valid if `#` only appears alone as the last level and `+` only alone in
/// a level.
pub fn valid_filter(f: &str) -> bool {
    if f.is_empty() {
        return false;
    }
    let levels: Vec<&str> = f.split('/').collect();
    for (i, l) in levels.iter().enumerate() {
        if l.contains('#') && (*l != "#" || i != levels.len() - 1) {
            return false;
        }
        if l.contains('+') && *l != "+" {
            return false;
        }
    }
    true
}

/// MQTT topic matching with `+` / `#` wildcards; `$`-topics are not matched by a leading
/// wildcard.
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    if topic.starts_with('$') && (filter.starts_with('+') || filter.starts_with('#')) {
        return false;
    }
    let mut f = filter.split('/');
    let mut t = topic.split('/');
    loop {
        match (f.next(), t.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(a), Some(b)) if a == b => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// Splits a byte stream into complete packets, keeping partial data (one per connection).
#[derive(Debug, Default, Clone)]
pub struct StreamDecoder {
    buf: Vec<u8>,
}

impl StreamDecoder {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
    /// Next complete packet, if any.
    pub fn next_packet(&mut self) -> Result<Option<Packet>, MqttError> {
        match decode(&self.buf)? {
            Some((p, n)) => {
                self.buf.drain(..n);
                Ok(Some(p))
            }
            None => Ok(None),
        }
    }
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

/// Human summary of the packets at the start of a TCP payload (for the packet log), e.g.
/// `MQTT CONNECT client=garden-node` or `PUBLISH garden/pot1 · 40 B · QoS 0`.
pub fn summarize(data: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut off = 0;
    while off < data.len() {
        match decode(&data[off..]) {
            Ok(Some((p, n))) => {
                parts.push(summary_of(&p, n));
                off += n;
            }
            _ => break,
        }
    }
    if parts.is_empty() {
        return None;
    }
    if off < data.len() {
        parts.push(format!("+{} B", data.len() - off));
    }
    Some(parts.join(" + "))
}

/// Summary line for one packet of `size` bytes.
pub fn summary_of(p: &Packet, size: usize) -> String {
    match p {
        Packet::Connect(c) => format!("MQTT CONNECT client={}", c.client_id),
        Packet::ConnAck { code, .. } => format!("MQTT CONNACK rc={code}"),
        Packet::Publish(p) => format!(
            "PUBLISH {} · {} B · QoS {}{}",
            p.topic,
            size,
            p.qos,
            if p.retain { " · retained" } else { "" }
        ),
        Packet::Subscribe { topics, .. } => format!(
            "MQTT SUBSCRIBE {}",
            topics
                .iter()
                .map(|(t, q)| format!("{t} (QoS {q})"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Packet::SubAck { codes, .. } => format!("MQTT SUBACK {codes:?}"),
        Packet::Unsubscribe { topics, .. } => format!("MQTT UNSUBSCRIBE {}", topics.join(", ")),
        Packet::PubAck(id) => format!("MQTT PUBACK id={id}"),
        other => format!("MQTT {}", other.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_every_packet() {
        let mut c = Connect::new("garden-node", 60);
        c.username = Some("u".into());
        c.password = Some(b"p".to_vec());
        c.will = Some(Will {
            topic: "garden/status".into(),
            payload: b"offline".to_vec(),
            qos: 1,
            retain: true,
        });
        let packets = vec![
            Packet::Connect(c),
            Packet::ConnAck {
                session_present: true,
                code: 0,
            },
            Packet::Publish(Publish {
                dup: false,
                qos: 0,
                retain: false,
                topic: "garden/pot1".into(),
                pid: None,
                payload: br#"{"m":42,"lux":180,"t":21}"#.to_vec(),
            }),
            Packet::Publish(Publish {
                dup: true,
                qos: 1,
                retain: true,
                topic: "a/b".into(),
                pid: Some(7),
                payload: vec![0; 300],
            }),
            Packet::PubAck(7),
            Packet::PubRec(8),
            Packet::PubRel(8),
            Packet::PubComp(8),
            Packet::Subscribe {
                pid: 3,
                topics: vec![("garden/#".into(), 1), ("+/x".into(), 0)],
            },
            Packet::SubAck {
                pid: 3,
                codes: vec![1, 0],
            },
            Packet::Unsubscribe {
                pid: 4,
                topics: vec!["garden/#".into()],
            },
            Packet::UnsubAck(4),
            Packet::PingReq,
            Packet::PingResp,
            Packet::Disconnect,
        ];
        let mut stream = Vec::new();
        for p in &packets {
            let e = encode(p);
            assert_eq!(decode(&e).unwrap(), Some((p.clone(), e.len())));
            // Incomplete input asks for more.
            assert_eq!(decode(&e[..e.len() - 1]).unwrap(), None);
            stream.extend(e);
        }
        let mut d = StreamDecoder::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(7) {
            d.push(chunk);
            while let Some(p) = d.next_packet().unwrap() {
                got.push(p);
            }
        }
        assert_eq!(got, packets);
        // The design's payload makes a 40-byte PUBLISH.
        assert_eq!(encode(&packets[2]).len(), 40);
        assert_eq!(
            summarize(&encode(&packets[2])).unwrap(),
            "PUBLISH garden/pot1 · 40 B · QoS 0"
        );
    }

    #[test]
    fn rejects_malformed() {
        assert!(decode(&[0x82, 2, 0, 1]).is_err()); // SUBSCRIBE without topics
        assert!(decode(&[0x80, 0]).is_err()); // SUBSCRIBE with wrong flags
        assert!(decode(&[0x36, 0]).is_err()); // PUBLISH QoS 3
        assert!(decode(&[0xf0, 0]).is_err());
        assert!(decode(&[0x30, 0xff, 0xff, 0xff, 0xff, 0x01]).is_err());
        let bad_topic = encode(&Packet::Publish(Publish {
            dup: false,
            qos: 0,
            retain: false,
            topic: "a/+".into(),
            pid: None,
            payload: vec![],
        }));
        assert!(decode(&bad_topic).is_err());
    }

    #[test]
    fn wildcard_matching() {
        assert!(topic_matches("garden/#", "garden/pot1"));
        assert!(topic_matches("garden/#", "garden"));
        assert!(topic_matches("#", "a/b/c"));
        assert!(topic_matches("+/pot1", "garden/pot1"));
        assert!(topic_matches("garden/+/t", "garden/pot1/t"));
        assert!(!topic_matches("garden/+", "garden/pot1/t"));
        assert!(!topic_matches("garden/pot2", "garden/pot1"));
        assert!(!topic_matches("#", "$SYS/uptime"));
        assert!(topic_matches("$SYS/#", "$SYS/uptime"));
        assert!(topic_matches("+/+", "/x"));
        assert!(
            valid_filter("a/+/#")
                && !valid_filter("a/#/b")
                && !valid_filter("a+")
                && !valid_filter("")
        );
    }
}
