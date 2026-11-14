//! S (voltage-controlled) and W (current-controlled) switches with hysteresis.
//!
//! The switch is ON once the control exceeds `vt + vh` (`it + ih`) and OFF below `vt − vh`;
//! inside the band it keeps its previous state. A state change forces another Newton
//! iteration. In transients the step is shortened so the threshold crossing is resolved.

use super::*;

#[derive(Debug, Clone)]
pub enum SwCtrl {
    Volt(usize, usize),
    Curr(String),
    Branch(usize),
}

#[derive(Debug, Clone)]
pub struct Switch {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub ctrl: SwCtrl,
    pub thr: f64,
    pub hyst: f64,
    pub ron: f64,
    pub roff: f64,
    pub init_on: Option<bool>,
    st: usize,
    s: G4,
    g_now: f64,
}

impl Switch {
    pub fn new(
        name: String,
        p: usize,
        n: usize,
        ctrl: SwCtrl,
        params: &ParamMap,
        current: bool,
        gmin: f64,
    ) -> (Self, Vec<String>) {
        let (thr, hyst) = if current {
            (params.or(&["it"], 0.0), params.or(&["ih"], 0.0))
        } else {
            (params.or(&["vt"], 0.0), params.or(&["vh"], 0.0))
        };
        let ron = params.or(&["ron"], 1.0);
        let roff = params.or(&["roff"], 1.0 / gmin.max(1e-15));
        let known: &[&str] = if current {
            &["it", "ih", "ron", "roff"]
        } else {
            &["vt", "vh", "ron", "roff"]
        };
        let warn = params.unknown(known);
        (
            Switch {
                name,
                p,
                n,
                ctrl,
                thr,
                hyst: hyst.abs(),
                ron,
                roff,
                init_on: None,
                st: 0,
                s: G4::default(),
                g_now: 0.0,
            },
            warn,
        )
    }

    fn control(&self, x: &[f64]) -> f64 {
        match self.ctrl {
            SwCtrl::Volt(a, b) => x[a] - x[b],
            SwCtrl::Branch(c) => x[c],
            SwCtrl::Curr(_) => 0.0,
        }
    }

    fn decide(&self, c: f64, prev_on: bool) -> bool {
        if c > self.thr + self.hyst {
            true
        } else if c < self.thr - self.hyst {
            false
        } else if self.hyst == 0.0 {
            c > self.thr
        } else {
            prev_on
        }
    }
}

impl Dev for Switch {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.st = s.states(2);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        if let SwCtrl::Curr(name) = &self.ctrl {
            let b = s.branch_of(name).ok_or_else(|| {
                SimError::invalid(format!(
                    "`{}`: controlling element `{name}` carries no branch current (use a V source)",
                    self.name
                ))
            })?;
            self.ctrl = SwCtrl::Branch(b);
        }
        self.s = s.g4(self.p, self.n);
        Ok(())
    }
    fn load(&mut self, ck: &mut Ckt) {
        let c = self.control(&ck.x);
        let old_on = ck.st[0][self.st] > 0.5;
        let on = match ck.init {
            Init::Jct | Init::Fix => match self.init_on {
                Some(v) => v,
                None => self.decide(c, old_on),
            },
            Init::Tran | Init::Pred => self.decide(c, ck.st[1][self.st] > 0.5),
            Init::Float => {
                // inside the hysteresis band keep this iteration's state (ngspice: "use state0
                // since INITTRAN or INITPRED already called")
                let on = self.decide(c, old_on);
                if on != old_on {
                    ck.nonconverged();
                }
                on
            }
        };
        ck.st[0][self.st] = if on { 1.0 } else { 0.0 };
        ck.st[0][self.st + 1] = c;
        self.g_now = if on { 1.0 / self.ron } else { 1.0 / self.roff };
        self.s.stamp(ck, self.g_now);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        self.s.stamp_ac(ac, C64::new(self.g_now, 0.0));
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        // Shorten the step when the control is heading for a threshold.
        let c0 = ck.st[0][self.st + 1];
        let c1 = ck.st[1][self.st + 1];
        let on = ck.st[0][self.st] > 0.5;
        let target = if on {
            self.thr - self.hyst
        } else {
            self.thr + self.hyst
        };
        let slope = (c0 - c1) / ck.delta;
        if slope == 0.0 {
            return f64::INFINITY;
        }
        let dt = (target - c0) / slope;
        if dt <= 0.0 {
            return f64::INFINITY;
        }
        // Land just before the crossing, then just after it, so the switching edge is sharp.
        if dt > 0.01 * ck.delta {
            dt * 0.995
        } else {
            dt + 0.01 * ck.delta
        }
    }
    fn is_nonlinear(&self) -> bool {
        true
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some((ck.x[self.p] - ck.x[self.n]) * self.g_now)
    }
    fn noise(&self, ck: &Ckt, _f: f64, out: &mut Vec<NoiseSrc>) {
        out.push(NoiseSrc {
            name: self.name.clone(),
            p: self.p,
            n: self.n,
            psd: 4.0 * KB * ck.temp * self.g_now,
        });
    }
}
