//! B sources: arbitrary expressions of node voltages, branch currents, time, temperature and
//! external variables, linearised with exact (automatic) derivatives.

use super::*;
use crate::expr::{Compiled, EvalScratch};

/// Where an expression input comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// `v(a) - v(b)` (node indices; 0 = ground).
    Diff(usize, usize),
    /// Current of a branch, resolved at setup.
    Curr(String),
    Branch(usize),
    Time,
    /// Circuit temperature in °C.
    Temp,
    Hertz,
    /// External variable (index into `Ckt::ext`).
    Ext(usize),
}

#[derive(Debug, Clone)]
pub struct Bsrc {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub is_current: bool,
    pub expr: Compiled,
    pub inputs: Vec<Input>,
    pub br: usize,
    slots: Vec<[Slot; 4]>,
    kcl: [Slot; 4],
    /// Last evaluated value and gradient (used for AC and output).
    last_v: f64,
    last_g: Vec<f64>,
    vals: Vec<f64>,
    scratch: EvalScratch,
    bad: bool,
    nonlinear: bool,
    timed: bool,
    /// Times where the expression switches discontinuously (`time > 1m`, `u(time-2m)`).
    pub time_bps: Vec<f64>,
}

impl Bsrc {
    pub fn new(
        name: String,
        p: usize,
        n: usize,
        is_current: bool,
        expr: Compiled,
        inputs: Vec<Input>,
    ) -> Self {
        let nonlinear = inputs
            .iter()
            .any(|i| matches!(i, Input::Diff(..) | Input::Curr(_) | Input::Branch(_)));
        let timed = inputs.iter().any(|i| matches!(i, Input::Time));
        Bsrc {
            name,
            p,
            n,
            is_current,
            expr,
            inputs,
            br: 0,
            slots: Vec::new(),
            kcl: [Slot::GROUND; 4],
            last_v: 0.0,
            last_g: Vec::new(),
            vals: Vec::new(),
            scratch: EvalScratch::default(),
            bad: false,
            nonlinear,
            timed,
            time_bps: Vec::new(),
        }
    }

    fn gather(&mut self, x: &[f64], time: f64, temp_k: f64, ext: &[f64]) {
        self.vals.clear();
        for i in &self.inputs {
            self.vals.push(match *i {
                Input::Diff(a, b) => x[a] - x[b],
                Input::Branch(c) => x[c],
                Input::Curr(_) => 0.0,
                Input::Time => time,
                Input::Temp => temp_k - KELVIN,
                Input::Hertz => 0.0,
                Input::Ext(k) => ext.get(k).copied().unwrap_or(0.0),
            });
        }
    }
}

impl Dev for Bsrc {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        if !self.is_current {
            self.br = s.alloc(format!("{}#branch", self.name), UKind::Branch);
        }
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        for i in self.inputs.iter_mut() {
            if let Input::Curr(name) = i {
                let b = s.branch_of(name).ok_or_else(|| {
                    SimError::invalid(format!(
                        "`{}`: i({name}) — only currents of V sources, inductors, E/H and voltage B sources can be used in expressions",
                        self.name
                    ))
                })?;
                *i = Input::Branch(b);
            }
        }
        let (p, n) = (self.p, self.n);
        self.slots.clear();
        if self.is_current {
            for i in &self.inputs {
                self.slots.push(match *i {
                    Input::Diff(a, b) => [s.slot(p, a), s.slot(p, b), s.slot(n, a), s.slot(n, b)],
                    Input::Branch(c) => [s.slot(p, c), s.slot(n, c), Slot::GROUND, Slot::GROUND],
                    _ => [Slot::GROUND; 4],
                });
            }
        } else {
            let br = self.br;
            self.kcl = [s.slot(p, br), s.slot(n, br), s.slot(br, p), s.slot(br, n)];
            for i in &self.inputs {
                self.slots.push(match *i {
                    Input::Diff(a, b) => [s.slot(br, a), s.slot(br, b), Slot::GROUND, Slot::GROUND],
                    Input::Branch(c) => [s.slot(br, c), Slot::GROUND, Slot::GROUND, Slot::GROUND],
                    _ => [Slot::GROUND; 4],
                });
            }
        }
        Ok(())
    }
    fn branch(&self) -> Option<usize> {
        (!self.is_current).then_some(self.br)
    }
    fn load(&mut self, ck: &mut Ckt) {
        let time = if ck.mode == Mode::Tran { ck.time } else { 0.0 };
        self.gather(&ck.x, time, ck.temp, &ck.ext);
        let mut g = std::mem::take(&mut self.last_g);
        let mut f = self.expr.eval_grad(&self.vals, &mut g, &mut self.scratch);
        if ck.mode == Mode::Dc && !self.nonlinear {
            // pure function of time/ext: scale with source stepping like an independent source
            f *= ck.srcfact;
        }
        self.bad = !f.is_finite() || g.iter().any(|d| !d.is_finite());
        if self.bad {
            ck.nonconverged();
            f = 0.0;
            g.iter_mut().for_each(|d| *d = 0.0);
        }
        let mut lin = 0.0;
        for (k, inp) in self.inputs.iter().enumerate() {
            if matches!(inp, Input::Diff(..) | Input::Branch(_)) {
                lin += g[k] * self.vals[k];
            }
        }
        let eq = f - lin;
        if self.is_current {
            for (k, inp) in self.inputs.iter().enumerate() {
                let gk = g[k];
                let sl = self.slots[k];
                match inp {
                    Input::Diff(..) => {
                        ck.add(sl[0], gk);
                        ck.add(sl[1], -gk);
                        ck.add(sl[2], -gk);
                        ck.add(sl[3], gk);
                    }
                    Input::Branch(_) => {
                        ck.add(sl[0], gk);
                        ck.add(sl[1], -gk);
                    }
                    _ => {}
                }
            }
            ck.add_rhs(self.p, -eq);
            ck.add_rhs(self.n, eq);
        } else {
            ck.add(self.kcl[0], 1.0);
            ck.add(self.kcl[1], -1.0);
            ck.add(self.kcl[2], 1.0);
            ck.add(self.kcl[3], -1.0);
            for (k, inp) in self.inputs.iter().enumerate() {
                let gk = g[k];
                let sl = self.slots[k];
                match inp {
                    Input::Diff(..) => {
                        ck.add(sl[0], -gk);
                        ck.add(sl[1], gk);
                    }
                    Input::Branch(_) => ck.add(sl[0], -gk),
                    _ => {}
                }
            }
            ck.add_rhs(self.br, eq);
        }
        self.last_v = f;
        self.last_g = g;
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        if !self.is_current {
            ac.add_re(self.kcl[0], 1.0);
            ac.add_re(self.kcl[1], -1.0);
            ac.add_re(self.kcl[2], 1.0);
            ac.add_re(self.kcl[3], -1.0);
        }
        for (k, inp) in self.inputs.iter().enumerate() {
            let gk = self.last_g.get(k).copied().unwrap_or(0.0);
            let sl = self.slots[k];
            match (inp, self.is_current) {
                (Input::Diff(..), true) => {
                    ac.add_re(sl[0], gk);
                    ac.add_re(sl[1], -gk);
                    ac.add_re(sl[2], -gk);
                    ac.add_re(sl[3], gk);
                }
                (Input::Branch(_), true) => {
                    ac.add_re(sl[0], gk);
                    ac.add_re(sl[1], -gk);
                }
                (Input::Diff(..), false) => {
                    ac.add_re(sl[0], -gk);
                    ac.add_re(sl[1], gk);
                }
                (Input::Branch(_), false) => ac.add_re(sl[0], -gk),
                _ => {}
            }
        }
    }
    fn conv_ok(&self, _ck: &Ckt) -> bool {
        !self.bad
    }
    fn is_nonlinear(&self) -> bool {
        self.nonlinear
    }
    fn is_time_varying(&self) -> bool {
        self.timed
    }
    fn breakpoints(&self, t: f64, t_end: f64, out: &mut Vec<f64>) {
        out.extend(
            self.time_bps
                .iter()
                .copied()
                .filter(|&b| b > t && b <= t_end),
        );
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(if self.is_current {
            self.last_v
        } else {
            ck.x[self.br]
        })
    }
}

/// Constant times at which an expression of `time` is discontinuous: comparisons with
/// `time` (`time > 1m`) and steps/ramps of `time - c` (`u(time-2m)`).
pub fn time_breakpoints(e: &crate::expr::Expr) -> Vec<f64> {
    use crate::expr::{BinOp, Expr, Func, RtVar};
    let mut out = Vec::new();
    let time_offset = |x: &Expr| -> Option<f64> {
        match x {
            Expr::Rt(RtVar::Time) => Some(0.0),
            Expr::Binary(BinOp::Sub, a, b) if matches!(**a, Expr::Rt(RtVar::Time)) => b.as_num(),
            Expr::Binary(BinOp::Add, a, b) if matches!(**a, Expr::Rt(RtVar::Time)) => {
                b.as_num().map(|v| -v)
            }
            _ => None,
        }
    };
    e.visit(&mut |n| match n {
        Expr::Binary(
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne,
            a,
            b,
        ) => {
            if let (Some(off), Some(c)) = (time_offset(a), b.as_num()) {
                out.push(c + off);
            } else if let (Some(c), Some(off)) = (a.as_num(), time_offset(b)) {
                out.push(c + off);
            }
        }
        Expr::Call(Func::U | Func::Uramp | Func::Sgn, args) => {
            if let Some(off) = args.first().and_then(time_offset) {
                out.push(off);
            }
        }
        _ => {}
    });
    out.retain(|t| *t > 0.0 && t.is_finite());
    out
}
