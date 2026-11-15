//! 4-channel oscilloscope (PLUG-5).
//!
//! * Acquisition: the host records each enabled channel's node (`bb_capture`) at the
//!   timebase's sample rate; every ~33 ms the model fetches the samples, finds the trigger and
//!   redraws its screen (`bb_set_panel`, needs `ui:panel`).
//! * Inputs load the circuit like the real thing: 1 MΩ ∥ 100 pF (1× incl. cable) or
//!   10 MΩ ∥ 12 pF (10× probe), SPICE elements of the part (`inject:load`).
//! * 10× probe compensation: a mis-adjusted trimmer shows the classic overshoot / rounding on
//!   square waves (first-order probe response, τ ≈ 117 µs).
//! * Coupling DC / AC (≈ 7 Hz high-pass) / GND; edge and pulse-width triggers; auto / normal /
//!   single; measurements; two time cursors; FFT of a channel.

use bb_plugin_sdk::prelude::*;
use std::f64::consts::PI;

const CH: usize = 4;
const TB: [f64; 25] = [
    1e-6, 2e-6, 5e-6, 1e-5, 2e-5, 5e-5, 1e-4, 2e-4, 5e-4, 1e-3, 2e-3, 5e-3, 1e-2, 2e-2, 5e-2, 0.1,
    0.2, 0.5, 1.0, 2.0, 5.0, 1e-7, 2e-7, 5e-7, 10.0,
];
const VDIV: [f64; 13] = [
    1e-3, 2e-3, 5e-3, 1e-2, 2e-2, 5e-2, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0,
];
const POINTS: usize = 500;

#[derive(Clone, Copy, PartialEq)]
enum Coupling {
    Dc,
    Ac,
    Gnd,
}

#[derive(Clone, Copy, PartialEq)]
enum TrigMode {
    Auto,
    Normal,
    Single,
}

#[derive(Clone)]
struct Channel {
    pin: usize,
    on: bool,
    vdiv: f64,
    offset: f64,
    coupling: Coupling,
    x10: bool,
    /// Probe compensation error (−1…1, 0 = adjusted).
    comp: f64,
}

struct Scope {
    ch: Vec<Channel>,
    timebase: f64,
    /// Trigger position from the left (divisions).
    hpos: f64,
    trig_src: usize,
    trig_level: f64,
    trig_rising: bool,
    trig_mode: TrigMode,
    /// Pulse trigger: 0 = edge, else trigger on a positive pulse wider (>0) or narrower (<0)
    /// than |pulse| seconds.
    pulse: f64,
    running: bool,
    armed: bool,
    frozen: bool,
    fft: Option<usize>,
    cursors: [f64; 2],
    last: PanelState,
    triggered: bool,
    meas: Meas,
}

#[derive(Default, Clone, Copy)]
struct Meas {
    freq: f64,
    duty: f64,
    vpp: f64,
    vmax: f64,
    vmin: f64,
    vavg: f64,
    vrms: f64,
    fft_peak: f64,
}

fn idx_of(list: &[f64], v: f64) -> usize {
    let mut best = 0;
    for (i, x) in list.iter().enumerate() {
        if (x / v).ln().abs() < (list[best] / v).ln().abs() {
            best = i;
        }
    }
    best
}

fn si(v: f64, unit: &str) -> String {
    let a = v.abs();
    let (s, p) = if a == 0.0 {
        (1.0, "")
    } else if a < 1e-6 {
        (1e9, "n")
    } else if a < 1e-3 {
        (1e6, "µ")
    } else if a < 1.0 {
        (1e3, "m")
    } else if a < 1e3 {
        (1.0, "")
    } else if a < 1e6 {
        (1e-3, "k")
    } else {
        (1e-6, "M")
    };
    let x = v * s;
    let digits = if x.abs() >= 100.0 {
        1
    } else if x.abs() >= 10.0 {
        2
    } else {
        3
    };
    format!("{:.*} {p}{unit}", digits, x)
}

impl Scope {
    fn sample_period(&self) -> f64 {
        self.timebase * 10.0 / POINTS as f64
    }

    fn apply_inputs(&self, ctx: &Ctx) {
        for (i, c) in self.ch.iter().enumerate() {
            let (r, cap) = if c.x10 {
                (10e6, 12e-12)
            } else {
                (1e6, 100e-12)
            };
            ctx.set_ext(&format!("r{}", i + 1), r);
            ctx.set_ext(&format!("c{}", i + 1), cap);
            ctx.capture(c.pin, if c.on { self.sample_period() } else { 0.0 });
        }
    }

    /// Displayed samples of channel `i` from `from` to `to`: coupling + probe response.
    fn channel_samples(&self, ctx: &Ctx, i: usize, from: f64, to: f64) -> Vec<(f64, f64)> {
        let c = &self.ch[i];
        let raw = ctx.samples(c.pin, from, to, 20_000);
        if c.coupling == Coupling::Gnd {
            return vec![(from, 0.0), (to, 0.0)];
        }
        let mut out = Vec::with_capacity(raw.len());
        let tau_probe = 117e-6;
        let tau_ac = 22e-3;
        let (mut hp_p, mut hp_ac) = (0.0, 0.0);
        let mut prev: Option<(f64, f64)> = None;
        let mut mean = 0.0;
        if c.coupling == Coupling::Ac && !raw.is_empty() {
            mean = raw.iter().map(|s| s.1).sum::<f64>() / raw.len() as f64;
        }
        for &(t, v) in &raw {
            let mut y = v;
            if let Some((tp, vp)) = prev {
                let dt = (t - tp).max(1e-12);
                if c.x10 && c.comp != 0.0 {
                    let a = tau_probe / (tau_probe + dt);
                    hp_p = a * (hp_p + v - vp);
                }
                if c.coupling == Coupling::Ac {
                    let a = tau_ac / (tau_ac + dt);
                    hp_ac = a * (hp_ac + v - vp);
                }
            } else if c.coupling == Coupling::Ac {
                hp_ac = v - mean;
            }
            if c.x10 {
                y += c.comp * hp_p;
            }
            if c.coupling == Coupling::Ac {
                y = hp_ac + if c.x10 { c.comp * hp_p } else { 0.0 };
            }
            prev = Some((t, v));
            out.push((t, y));
        }
        out
    }

    /// Latest trigger time in `s` (time-ordered samples) leaving a full screen after it.
    fn find_trigger(&self, s: &[(f64, f64)], latest: f64) -> Option<f64> {
        let lvl = self.trig_level;
        let mut rise: Option<f64> = None;
        let mut best = None;
        for w in s.windows(2) {
            let ((t0, v0), (t1, v1)) = (w[0], w[1]);
            let up = v0 < lvl && v1 >= lvl;
            let down = v0 > lvl && v1 <= lvl;
            let tc = if (v1 - v0).abs() > 1e-15 {
                t0 + (t1 - t0) * ((lvl - v0) / (v1 - v0)).clamp(0.0, 1.0)
            } else {
                t1
            };
            if self.pulse == 0.0 {
                if ((self.trig_rising && up) || (!self.trig_rising && down)) && tc <= latest {
                    best = Some(tc);
                }
            } else {
                if up {
                    rise = Some(tc);
                }
                if down && let Some(r) = rise {
                    let width = tc - r;
                    let ok = if self.pulse > 0.0 {
                        width > self.pulse
                    } else {
                        width < -self.pulse
                    };
                    if ok && tc <= latest {
                        best = Some(tc);
                    }
                    rise = None;
                }
            }
        }
        best
    }

    fn measure(&self, s: &[(f64, f64)], from: f64, to: f64) -> Meas {
        let mut m = Meas::default();
        if s.len() < 2 {
            return m;
        }
        let (mut vmax, mut vmin) = (f64::NEG_INFINITY, f64::INFINITY);
        let (mut sum, mut sum2, mut span) = (0.0, 0.0, 0.0);
        for w in s.windows(2) {
            let (t0, v0) = w[0];
            let t1 = w[1].0.min(to);
            let t0 = t0.max(from);
            if t1 <= t0 {
                continue;
            }
            let dt = t1 - t0;
            sum += v0 * dt;
            sum2 += v0 * v0 * dt;
            span += dt;
            vmax = vmax.max(v0);
            vmin = vmin.min(v0);
        }
        if span <= 0.0 {
            return m;
        }
        m.vmax = vmax;
        m.vmin = vmin;
        m.vpp = vmax - vmin;
        m.vavg = sum / span;
        m.vrms = (sum2 / span).sqrt();
        // frequency / duty by reciprocal counting around the mid level
        let mid = 0.5 * (vmax + vmin);
        let hyst = 0.1 * m.vpp;
        let mut high = s[0].1 > mid;
        let mut rises = Vec::new();
        let mut hi_time = 0.0;
        let mut last_t = s[0].0;
        for &(t, v) in s {
            if high {
                hi_time += t - last_t;
            }
            last_t = t;
            if !high && v > mid + hyst {
                high = true;
                rises.push(t);
            } else if high && v < mid - hyst {
                high = false;
            }
        }
        if rises.len() >= 2 && m.vpp > 1e-3 {
            let span = rises[rises.len() - 1] - rises[0];
            m.freq = (rises.len() - 1) as f64 / span;
            // duty over the whole periods
            let mut hi = 0.0;
            let mut h = false;
            let mut lt = rises[0];
            for &(t, v) in s
                .iter()
                .filter(|x| x.0 >= rises[0] && x.0 <= rises[rises.len() - 1])
            {
                if h {
                    hi += t - lt;
                }
                lt = t;
                if !h && v > mid + hyst {
                    h = true;
                } else if h && v < mid - hyst {
                    h = false;
                }
            }
            m.duty = (hi / span).clamp(0.0, 1.0);
        }
        let _ = hi_time;
        m
    }

    /// Magnitude spectrum (dB) of uniformly resampled data; returns (points, peak Hz).
    fn fft(&self, s: &[(f64, f64)], from: f64, to: f64) -> (Vec<[f32; 2]>, f64) {
        let n = 512;
        let dt = (to - from) / n as f64;
        let mut x = vec![0.0f64; n];
        let mut j = 0;
        for (k, xk) in x.iter_mut().enumerate() {
            let t = from + k as f64 * dt;
            while j + 1 < s.len() && s[j + 1].0 <= t {
                j += 1;
            }
            let w = 0.5 - 0.5 * (2.0 * PI * k as f64 / (n - 1) as f64).cos();
            *xk = s.get(j).map(|p| p.1).unwrap_or(0.0) * w;
        }
        let mean = x.iter().sum::<f64>() / n as f64;
        let mut re: Vec<f64> = x.iter().map(|v| v - mean).collect();
        let mut im = vec![0.0f64; n];
        fft_in_place(&mut re, &mut im);
        let mags: Vec<f64> = (0..n / 2)
            .map(|b| (re[b] * re[b] + im[b] * im[b]).sqrt() * 2.0 / n as f64)
            .collect();
        let fs = 1.0 / dt;
        let peak = mags
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(b, _)| b as f64 * fs / n as f64)
            .unwrap_or(0.0);
        let pts = mags
            .iter()
            .enumerate()
            .map(|(b, m)| {
                let db = 20.0 * (m.max(1e-9)).log10();
                [
                    (b as f64 / (n / 2) as f64 * 10.0) as f32,
                    ((db + 40.0) / 20.0) as f32,
                ]
            })
            .collect();
        (pts, peak)
    }

    fn frame(&mut self, ctx: &Ctx) {
        if !self.running || self.frozen {
            return;
        }
        let now = ctx.now();
        let window = self.timebase * 10.0;
        let pre = self.hpos * self.timebase;
        let post = window - pre;
        let search_from = (now - window * 4.0).max(0.0);
        let src = self.channel_samples(ctx, self.trig_src, search_from, now);
        let trig = self.find_trigger(&src, now - post);
        self.triggered = trig.is_some();
        let t0 = match (trig, self.trig_mode) {
            (Some(t), _) => t - pre,
            (None, TrigMode::Auto) => (now - window).max(0.0),
            (None, _) => return, // normal / single: keep the last picture
        };
        if self.trig_mode == TrigMode::Single && trig.is_some() && self.armed {
            self.armed = false;
            self.frozen = true;
        }
        let t1 = t0 + window;
        let mut p = PanelState::default();
        let mut screen = ScreenState::default();
        let mut first: Option<Vec<(f64, f64)>> = None;
        for i in 0..CH {
            let c = self.ch[i].clone();
            if !c.on {
                continue;
            }
            let s = self.channel_samples(ctx, i, t0, t1);
            let mut pts: Vec<[f32; 2]> = Vec::with_capacity(POINTS * 2);
            let mut last_y = None;
            for &(t, v) in &s {
                let x = ((t - t0) / self.timebase).clamp(0.0, 10.0) as f32;
                let y = ((v - c.offset) / c.vdiv).clamp(-5.0, 5.0) as f32;
                if let Some(ly) = last_y {
                    pts.push([x, ly]);
                }
                pts.push([x, y]);
                last_y = Some(y);
            }
            if let Some(ly) = last_y {
                pts.push([10.0, ly]);
            }
            // decimate (keeps edges: consecutive duplicates in x are kept)
            if pts.len() > 4000 {
                let step = pts.len() / 2000;
                pts = pts.into_iter().step_by(step.max(1)).collect();
            }
            screen.traces.push(Trace {
                channel: format!("CH{}", i + 1),
                points: pts,
                digital: false,
            });
            if first.is_none() {
                first = Some(s);
            }
        }
        // measurements on the trigger source (else the first channel)
        let msrc = self.channel_samples(ctx, self.trig_src, t0, t1);
        self.meas = self.measure(&msrc, t0, t1);
        if let Some(f) = self.fft {
            let s = self.channel_samples(ctx, f, t0, t1);
            let (pts, peak) = self.fft(&s, t0, t1);
            self.meas.fft_peak = peak;
            screen.traces.push(Trace {
                channel: "FFT".into(),
                points: pts,
                digital: false,
            });
            screen.markers.push(Marker {
                x: (peak / (POINTS as f64 / 2.0 / (t1 - t0) * 2.0) * 10.0).min(10.0) as f32,
                y: 3.5,
                text: format!("peak {}", si(peak, "Hz")),
                channel: "FFT".into(),
            });
        }
        let _ = first;
        screen.cursors = vec![self.cursors[0] as f32, self.cursors[1] as f32];
        screen.markers.push(Marker {
            x: self.hpos as f32,
            y: ((self.trig_level - self.ch[self.trig_src].offset) / self.ch[self.trig_src].vdiv)
                as f32,
            text: "T".into(),
            channel: format!("CH{}", self.trig_src + 1),
        });
        screen.header = format!(
            "CH{} {}/div · {}/div · {}",
            self.trig_src + 1,
            si(self.ch[self.trig_src].vdiv, "V"),
            si(self.timebase, "s"),
            if self.frozen {
                "Stop"
            } else if self.triggered {
                "Trig'd"
            } else {
                "Auto"
            }
        );
        p.screens.insert("screen".into(), screen);
        let m = self.meas;
        p.readouts = vec![
            ("Freq".into(), si(m.freq, "Hz")),
            ("Duty".into(), format!("{:.1} %", m.duty * 100.0)),
            ("Vpp".into(), si(m.vpp, "V")),
            ("Vavg".into(), si(m.vavg, "V")),
            ("Vrms".into(), si(m.vrms, "V")),
        ];
        let dtc = (self.cursors[1] - self.cursors[0]) * self.timebase;
        p.readouts.push(("Δt".into(), si(dtc.abs(), "s")));
        if dtc != 0.0 {
            p.readouts.push(("1/Δt".into(), si(1.0 / dtc.abs(), "Hz")));
        }
        if self.fft.is_some() {
            p.readouts.push(("FFT peak".into(), si(m.fft_peak, "Hz")));
        }
        p.leds
            .insert("trig".into(), if self.triggered { 1.0 } else { 0.0 });
        p.controls.insert("timebase".into(), self.timebase);
        p.controls.insert("trig_level".into(), self.trig_level);
        for (i, c) in self.ch.iter().enumerate() {
            p.controls.insert(format!("vdiv{}", i + 1), c.vdiv);
            p.controls.insert(format!("on{}", i + 1), c.on as u8 as f64);
        }
        self.last = p;
        ctx.set_panel(&self.last);
        ctx.set_readout(&format!(
            "{} · {}",
            si(self.meas.freq, "Hz"),
            si(self.meas.vpp, "Vpp")
        ));
        ctx.set_readings(&[
            ("triggered", self.triggered as u8 as f64),
            ("frequency", m.freq),
            ("duty", m.duty),
            ("vpp", m.vpp),
            ("vmax", m.vmax),
            ("vmin", m.vmin),
            ("vavg", m.vavg),
            ("vrms", m.vrms),
            ("fft_peak", m.fft_peak),
            ("timebase", self.timebase),
            ("frozen", self.frozen as u8 as f64),
            (
                "trace_points",
                self.last
                    .screens
                    .get("screen")
                    .map(|s| s.traces.iter().map(|t| t.points.len()).sum::<usize>())
                    .unwrap_or(0) as f64,
            ),
        ]);
    }
}

/// Iterative radix-2 FFT (n a power of two).
fn fft_in_place(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

impl Part for Scope {
    fn init(cfg: &Config) -> Result<Self, String> {
        let mut ch = Vec::new();
        for i in 0..CH {
            let pin = cfg
                .pin(&format!("CH{}", i + 1))
                .ok_or_else(|| format!("no CH{} pin", i + 1))?;
            ch.push(Channel {
                pin,
                on: i == 0,
                vdiv: 2.0,
                offset: 0.0,
                coupling: Coupling::Dc,
                x10: true,
                comp: 0.0,
            });
        }
        let part = cfg.part.clone().unwrap_or_default();
        let tb = part.prop("timebase").unwrap_or(1e-3);
        Ok(Scope {
            ch,
            timebase: TB[idx_of(&TB, tb)],
            hpos: 5.0,
            trig_src: 0,
            trig_level: part.prop("trigger_level").unwrap_or(2.5),
            trig_rising: true,
            trig_mode: TrigMode::Auto,
            pulse: 0.0,
            running: true,
            armed: true,
            frozen: false,
            fft: None,
            cursors: [2.0, 8.0],
            last: PanelState::default(),
            triggered: false,
            meas: Meas::default(),
        })
    }

    fn on_start(&mut self, ctx: &Ctx) {
        self.apply_inputs(ctx);
        ctx.tick_every(1.0 / 30.0);
    }

    fn on_tick(&mut self, ctx: &Ctx) {
        self.frame(ctx);
    }

    fn on_input(&mut self, ctx: &Ctx, input: &Input) {
        let Input::Value { key, value } = input else {
            return;
        };
        let v = *value;
        let chan = |k: &str, p: &str| -> Option<usize> {
            k.strip_prefix(p)?
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=CH).contains(n))
                .map(|n| n - 1)
        };
        match key.as_str() {
            "timebase" => self.timebase = TB[idx_of(&TB, v.max(1e-7))],
            "hpos" => self.hpos = v.clamp(0.0, 10.0),
            "trig_level" => self.trig_level = v,
            "trig_source" => self.trig_src = (v as usize).min(CH - 1),
            "trig_edge" => self.trig_rising = v < 0.5,
            "trig_mode" => {
                self.trig_mode = match v as i32 {
                    1 => TrigMode::Normal,
                    2 => TrigMode::Single,
                    _ => TrigMode::Auto,
                };
                self.armed = true;
                self.frozen = false;
            }
            "trig_pulse" => self.pulse = v,
            "single" if v > 0.5 => {
                self.trig_mode = TrigMode::Single;
                self.armed = true;
                self.frozen = false;
            }
            "run" if v > 0.5 => {
                self.running = !self.running;
                self.frozen = false;
            }
            "fft" => {
                self.fft = if v >= 1.0 {
                    Some((v as usize - 1).min(CH - 1))
                } else {
                    None
                }
            }
            "cursor1" => self.cursors[0] = v.clamp(0.0, 10.0),
            "cursor2" => self.cursors[1] = v.clamp(0.0, 10.0),
            k => {
                if let Some(i) = chan(k, "vdiv") {
                    self.ch[i].vdiv = VDIV[idx_of(&VDIV, v.abs().max(1e-3))];
                } else if let Some(i) = chan(k, "offset") {
                    self.ch[i].offset = v;
                } else if let Some(i) = chan(k, "on") {
                    self.ch[i].on = v > 0.5;
                } else if let Some(i) = chan(k, "coupling") {
                    self.ch[i].coupling = match v as i32 {
                        1 => Coupling::Ac,
                        2 => Coupling::Gnd,
                        _ => Coupling::Dc,
                    };
                } else if let Some(i) = chan(k, "probe") {
                    self.ch[i].x10 = v > 0.5;
                } else if let Some(i) = chan(k, "comp") {
                    self.ch[i].comp = v.clamp(-1.0, 1.0);
                }
            }
        }
        self.apply_inputs(ctx);
        self.frame(ctx);
    }
}

export_part!(Scope);
