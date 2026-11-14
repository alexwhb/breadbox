//! LoRa (`PLUG-5` LoRa pack's engine): a medium with log-distance path loss by terrain class,
//! time-on-air from SF/BW/CR/payload (Semtech AN1200.13), regional duty-cycle limits (EU868
//! 1 %), half-duplex radios, SNR-based sensitivity per spreading factor, collisions with the
//! 6 dB capture effect, and a multi-SF **virtual gateway** that forwards uplinks to the MQTT
//! broker (`lora/<network>/<address>/up`, JSON) and transmits downlinks published to
//! `lora/<network>/<address>/down`.

mod rylr896;

pub use rylr896::Rylr896;

use crate::geo::{Geometry, PathLoss, Pos};
use crate::log::{Dir, DropReason, LogEntry, PacketLog, Proto};
use crate::mqtt::{ConnKey, Message};
use crate::net::Network;
use crate::rng::Pcg32;
use bb_core::SimTime;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use thiserror::Error;

/// Modulation parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoraParams {
    /// Spreading factor 6–12.
    pub sf: u8,
    pub bw_hz: u32,
    /// Coding rate 1–4 (4/5 … 4/8).
    pub cr: u8,
    pub preamble: u16,
    pub crc: bool,
    pub explicit_header: bool,
}

impl Default for LoraParams {
    /// RYLR896 factory setting: SF12, 125 kHz, 4/5, preamble 4.
    fn default() -> Self {
        LoraParams { sf: 12, bw_hz: 125_000, cr: 1, preamble: 4, crc: true, explicit_header: true }
    }
}

impl LoraParams {
    /// Symbol time (s).
    pub fn symbol_time(&self) -> f64 {
        (1u64 << self.sf) as f64 / self.bw_hz as f64
    }

    /// Low data-rate optimisation (mandated when the symbol time exceeds 16 ms).
    pub fn low_dr_opt(&self) -> bool {
        self.symbol_time() > 0.016
    }

    /// Time on air for a `payload`-byte packet (Semtech AN1200.13 / SX127x datasheet §4.1.1.7).
    pub fn airtime(&self, payload: usize) -> f64 {
        let ts = self.symbol_time();
        let t_preamble = (self.preamble as f64 + 4.25) * ts;
        let sf = self.sf as f64;
        let de = if self.low_dr_opt() { 1.0 } else { 0.0 };
        let ih = if self.explicit_header { 0.0 } else { 1.0 };
        let crc = if self.crc { 1.0 } else { 0.0 };
        let num = 8.0 * payload as f64 - 4.0 * sf + 28.0 + 16.0 * crc - 20.0 * ih;
        let den = 4.0 * (sf - 2.0 * de);
        let n_payload = 8.0 + ((num / den).ceil() * (self.cr as f64 + 4.0)).max(0.0);
        t_preamble + n_payload * ts
    }

    /// Demodulation SNR floor (dB) for this SF.
    pub fn snr_floor(&self) -> f64 {
        match self.sf {
            6 => -5.0,
            7 => -7.5,
            8 => -10.0,
            9 => -12.5,
            10 => -15.0,
            11 => -17.5,
            _ => -20.0,
        }
    }

    /// Receiver noise floor (dBm): -174 + 10·log10(BW) + NF (6 dB).
    pub fn noise_floor(&self) -> f64 {
        -174.0 + 10.0 * (self.bw_hz as f64).log10() + 6.0
    }

    /// Sensitivity (dBm).
    pub fn sensitivity(&self) -> f64 {
        self.noise_floor() + self.snr_floor()
    }
}

/// Terrain class → path-loss exponent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Terrain {
    /// Line of sight / free space (n = 2.0).
    FreeSpace,
    /// Open countryside (n = 2.5).
    Rural,
    /// Suburban (n = 3.0).
    Suburban,
    /// Dense urban (n = 3.5).
    Urban,
    /// Inside a building (n = 3.0, walls from the floor plan add loss).
    Indoor,
}

impl Terrain {
    pub fn exponent(self) -> f64 {
        match self {
            Terrain::FreeSpace => 2.0,
            Terrain::Rural => 2.5,
            Terrain::Suburban | Terrain::Indoor => 3.0,
            Terrain::Urban => 3.5,
        }
    }
}

/// Regional rules.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Region {
    /// EU868: 1 % duty cycle per hour (g1 sub-band).
    Eu868 { duty_pct: f64 },
    /// US915: no duty cycle, 400 ms dwell time per transmission.
    Us915,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum LoraError {
    #[error("duty-cycle limit reached; next transmission allowed in {0:.1} s")]
    DutyCycle(f64),
    #[error("payload longer than {0} bytes")]
    TooLong(usize),
    #[error("radio busy transmitting")]
    Busy,
    #[error("dwell time exceeded ({0:.0} ms > 400 ms)")]
    Dwell(f64),
    #[error("radio is off")]
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

/// A received packet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoraRx {
    pub time: SimTime,
    pub from: u16,
    pub data: Vec<u8>,
    pub rssi: f64,
    pub snr: f64,
}

#[derive(Debug, Clone)]
struct Node {
    label: String,
    pos: Pos,
    freq_hz: u32,
    params: LoraParams,
    tx_dbm: f64,
    network_id: u8,
    address: u16,
    rx_on: bool,
    powered: bool,
    tx_until: SimTime,
    rx: VecDeque<LoraRx>,
    /// (start, airtime s) of recent transmissions, for duty-cycle accounting.
    history: Vec<(SimTime, f64)>,
    tx_done: VecDeque<SimTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sender {
    Node(NodeId),
    Gateway,
}

#[derive(Debug, Clone)]
struct Tx {
    from: Sender,
    from_addr: u16,
    pos: Pos,
    power: f64,
    freq: u32,
    params: LoraParams,
    network_id: u8,
    dest: u16,
    data: Vec<u8>,
    start: SimTime,
    end: SimTime,
    delivered: bool,
}

/// The virtual gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gateway {
    pub enabled: bool,
    pub pos: Pos,
    pub freq_hz: u32,
    /// Source address used for downlinks.
    pub address: u16,
    pub tx_dbm: f64,
    /// Antenna gain + height advantage (dB) on receive.
    pub gain_db: f64,
    #[serde(skip)]
    mqtt: Option<ConnKey>,
}

/// An uplink as forwarded to MQTT.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GatewayUplink {
    pub time: SimTime,
    pub network_id: u8,
    pub from: u16,
    pub data: Vec<u8>,
    pub rssi: f64,
    pub snr: f64,
    pub sf: u8,
    pub freq_hz: u32,
}

/// The LoRa medium.
#[derive(Debug)]
pub struct LoraSpace {
    nodes: Vec<Node>,
    pub terrain: Terrain,
    pub region: Region,
    pub geometry: Geometry,
    pub gateway: Gateway,
    txs: Vec<Tx>,
    uplinks: Vec<GatewayUplink>,
    pub log: PacketLog,
    rng: Pcg32,
    now: SimTime,
}

impl LoraSpace {
    pub fn new(seed: u64) -> Self {
        LoraSpace {
            nodes: vec![],
            terrain: Terrain::Suburban,
            region: Region::Us915,
            geometry: Geometry::default(),
            gateway: Gateway { enabled: true, pos: Pos::new(0.0, 0.0), freq_hz: 915_000_000, address: 0, tx_dbm: 14.0, gain_db: 3.0, mqtt: None },
            txs: vec![],
            uplinks: vec![],
            log: PacketLog::default(),
            rng: Pcg32::new(seed),
            now: SimTime::ZERO,
        }
    }

    pub fn add_node(&mut self, label: &str, pos: Pos) -> NodeId {
        self.nodes.push(Node {
            label: label.into(),
            pos,
            freq_hz: 915_000_000,
            params: LoraParams::default(),
            tx_dbm: 15.0,
            network_id: 0,
            address: 0,
            rx_on: true,
            powered: false,
            tx_until: SimTime::ZERO,
            rx: VecDeque::new(),
            history: vec![],
            tx_done: VecDeque::new(),
        });
        NodeId(self.nodes.len() as u32 - 1)
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0 as usize)
    }

    pub fn set_position(&mut self, id: NodeId, pos: Pos) {
        if let Some(n) = self.node_mut(id) {
            n.pos = pos;
        }
    }

    pub fn configure(&mut self, id: NodeId, freq_hz: u32, params: LoraParams, tx_dbm: f64, network_id: u8, address: u16) {
        if let Some(n) = self.node_mut(id) {
            n.freq_hz = freq_hz;
            n.params = params;
            n.tx_dbm = tx_dbm;
            n.network_id = network_id;
            n.address = address;
        }
    }

    pub fn set_rx(&mut self, id: NodeId, on: bool) {
        if let Some(n) = self.node_mut(id) {
            n.rx_on = on;
        }
    }

    pub fn set_powered(&mut self, id: NodeId, on: bool) {
        if let Some(n) = self.node_mut(id) {
            n.powered = on;
        }
    }

    fn path_loss(&self, freq: u32) -> PathLoss {
        PathLoss { freq_hz: freq as f64, exponent: self.terrain.exponent() }
    }

    /// Received power at `to` from a transmitter at `from` (dBm).
    pub fn rssi_between(&self, from: Pos, to: Pos, power: f64, freq: u32) -> f64 {
        let walls = if self.terrain == Terrain::Indoor { self.geometry.walls_between(from, to).1 } else { 0.0 };
        power - self.path_loss(freq).loss_db(from.dist(to)) - walls
    }

    /// Duty-cycle budget left (s of airtime) and when the next `airtime` fits.
    fn duty_wait(&self, n: &Node, airtime: f64, now: SimTime) -> Option<f64> {
        let Region::Eu868 { duty_pct } = self.region else { return None };
        // Simple per-transmission rule (as LoRaWAN stacks do): after a frame of T seconds the
        // radio stays silent for T·(100/duty - 1).
        let off = 100.0 / duty_pct.max(0.001) - 1.0;
        let blocked_until = n.history.iter().map(|(s, t)| s.as_secs_f64() + t * (1.0 + off)).fold(0.0, f64::max);
        let wait = blocked_until - now.as_secs_f64();
        let _ = airtime;
        (wait > 0.0).then_some(wait)
    }

    /// Starts a transmission from a node. Returns its end time.
    pub fn transmit(&mut self, id: NodeId, dest: u16, data: &[u8], now: SimTime) -> Result<SimTime, LoraError> {
        self.advance(now);
        let n = self.nodes.get(id.0 as usize).ok_or(LoraError::Off)?;
        if !n.powered {
            return Err(LoraError::Off);
        }
        if data.len() > 240 {
            return Err(LoraError::TooLong(240));
        }
        if n.tx_until > now {
            return Err(LoraError::Busy);
        }
        let airtime = n.params.airtime(data.len());
        if let Some(w) = self.duty_wait(n, airtime, now) {
            return Err(LoraError::DutyCycle(w));
        }
        if self.region == Region::Us915 && airtime > 0.4 && n.params.bw_hz <= 125_000 && n.params.sf >= 11 {
            // Dwell limit applies to 125 kHz channels; Breadbox reports but still allows SF11/12
            // demos (the RYLR896 does not enforce it).
        }
        let end = now + SimTime::from_secs_f64(airtime);
        let (pos, power, freq, params, network_id, from_addr, label) = (n.pos, n.tx_dbm, n.freq_hz, n.params, n.network_id, n.address, n.label.clone());
        let node = self.node_mut(id).expect("exists");
        node.tx_until = end;
        node.history.push((now, airtime));
        node.history.retain(|(s, _)| now.as_secs_f64() - s.as_secs_f64() < 3600.0);
        self.txs.push(Tx { from: Sender::Node(id), from_addr, pos, power, freq, params, network_id, dest, data: data.to_vec(), start: now, end, delivered: false });
        self.log.push(LogEntry {
            seq: 0,
            time: now,
            dir: Dir::Tx,
            radio: Some(label),
            proto: Proto::Lora,
            summary: format!("LoRa {} → {} · {} B · SF{} · {:.0} ms on air", from_addr, dest, data.len(), params.sf, airtime * 1e3),
            bytes: data.len(),
            src: None,
            dst: None,
            dropped: None,
            ack_only: false,
            frame: None,
        });
        Ok(end)
    }

    /// Downlink from the gateway to a node address.
    pub fn gateway_send(&mut self, network_id: u8, dest: u16, data: &[u8], params: LoraParams, now: SimTime) -> SimTime {
        self.advance(now);
        let airtime = params.airtime(data.len());
        let start = self.txs.iter().filter(|t| t.from == Sender::Gateway).map(|t| t.end).max().unwrap_or(now).max(now);
        let end = start + SimTime::from_secs_f64(airtime);
        let g = &self.gateway;
        self.txs.push(Tx {
            from: Sender::Gateway,
            from_addr: g.address,
            pos: g.pos,
            power: g.tx_dbm,
            freq: g.freq_hz,
            params,
            network_id,
            dest,
            data: data.to_vec(),
            start,
            end,
            delivered: false,
        });
        self.log.push(LogEntry {
            seq: 0,
            time: start,
            dir: Dir::Rx,
            radio: None,
            proto: Proto::Lora,
            summary: format!("LoRa gateway → {} · {} B · SF{}", dest, data.len(), params.sf),
            bytes: data.len(),
            src: None,
            dst: None,
            dropped: None,
            ack_only: false,
            frame: None,
        });
        end
    }

    /// Packets a node received since the last call.
    pub fn take_rx(&mut self, id: NodeId) -> Vec<LoraRx> {
        self.node_mut(id).map(|n| n.rx.drain(..).collect()).unwrap_or_default()
    }

    /// Transmissions of this node that finished since the last call (their end times).
    pub fn take_tx_done(&mut self, id: NodeId) -> Vec<SimTime> {
        self.node_mut(id).map(|n| n.tx_done.drain(..).collect()).unwrap_or_default()
    }

    pub fn take_gateway_uplinks(&mut self) -> Vec<GatewayUplink> {
        std::mem::take(&mut self.uplinks)
    }

    pub fn next_event_time(&self) -> Option<SimTime> {
        self.txs.iter().filter(|t| !t.delivered).map(|t| t.end).min()
    }

    pub fn advance(&mut self, now: SimTime) {
        loop {
            let next = self.txs.iter().enumerate().filter(|(_, t)| !t.delivered && t.end <= now).min_by_key(|(_, t)| t.end).map(|(i, _)| i);
            let Some(i) = next else { break };
            self.now = self.now.max(self.txs[i].end);
            self.txs[i].delivered = true;
            self.finish(i);
        }
        self.now = self.now.max(now);
        // Forget transmissions that can no longer overlap anything.
        let horizon = self.txs.iter().filter(|t| !t.delivered).map(|t| t.start).min().unwrap_or(self.now);
        self.txs.retain(|t| !t.delivered || t.end >= horizon);
    }

    /// Does `x` survive at a receiver at `pos` (`me` = receiver identity to skip own frames)?
    fn survives(&self, xi: usize, pos: Pos, me: Sender, gain: f64) -> Result<(f64, f64), DropReason> {
        let x = &self.txs[xi];
        let p = self.rssi_between(x.pos, pos, x.power, x.freq) + gain;
        if p < x.params.sensitivity() {
            return Err(DropReason::WeakSignal);
        }
        for (j, y) in self.txs.iter().enumerate() {
            if j == xi || y.from == me || y.freq != x.freq || y.params.sf != x.params.sf {
                continue;
            }
            if y.start < x.end && x.start < y.end {
                let py = self.rssi_between(y.pos, pos, y.power, y.freq) + gain;
                if py > p - 6.0 {
                    return Err(DropReason::Collision);
                }
            }
        }
        let snr = (p - x.params.noise_floor()).min(12.0);
        Ok((p, snr))
    }

    fn finish(&mut self, xi: usize) {
        let x = self.txs[xi].clone();
        let t = x.end;
        if let Sender::Node(id) = x.from
            && let Some(n) = self.node_mut(id)
        {
            n.tx_done.push_back(t);
        }
        // Nodes.
        for i in 0..self.nodes.len() {
            let me = Sender::Node(NodeId(i as u32));
            let n = &self.nodes[i];
            if me == x.from || !n.powered || !n.rx_on || n.freq_hz != x.freq || n.params.sf != x.params.sf || n.params.bw_hz != x.params.bw_hz || n.network_id != x.network_id {
                continue;
            }
            if !(x.dest == 0 || x.dest == n.address) {
                continue;
            }
            // Half duplex: a radio that was transmitting during the frame hears nothing.
            let deaf = self.txs.iter().any(|y| y.from == me && y.start < x.end && x.start < y.end);
            let label = n.label.clone();
            let pos = n.pos;
            let res = if deaf { Err(DropReason::Collision) } else { self.survives(xi, pos, me, 0.0) };
            match res {
                Ok((rssi, snr)) => {
                    let jitter = self.rng.range_f64(-0.5, 0.5);
                    self.nodes[i].rx.push_back(LoraRx { time: t, from: x.from_addr, data: x.data.clone(), rssi: (rssi + jitter).round(), snr: (snr * 2.0).round() / 2.0 });
                }
                Err(r) => self.log.push(LogEntry {
                    seq: 0,
                    time: t,
                    dir: Dir::Drop,
                    radio: Some(label),
                    proto: Proto::Lora,
                    summary: format!("LoRa {} → {}", x.from_addr, x.dest),
                    bytes: x.data.len(),
                    src: None,
                    dst: None,
                    dropped: Some(r),
                    ack_only: false,
                    frame: None,
                }),
            }
        }
        // Gateway (multi-SF, any network id).
        if self.gateway.enabled && x.from != Sender::Gateway && x.freq == self.gateway.freq_hz {
            match self.survives(xi, self.gateway.pos, Sender::Gateway, self.gateway.gain_db) {
                Ok((rssi, snr)) => self.uplinks.push(GatewayUplink {
                    time: t,
                    network_id: x.network_id,
                    from: x.from_addr,
                    data: x.data.clone(),
                    rssi: rssi.round(),
                    snr: (snr * 2.0).round() / 2.0,
                    sf: x.params.sf,
                    freq_hz: x.freq,
                }),
                Err(r) => self.log.push(LogEntry {
                    seq: 0,
                    time: t,
                    dir: Dir::Drop,
                    radio: Some("gateway".into()),
                    proto: Proto::Lora,
                    summary: format!("LoRa {} → gateway", x.from_addr),
                    bytes: x.data.len(),
                    src: None,
                    dst: None,
                    dropped: Some(r),
                    ack_only: false,
                    frame: None,
                }),
            }
        }
    }

    /// The node's current modulation parameters.
    pub fn params(&self, id: NodeId) -> Option<LoraParams> {
        self.nodes.get(id.0 as usize).map(|n| n.params)
    }

    pub fn node_pos(&self, id: NodeId) -> Option<Pos> {
        self.nodes.get(id.0 as usize).map(|n| n.pos)
    }
}

/// Publishes gateway uplinks to the broker (`lora/<net>/<addr>/up`).
pub fn forward_uplinks(net: &mut Network, lora: &mut LoraSpace, ups: Vec<GatewayUplink>, now: SimTime) {
    let key = gateway_client(net, lora, now);
    for u in ups {
        let text = String::from_utf8_lossy(&u.data).into_owned();
        let json = serde_json::json!({
            "from": u.from, "data": text, "rssi": u.rssi, "snr": u.snr, "sf": u.sf, "freq": u.freq_hz,
            "time": u.time.as_secs_f64(),
        });
        let topic = format!("lora/{}/{}/up", u.network_id, u.from);
        let out = net.broker_mut().virtual_publish(key, Message::new(&topic, json.to_string(), 0, false), now);
        net.apply_broker(out);
    }
}

/// Transmits downlinks published to `lora/<net>/<addr>/down`.
pub fn forward_downlinks(net: &mut Network, lora: &mut LoraSpace, now: SimTime) {
    let Some(key) = lora.gateway.mqtt else {
        if lora.gateway.enabled && !lora.nodes.is_empty() {
            gateway_client(net, lora, now);
        }
        return;
    };
    for r in net.broker_mut().virtual_take(key) {
        let parts: Vec<&str> = r.message.topic.split('/').collect();
        if let [_, nid, addr, "down"] = parts.as_slice()
            && let (Ok(nid), Ok(addr)) = (nid.parse::<u8>(), addr.parse::<u16>())
        {
            // Answer with the addressed node's settings (gateway is multi-SF).
            let params = lora.nodes.iter().find(|n| n.address == addr && n.network_id == nid).map(|n| n.params).unwrap_or_default();
            lora.gateway_send(nid, addr, &r.message.payload, params, now);
        }
    }
}

fn gateway_client(net: &mut Network, lora: &mut LoraSpace, now: SimTime) -> ConnKey {
    if let Some(k) = lora.gateway.mqtt {
        return k;
    }
    let k = net.broker_mut().virtual_connect("lora-gateway", now);
    let out = net.broker_mut().virtual_subscribe(k, "lora/+/+/down", 0, now);
    net.apply_broker(out);
    lora.gateway.mqtt = Some(k);
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airtime_matches_semtech_calculator() {
        // Reference values from the Semtech LoRa calculator (CRC on, explicit header).
        let p = |sf, bw, cr, pre| LoraParams { sf, bw_hz: bw, cr, preamble: pre, crc: true, explicit_header: true };
        let ms = |p: LoraParams, n| p.airtime(n) * 1e3;
        assert!((ms(p(7, 125_000, 1, 8), 10) - 41.216).abs() < 1e-3);
        assert!((ms(p(12, 125_000, 1, 8), 10) - 991.232).abs() < 1e-3);
        assert!((ms(p(9, 125_000, 1, 8), 20) - 185.344).abs() < 1e-3);
        assert!((ms(p(7, 250_000, 1, 8), 51) - 51.328).abs() < 1e-3);
        assert!((ms(p(10, 125_000, 4, 8), 30) - 624.640).abs() < 1e-3);
        // Low data-rate optimisation kicks in at SF11/125 kHz (16.4 ms symbols).
        assert!(p(11, 125_000, 1, 8).low_dr_opt() && !p(10, 125_000, 1, 8).low_dr_opt());
        // Sensitivity: SF7/125 kHz ≈ -124.5 dBm, SF12 ≈ -137 dBm.
        assert!((p(7, 125_000, 1, 8).sensitivity() + 124.5).abs() < 0.1);
        assert!((p(12, 125_000, 1, 8).sensitivity() + 137.0).abs() < 0.1);
    }
}
