//! Handheld DMM (PLUG-5): a 6000-count meter that behaves like the real thing.
//!
//! * The input (10 MΩ ∥ 100 pF), the ohms source, and the 10 A / mA shunts are SPICE elements
//!   of the part, so the meter loads the circuit exactly like a real one (`inject:load`).
//! * Readings integrate element charge/energy over a 100 ms conversion: DC = mean, AC =
//!   AC-coupled true RMS (√(E[v²] − E[v]²)), current = mean shunt current.
//! * Auto-range with a 350 ms settling blank, ±(0.05 % + 1) accuracy (a fixed gain error per
//!   meter + last-digit flicker from noise), HOLD / MIN MAX / RANGE, continuity beep, blown
//!   fuses, frequency counter (reciprocal counting).

use bb_plugin_sdk::prelude::*;

const T_CONV: u64 = 1;
const CONV: f64 = 0.1;
const DISPLAY_EVERY: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Off,
    AcV,
    DcV,
    MilliV,
    Ohm,
    Continuity,
    /// Capacitance (⊣⊢): times the RC charge through the ohms source.
    Cap,
    Amp,
    MilliAmp,
    Hz,
}

/// Dial positions in panel order.
const DIAL: [Mode; 10] = [
    Mode::Off,
    Mode::AcV,
    Mode::DcV,
    Mode::MilliV,
    Mode::Ohm,
    Mode::Continuity,
    Mode::Cap,
    Mode::Amp,
    Mode::MilliAmp,
    Mode::Hz,
];

/// One range: full scale (SI), display scale, decimals, unit.
#[derive(Clone, Copy)]
struct Range {
    full: f64,
    scale: f64,
    decimals: usize,
    unit: &'static str,
    /// Ω ranges: reference resistor of the ohms source.
    rref: f64,
}

const fn r(full: f64, scale: f64, decimals: usize, unit: &'static str, rref: f64) -> Range {
    Range {
        full,
        scale,
        decimals,
        unit,
        rref,
    }
}

const V_RANGES: [Range; 4] = [
    r(0.6, 1e3, 1, "mV", 0.0),
    r(6.0, 1.0, 3, "V", 0.0),
    r(60.0, 1.0, 2, "V", 0.0),
    r(600.0, 1.0, 1, "V", 0.0),
];
const MV_RANGES: [Range; 1] = [r(0.6, 1e3, 1, "mV", 0.0)];
const OHM_RANGES: [Range; 6] = [
    r(600.0, 1.0, 1, "Ω", 1e3),
    r(6e3, 1e-3, 3, "kΩ", 1e4),
    r(60e3, 1e-3, 2, "kΩ", 1e5),
    r(600e3, 1e-3, 1, "kΩ", 1e6),
    r(6e6, 1e-6, 3, "MΩ", 1e7),
    r(60e6, 1e-6, 2, "MΩ", 1e8),
];
const CONT_RANGES: [Range; 1] = [r(600.0, 1.0, 1, "Ω", 1e3)];
/// Capacitance ranges: the ohms source's resistor makes τ ≈ 55 ms at full scale.
const CAP_RANGES: [Range; 6] = [
    r(60e-9, 1e9, 2, "nF", 1e6),
    r(600e-9, 1e9, 1, "nF", 1e5),
    r(6e-6, 1e6, 3, "µF", 1e4),
    r(60e-6, 1e6, 2, "µF", 1e3),
    r(600e-6, 1e6, 1, "µF", 1e2),
    r(6e-3, 1e3, 3, "mF", 1e1),
];
/// Charge voltage of the capacitance test.
const CAP_V: f64 = 1.0;
/// The meter's own input capacitance (part template), subtracted from the reading.
const C_IN: f64 = 100e-12;
const A_RANGES: [Range; 2] = [r(6.0, 1.0, 3, "A", 0.0), r(10.0, 1.0, 2, "A", 0.0)];
const MA_RANGES: [Range; 2] = [r(0.06, 1e3, 2, "mA", 0.0), r(0.6, 1e3, 1, "mA", 0.0)];
const HZ_RANGES: [Range; 1] = [r(1e6, 1.0, 1, "Hz", 0.0)];

fn ranges(m: Mode) -> &'static [Range] {
    match m {
        Mode::AcV | Mode::DcV => &V_RANGES,
        Mode::MilliV => &MV_RANGES,
        Mode::Ohm => &OHM_RANGES,
        Mode::Continuity => &CONT_RANGES,
        Mode::Cap => &CAP_RANGES,
        Mode::Amp => &A_RANGES,
        Mode::MilliAmp => &MA_RANGES,
        Mode::Hz => &HZ_RANGES,
        Mode::Off => &[],
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MinMax {
    Live,
    Max,
    Min,
    Avg,
}

#[derive(Default, Clone, Copy)]
struct Integrals {
    t: f64,
    q_in: f64,
    e_in: f64,
    q_ohm: f64,
    q_a: f64,
    q_ma: f64,
}

struct Dmm {
    vo: usize,
    rin: f64,
    counts: f64,
    autorange_s: f64,
    gain_err: f64,
    noise_counts: f64,
    mode: Mode,
    dial: usize,
    range: usize,
    auto: bool,
    settle_until: f64,
    last: Integrals,
    acc: Vec<f64>,
    shown: Option<f64>,
    text: String,
    hold: bool,
    minmax: MinMax,
    mm: (f64, f64, f64, u32),
    fuse_a: bool,
    fuse_ma: bool,
    // frequency counter
    rises: Vec<f64>,
    vmin: f64,
    vmax: f64,
    beeping: bool,
    alert: bool,
    /// Last conversion's mean and total RMS of V(VΩ − COM).
    v_mean: f64,
    v_rms: f64,
    /// Capacitance test: discharge (since) → charge (from t0) → crossing → value.
    cap_phase: CapPhase,
    cap_value: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum CapPhase {
    Discharge { since: f64 },
    Charge { t0: f64 },
}

fn spec_str(cfg: &Config, key: &str) -> Option<String> {
    Some(cfg.specs.get(key)?.as_str()?.to_string())
}

impl Dmm {
    fn rng(&self) -> &'static [Range] {
        ranges(self.mode)
    }

    fn cur(&self) -> Option<Range> {
        self.rng().get(self.range).copied()
    }

    fn integrals(ctx: &Ctx) -> Integrals {
        Integrals {
            t: ctx.now(),
            q_in: ctx.element_charge("R:in").unwrap_or(0.0),
            e_in: ctx.element_energy("R:in").unwrap_or(0.0),
            q_ohm: ctx.element_charge("R:ohm").unwrap_or(0.0),
            q_a: ctx.element_charge("R:a").unwrap_or(0.0),
            q_ma: ctx.element_charge("R:ma").unwrap_or(0.0),
        }
    }

    /// Ohms source + shunt fuses for the current mode/range.
    fn apply_circuit(&self, ctx: &Ctx) {
        let (v, rref) = match self.mode {
            Mode::Ohm | Mode::Continuity => (1.0, self.cur().map(|r| r.rref).unwrap_or(1e3)),
            // the capacitance test charges / discharges through the range resistor; the
            // source voltage is switched by the test itself (`cap_step`)
            Mode::Cap => (
                match self.cap_phase {
                    CapPhase::Charge { .. } => CAP_V,
                    CapPhase::Discharge { .. } => 0.0,
                },
                self.cur().map(|r| r.rref).unwrap_or(1e4),
            ),
            _ => (0.0, 1e12),
        };
        ctx.set_ext("vohm", v);
        ctx.set_ext("rohm", rref);
        ctx.set_ext("ra", if self.fuse_a { 1e12 } else { 0.03 });
        ctx.set_ext("rma", if self.fuse_ma { 1e12 } else { 1.8 });
    }

    fn set_mode(&mut self, ctx: &Ctx, dial: usize) {
        self.dial = dial.min(DIAL.len() - 1);
        self.mode = DIAL[self.dial];
        self.range = match self.mode {
            Mode::DcV | Mode::AcV => 1,
            _ => 0,
        };
        self.auto = true;
        self.hold = false;
        self.minmax = MinMax::Live;
        self.acc.clear();
        self.shown = None;
        self.settle_until = ctx.now() + self.autorange_s;
        self.rises.clear();
        self.cap_phase = CapPhase::Discharge { since: ctx.now() };
        self.cap_value = None;
        self.apply_circuit(ctx);
        self.last = Dmm::integrals(ctx);
        if self.mode == Mode::Hz {
            ctx.watch(self.vo, 1.0, 1.5);
        }
    }

    fn change_range(&mut self, ctx: &Ctx, to: usize) {
        if to != self.range {
            self.range = to;
            self.acc.clear();
            if !self.hold {
                self.shown = None;
                self.text.clear();
            }
            self.settle_until = ctx.now() + self.autorange_s;
            self.cap_phase = CapPhase::Discharge { since: ctx.now() };
            self.cap_value = None;
            self.apply_circuit(ctx);
        }
    }

    /// Thevenin resistance seen by the capacitor (range resistor ∥ input) and the voltage it
    /// charges towards.
    fn cap_source(&self) -> (f64, f64) {
        let rref = self.cur().map(|r| r.rref).unwrap_or(1e4);
        (
            rref * self.rin / (rref + self.rin),
            CAP_V * self.rin / (rref + self.rin),
        )
    }

    /// One step of the capacitance test (at each conversion): once the capacitor is
    /// discharged, switch the source on and watch for 63.2 % of the final voltage; no
    /// crossing within 8 τ of full scale = over range.
    fn cap_step(&mut self, ctx: &Ctx) {
        let full = self.cur().map(|r| r.full).unwrap_or(6e-6);
        let (r_th, v_inf) = self.cap_source();
        let tau_full = r_th * (full + C_IN);
        let now = ctx.now();
        match self.cap_phase {
            CapPhase::Discharge { since } => {
                let v = ctx.pin_voltage(self.vo).abs();
                if v < 0.002 * v_inf || now - since > 12.0 * tau_full {
                    self.cap_phase = CapPhase::Charge { t0: now };
                    let thr = (1.0 - (-1.0f64).exp()) * v_inf;
                    ctx.watch(self.vo, 0.5 * thr, thr);
                    self.apply_circuit(ctx);
                }
            }
            CapPhase::Charge { t0 } => {
                if now - t0 > 8.0 * tau_full {
                    self.cap_value = Some(f64::INFINITY);
                    self.cap_phase = CapPhase::Discharge { since: now };
                    self.apply_circuit(ctx);
                }
            }
        }
    }

    /// One conversion's value (SI units) from the integrals since the last one.
    fn measure(&mut self, ctx: &Ctx) -> Option<f64> {
        let now = Dmm::integrals(ctx);
        let d = Integrals {
            t: now.t - self.last.t,
            q_in: now.q_in - self.last.q_in,
            e_in: now.e_in - self.last.e_in,
            q_ohm: now.q_ohm - self.last.q_ohm,
            q_a: now.q_a - self.last.q_a,
            q_ma: now.q_ma - self.last.q_ma,
        };
        self.last = now;
        if d.t <= 0.0 {
            return None;
        }
        let vmean = d.q_in / d.t * self.rin;
        self.v_mean = vmean;
        self.v_rms = (d.e_in / d.t * self.rin).max(0.0).sqrt();
        let fuse = |q: f64, limit: f64| (q / d.t).abs() > limit;
        // fuses blow on any range
        if !self.fuse_a && fuse(d.q_a, 11.0) {
            self.fuse_a = true;
            ctx.warn("10 A fuse blown — replace it (panel: Replace fuse)");
            self.apply_circuit(ctx);
        }
        if !self.fuse_ma && fuse(d.q_ma, 0.44) {
            self.fuse_ma = true;
            ctx.warn("440 mA fuse blown — replace it (panel: Replace fuse)");
            self.apply_circuit(ctx);
        }
        // input alert: leads in a current jack while measuring voltage
        self.alert = matches!(self.mode, Mode::DcV | Mode::AcV | Mode::MilliV | Mode::Ohm)
            && ((d.q_a / d.t).abs() > 0.05 || (d.q_ma / d.t).abs() > 0.05);
        Some(match self.mode {
            Mode::Off => return None,
            Mode::DcV | Mode::MilliV => vmean,
            Mode::AcV => {
                let v2 = d.e_in / d.t * self.rin;
                (v2 - vmean * vmean).max(0.0).sqrt()
            }
            Mode::Ohm | Mode::Continuity => {
                let i_src = d.q_ohm / d.t;
                let i_x = i_src - vmean / self.rin;
                if vmean >= 0.98 || i_x <= 1e-12 {
                    f64::INFINITY
                } else {
                    (vmean / i_x).max(0.0)
                }
            }
            Mode::Cap => {
                self.cap_step(ctx);
                return self.cap_value.take();
            }
            Mode::Amp => d.q_a / d.t,
            Mode::MilliAmp => d.q_ma / d.t,
            Mode::Hz => self.frequency(ctx.now()),
        })
    }

    fn frequency(&mut self, now: f64) -> f64 {
        // keep the last ~1 s of rising edges
        self.rises.retain(|t| now - t < 1.0);
        if self.rises.len() >= 2 {
            let n = self.rises.len();
            let span = self.rises[n - 1] - self.rises[0];
            if span > 0.0 && now - self.rises[n - 1] < 2.0 * span / (n - 1) as f64 + 0.05 {
                return (n - 1) as f64 / span;
            }
        }
        0.0
    }

    /// Quantise like the display (with gain error and last-digit noise).
    fn quantise(&self, ctx: &Ctx, v: f64, rg: Range) -> f64 {
        let lsb = 1.0 / (rg.scale * libm_pow10(rg.decimals));
        let noisy = v * (1.0 + self.gain_err) + ctx.gauss() * self.noise_counts * lsb;
        (noisy / lsb).round() * lsb
    }

    fn format(v: f64, rg: Range) -> String {
        if !v.is_finite() {
            return "OL".into();
        }
        let x = v * rg.scale;
        format!("{:.*}", rg.decimals, x)
    }

    fn conversion(&mut self, ctx: &Ctx) {
        let Some(v) = self.measure(ctx) else {
            self.publish(ctx);
            return;
        };
        let Some(rg) = self.cur() else { return };
        // auto-range
        if self.auto && self.mode != Mode::Hz {
            let n = self.rng().len();
            let over = !v.is_finite() || v.abs() > rg.full * 1.0;
            if over && self.range + 1 < n {
                let to = self.range + 1;
                self.change_range(ctx, to);
                self.publish(ctx);
                return;
            }
            if self.range > 0 {
                let lower = self.rng()[self.range - 1];
                if v.is_finite() && v.abs() < lower.full * 0.95 {
                    let to = self.range - 1;
                    self.change_range(ctx, to);
                    self.publish(ctx);
                    return;
                }
            }
        }
        // continuity beep
        if self.mode == Mode::Continuity {
            let short = v.is_finite() && v < 50.0;
            if short {
                ctx.beep(2730.0, CONV);
            }
            self.beeping = short;
        }
        if ctx.now() < self.settle_until {
            if !self.hold {
                self.shown = None;
                self.text.clear();
            }
            self.publish(ctx);
            return;
        }
        self.acc.push(v);
        // a capacitance reading is one whole charge cycle: show each one
        let every = if self.mode == Mode::Cap {
            1
        } else {
            DISPLAY_EVERY
        };
        if self.acc.len() as u32 >= every || self.shown.is_none() {
            let finite: Vec<f64> = self.acc.iter().copied().filter(|x| x.is_finite()).collect();
            let mean = if finite.len() < self.acc.len() {
                f64::INFINITY
            } else {
                finite.iter().sum::<f64>() / finite.len().max(1) as f64
            };
            self.acc.clear();
            let q = if mean.is_finite() {
                let q = self.quantise(ctx, mean, rg);
                let lsb = 1.0 / (rg.scale * libm_pow10(rg.decimals));
                if (q / lsb).abs().round() > self.counts.min(rg.full / lsb + 0.5) {
                    f64::INFINITY
                } else {
                    q
                }
            } else {
                mean
            };
            if q.is_finite() {
                let (mx, mn, sum, k) = self.mm;
                self.mm = if k == 0 {
                    (q, q, q, 1)
                } else {
                    (mx.max(q), mn.min(q), sum + q, k + 1)
                };
            }
            if !self.hold {
                let show = match self.minmax {
                    MinMax::Live => q,
                    MinMax::Max => self.mm.0,
                    MinMax::Min => self.mm.1,
                    MinMax::Avg => self.mm.2 / self.mm.3.max(1) as f64,
                };
                self.shown = Some(show);
                self.text = Dmm::format(show, rg);
            }
        }
        self.publish(ctx);
    }

    fn annunciators(&self) -> Vec<String> {
        let mut a = Vec::new();
        if self.mode == Mode::Off {
            return a;
        }
        a.push(if self.auto { "AUTO" } else { "MANU" }.into());
        match self.mode {
            Mode::AcV => a.push("AC".into()),
            Mode::DcV | Mode::MilliV | Mode::Amp | Mode::MilliAmp => a.push("DC".into()),
            Mode::Continuity => a.push("·)))".into()),
            Mode::Cap => a.push("⊣⊢".into()),
            _ => {}
        }
        if self.hold {
            a.push("HOLD".into());
        }
        match self.minmax {
            MinMax::Max => a.push("MAX".into()),
            MinMax::Min => a.push("MIN".into()),
            MinMax::Avg => a.push("AVG".into()),
            MinMax::Live => {}
        }
        if self.alert {
            a.push("LEAD".into());
        }
        if (self.fuse_a && self.mode == Mode::Amp) || (self.fuse_ma && self.mode == Mode::MilliAmp)
        {
            a.push("FUSE".into());
        }
        a
    }

    fn publish(&self, ctx: &Ctx) {
        let rg = self.cur();
        let unit = rg.map(|r| r.unit).unwrap_or("");
        let text = if self.mode == Mode::Off {
            String::new()
        } else if (self.fuse_a && self.mode == Mode::Amp)
            || (self.fuse_ma && self.mode == Mode::MilliAmp)
        {
            "FUSE".into()
        } else {
            self.text.clone()
        };
        let mut p = PanelState::default();
        p.displays.insert(
            "lcd".into(),
            DisplayState {
                text: text.clone(),
                unit: if self.mode == Mode::Off {
                    String::new()
                } else {
                    unit.into()
                },
                annunciators: self.annunciators(),
                bar: match (self.shown, rg) {
                    (Some(v), Some(r)) if v.is_finite() => Some((v.abs() / r.full).min(1.0) as f32),
                    _ => None,
                },
            },
        );
        p.controls.insert("dial".into(), self.dial as f64);
        p.controls.insert("hold".into(), self.hold as u8 as f64);
        p.leds
            .insert("beep".into(), if self.beeping { 1.0 } else { 0.0 });
        if let Some(r) = rg {
            let full = Dmm::format(r.full * 0.99999, r);
            p.readouts
                .push(("Range".into(), format!("{full} {}", r.unit)));
            if matches!(self.mode, Mode::Ohm | Mode::Continuity | Mode::Cap) {
                p.readouts.push((
                    "Test current".into(),
                    format!("{:.0} µA", 1.0 / r.rref * 1e6),
                ));
            }
        }
        p.readouts.push(("Input".into(), "10 MΩ ∥ 100 pF".into()));
        ctx.set_panel(&p);
        ctx.set_readout(&if text.is_empty() {
            String::from("—")
        } else {
            format!("{text} {unit}")
        });
        let value = self.shown.unwrap_or(f64::NAN);
        ctx.set_readings(&[
            ("value", if value.is_finite() { value } else { f64::NAN }),
            ("overload", (value.is_infinite()) as u8 as f64),
            (
                "settling",
                (self.shown.is_none() && self.mode != Mode::Off && !self.hold) as u8 as f64,
            ),
            ("mode", self.dial as f64),
            ("range_full", rg.map(|r| r.full).unwrap_or(0.0)),
            ("auto", self.auto as u8 as f64),
            ("hold", self.hold as u8 as f64),
            ("fuse_a", self.fuse_a as u8 as f64),
            ("fuse_ma", self.fuse_ma as u8 as f64),
            ("beep", self.beeping as u8 as f64),
            ("v_mean", self.v_mean),
            ("v_rms", self.v_rms),
        ]);
    }
}

fn libm_pow10(n: usize) -> f64 {
    let mut x = 1.0;
    for _ in 0..n {
        x *= 10.0;
    }
    x
}

impl Part for Dmm {
    fn init(cfg: &Config) -> Result<Self, String> {
        let vo = cfg.pin("VO").ok_or("the DMM part has no VO pin")?;
        let counts = cfg.spec_f64("counts").unwrap_or(6000.0);
        let autorange_ms = cfg.spec_f64("autorange_ms").unwrap_or(350.0);
        let accuracy = spec_str(cfg, "dcv_accuracy").unwrap_or_else(|| "0.05% + 1".into());
        let pct: f64 = accuracy
            .split('%')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0.05);
        let start = cfg
            .part
            .as_ref()
            .and_then(|p| p.props.get("mode"))
            .and_then(|m| {
                m.as_str()
                    .or_else(|| m.get("Text").and_then(|t| t.as_str()))
            })
            .and_then(|m| {
                [
                    "off",
                    "ac_v",
                    "dc_v",
                    "mv",
                    "ohm",
                    "continuity",
                    "cap",
                    "a",
                    "ma",
                    "hz",
                ]
                .iter()
                .position(|x| *x == m)
            })
            .unwrap_or(2);
        Ok(Dmm {
            vo,
            rin: 10e6,
            counts,
            autorange_s: autorange_ms / 1000.0,
            // a fixed gain error within ±60 % of the % part of the spec (set in on_start)
            gain_err: pct / 100.0,
            noise_counts: 0.35,
            mode: DIAL[start],
            dial: start,
            range: 1,
            auto: true,
            settle_until: 0.0,
            last: Integrals::default(),
            acc: Vec::new(),
            shown: None,
            text: String::new(),
            hold: false,
            minmax: MinMax::Live,
            mm: (0.0, 0.0, 0.0, 0),
            fuse_a: false,
            fuse_ma: false,
            rises: Vec::new(),
            vmin: f64::INFINITY,
            vmax: f64::NEG_INFINITY,
            beeping: false,
            alert: false,
            v_mean: 0.0,
            v_rms: 0.0,
            cap_phase: CapPhase::Discharge { since: 0.0 },
            cap_value: None,
        })
    }

    fn on_start(&mut self, ctx: &Ctx) {
        self.gain_err *= (ctx.rng() - 0.5) * 1.2;
        for e in ["R:in", "R:ohm", "R:a", "R:ma"] {
            ctx.track_element(e);
        }
        let d = self.dial;
        self.set_mode(ctx, d);
        ctx.wake_in(CONV, T_CONV);
        self.publish(ctx);
    }

    fn on_wake(&mut self, ctx: &Ctx, token: u64) {
        if token == T_CONV {
            self.conversion(ctx);
            if self.mode == Mode::Hz {
                // re-centre the counter's thresholds on the signal
                let v = ctx.pin_voltage(self.vo);
                self.vmin = self.vmin.min(v);
                self.vmax = self.vmax.max(v);
                if self.vmax - self.vmin > 0.2 {
                    let mid = 0.5 * (self.vmin + self.vmax);
                    let h = 0.1 * (self.vmax - self.vmin);
                    ctx.watch(self.vo, mid - h, mid + h);
                }
            }
            ctx.wake_in(CONV, T_CONV);
        }
    }

    fn on_edge(&mut self, ctx: &Ctx, pin: usize, high: bool) {
        if pin == self.vo
            && high
            && self.mode == Mode::Cap
            && let CapPhase::Charge { t0 } = self.cap_phase
        {
            // the crossing of 63.2 % of the final voltage is one time constant
            let (r_th, _) = self.cap_source();
            let c = (ctx.now() - t0) / r_th - C_IN;
            self.cap_value = Some(c.max(0.0));
            self.cap_phase = CapPhase::Discharge { since: ctx.now() };
            self.apply_circuit(ctx);
            return;
        }
        if pin == self.vo && high && self.mode == Mode::Hz {
            self.rises.push(ctx.now());
            if self.rises.len() > 20_000 {
                self.rises.drain(..10_000);
            }
        }
        if pin == self.vo && self.mode == Mode::Hz {
            let v = ctx.pin_voltage(self.vo);
            self.vmin = self.vmin.min(v);
            self.vmax = self.vmax.max(v);
        }
    }

    fn on_input(&mut self, ctx: &Ctx, input: &Input) {
        if let Input::Value { key, value } = input {
            match key.as_str() {
                "dial" => self.set_mode(ctx, *value as usize),
                "hold" if *value > 0.5 => self.hold = !self.hold,
                "minmax" if *value > 0.5 => {
                    self.minmax = match self.minmax {
                        MinMax::Live => {
                            self.mm = (0.0, 0.0, 0.0, 0);
                            MinMax::Max
                        }
                        MinMax::Max => MinMax::Min,
                        MinMax::Min => MinMax::Avg,
                        MinMax::Avg => MinMax::Live,
                    };
                }
                "range" if *value > 0.5 => {
                    let n = self.rng().len().max(1);
                    let to = if self.auto {
                        self.range
                    } else {
                        (self.range + 1) % n
                    };
                    self.auto = false;
                    self.change_range(ctx, to);
                }
                "range_auto" if *value > 0.5 => self.auto = true,
                "replace_fuse" if *value > 0.5 => {
                    self.fuse_a = false;
                    self.fuse_ma = false;
                    self.apply_circuit(ctx);
                }
                _ => {}
            }
            self.publish(ctx);
        }
    }
}

export_part!(Dmm);
