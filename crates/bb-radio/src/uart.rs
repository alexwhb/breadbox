//! Pure UART helpers used to wire UART modules (ESP-01, HM-10, HC-05, RYLR896) to MCU pins.
//!
//! * [`UartDecoder`] turns line-level edges stamped with [`SimTime`] into bytes (any baud and
//!   frame format; framing / parity errors are reported, glitches shorter than half a bit are
//!   ignored).
//! * [`UartEncoder`] paces bytes on a line (one frame after the other, never earlier than
//!   requested) and turns each [`UartFrame`] into the exact level transitions to schedule on a
//!   pin (e.g. with `bb_avr::Avr::schedule_pin_input`).
//!
//! Lines idle high (TTL UART). Times are picoseconds; bit edges are rounded to the nearest
//! picosecond from the exact `1e12 / baud` bit period, so long frames never drift.

use bb_core::SimTime;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
}

/// Frame format + speed of one UART line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UartConfig {
    pub baud: u32,
    /// 5–9 data bits.
    pub data_bits: u8,
    pub parity: Parity,
    /// 1 or 2 stop bits.
    pub stop_bits: u8,
}

impl Default for UartConfig {
    fn default() -> Self {
        UartConfig::new(9600)
    }
}

impl UartConfig {
    /// 8N1 at `baud`.
    pub const fn new(baud: u32) -> Self {
        UartConfig {
            baud,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
        }
    }

    pub fn with_format(mut self, data_bits: u8, parity: Parity, stop_bits: u8) -> Self {
        self.data_bits = data_bits.clamp(5, 9);
        self.parity = parity;
        self.stop_bits = stop_bits.clamp(1, 2);
        self
    }

    /// Bit period in picoseconds (fractional).
    pub fn bit_ps(&self) -> f64 {
        1e12 / self.baud.max(1) as f64
    }

    /// Bits per frame: start + data + parity + stop.
    pub fn frame_bits(&self) -> u32 {
        1 + self.data_bits as u32 + u32::from(self.parity != Parity::None) + self.stop_bits as u32
    }

    /// Duration of one frame.
    pub fn frame_time(&self) -> SimTime {
        SimTime::from_ps((self.bit_ps() * self.frame_bits() as f64).round() as u64)
    }

    /// Time of bit boundary `k` of a frame starting at `start`.
    pub fn bit_edge(&self, start: SimTime, k: u32) -> SimTime {
        SimTime::from_ps(start.as_ps() + (self.bit_ps() * k as f64).round() as u64)
    }

    fn parity_bit(&self, data: u16) -> Option<bool> {
        let ones = (data & ((1u16 << self.data_bits) - 1)).count_ones();
        match self.parity {
            Parity::None => None,
            Parity::Even => Some(ones % 2 == 1),
            Parity::Odd => Some(ones % 2 == 0),
        }
    }

    /// Line levels of every bit of a frame (start, data LSB first, parity, stop).
    pub fn frame_levels(&self, data: u16) -> Vec<bool> {
        let mut v = Vec::with_capacity(self.frame_bits() as usize);
        v.push(false);
        for b in 0..self.data_bits {
            v.push(data >> b & 1 != 0);
        }
        if let Some(p) = self.parity_bit(data) {
            v.push(p);
        }
        for _ in 0..self.stop_bits {
            v.push(true);
        }
        v
    }
}

/// One frame scheduled on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UartFrame {
    /// Leading edge of the start bit.
    pub start: SimTime,
    pub data: u16,
    pub config: UartConfig,
}

impl UartFrame {
    /// End of the last stop bit.
    pub fn end(&self) -> SimTime {
        self.config.bit_edge(self.start, self.config.frame_bits())
    }

    /// Level transitions of this frame: `(time, level)` pairs, starting with the falling edge
    /// of the start bit and ending with the line back high (idle). Consecutive equal bits
    /// produce no transition.
    pub fn transitions(&self) -> Vec<(SimTime, bool)> {
        let mut out = Vec::new();
        let mut level = true;
        for (k, &l) in self.config.frame_levels(self.data).iter().enumerate() {
            if l != level {
                out.push((self.config.bit_edge(self.start, k as u32), l));
                level = l;
            }
        }
        out
    }
}

/// Paces bytes onto a TX line: each frame starts at `max(ready_at, end of previous frame)`.
#[derive(Debug, Clone, PartialEq)]
pub struct UartEncoder {
    config: UartConfig,
    free_at: SimTime,
    /// Extra idle time inserted between frames (0 = back-to-back, like a hardware UART).
    pub inter_frame_gap: SimTime,
}

impl UartEncoder {
    pub fn new(config: UartConfig) -> Self {
        UartEncoder {
            config,
            free_at: SimTime::ZERO,
            inter_frame_gap: SimTime::ZERO,
        }
    }

    pub fn config(&self) -> UartConfig {
        self.config
    }

    /// Changes speed/format for frames pushed from now on (frames already scheduled keep
    /// theirs, like a real UART switching after its shift register drains).
    pub fn set_config(&mut self, config: UartConfig) {
        self.config = config;
    }

    /// When the line becomes idle after the last scheduled frame.
    pub fn idle_at(&self) -> SimTime {
        self.free_at
    }

    /// Schedules one frame; returns it.
    pub fn push(&mut self, data: u16, ready_at: SimTime) -> UartFrame {
        let start = ready_at.max(self.free_at);
        let f = UartFrame {
            start,
            data,
            config: self.config,
        };
        self.free_at = f.end() + self.inter_frame_gap;
        f
    }

    /// Schedules `bytes` back to back from `ready_at`.
    pub fn push_bytes(&mut self, bytes: &[u8], ready_at: SimTime) -> Vec<UartFrame> {
        bytes
            .iter()
            .map(|&b| self.push(b as u16, ready_at))
            .collect()
    }

    /// Convenience: schedules `bytes` and returns every level transition in time order.
    pub fn encode(&mut self, bytes: &[u8], ready_at: SimTime) -> Vec<(SimTime, bool)> {
        self.push_bytes(bytes, ready_at)
            .iter()
            .flat_map(|f| f.transitions())
            .collect()
    }
}

/// A received frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxByte {
    /// Leading edge of the start bit.
    pub start: SimTime,
    /// Sample point of the (last) stop bit: the instant a hardware UART would raise RXC.
    pub time: SimTime,
    pub data: u16,
    /// A stop bit was sampled low (includes BREAK conditions).
    pub framing_error: bool,
    pub parity_error: bool,
}

impl RxByte {
    pub fn ok(&self) -> bool {
        !self.framing_error && !self.parity_error
    }
}

/// Decodes a UART line from its edges (16× oversampling equivalent: mid-bit sampling from the
/// start edge, start bit re-checked at its centre to reject glitches).
#[derive(Debug, Clone, PartialEq)]
pub struct UartDecoder {
    config: UartConfig,
    /// Current line level.
    level: bool,
    /// Pending edges not yet consumed by a frame decision, in time order.
    edges: Vec<(SimTime, bool)>,
    /// Level before `edges[0]`.
    base_level: bool,
    /// Earliest time the next start bit may begin (end of the previous frame's last sample).
    ready_from: SimTime,
    out: Vec<RxByte>,
    /// Counters for the UI / tests.
    pub frames: u64,
    pub errors: u64,
}

impl UartDecoder {
    pub fn new(config: UartConfig) -> Self {
        UartDecoder {
            config,
            level: true,
            edges: Vec::new(),
            base_level: true,
            ready_from: SimTime::ZERO,
            out: Vec::new(),
            frames: 0,
            errors: 0,
        }
    }

    pub fn config(&self) -> UartConfig {
        self.config
    }

    /// Changes speed/format (takes effect for the next start bit).
    pub fn set_config(&mut self, config: UartConfig) {
        self.config = config;
    }

    /// Current line level.
    pub fn level(&self) -> bool {
        self.level
    }

    /// Feeds a line level at time `t` (edges must arrive in time order; repeated levels are
    /// ignored).
    pub fn edge(&mut self, t: SimTime, level: bool) {
        if level == self.level {
            return;
        }
        // Frames whose last sample lies before this edge are fully determined.
        self.decide(t, false);
        self.edges.push((t, level));
        self.level = level;
    }

    /// Declares that the line holds its current level up to `now`; completes frames whose
    /// last sample point is at or before `now`. Returns (and keeps) nothing — use
    /// [`UartDecoder::take`].
    pub fn advance(&mut self, now: SimTime) {
        self.decide(now, true);
    }

    /// Advances to `now` and returns all completed bytes.
    pub fn poll(&mut self, now: SimTime) -> Vec<RxByte> {
        self.advance(now);
        self.take()
    }

    /// Completed bytes since the last call.
    pub fn take(&mut self) -> Vec<RxByte> {
        std::mem::take(&mut self.out)
    }

    /// True while a frame is being received (start bit seen, not yet complete).
    pub fn busy(&self) -> bool {
        self.find_start().is_some()
    }

    /// When the frame in progress will be decided (its last stop-bit sample point). A
    /// co-simulator stops the transmitting MCU there so the byte reaches the module at its
    /// exact time and the module's answer is never scheduled in the MCU's past.
    pub fn next_decision(&self) -> Option<SimTime> {
        let i = self.find_start()?;
        let start = self.edges[i].0;
        let cfg = self.config;
        let k = cfg.frame_bits() - 1;
        Some(SimTime::from_ps((start.as_ps() as f64 + cfg.bit_ps() / 2.0 + cfg.bit_ps() * k as f64).round() as u64))
    }

    fn level_at(&self, t: SimTime) -> bool {
        let i = self.edges.partition_point(|&(et, _)| et <= t);
        if i == 0 {
            self.base_level
        } else {
            self.edges[i - 1].1
        }
    }

    /// Index of the first falling edge at/after `ready_from`.
    fn find_start(&self) -> Option<usize> {
        let mut prev = self.base_level;
        for (i, &(t, l)) in self.edges.iter().enumerate() {
            if prev && !l && t >= self.ready_from {
                return Some(i);
            }
            prev = l;
        }
        None
    }

    fn drop_edges_before(&mut self, t: SimTime) {
        let i = self.edges.partition_point(|&(et, _)| et <= t);
        if i > 0 {
            self.base_level = self.edges[i - 1].1;
            self.edges.drain(..i);
        }
    }

    /// Decides every frame whose last sample point is `< limit` (or `<= limit` if `inclusive`).
    fn decide(&mut self, limit: SimTime, inclusive: bool) {
        let cfg = self.config;
        let half = cfg.bit_ps() / 2.0;
        while let Some(i) = self.find_start() {
            let start = self.edges[i].0;
            let sample = |k: u32| SimTime::from_ps((start.as_ps() as f64 + half + cfg.bit_ps() * k as f64).round() as u64);
            let last = sample(cfg.frame_bits() - 1);
            let decided = if inclusive { last <= limit } else { last < limit };
            // A glitch (start bit high again at its centre) can be rejected early.
            let centre = sample(0);
            let centre_known = if inclusive { centre <= limit } else { centre < limit };
            if centre_known && self.level_at(centre) {
                self.drop_edges_before(start);
                // Skip this falling edge: require the next one.
                self.ready_from = SimTime::from_ps(start.as_ps() + 1);
                continue;
            }
            if !decided {
                break;
            }
            let mut data = 0u16;
            let mut ones = 0;
            for b in 0..cfg.data_bits as u32 {
                if self.level_at(sample(1 + b)) {
                    data |= 1 << b;
                    ones += 1;
                }
            }
            let mut k = 1 + cfg.data_bits as u32;
            let parity_error = match cfg.parity {
                Parity::None => false,
                p => {
                    let bit = self.level_at(sample(k)) as u32;
                    k += 1;
                    let total = ones + bit;
                    match p {
                        Parity::Even => total % 2 != 0,
                        _ => total % 2 != 1,
                    }
                }
            };
            let mut framing_error = false;
            for s in 0..cfg.stop_bits as u32 {
                framing_error |= !self.level_at(sample(k + s));
            }
            self.frames += 1;
            if framing_error || parity_error {
                self.errors += 1;
            }
            self.out.push(RxByte {
                start,
                time: last,
                data,
                framing_error,
                parity_error,
            });
            self.ready_from = last;
            self.drop_edges_before(last);
        }
        // Without a pending start bit, edges older than `limit` carry no information.
        if self.find_start().is_none() && !self.edges.is_empty() {
            self.base_level = self.level;
            self.edges.clear();
        }
    }
}

/// Decodes a complete edge list in one go (tests, logic-analyser style decoding).
pub fn decode_edges(edges: &[(SimTime, bool)], config: UartConfig, end: SimTime) -> Vec<RxByte> {
    let mut d = UartDecoder::new(config);
    for &(t, l) in edges {
        d.edge(t, l);
    }
    d.poll(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Pcg32;

    fn roundtrip(cfg: UartConfig, data: &[u16], jitter_frac: f64, baud_err: f64, seed: u64) -> Vec<RxByte> {
        let mut enc_cfg = cfg;
        enc_cfg.baud = (cfg.baud as f64 * (1.0 + baud_err)).round() as u32;
        let mut enc = UartEncoder::new(enc_cfg);
        let mut rng = Pcg32::new(seed);
        let mut edges = Vec::new();
        let mut t = SimTime::from_us(100);
        for &d in data {
            let f = enc.push(d, t);
            for (et, l) in f.transitions() {
                let j = rng.range_f64(-jitter_frac, jitter_frac) * cfg.bit_ps();
                edges.push((SimTime::from_ps((et.as_ps() as f64 + j).max(0.0) as u64), l));
            }
            t = f.end();
        }
        let mut dec = UartDecoder::new(cfg);
        let mut out = Vec::new();
        // Feed edges interleaved with polls, like a co-simulator would.
        for (i, &(et, l)) in edges.iter().enumerate() {
            dec.edge(et, l);
            if i % 3 == 0 {
                out.extend(dec.poll(et));
            }
        }
        out.extend(dec.poll(t + SimTime::from_ms(5)));
        out
    }

    #[test]
    fn roundtrip_9600_and_115200_with_jitter() {
        for baud in [9600, 115_200, 74_880, 38_400] {
            let cfg = UartConfig::new(baud);
            let data: Vec<u16> = (0..=255).collect();
            let rx = roundtrip(cfg, &data, 0.08, 0.0, baud as u64);
            let got: Vec<u16> = rx.iter().map(|r| r.data).collect();
            assert_eq!(got, data, "baud {baud}");
            assert!(rx.iter().all(|r| r.ok()));
        }
    }

    #[test]
    fn tolerates_baud_mismatch_and_formats() {
        let data: Vec<u16> = b"Hello, AT+CWJAP\r\n".iter().map(|&b| b as u16).collect();
        let rx = roundtrip(UartConfig::new(9600), &data, 0.02, 0.025, 1);
        assert_eq!(rx.iter().map(|r| r.data).collect::<Vec<_>>(), data);
        for (bits, par, stop) in [(7, Parity::Even, 2), (8, Parity::Odd, 1), (9, Parity::None, 1), (5, Parity::Even, 1)] {
            let cfg = UartConfig::new(19_200).with_format(bits, par, stop);
            let mask = (1u16 << bits) - 1;
            let data: Vec<u16> = (0..40u16).map(|i| (i * 37) & mask).collect();
            let rx = roundtrip(cfg, &data, 0.05, 0.0, 3);
            assert_eq!(rx.iter().map(|r| r.data).collect::<Vec<_>>(), data);
            assert!(rx.iter().all(|r| r.ok()), "{bits}{par:?}{stop}");
        }
    }

    #[test]
    fn framing_and_parity_errors() {
        let cfg = UartConfig::new(9600).with_format(8, Parity::Even, 1);
        let f = UartFrame { start: SimTime::from_us(10), data: 0x41, config: cfg };
        // Corrupt: flip the parity bit and hold the stop bit low.
        let mut levels = cfg.frame_levels(0x41);
        let n = levels.len();
        levels[n - 2] = !levels[n - 2];
        levels[n - 1] = false;
        let mut edges = Vec::new();
        let mut lvl = true;
        for (k, &l) in levels.iter().enumerate() {
            if l != lvl {
                edges.push((cfg.bit_edge(f.start, k as u32), l));
                lvl = l;
            }
        }
        edges.push((cfg.bit_edge(f.start, n as u32 + 2), true));
        let rx = decode_edges(&edges, cfg, SimTime::from_ms(10));
        assert_eq!(rx.len(), 1);
        assert_eq!(rx[0].data, 0x41);
        assert!(rx[0].parity_error && rx[0].framing_error);
    }

    #[test]
    fn rejects_glitches_and_baud_mismatch_gives_garbage() {
        let cfg = UartConfig::new(9600);
        // 2 µs low pulse: not a start bit.
        let edges = vec![(SimTime::from_us(50), false), (SimTime::from_us(52), true)];
        assert!(decode_edges(&edges, cfg, SimTime::from_ms(5)).is_empty());
        // ESP8266 boot text at 74880 baud seen by a 9600-baud receiver is garbage, not text.
        let mut enc = UartEncoder::new(UartConfig::new(74_880));
        let edges = enc.encode(b"ets Jan  8 2013,rst cause:2, boot mode:(3,6)\r\n", SimTime::from_us(10));
        let rx = decode_edges(&edges, cfg, SimTime::from_ms(20));
        let text: Vec<u8> = rx.iter().map(|r| r.data as u8).collect();
        assert!(!rx.is_empty());
        assert!(!String::from_utf8_lossy(&text).contains("ets"));
    }

    #[test]
    fn encoder_paces_frames_and_transitions_are_exact() {
        let mut enc = UartEncoder::new(UartConfig::new(9600));
        let a = enc.push(b'U' as u16, SimTime::from_us(0));
        let b = enc.push(b'U' as u16, SimTime::from_us(0));
        assert_eq!(b.start, a.end());
        assert_eq!(a.end().as_ps(), 1_041_666_667);
        // 'U' = 0x55: alternating bits → 10 transitions (start..bit7) then idle.
        let tr = a.transitions();
        assert_eq!(tr.len(), 10);
        assert_eq!(tr[0], (SimTime::ZERO, false));
        assert_eq!(tr[1].0.as_ps(), 104_166_667);
        // A late ready time leaves a gap.
        let c = enc.push(0, SimTime::from_ms(50));
        assert_eq!(c.start, SimTime::from_ms(50));
        let mut d = UartDecoder::new(UartConfig::new(9600));
        for (t, l) in c.transitions() {
            d.edge(t, l);
        }
        assert!(d.busy());
        assert!(d.poll(c.start + SimTime::from_us(500)).is_empty());
        let rx = d.poll(c.end());
        assert_eq!(rx.len(), 1);
        assert_eq!(rx[0].data, 0);
        assert!(rx[0].time < c.end());
    }
}
