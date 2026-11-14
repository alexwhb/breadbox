//! Independent sources (V, I with all SPICE waveforms) and Breadbox co-simulation ports.

use super::*;
use crate::elab::{Source, Wave};
use bb_core::PinDrive;
use std::f64::consts::PI;

/// Waveform evaluator with SPICE defaults resolved from the transient step/stop times.
#[derive(Debug, Clone)]
pub struct WaveEval {
    pub dc: f64,
    pub wave: Option<Wave>,
    pub tstep: f64,
    pub tstop: f64,
}

impl WaveEval {
    /// Value at time `t`.
    pub fn value(&self, t: f64) -> f64 {
        let Some(w) = &self.wave else { return self.dc };
        let tstep = if self.tstep > 0.0 {
            self.tstep
        } else {
            self.tstop / 50.0
        };
        match *w {
            Wave::Pulse {
                v1,
                v2,
                td,
                tr,
                tf,
                pw,
                per,
                np,
            } => {
                let tr = tr.filter(|&x| x > 0.0).unwrap_or(tstep);
                let tf = tf.filter(|&x| x > 0.0).unwrap_or(tstep);
                let pw = pw.unwrap_or(self.tstop);
                let per = per.filter(|&x| x > 0.0).unwrap_or(self.tstop);
                let mut time = t - td;
                if time > 0.0 && per > 0.0 && time >= per {
                    let k = (time / per).floor();
                    if np.is_none_or(|n| k < n) {
                        time -= k * per;
                    } else {
                        return v1;
                    }
                }
                if time <= 0.0 || time >= tr + pw + tf {
                    v1
                } else if time >= tr && time <= tr + pw {
                    v2
                } else if time < tr {
                    v1 + (v2 - v1) * time / tr
                } else {
                    v2 + (v1 - v2) * (time - (tr + pw)) / tf
                }
            }
            Wave::Sin {
                vo,
                va,
                freq,
                td,
                theta,
                phase,
            } => {
                let f = freq.filter(|&x| x > 0.0).unwrap_or(1.0 / self.tstop);
                let ph = phase * PI / 180.0;
                if t <= td {
                    vo + va * ph.sin()
                } else {
                    let tt = t - td;
                    vo + va * (-tt * theta).exp() * (2.0 * PI * f * tt + ph).sin()
                }
            }
            Wave::Exp {
                v1,
                v2,
                td1,
                tau1,
                td2,
                tau2,
            } => {
                let tau1 = tau1.filter(|&x| x > 0.0).unwrap_or(tstep);
                let td2 = td2.unwrap_or(td1 + tstep);
                let tau2 = tau2.filter(|&x| x > 0.0).unwrap_or(tstep);
                if t <= td1 {
                    v1
                } else if t <= td2 {
                    v1 + (v2 - v1) * (1.0 - (-(t - td1) / tau1).exp())
                } else {
                    v1 + (v2 - v1) * (1.0 - (-(t - td1) / tau1).exp())
                        + (v1 - v2) * (1.0 - (-(t - td2) / tau2).exp())
                }
            }
            Wave::Pwl {
                ref pts,
                repeat,
                delay,
            } => pwl_value(pts, repeat, delay, t),
            Wave::Sffm {
                vo,
                va,
                fc,
                mdi,
                fs,
            } => {
                let fc = fc.filter(|&x| x > 0.0).unwrap_or(1.0 / self.tstop);
                let fs = fs.filter(|&x| x > 0.0).unwrap_or(1.0 / self.tstop);
                vo + va * (2.0 * PI * fc * t + mdi * (2.0 * PI * fs * t).sin()).sin()
            }
            Wave::Am { va, vo, mf, fc, td } => {
                if t <= td {
                    0.0
                } else {
                    let tt = t - td;
                    va * (vo + (2.0 * PI * mf * tt).sin()) * (2.0 * PI * fc * tt).sin()
                }
            }
        }
    }

    /// Value used for the DC operating point: the DC value if given, else the waveform at t=0.
    pub fn dc_value(&self, dc_given: bool) -> f64 {
        if dc_given || self.wave.is_none() {
            self.dc
        } else {
            self.value(0.0)
        }
    }

    /// Breakpoints in `(t, t_end]`.
    pub fn breakpoints(&self, t: f64, t_end: f64, out: &mut Vec<f64>) {
        let Some(w) = &self.wave else { return };
        let tstep = if self.tstep > 0.0 {
            self.tstep
        } else {
            self.tstop / 50.0
        };
        let mut push = |x: f64| {
            if x > t && x <= t_end {
                out.push(x);
            }
        };
        match *w {
            Wave::Pulse {
                td,
                tr,
                tf,
                pw,
                per,
                np,
                ..
            } => {
                let tr = tr.filter(|&x| x > 0.0).unwrap_or(tstep);
                let tf = tf.filter(|&x| x > 0.0).unwrap_or(tstep);
                let pw = pw.unwrap_or(self.tstop);
                let per = per.filter(|&x| x > 0.0).unwrap_or(self.tstop);
                let k0 = if t > td && per > 0.0 {
                    ((t - td) / per).floor().max(0.0)
                } else {
                    0.0
                };
                let mut k = k0;
                loop {
                    if np.is_some_and(|n| k >= n) {
                        break;
                    }
                    let base = td + k * per;
                    if base > t_end {
                        break;
                    }
                    for x in [base, base + tr, base + tr + pw, base + tr + pw + tf] {
                        push(x);
                    }
                    k += 1.0;
                    if per <= 0.0 || k - k0 > 1e6 {
                        break;
                    }
                }
            }
            Wave::Sin { td, .. } => push(td),
            Wave::Exp { td1, td2, .. } => {
                push(td1);
                push(td2.unwrap_or(td1 + tstep));
            }
            Wave::Pwl {
                ref pts,
                repeat,
                delay,
            } => {
                let Some(&(tl, _)) = pts.last() else { return };
                match repeat {
                    None => {
                        for &(tp, _) in pts {
                            push(tp + delay);
                        }
                    }
                    Some(r) => {
                        let period = tl - r;
                        for &(tp, _) in pts {
                            push(tp + delay);
                        }
                        if period > 0.0 {
                            let start = (((t - delay - tl) / period).floor()).max(0.0);
                            let mut k = start;
                            loop {
                                let off = delay + tl + k * period - r;
                                if off + r > t_end {
                                    break;
                                }
                                for &(tp, _) in pts.iter().filter(|p| p.0 > r) {
                                    push(tp + off);
                                }
                                k += 1.0;
                                if k - start > 1e6 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Wave::Am { td, .. } => push(td),
            Wave::Sffm { .. } => {}
        }
    }
}

fn pwl_value(pts: &[(f64, f64)], repeat: Option<f64>, delay: f64, t: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    let mut tt = t - delay;
    let (t0, v0) = pts[0];
    let (tl, vl) = pts[pts.len() - 1];
    if tt <= t0 {
        return v0;
    }
    if tt >= tl {
        match repeat {
            Some(r) if tl > r => {
                let period = tl - r;
                tt = r + (tt - tl) % period;
                // the repeated segment starts from the value at r
                if tt < r {
                    return vl;
                }
            }
            _ => return vl,
        }
    }
    let i = pts.partition_point(|p| p.0 <= tt).clamp(1, pts.len() - 1);
    let (ta, va) = pts[i - 1];
    let (tb, vb) = pts[i];
    if tb <= ta {
        vb
    } else {
        va + (vb - va) * (tt - ta) / (tb - ta)
    }
}

/// Independent voltage source.
#[derive(Debug, Clone)]
pub struct Vsrc {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub w: WaveEval,
    pub dc_given: bool,
    pub ac: C64,
    pub br: usize,
    s: [Slot; 4],
}

impl Vsrc {
    pub fn new(name: String, p: usize, n: usize, src: &Source, dc: f64, dc_given: bool) -> Self {
        Vsrc {
            name,
            p,
            n,
            w: WaveEval {
                dc,
                wave: src.wave.clone(),
                tstep: 0.0,
                tstop: 1.0,
            },
            dc_given,
            ac: C64::from_polar(src.ac_mag, src.ac_phase_deg.to_radians()),
            br: 0,
            s: [Slot::GROUND; 4],
        }
    }
    /// Source value in the current mode.
    pub fn value(&self, ck: &Ckt) -> f64 {
        if ck.mode == Mode::Dc {
            let v = if ck.tran_op {
                self.w.value(0.0)
            } else {
                self.w.dc_value(self.dc_given)
            };
            v * ck.srcfact
        } else {
            self.w.value(ck.time)
        }
    }
}

impl Dev for Vsrc {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.br = s.alloc(format!("{}#branch", self.name), UKind::Branch);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.s = [
            s.slot(self.p, self.br),
            s.slot(self.n, self.br),
            s.slot(self.br, self.p),
            s.slot(self.br, self.n),
        ];
        Ok(())
    }
    fn branch(&self) -> Option<usize> {
        Some(self.br)
    }
    fn load(&mut self, ck: &mut Ckt) {
        ck.add(self.s[0], 1.0);
        ck.add(self.s[1], -1.0);
        ck.add(self.s[2], 1.0);
        ck.add(self.s[3], -1.0);
        let v = self.value(ck);
        ck.add_rhs(self.br, v);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        ac.add_re(self.s[0], 1.0);
        ac.add_re(self.s[1], -1.0);
        ac.add_re(self.s[2], 1.0);
        ac.add_re(self.s[3], -1.0);
    }
    fn ac_excite(&self, rhs: &mut [C64]) {
        rhs[self.br] += self.ac;
    }
    fn breakpoints(&self, t: f64, t_end: f64, out: &mut Vec<f64>) {
        self.w.breakpoints(t, t_end, out);
    }
    fn prepare_tran(&mut self, tstep: f64, tstop: f64) {
        self.w.tstep = tstep;
        self.w.tstop = tstop;
    }
    fn is_time_varying(&self) -> bool {
        self.w.wave.is_some()
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(ck.x[self.br])
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "dc" | "value" => {
                self.w.dc = v;
                self.dc_given = true;
                true
            }
            "acmag" => {
                self.ac = C64::from_polar(v, self.ac.arg());
                true
            }
            _ => false,
        }
    }
    fn set_source_value(&mut self, v: f64) -> bool {
        self.w.dc = v;
        self.dc_given = true;
        true
    }
}

/// Independent current source (current flows from `p` through the source to `n`).
#[derive(Debug, Clone)]
pub struct Isrc {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub w: WaveEval,
    pub dc_given: bool,
    pub ac: C64,
}

impl Isrc {
    pub fn new(name: String, p: usize, n: usize, src: &Source, dc: f64, dc_given: bool) -> Self {
        Isrc {
            name,
            p,
            n,
            w: WaveEval {
                dc,
                wave: src.wave.clone(),
                tstep: 0.0,
                tstop: 1.0,
            },
            dc_given,
            ac: C64::from_polar(src.ac_mag, src.ac_phase_deg.to_radians()),
        }
    }
    fn value(&self, ck: &Ckt) -> f64 {
        if ck.mode == Mode::Dc {
            let v = if ck.tran_op {
                self.w.value(0.0)
            } else {
                self.w.dc_value(self.dc_given)
            };
            v * ck.srcfact
        } else {
            self.w.value(ck.time)
        }
    }
}

impl Dev for Isrc {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_matrix(&mut self, _s: &mut Setup) -> Result<(), SimError> {
        Ok(())
    }
    fn load(&mut self, ck: &mut Ckt) {
        let i = self.value(ck);
        ck.add_rhs(self.p, -i);
        ck.add_rhs(self.n, i);
    }
    fn ac_excite(&self, rhs: &mut [C64]) {
        rhs[self.p] -= self.ac;
        rhs[self.n] += self.ac;
        rhs[0] = C64::default();
    }
    fn breakpoints(&self, t: f64, t_end: f64, out: &mut Vec<f64>) {
        self.w.breakpoints(t, t_end, out);
    }
    fn prepare_tran(&mut self, tstep: f64, tstop: f64) {
        self.w.tstep = tstep;
        self.w.tstop = tstop;
    }
    fn is_time_varying(&self) -> bool {
        self.w.wave.is_some()
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(self.value(ck))
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "dc" | "value" => {
                self.w.dc = v;
                self.dc_given = true;
                true
            }
            _ => false,
        }
    }
    fn set_source_value(&mut self, v: f64) -> bool {
        self.w.dc = v;
        self.dc_given = true;
        true
    }
}

/// Co-simulation port: drives node `p` (relative to `n`) according to a [`PinDrive`].
#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub drive: PinDrive,
    pub leak: f64,
    s: G4,
}

impl Port {
    pub fn new(name: String, p: usize, n: usize, drive: PinDrive, leak: f64) -> Self {
        Port {
            name,
            p,
            n,
            drive,
            leak,
            s: G4::default(),
        }
    }
    /// Norton equivalent: (conductance, current injected into `p`).
    pub fn norton(&self) -> (f64, f64) {
        match self.drive {
            PinDrive::HighZ => (self.leak, 0.0),
            PinDrive::Thevenin { v, r } | PinDrive::Pull { v, r } => {
                let g = 1.0 / r.max(1e-6);
                (g + self.leak, v * g)
            }
            PinDrive::Current { i } => (self.leak, i),
        }
    }
}

impl Dev for Port {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.s = s.g4(self.p, self.n);
        Ok(())
    }
    fn load(&mut self, ck: &mut Ckt) {
        let (g, i) = self.norton();
        self.s.stamp(ck, g);
        let i = i * if ck.mode == Mode::Dc { ck.srcfact } else { 1.0 };
        ck.add_rhs(self.p, i);
        ck.add_rhs(self.n, -i);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        let (g, _) = self.norton();
        self.s.stamp_ac(ac, C64::new(g, 0.0));
    }
    /// Current delivered by the port into node `p` (positive when sourcing).
    fn current(&self, ck: &Ckt) -> Option<f64> {
        let (g, i) = self.norton();
        Some(i - g * (ck.x[self.p] - ck.x[self.n]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(w: Wave, t: f64) -> f64 {
        WaveEval {
            dc: 0.0,
            wave: Some(w),
            tstep: 1e-6,
            tstop: 1e-3,
        }
        .value(t)
    }

    #[test]
    fn pulse_shape() {
        let w = Wave::Pulse {
            v1: 0.0,
            v2: 5.0,
            td: 1e-6,
            tr: Some(1e-6),
            tf: Some(2e-6),
            pw: Some(3e-6),
            per: Some(10e-6),
            np: None,
        };
        assert_eq!(ev(w.clone(), 0.0), 0.0);
        assert!((ev(w.clone(), 1.5e-6) - 2.5).abs() < 1e-9);
        assert_eq!(ev(w.clone(), 3e-6), 5.0);
        assert!((ev(w.clone(), 6e-6) - 2.5).abs() < 1e-9);
        assert_eq!(ev(w.clone(), 8e-6), 0.0);
        assert!((ev(w.clone(), 11.5e-6) - 2.5).abs() < 1e-9);
        let mut bps = Vec::new();
        WaveEval {
            dc: 0.0,
            wave: Some(w),
            tstep: 1e-6,
            tstop: 1e-3,
        }
        .breakpoints(0.0, 20e-6, &mut bps);
        assert!(bps.contains(&2e-6) && bps.contains(&5e-6) && bps.contains(&7e-6));
    }

    #[test]
    fn pwl_repeat() {
        let w = Wave::Pwl {
            pts: vec![(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)],
            repeat: Some(0.0),
            delay: 0.0,
        };
        assert!((ev(w.clone(), 0.5) - 0.5).abs() < 1e-12);
        assert!((ev(w.clone(), 2.5) - 0.5).abs() < 1e-12);
        assert!((ev(w.clone(), 3.5) - 0.5).abs() < 1e-12);
        let w = Wave::Pwl {
            pts: vec![(0.0, 0.0), (1.0, 1.0)],
            repeat: None,
            delay: 1.0,
        };
        assert_eq!(ev(w.clone(), 0.5), 0.0);
        assert_eq!(ev(w, 5.0), 1.0);
    }

    #[test]
    fn sin_exp_sffm_am() {
        let w = Wave::Sin {
            vo: 1.0,
            va: 2.0,
            freq: Some(1e3),
            td: 0.0,
            theta: 0.0,
            phase: 0.0,
        };
        assert!((ev(w, 0.25e-3) - 3.0).abs() < 1e-9);
        let w = Wave::Exp {
            v1: 0.0,
            v2: 1.0,
            td1: 0.0,
            tau1: Some(1e-3),
            td2: Some(1.0),
            tau2: Some(1e-3),
        };
        assert!((ev(w, 1e-3) - (1.0 - (-1.0f64).exp())).abs() < 1e-12);
        let w = Wave::Sffm {
            vo: 0.0,
            va: 1.0,
            fc: Some(1e3),
            mdi: 0.0,
            fs: Some(1.0),
        };
        assert!((ev(w, 0.25e-3) - 1.0).abs() < 1e-9);
        let w = Wave::Am {
            va: 1.0,
            vo: 1.0,
            mf: 0.0,
            fc: 1e3,
            td: 0.0,
        };
        assert!((ev(w, 0.25e-3) - 1.0).abs() < 1e-9);
    }
}
