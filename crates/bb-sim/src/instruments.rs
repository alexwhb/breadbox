//! Instruments runtime: live probes (WB-6, value/min/max/avg, duty, frequency), the built-in
//! 2-channel mini scope (PLUG-6: ring buffers, trigger, timebase, measurements), a basic DC
//! meter, and the hooks plugin instruments use (attach to nets, read V/I/f, inject a load).

use crate::bench::NetRt;
use bb_circuit::netlist::BuiltNetlist;
use bb_circuit::project::{Endpoint, InstrumentDef, ProbeDef, Project, Quantity};
use bb_circuit::NetId;
use bb_core::SimTime;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Live value of a probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeReading {
    pub id: String,
    pub label: String,
    pub quantity: Quantity,
    pub value: f64,
    pub unit: String,
    /// Formatted like the design (`2.598 V`, `48 %`, `HIGH`).
    pub text: String,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub net: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProbeRt {
    pub def: ProbeDef,
    pub net: Option<NetId>,
    last_v: f64,
    last_t: SimTime,
    started: Option<SimTime>,
    integral: f64,
    min: f64,
    max: f64,
    // logic level with hysteresis; cumulative high time; recent rising edges (reciprocal
    // counting: frequency and duty over whole periods)
    threshold: f64,
    high: bool,
    high_total: f64,
    rises: VecDeque<(SimTime, f64)>,
}

impl ProbeRt {
    fn new(def: ProbeDef, net: Option<NetId>) -> ProbeRt {
        ProbeRt {
            def,
            net,
            last_v: 0.0,
            last_t: SimTime::ZERO,
            started: None,
            integral: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            threshold: 2.5,
            high: false,
            high_total: 0.0,
            rises: VecDeque::new(),
        }
    }

    fn update(&mut self, t: SimTime, v: f64) {
        if self.started.is_none() {
            self.started = Some(t);
            self.last_t = t;
            self.last_v = v;
            self.high = v > self.threshold;
        }
        let dt = (t - self.last_t).as_secs_f64();
        if dt > 0.0 {
            self.integral += self.last_v * dt;
            if self.high {
                self.high_total += dt;
            }
        }
        self.last_t = t;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        let now_high = if self.high {
            v > self.threshold * 0.9
        } else {
            v > self.threshold * 1.1
        };
        if now_high && !self.high {
            self.rises.push_back((t, self.high_total));
            // keep about one second (and at most 2000 edges)
            while self.rises.len() > 2000
                || (self.rises.len() > 2
                    && (t - self.rises[0].0).as_secs_f64() > 1.0
                    && (t - self.rises[1].0).as_secs_f64() > 1.0)
            {
                self.rises.pop_front();
            }
        }
        self.high = now_high;
        self.last_v = v;
    }

    /// (frequency, duty) over the recorded whole periods.
    fn freq_duty(&self, now: SimTime) -> (f64, f64) {
        if self.rises.len() >= 2 {
            let (t0, h0) = self.rises[0];
            let (t1, h1) = self.rises[self.rises.len() - 1];
            let span = (t1 - t0).as_secs_f64();
            let period = span / (self.rises.len() - 1) as f64;
            // a signal that stopped toggling has no frequency
            if span > 0.0 && (now - t1).as_secs_f64() < 2.0 * period + 0.05 {
                return ((self.rises.len() - 1) as f64 / span, ((h1 - h0) / span).clamp(0.0, 1.0));
            }
        }
        (0.0, if self.high { 1.0 } else { 0.0 })
    }

    fn reading(&self, now: SimTime, nets: &[NetRt]) -> ProbeReading {
        let span = self
            .started
            .map(|s| (now - s).as_secs_f64())
            .unwrap_or(0.0);
        let tail = (now - self.last_t).as_secs_f64().max(0.0);
        let avg = if span > 0.0 {
            (self.integral + self.last_v * tail) / span
        } else {
            self.last_v
        };
        let (freq, duty) = self.freq_duty(now);
        let (value, unit, text) = match self.def.quantity {
            Quantity::Voltage | Quantity::Current => (
                self.last_v,
                "V".to_string(),
                format!("{:.3} V", self.last_v),
            ),
            Quantity::Duty => (
                duty * 100.0,
                "%".into(),
                format!("{:.0} %", duty * 100.0),
            ),
            Quantity::Frequency => (freq, "Hz".into(), bb_core::si::format(freq, "Hz")),
            Quantity::Logic => (
                if self.high { 1.0 } else { 0.0 },
                String::new(),
                if self.high { "HIGH".into() } else { "LOW".into() },
            ),
        };
        ProbeReading {
            id: self.def.id.clone(),
            label: self.def.label.clone(),
            quantity: self.def.quantity,
            value,
            unit,
            text,
            min: if self.min.is_finite() { self.min } else { 0.0 },
            max: if self.max.is_finite() { self.max } else { 0.0 },
            avg,
            net: self.net.map(|n| nets[n.0].name.clone()),
        }
    }
}

/// Scope trigger settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// 0 = CH1, 1 = CH2.
    pub source: usize,
    pub level: f64,
    pub rising: bool,
    /// `auto` (free-runs without a trigger), `normal`, `single`.
    pub mode: String,
}

impl Default for Trigger {
    fn default() -> Self {
        Trigger {
            source: 0,
            level: 2.5,
            rising: true,
            mode: "auto".into(),
        }
    }
}

/// What the mini scope shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ScopeFrame {
    pub id: String,
    /// Seconds per division (10 divisions across).
    pub timebase: f64,
    /// Volts per division per channel.
    pub vdiv: [f64; 2],
    /// Per channel: (time from the left edge in s, volts) points of a step plot.
    pub traces: Vec<Vec<(f64, f64)>>,
    pub triggered: bool,
    pub frequency: f64,
    /// Duty cycle of CH1 (0…1).
    pub duty: f64,
    pub vpp: f64,
    pub vavg: f64,
    /// Labels like `D9 · 2 V/div · 1 ms/div`.
    pub label: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopeChannel {
    pub endpoint: Endpoint,
    pub net: Option<NetId>,
    pub buf: VecDeque<(SimTime, f64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopeRt {
    pub def: InstrumentDef,
    pub channels: [Option<ScopeChannel>; 2],
    pub timebase: f64,
    pub vdiv: [f64; 2],
    pub trigger: Trigger,
    pub armed: bool,
    pub frozen: Option<ScopeFrame>,
}

const SCOPE_CAP: usize = 200_000;

impl ScopeRt {
    fn push(&mut self, net: NetId, t: SimTime, v: f64) {
        for c in self.channels.iter_mut().flatten() {
            if c.net != Some(net) {
                continue;
            }
            if let Some(&(_, last)) = c.buf.back()
                && (last - v).abs() < 1e-4
            {
                continue;
            }
            c.buf.push_back((t, v));
            if c.buf.len() > SCOPE_CAP {
                c.buf.pop_front();
            }
            // drop history older than 20 screens
            let keep = SimTime::from_secs_f64(self.timebase * 10.0 * 20.0);
            while c.buf.len() > 2 && c.buf.front().is_some_and(|f| t - f.0 > keep) {
                c.buf.pop_front();
            }
        }
    }

    fn value_at(buf: &VecDeque<(SimTime, f64)>, t: SimTime) -> f64 {
        let i = buf.partition_point(|(bt, _)| *bt <= t);
        if i == 0 {
            buf.front().map(|x| x.1).unwrap_or(0.0)
        } else {
            buf[i - 1].1
        }
    }

    fn frame(&mut self, now: SimTime, nets: &[NetRt]) -> ScopeFrame {
        if let Some(f) = &self.frozen {
            return f.clone();
        }
        let window = SimTime::from_secs_f64(self.timebase * 10.0);
        let pre = SimTime::from_secs_f64(self.timebase); // trigger one division in
        let latest_start = now.saturating_sub(window);
        let mut start = latest_start;
        let mut triggered = false;
        if let Some(src) = self.channels.get(self.trigger.source).and_then(|c| c.as_ref()) {
            let buf = &src.buf;
            let lvl = self.trigger.level;
            // search backwards for the last crossing that leaves a full window after it
            for i in (1..buf.len()).rev() {
                let (t1, v1) = buf[i];
                let (_, v0) = buf[i - 1];
                let crossed = if self.trigger.rising {
                    v0 < lvl && v1 >= lvl
                } else {
                    v0 > lvl && v1 <= lvl
                };
                if crossed && t1 + window.saturating_sub(pre) <= now {
                    start = t1.saturating_sub(pre);
                    triggered = true;
                    break;
                }
                if (now - t1).as_secs_f64() > window.as_secs_f64() * 4.0 {
                    break;
                }
            }
        }
        if !triggered && self.trigger.mode == "normal" {
            // keep the previous picture
            start = latest_start;
        }
        let end = start + window;
        let mut traces = Vec::new();
        let mut meas = (0.0, 0.0, 0.0, 0.0);
        for (ci, ch) in self.channels.iter().enumerate() {
            let Some(c) = ch else {
                traces.push(Vec::new());
                continue;
            };
            let mut pts = vec![(0.0, Self::value_at(&c.buf, start))];
            let i0 = c.buf.partition_point(|(t, _)| *t <= start);
            for &(t, v) in c.buf.iter().skip(i0) {
                if t > end {
                    break;
                }
                pts.push(((t - start).as_secs_f64(), v));
            }
            pts.push((window.as_secs_f64(), Self::value_at(&c.buf, end.min(now))));
            if ci == 0 {
                // measurements over the window (step-wise)
                let mut hi_t = 0.0;
                let mut sum = 0.0;
                let (mut vmin, mut vmax) = (f64::INFINITY, f64::NEG_INFINITY);
                let mut rises = Vec::new();
                let mid = 0.5 * (pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max)
                    + pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min));
                for w in pts.windows(2) {
                    let dt = w[1].0 - w[0].0;
                    sum += w[0].1 * dt;
                    if w[0].1 > mid {
                        hi_t += dt;
                    }
                    if w[0].1 <= mid && w[1].1 > mid {
                        rises.push(w[1].0);
                    }
                    vmin = vmin.min(w[0].1);
                    vmax = vmax.max(w[0].1);
                }
                let span = window.as_secs_f64();
                let freq = if rises.len() >= 2 {
                    (rises.len() - 1) as f64 / (rises[rises.len() - 1] - rises[0])
                } else {
                    0.0
                };
                meas = (freq, hi_t / span, vmax - vmin, sum / span);
            }
            traces.push(pts);
        }
        let ch1 = self.channels[0]
            .as_ref()
            .and_then(|c| c.net)
            .map(|n| nets[n.0].name.clone())
            .unwrap_or_default();
        let frame = ScopeFrame {
            id: self.def.id.clone(),
            timebase: self.timebase,
            vdiv: self.vdiv,
            traces,
            triggered,
            frequency: meas.0,
            duty: meas.1,
            vpp: meas.2,
            vavg: meas.3,
            label: format!(
                "{ch1} · {}/div · {}/div",
                bb_core::si::format(self.vdiv[0], "V"),
                bb_core::si::format(self.timebase, "s")
            ),
        };
        if triggered && self.trigger.mode == "single" && self.armed {
            self.armed = false;
            self.frozen = Some(frame.clone());
        }
        frame
    }
}

/// A basic meter: DC volts between two endpoints (the Handheld DMM plugin replaces it).
#[derive(Debug, Clone)]
pub(crate) struct MeterRt {
    pub def: InstrumentDef,
    pub plus: Option<NetId>,
    pub com: Option<NetId>,
}

/// Meter reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeterReading {
    pub id: String,
    pub mode: String,
    pub value: f64,
    /// 6000-count display text (`1.882`).
    pub display: String,
    pub unit: String,
}

/// All instruments of a bench.
#[derive(Debug, Clone, Default)]
pub struct Instruments {
    pub(crate) probes: Vec<ProbeRt>,
    pub(crate) scopes: Vec<ScopeRt>,
    pub(crate) meters: Vec<MeterRt>,
    watched: Vec<NetId>,
}

fn endpoint_net(built: &BuiltNetlist, e: &Endpoint) -> Option<NetId> {
    match e {
        Endpoint::Pin { part, pin } => built.connectivity.net_of(part, pin),
        Endpoint::Free { .. } => None,
    }
}

impl Instruments {
    pub(crate) fn from_project(p: &Project) -> Instruments {
        let mut ins = Instruments::default();
        for pr in &p.probes {
            ins.probes.push(ProbeRt::new(pr.clone(), None));
        }
        for i in &p.instruments {
            ins.add_instrument(i.clone());
        }
        ins
    }

    fn add_instrument(&mut self, i: InstrumentDef) {
        match i.kind.as_str() {
            "mini_scope" => {
                let s = &i.settings;
                let ch = |k: &str| {
                    i.probes.get(k).map(|e| ScopeChannel {
                        endpoint: e.clone(),
                        net: None,
                        buf: VecDeque::new(),
                    })
                };
                let trig: Trigger = s
                    .get("trigger")
                    .and_then(|t| serde_json::from_value(t.clone()).ok())
                    .unwrap_or_default();
                let trig = Trigger {
                    source: if s
                        .get("trigger")
                        .and_then(|t| t.get("source"))
                        .and_then(|x| x.as_str())
                        == Some("ch2")
                    {
                        1
                    } else {
                        trig.source
                    },
                    rising: s
                        .get("trigger")
                        .and_then(|t| t.get("edge"))
                        .and_then(|x| x.as_str())
                        .map(|e| e != "falling")
                        .unwrap_or(trig.rising),
                    ..trig
                };
                let vdiv = s.get("vdiv").and_then(|v| v.as_f64()).unwrap_or(2.0);
                self.scopes.push(ScopeRt {
                    channels: [ch("ch1"), ch("ch2")],
                    timebase: s.get("timebase").and_then(|v| v.as_f64()).unwrap_or(1e-3),
                    vdiv: [vdiv, s.get("vdiv2").and_then(|v| v.as_f64()).unwrap_or(vdiv)],
                    trigger: trig,
                    armed: true,
                    frozen: None,
                    def: i,
                });
            }
            _ => self.meters.push(MeterRt {
                def: i,
                plus: None,
                com: None,
            }),
        }
    }

    /// Resolves endpoints to nets after a (re)build.
    pub(crate) fn bind(&mut self, built: &BuiltNetlist, nets: &[NetRt]) {
        let mut watched = Vec::new();
        for p in self.probes.iter_mut() {
            p.net = endpoint_net(built, &p.def.target);
            if let Some(n) = p.net {
                watched.push(n);
            }
            let _ = nets;
        }
        for s in self.scopes.iter_mut() {
            for c in s.channels.iter_mut().flatten() {
                c.net = endpoint_net(built, &c.endpoint);
                if let Some(n) = c.net {
                    watched.push(n);
                }
            }
        }
        for m in self.meters.iter_mut() {
            m.plus = m.def.probes.get("plus").and_then(|e| endpoint_net(built, e));
            m.com = m.def.probes.get("com").and_then(|e| endpoint_net(built, e));
        }
        watched.sort();
        watched.dedup();
        self.watched = watched;
    }

    pub(crate) fn watched_nets(&self) -> Vec<NetId> {
        self.watched.clone()
    }

    /// A net changed value at `t` (digital nets: every drive change; analog: every sync point).
    pub(crate) fn on_net_change(&mut self, net: NetId, t: SimTime, v: f64) {
        if !self.watched.contains(&net) {
            return;
        }
        for p in self.probes.iter_mut().filter(|p| p.net == Some(net)) {
            p.update(t, v);
        }
        for s in self.scopes.iter_mut() {
            s.push(net, t, v);
        }
    }

    /// Adds a probe (WB-6 probe tool); returns its id.
    pub fn add_probe(&mut self, def: ProbeDef, built: &BuiltNetlist) -> String {
        let id = def.id.clone();
        let net = endpoint_net(built, &def.target);
        self.probes.retain(|p| p.def.id != id);
        self.probes.push(ProbeRt::new(def, net));
        if let Some(n) = net
            && !self.watched.contains(&n)
        {
            self.watched.push(n);
        }
        id
    }

    pub fn remove_probe(&mut self, id: &str) {
        self.probes.retain(|p| p.def.id != id);
    }

    /// Scope controls.
    pub fn set_scope(&mut self, id: &str, timebase: Option<f64>, vdiv: Option<[f64; 2]>, trigger: Option<Trigger>) {
        if let Some(s) = self.scopes.iter_mut().find(|s| s.def.id == id) {
            if let Some(t) = timebase {
                s.timebase = t.max(1e-7);
            }
            if let Some(v) = vdiv {
                s.vdiv = v;
            }
            if let Some(t) = trigger {
                s.trigger = t;
                s.armed = true;
                s.frozen = None;
            }
        }
    }

    pub(crate) fn probe_readings(&self, now: SimTime, nets: &[NetRt]) -> Vec<ProbeReading> {
        self.probes.iter().map(|p| p.reading(now, nets)).collect()
    }

    pub(crate) fn scope_frames(&mut self, now: SimTime, nets: &[NetRt]) -> Vec<ScopeFrame> {
        self.scopes.iter_mut().map(|s| s.frame(now, nets)).collect()
    }

    pub(crate) fn meter_readings(&self, volt: &dyn Fn(NetId) -> f64) -> Vec<MeterReading> {
        self.meters
            .iter()
            .map(|m| {
                let v = m.plus.map(volt).unwrap_or(0.0) - m.com.map(volt).unwrap_or(0.0);
                MeterReading {
                    id: m.def.id.clone(),
                    mode: "dc_v".into(),
                    value: v,
                    display: six_thousand_counts(v),
                    unit: "V".into(),
                }
            })
            .collect()
    }
}

/// Formats like a 6000-count meter on auto-range (600m / 6 / 60 / 600 V).
pub fn six_thousand_counts(v: f64) -> String {
    let a = v.abs();
    if a < 0.6 {
        format!("{:.1}m", v * 1e3)
    } else if a < 6.0 {
        format!("{v:.3}")
    } else if a < 60.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.1}")
    }
}
