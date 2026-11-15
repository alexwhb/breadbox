//! Packet log (`RF-4`): one row per frame on the virtual air — time, direction (`tx` / `rx` /
//! `!` for fault-injector drops), the radio it belongs to, a protocol-aware summary
//! (`DHCP REQUEST 10.0.0.42`, `MQTT CONNECT client=garden-node`,
//! `PUBLISH garden/pot1 · 40 B · QoS 0`), the payload size and, for IP traffic, a synthetic
//! Ethernet/IPv4 frame so the log exports to **pcap-ng** (Wireshark) as well as CSV.

use bb_core::SimTime;
use serde::Serialize;
use std::collections::VecDeque;
use std::net::SocketAddrV4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Dir {
    /// Sent by a bench radio.
    Tx,
    /// Received by a bench radio.
    Rx,
    /// Lost (fault injector, AP down, weak signal, collision, …).
    Drop,
}

impl Dir {
    pub fn label(self) -> &'static str {
        match self {
            Dir::Tx => "tx",
            Dir::Rx => "rx",
            Dir::Drop => "!",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Proto {
    Wifi,
    Dhcp,
    Dns,
    Ntp,
    Tcp,
    Udp,
    Icmp,
    Mqtt,
    Http,
    Ble,
    Lora,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Wifi => "Wi-Fi",
            Proto::Dhcp => "DHCP",
            Proto::Dns => "DNS",
            Proto::Ntp => "NTP",
            Proto::Tcp => "TCP",
            Proto::Udp => "UDP",
            Proto::Icmp => "ICMP",
            Proto::Mqtt => "MQTT",
            Proto::Http => "HTTP",
            Proto::Ble => "BLE",
            Proto::Lora => "LoRa",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum DropReason {
    /// Random loss from the network-conditions panel.
    Loss { pct: f64 },
    /// The access point is down ("drop the AP at t = N s").
    ApDown,
    /// The receiver is not associated / unknown destination.
    NoRoute,
    /// Signal below the receiver's sensitivity.
    WeakSignal,
    /// LoRa: overlapping transmission on the same channel/SF.
    Collision,
}

impl DropReason {
    pub fn text(&self) -> String {
        match self {
            DropReason::Loss { pct } => {
                format!("dropped by fault injector (loss {}%)", fmt_pct(*pct))
            }
            DropReason::ApDown => "dropped: access point down".into(),
            DropReason::NoRoute => "dropped: no route to host".into(),
            DropReason::WeakSignal => "dropped: signal below sensitivity".into(),
            DropReason::Collision => "dropped: collision".into(),
        }
    }
}

fn fmt_pct(p: f64) -> String {
    if (p - p.round()).abs() < 1e-9 {
        format!("{}", p.round() as i64)
    } else {
        format!("{p:.1}")
    }
}

/// One row of the packet log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogEntry {
    /// Monotonic row number (stable across ring-buffer trimming; lets the UI fetch increments).
    pub seq: u64,
    pub time: SimTime,
    pub dir: Dir,
    /// Label of the bench radio involved ("U2"), if any.
    pub radio: Option<String>,
    pub proto: Proto,
    pub summary: String,
    /// Application payload bytes (the MQTT packet for MQTT rows, the datagram for UDP, …).
    pub bytes: usize,
    pub src: Option<SocketAddrV4>,
    pub dst: Option<SocketAddrV4>,
    pub dropped: Option<DropReason>,
    /// Pure TCP ACK (hidden from the default view).
    pub ack_only: bool,
    /// Synthetic Ethernet frame for pcap export (IP traffic only).
    #[serde(skip)]
    pub frame: Option<Vec<u8>>,
}

impl LogEntry {
    /// Text of the "frame" column: the summary, or the drop reason for `!` rows.
    pub fn text(&self) -> String {
        match &self.dropped {
            Some(r) => r.text(),
            None => self.summary.clone(),
        }
    }

    /// Time column as in the design: `01.870`, or `m:ss.mmm` past a minute.
    pub fn time_label(&self) -> String {
        time_label(self.time)
    }
}

/// `ss.mmm` (or `m:ss.mmm`, `h:mm:ss.mmm`).
pub fn time_label(t: SimTime) -> String {
    let ms = t.as_millis();
    let (h, m, s, f) = (
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000,
    );
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{f:03}")
    } else if m > 0 {
        format!("{m}:{s:02}.{f:03}")
    } else {
        format!("{s:02}.{f:03}")
    }
}

/// Ring buffer of log rows.
#[derive(Debug, Clone)]
pub struct PacketLog {
    entries: VecDeque<LogEntry>,
    pub capacity: usize,
    next_seq: u64,
}

impl Default for PacketLog {
    fn default() -> Self {
        PacketLog {
            entries: VecDeque::new(),
            capacity: 20_000,
            next_seq: 0,
        }
    }
}

impl PacketLog {
    pub fn push(&mut self, mut e: LogEntry) {
        e.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(e);
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    /// Rows with `seq >= from` (incremental UI updates).
    pub fn since(&self, from: u64) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter().filter(move |e| e.seq >= from)
    }

    /// Rows for the default view (no pure ACKs).
    pub fn visible(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter().filter(|e| !e.ack_only)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sequence number the next row will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// CSV export: `time_s,dir,radio,proto,src,dst,bytes,frame,dropped`.
    pub fn to_csv(&self) -> String {
        let mut s = String::from("time_s,dir,radio,proto,src,dst,bytes,frame,dropped\n");
        for e in &self.entries {
            let q = |x: &str| {
                if x.contains([',', '"', '\n']) {
                    format!("\"{}\"", x.replace('"', "\"\""))
                } else {
                    x.to_string()
                }
            };
            s.push_str(&format!(
                "{:.6},{},{},{},{},{},{},{},{}\n",
                e.time.as_secs_f64(),
                e.dir.label(),
                q(e.radio.as_deref().unwrap_or("")),
                e.proto.label(),
                e.src.map(|a| a.to_string()).unwrap_or_default(),
                e.dst.map(|a| a.to_string()).unwrap_or_default(),
                e.bytes,
                q(&e.summary),
                q(&e.dropped.as_ref().map(|d| d.text()).unwrap_or_default()),
            ));
        }
        s
    }

    /// pcap-ng export (Ethernet link type, µs timestamps). Rows without an IP frame (Wi-Fi
    /// management, BLE, LoRa) are skipped; drops carry an `opt_comment`.
    pub fn to_pcapng(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // Section header block.
        let mut shb = Vec::new();
        shb.extend_from_slice(&0x1A2B_3C4Du32.to_le_bytes());
        shb.extend_from_slice(&1u16.to_le_bytes());
        shb.extend_from_slice(&0u16.to_le_bytes());
        shb.extend_from_slice(&u64::MAX.to_le_bytes());
        push_option(&mut shb, 4, b"Breadbox virtual RF space"); // shb_userappl
        end_options(&mut shb);
        push_block(&mut out, 0x0A0D_0D0A, &shb);
        // Interface description block.
        let mut idb = Vec::new();
        idb.extend_from_slice(&1u16.to_le_bytes()); // LINKTYPE_ETHERNET
        idb.extend_from_slice(&0u16.to_le_bytes());
        idb.extend_from_slice(&0u32.to_le_bytes());
        push_option(&mut idb, 2, b"sim0"); // if_name
        push_option(&mut idb, 9, &[6]); // if_tsresol = 10^-6
        end_options(&mut idb);
        push_block(&mut out, 1, &idb);
        for e in &self.entries {
            let Some(frame) = &e.frame else { continue };
            let us = e.time.as_ps() / 1_000_000;
            let mut epb = Vec::new();
            epb.extend_from_slice(&0u32.to_le_bytes());
            epb.extend_from_slice(&((us >> 32) as u32).to_le_bytes());
            epb.extend_from_slice(&(us as u32).to_le_bytes());
            epb.extend_from_slice(&(frame.len() as u32).to_le_bytes());
            epb.extend_from_slice(&(frame.len() as u32).to_le_bytes());
            epb.extend_from_slice(frame);
            pad4(&mut epb);
            push_option(&mut epb, 1, e.text().as_bytes()); // opt_comment
            end_options(&mut epb);
            push_block(&mut out, 6, &epb);
        }
        out
    }
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn push_option(v: &mut Vec<u8>, code: u16, data: &[u8]) {
    v.extend_from_slice(&code.to_le_bytes());
    v.extend_from_slice(&(data.len() as u16).to_le_bytes());
    v.extend_from_slice(data);
    pad4(v);
}

fn end_options(v: &mut Vec<u8>) {
    v.extend_from_slice(&[0, 0, 0, 0]);
}

fn push_block(out: &mut Vec<u8>, ty: u32, body: &[u8]) {
    let total = (12 + body.len()) as u32;
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(&total.to_le_bytes());
}

/// One packet read back from a pcap-ng file: (timestamp µs, frame, comment).
pub type PcapPacket = (u64, Vec<u8>, Option<String>);

/// Minimal pcap-ng reader used by tests: returns (timestamp µs, frame, comment) per EPB.
pub fn read_pcapng(data: &[u8]) -> Option<Vec<PcapPacket>> {
    let mut out = vec![];
    let mut i = 0;
    let u32le = |b: &[u8], i: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
    };
    while i < data.len() {
        let ty = u32le(data, i)?;
        let len = u32le(data, i + 4)? as usize;
        if len < 12 || i + len > data.len() || u32le(data, i + len - 4)? as usize != len {
            return None;
        }
        if ty == 6 {
            let b = &data[i + 8..i + len - 4];
            let ts = ((u32le(b, 4)? as u64) << 32) | u32le(b, 8)? as u64;
            let cap = u32le(b, 12)? as usize;
            let frame = b.get(20..20 + cap)?.to_vec();
            let mut j = 20 + cap.div_ceil(4) * 4;
            let mut comment = None;
            while j + 4 <= b.len() {
                let code = u16::from_le_bytes([b[j], b[j + 1]]);
                let l = u16::from_le_bytes([b[j + 2], b[j + 3]]) as usize;
                if code == 0 {
                    break;
                }
                if code == 1 {
                    comment = Some(String::from_utf8_lossy(b.get(j + 4..j + 4 + l)?).into_owned());
                }
                j += 4 + l.div_ceil(4) * 4;
            }
            out.push((ts, frame, comment));
        }
        i += len;
    }
    Some(out)
}
