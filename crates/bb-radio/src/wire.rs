//! Byte-level encoders/decoders for the protocols the virtual network carries: synthetic
//! Ethernet/IPv4/TCP/UDP/ICMP frames (packet-log pcap export), DHCP, DNS and NTP messages.
//! Everything is real wire format so captures open cleanly in Wireshark.

use std::net::{Ipv4Addr, SocketAddrV4};

/// TCP flag bits.
pub mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const PSH: u8 = 0x08;
    pub const ACK: u8 = 0x10;
}

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

fn sum16(data: &[u8], mut acc: u32) -> u32 {
    let (chunks, rem) = data.as_chunks::<2>();
    for c in chunks {
        acc += u16::from_be_bytes(*c) as u32;
    }
    if let [b] = rem {
        acc += (*b as u32) << 8;
    }
    acc
}

fn fold(mut acc: u32) -> u16 {
    while acc > 0xffff {
        acc = (acc & 0xffff) + (acc >> 16);
    }
    !(acc as u16)
}

/// Internet checksum (RFC 1071).
pub fn checksum(data: &[u8]) -> u16 {
    fold(sum16(data, 0))
}

fn pseudo_checksum(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, segment: &[u8]) -> u16 {
    let mut acc = sum16(&src.octets(), 0);
    acc = sum16(&dst.octets(), acc);
    acc += proto as u32;
    acc += segment.len() as u32;
    fold(sum16(segment, acc))
}

/// IPv4 header + payload.
pub fn ipv4_packet(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, ident: u16, payload: &[u8]) -> Vec<u8> {
    let total = 20 + payload.len();
    let mut h = vec![
        0x45,
        0,
        (total >> 8) as u8,
        total as u8,
        (ident >> 8) as u8,
        ident as u8,
        0x40, // don't fragment
        0,
        64,
        proto,
        0,
        0,
    ];
    h.extend_from_slice(&src.octets());
    h.extend_from_slice(&dst.octets());
    let c = checksum(&h);
    h[10] = (c >> 8) as u8;
    h[11] = c as u8;
    h.extend_from_slice(payload);
    h
}

/// TCP segment (20-byte header, no options) with a valid checksum.
pub fn tcp_segment(
    src: SocketAddrV4,
    dst: SocketAddrV4,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    data: &[u8],
) -> Vec<u8> {
    let mut s = Vec::with_capacity(20 + data.len());
    s.extend_from_slice(&src.port().to_be_bytes());
    s.extend_from_slice(&dst.port().to_be_bytes());
    s.extend_from_slice(&seq.to_be_bytes());
    s.extend_from_slice(&ack.to_be_bytes());
    s.push(5 << 4);
    s.push(flags);
    s.extend_from_slice(&window.to_be_bytes());
    s.extend_from_slice(&[0, 0, 0, 0]);
    s.extend_from_slice(data);
    let c = pseudo_checksum(*src.ip(), *dst.ip(), PROTO_TCP, &s);
    s[16] = (c >> 8) as u8;
    s[17] = c as u8;
    s
}

/// UDP datagram with a valid checksum.
pub fn udp_datagram(src: SocketAddrV4, dst: SocketAddrV4, data: &[u8]) -> Vec<u8> {
    let len = 8 + data.len();
    let mut s = Vec::with_capacity(len);
    s.extend_from_slice(&src.port().to_be_bytes());
    s.extend_from_slice(&dst.port().to_be_bytes());
    s.extend_from_slice(&(len as u16).to_be_bytes());
    s.extend_from_slice(&[0, 0]);
    s.extend_from_slice(data);
    let mut c = pseudo_checksum(*src.ip(), *dst.ip(), PROTO_UDP, &s);
    if c == 0 {
        c = 0xffff;
    }
    s[6] = (c >> 8) as u8;
    s[7] = c as u8;
    s
}

/// ICMP echo request (type 8) or reply (type 0).
pub fn icmp_echo(reply: bool, id: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let mut s = vec![if reply { 0 } else { 8 }, 0, 0, 0];
    s.extend_from_slice(&id.to_be_bytes());
    s.extend_from_slice(&seq.to_be_bytes());
    s.extend_from_slice(data);
    let c = checksum(&s);
    s[2] = (c >> 8) as u8;
    s[3] = c as u8;
    s
}

/// Ethernet II frame carrying IPv4.
pub fn ethernet_frame(dst: [u8; 6], src: [u8; 6], ip_packet: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(14 + ip_packet.len());
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&[0x08, 0x00]);
    f.extend_from_slice(ip_packet);
    f
}

/// Formats a MAC address `aa:bb:cc:dd:ee:ff`.
pub fn mac_string(m: [u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

// ------------------------------------------------------------------------------------- DHCP

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl DhcpType {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => DhcpType::Discover,
            2 => DhcpType::Offer,
            3 => DhcpType::Request,
            4 => DhcpType::Decline,
            5 => DhcpType::Ack,
            6 => DhcpType::Nak,
            7 => DhcpType::Release,
            8 => DhcpType::Inform,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            DhcpType::Discover => "DISCOVER",
            DhcpType::Offer => "OFFER",
            DhcpType::Request => "REQUEST",
            DhcpType::Decline => "DECLINE",
            DhcpType::Ack => "ACK",
            DhcpType::Nak => "NAK",
            DhcpType::Release => "RELEASE",
            DhcpType::Inform => "INFORM",
        }
    }
}

/// The subset of a DHCP message Breadbox uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpMessage {
    /// 1 = BOOTREQUEST, 2 = BOOTREPLY.
    pub op: u8,
    pub xid: u32,
    pub ciaddr: Ipv4Addr,
    pub yiaddr: Ipv4Addr,
    pub siaddr: Ipv4Addr,
    pub chaddr: [u8; 6],
    pub msg_type: DhcpType,
    pub requested_ip: Option<Ipv4Addr>,
    pub server_id: Option<Ipv4Addr>,
    pub lease_secs: Option<u32>,
    pub subnet_mask: Option<Ipv4Addr>,
    pub router: Option<Ipv4Addr>,
    pub dns: Option<Ipv4Addr>,
    pub hostname: Option<String>,
    pub domain: Option<String>,
}

impl DhcpMessage {
    pub fn request(msg_type: DhcpType, xid: u32, chaddr: [u8; 6]) -> Self {
        DhcpMessage {
            op: 1,
            xid,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            chaddr,
            msg_type,
            requested_ip: None,
            server_id: None,
            lease_secs: None,
            subnet_mask: None,
            router: None,
            dns: None,
            hostname: None,
            domain: None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut m = vec![0u8; 236];
        m[0] = self.op;
        m[1] = 1;
        m[2] = 6;
        m[4..8].copy_from_slice(&self.xid.to_be_bytes());
        if self.op == 1 {
            m[10] = 0x80; // broadcast flag: clients have no address yet
        }
        m[12..16].copy_from_slice(&self.ciaddr.octets());
        m[16..20].copy_from_slice(&self.yiaddr.octets());
        m[20..24].copy_from_slice(&self.siaddr.octets());
        m[28..34].copy_from_slice(&self.chaddr);
        m.extend_from_slice(&[99, 130, 83, 99]);
        m.extend_from_slice(&[53, 1, self.msg_type as u8]);
        let ip_opt = |m: &mut Vec<u8>, code: u8, ip: Option<Ipv4Addr>| {
            if let Some(ip) = ip {
                m.extend_from_slice(&[code, 4]);
                m.extend_from_slice(&ip.octets());
            }
        };
        ip_opt(&mut m, 50, self.requested_ip);
        ip_opt(&mut m, 54, self.server_id);
        if let Some(l) = self.lease_secs {
            m.extend_from_slice(&[51, 4]);
            m.extend_from_slice(&l.to_be_bytes());
        }
        ip_opt(&mut m, 1, self.subnet_mask);
        ip_opt(&mut m, 3, self.router);
        ip_opt(&mut m, 6, self.dns);
        for (code, s) in [(12u8, &self.hostname), (15u8, &self.domain)] {
            if let Some(s) = s {
                let b = &s.as_bytes()[..s.len().min(255)];
                m.push(code);
                m.push(b.len() as u8);
                m.extend_from_slice(b);
            }
        }
        if self.op == 1 {
            // Parameter request list: mask, router, DNS, domain, lease.
            m.extend_from_slice(&[55, 5, 1, 3, 6, 15, 51]);
        }
        m.push(255);
        while m.len() < 300 {
            m.push(0);
        }
        m
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < 240 || b[236..240] != [99, 130, 83, 99] {
            return None;
        }
        let ip = |s: &[u8]| Ipv4Addr::new(s[0], s[1], s[2], s[3]);
        let mut chaddr = [0u8; 6];
        chaddr.copy_from_slice(&b[28..34]);
        let mut msg = DhcpMessage {
            op: b[0],
            xid: u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            ciaddr: ip(&b[12..16]),
            yiaddr: ip(&b[16..20]),
            siaddr: ip(&b[20..24]),
            chaddr,
            msg_type: DhcpType::Discover,
            requested_ip: None,
            server_id: None,
            lease_secs: None,
            subnet_mask: None,
            router: None,
            dns: None,
            hostname: None,
            domain: None,
        };
        let mut found_type = false;
        let mut i = 240;
        while i < b.len() {
            let code = b[i];
            if code == 255 {
                break;
            }
            if code == 0 {
                i += 1;
                continue;
            }
            let len = *b.get(i + 1)? as usize;
            let v = b.get(i + 2..i + 2 + len)?;
            match (code, len) {
                (53, 1) => {
                    msg.msg_type = DhcpType::from_u8(v[0])?;
                    found_type = true;
                }
                (50, 4) => msg.requested_ip = Some(ip(v)),
                (54, 4) => msg.server_id = Some(ip(v)),
                (51, 4) => msg.lease_secs = Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]])),
                (1, 4) => msg.subnet_mask = Some(ip(v)),
                (3, l) if l >= 4 => msg.router = Some(ip(v)),
                (6, l) if l >= 4 => msg.dns = Some(ip(v)),
                (12, _) => msg.hostname = Some(String::from_utf8_lossy(v).into_owned()),
                (15, _) => msg.domain = Some(String::from_utf8_lossy(v).into_owned()),
                _ => {}
            }
            i += 2 + len;
        }
        found_type.then_some(msg)
    }
}

// -------------------------------------------------------------------------------------- DNS

/// A DNS message with A-record questions/answers only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsMessage {
    pub id: u16,
    pub response: bool,
    /// RCODE (0 = no error, 3 = NXDOMAIN).
    pub rcode: u8,
    pub name: String,
    /// Question type (1 = A).
    pub qtype: u16,
    pub answers: Vec<(Ipv4Addr, u32)>,
}

impl DnsMessage {
    pub fn query(id: u16, name: &str) -> Self {
        DnsMessage {
            id,
            response: false,
            rcode: 0,
            name: name.to_string(),
            qtype: 1,
            answers: vec![],
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(&self.id.to_be_bytes());
        let flags: u16 = if self.response {
            0x8180 | self.rcode as u16
        } else {
            0x0100
        };
        m.extend_from_slice(&flags.to_be_bytes());
        m.extend_from_slice(&1u16.to_be_bytes());
        m.extend_from_slice(&(self.answers.len() as u16).to_be_bytes());
        m.extend_from_slice(&[0, 0, 0, 0]);
        for label in self.name.trim_end_matches('.').split('.') {
            let l = &label.as_bytes()[..label.len().min(63)];
            m.push(l.len() as u8);
            m.extend_from_slice(l);
        }
        m.push(0);
        m.extend_from_slice(&self.qtype.to_be_bytes());
        m.extend_from_slice(&1u16.to_be_bytes());
        for (ip, ttl) in &self.answers {
            m.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
            m.extend_from_slice(&ttl.to_be_bytes());
            m.extend_from_slice(&[0, 4]);
            m.extend_from_slice(&ip.octets());
        }
        m
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < 12 {
            return None;
        }
        let id = u16::from_be_bytes([b[0], b[1]]);
        let flags = u16::from_be_bytes([b[2], b[3]]);
        let qd = u16::from_be_bytes([b[4], b[5]]);
        let an = u16::from_be_bytes([b[6], b[7]]);
        if qd != 1 {
            return None;
        }
        let mut i = 12;
        let mut labels = Vec::new();
        loop {
            let l = *b.get(i)? as usize;
            i += 1;
            if l == 0 {
                break;
            }
            if l & 0xc0 != 0 {
                return None;
            }
            labels.push(String::from_utf8_lossy(b.get(i..i + l)?).into_owned());
            i += l;
        }
        let qtype = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]);
        i += 4;
        let mut answers = Vec::new();
        for _ in 0..an {
            // name (pointer or labels)
            if *b.get(i)? & 0xc0 == 0xc0 {
                i += 2;
            } else {
                loop {
                    let l = *b.get(i)? as usize;
                    i += 1 + l;
                    if l == 0 {
                        break;
                    }
                }
            }
            let rtype = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]);
            let ttl = u32::from_be_bytes([
                *b.get(i + 4)?,
                *b.get(i + 5)?,
                *b.get(i + 6)?,
                *b.get(i + 7)?,
            ]);
            let rdlen = u16::from_be_bytes([*b.get(i + 8)?, *b.get(i + 9)?]) as usize;
            i += 10;
            let rd = b.get(i..i + rdlen)?;
            if rtype == 1 && rdlen == 4 {
                answers.push((Ipv4Addr::new(rd[0], rd[1], rd[2], rd[3]), ttl));
            }
            i += rdlen;
        }
        Some(DnsMessage {
            id,
            response: flags & 0x8000 != 0,
            rcode: (flags & 0xf) as u8,
            name: labels.join("."),
            qtype,
            answers,
        })
    }
}

// -------------------------------------------------------------------------------------- NTP

/// Seconds between the NTP era (1900) and the Unix epoch.
pub const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// NTP timestamp (seconds since 1900 + 2^-32 fractions) for a Unix time in seconds.
pub fn ntp_timestamp(unix_secs: f64) -> u64 {
    let s = unix_secs + NTP_UNIX_OFFSET as f64;
    let whole = s.floor();
    (((whole as u64) & 0xffff_ffff) << 32) | (((s - whole) * 4_294_967_296.0) as u64 & 0xffff_ffff)
}

/// Unix seconds from an NTP timestamp.
pub fn ntp_to_unix(ts: u64) -> f64 {
    (ts >> 32) as f64 - NTP_UNIX_OFFSET as f64 + (ts & 0xffff_ffff) as f64 / 4_294_967_296.0
}

/// A 48-byte NTPv4 client request.
pub fn ntp_request(transmit_unix: f64) -> Vec<u8> {
    let mut m = vec![0u8; 48];
    m[0] = 0x23; // LI 0, VN 4, mode 3 (client)
    m[40..48].copy_from_slice(&ntp_timestamp(transmit_unix).to_be_bytes());
    m
}

/// Server reply to `request` with receive/transmit timestamps `now_unix`.
pub fn ntp_reply(request: &[u8], now_unix: f64) -> Option<Vec<u8>> {
    if request.len() < 48 || request[0] & 0x7 != 3 {
        return None;
    }
    let mut m = vec![0u8; 48];
    m[0] = 0x24; // LI 0, VN 4, mode 4 (server)
    m[1] = 1; // stratum 1 (primary reference: the sim clock)
    m[2] = request[2];
    m[3] = 0xec; // precision ~ 2^-20 s
    m[12..16].copy_from_slice(b"SIM\0");
    let ts = ntp_timestamp(now_unix).to_be_bytes();
    m[16..24].copy_from_slice(&ts);
    m[24..32].copy_from_slice(&request[40..48]);
    m[32..40].copy_from_slice(&ts);
    m[40..48].copy_from_slice(&ts);
    Some(m)
}

/// Transmit timestamp (Unix seconds) of an NTP reply.
pub fn ntp_reply_time(reply: &[u8]) -> Option<f64> {
    if reply.len() < 48 || reply[0] & 0x7 != 4 {
        return None;
    }
    let ts = u64::from_be_bytes(reply[40..48].try_into().ok()?);
    Some(ntp_to_unix(ts))
}

/// Civil date-time (UTC) from Unix seconds: (year, month 1-12, day, hour, min, sec, weekday 0=Sun).
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let wd = ((days + 4).rem_euclid(7)) as u32;
    // Howard Hinnant's days_from_civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, h, mi, s, wd)
}

/// `asctime`-style text, as ESP-AT's `+CIPSNTPTIME:` prints it: `Thu Jan  1 00:00:00 1970`.
pub fn asctime(unix: i64) -> String {
    const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d, h, mi, s, wd) = civil_from_unix(unix);
    format!(
        "{} {} {:>2} {:02}:{:02}:{:02} {}",
        WD[wd as usize],
        MO[m as usize - 1],
        d,
        h,
        mi,
        s,
        y
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_verify_to_zero() {
        let src = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 42), 50_000);
        let dst = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 1883);
        let seg = tcp_segment(
            src,
            dst,
            1,
            2,
            tcp_flags::ACK | tcp_flags::PSH,
            5840,
            b"hello",
        );
        assert_eq!(pseudo_checksum(*src.ip(), *dst.ip(), PROTO_TCP, &seg), 0);
        let ip = ipv4_packet(*src.ip(), *dst.ip(), PROTO_TCP, 7, &seg);
        assert_eq!(checksum(&ip[..20]), 0);
        let udp = udp_datagram(src, dst, b"odd");
        assert_eq!(pseudo_checksum(*src.ip(), *dst.ip(), PROTO_UDP, &udp), 0);
        assert_eq!(checksum(&icmp_echo(false, 1, 2, b"abc")), 0);
    }

    #[test]
    fn dhcp_dns_ntp_round_trip() {
        let mut m = DhcpMessage::request(DhcpType::Request, 0xdead_beef, [1, 2, 3, 4, 5, 6]);
        m.requested_ip = Some(Ipv4Addr::new(10, 0, 0, 42));
        m.server_id = Some(Ipv4Addr::new(10, 0, 0, 1));
        m.hostname = Some("garden-node".into());
        let enc = m.encode();
        assert!(enc.len() >= 300);
        assert_eq!(DhcpMessage::decode(&enc).unwrap(), m);

        let mut q = DnsMessage::query(77, "broker.sim");
        assert_eq!(DnsMessage::decode(&q.encode()).unwrap(), q);
        q.response = true;
        q.answers.push((Ipv4Addr::new(10, 0, 0, 1), 60));
        assert_eq!(DnsMessage::decode(&q.encode()).unwrap(), q);

        let req = ntp_request(1_000.25);
        let rep = ntp_reply(&req, 1_767_225_600.5).unwrap();
        assert!((ntp_reply_time(&rep).unwrap() - 1_767_225_600.5).abs() < 1e-6);
        assert_eq!(asctime(0), "Thu Jan  1 00:00:00 1970");
        assert_eq!(asctime(1_767_225_600), "Thu Jan  1 00:00:00 2026");
        assert_eq!(asctime(1_794_657_600), "Sat Nov 14 12:00:00 2026");
    }
}
