//! Transient time stepping shared by `.tran` and the co-simulation [`crate::Session`].
//!
//! Follows SPICE3's `DCtran`: variable step with local-truncation-error control on every charge
//! state (trapezoidal, Gear-2 or backward Euler; order drops to 1 at breakpoints), breakpoints
//! collected from sources (and T-line corners), step cut by 8 on Newton failure, and a
//! "timestep too small" diagnostic naming the unknowns/elements that refused to converge.

use crate::devices::{Dev, Device, Init, Mode};
use crate::elab::Method;
use crate::engine::{Engine, NrFail};
use crate::error::SimError;

/// Integration state of a transient run.
#[derive(Debug, Clone)]
pub struct Stepper {
    /// Time of the last accepted point.
    pub time: f64,
    /// Proposed next step.
    pub delta: f64,
    pub tmax: f64,
    pub delmin: f64,
    pub order: usize,
    pub max_order: usize,
    /// Sorted future breakpoints (strictly after `time`).
    bps: Vec<f64>,
    /// Breakpoints have been collected up to this time.
    bp_horizon: f64,
    /// Window used when collecting breakpoints lazily.
    bp_window: f64,
    /// The last accepted point was a breakpoint.
    at_bp: bool,
    save_delta: f64,
    first: bool,
    /// Solution at the last accepted point.
    pub x_acc: Vec<f64>,
    pub accepted: u64,
    pub rejected: u64,
    pub cuts: u64,
    /// Hard stop time (breakpoint) for netlist transients; infinity for sessions.
    pub tstop: f64,
}

impl Stepper {
    /// Prepares stepping from the current engine state at `t0` (operating point or UIC already
    /// solved). `tstep` is the print step, `tmax` the maximum step.
    pub fn new(e: &mut Engine, t0: f64, tstep: f64, tstop: f64, tmax: f64) -> Stepper {
        let ck = &mut e.ck;
        ck.mode = Mode::Tran;
        ck.time = t0;
        // flat history
        for k in 1..4 {
            let (a, b) = ck.st.split_at_mut(k);
            b[0].copy_from_slice(&a[0]);
        }
        ck.delta_old = [tmax; 4];
        let span = if tstop.is_finite() {
            tstop - t0
        } else {
            tmax * 50.0
        };
        let tstep_eff = if tstep > 0.0 { tstep } else { tmax };
        let delta = (span / 100.0).min(tstep_eff) / 10.0;
        let max_order = if ck.method == Method::BackwardEuler {
            1
        } else {
            2
        };
        let x_acc = ck.x.clone();
        let mut s = Stepper {
            time: t0,
            delta,
            tmax,
            delmin: 1e-11 * tmax,
            order: 1,
            max_order,
            bps: Vec::new(),
            bp_horizon: t0,
            bp_window: if tstop.is_finite() {
                tstop - t0
            } else {
                (tmax * 1000.0).max(1e-3)
            },
            at_bp: true,
            save_delta: span / 50.0,
            first: true,
            x_acc,
            accepted: 0,
            rejected: 0,
            cuts: 0,
            tstop,
        };
        s.refill_bps(e);
        s
    }

    fn refill_bps(&mut self, e: &Engine) {
        if self.bp_horizon >= self.tstop {
            return;
        }
        let from = self.bp_horizon;
        let to = (from + self.bp_window.max(8.0 * self.tmax)).min(self.tstop);
        let mut v = Vec::new();
        for d in &e.devices {
            d.breakpoints(from, to, &mut v);
        }
        if to >= self.tstop && self.tstop.is_finite() {
            v.push(self.tstop);
        }
        v.retain(|&t| t > self.time);
        self.bps.extend(v);
        self.bps.sort_by(|a, b| a.total_cmp(b));
        self.bps
            .dedup_by(|a, b| (*a - *b).abs() <= 1e-15 + 1e-12 * a.abs());
        self.bp_horizon = to;
    }

    /// Adds a breakpoint (e.g. a scheduled input change).
    pub fn add_breakpoint(&mut self, t: f64) {
        if t > self.time {
            let i = self.bps.partition_point(|&b| b < t);
            if self.bps.get(i).is_none_or(|&b| (b - t).abs() > 1e-15) {
                self.bps.insert(i, t);
            }
        }
    }

    /// Re-collects breakpoints after device parameters changed.
    pub fn reset_breakpoints(&mut self, e: &Engine) {
        self.bps.clear();
        self.bp_horizon = self.time;
        self.refill_bps(e);
    }

    fn next_bp(&mut self, e: &Engine) -> f64 {
        // keep the collection horizon comfortably ahead of the next step
        if self.bp_horizon < self.tstop && self.time + 4.0 * self.tmax >= self.bp_horizon {
            if self.bp_horizon < self.time {
                self.bp_horizon = self.time;
            }
            self.refill_bps(e);
        }
        let cut = self.time + self.delmin.max(1e-18);
        let past = self.bps.partition_point(|&b| b <= cut);
        if past > 0 {
            self.bps.drain(..past);
        }
        match self.bps.first() {
            Some(&b) => b,
            None if self.tstop.is_finite() => self.tstop,
            None => f64::INFINITY,
        }
    }

    /// Takes one accepted step that does not go beyond `limit`. Returns the new time.
    pub fn step(&mut self, e: &mut Engine, limit: f64) -> Result<f64, SimError> {
        let itl4 = e.ck.opts.itl4.max(3);
        let mut hit_limit = false;
        let mut failed_tries = 0u32;
        let mut resume = false;
        e.ck.switched = false;
        loop {
            let next_bp = self.next_bp(e);
            self.delta = self.delta.min(self.tmax);
            let mut hit_bp = false;
            if self.at_bp {
                self.order = 1;
                let gap = (next_bp - self.time).min(self.save_delta);
                if gap.is_finite() {
                    self.delta = self.delta.min(0.1 * gap);
                }
                if self.first {
                    self.delta /= 10.0;
                }
                self.delta = self.delta.max(self.delmin * 2.0);
            }
            if self.time + self.delta >= next_bp - self.delmin {
                self.save_delta = self.delta;
                self.delta = next_bp - self.time;
                hit_bp = true;
            }
            if self.time + self.delta >= limit - self.delmin.max(1e-18) {
                self.delta = limit - self.time;
                hit_limit = true;
                if next_bp <= limit + self.delmin {
                    hit_bp = true;
                }
            }
            if self.delta <= 0.0 {
                return Ok(self.time);
            }
            {
                let ck = &mut e.ck;
                ck.time = self.time + self.delta;
                ck.delta = self.delta;
                ck.delta_old[0] = self.delta;
                ck.order = self.order;
                ck.compute_coeffs();
                // a retry after Newton ran out of iterations resumes from its last iterate
                // (voltages and device states), with this shorter step's coefficients
                ck.init = if self.first {
                    Init::Tran
                } else if resume {
                    Init::Float
                } else {
                    Init::Pred
                };
                ck.mode = Mode::Tran;
            }
            match e.newton(itl4) {
                Err(f) => {
                    self.rejected += 1;
                    self.cuts += 1;
                    log::trace!(
                        "t = {:.6e}: Newton failed, cutting step {:.3e}",
                        self.time,
                        self.delta
                    );
                    // Like ngspice, a retry that only ran out of iterations starts from the
                    // last iterate: a step across a discontinuity (a switch closing onto an
                    // LED) may need more than ITL4 iterations in total, and restarting from
                    // the accepted point would repeat the same failing ones at every cut.
                    failed_tries += 1;
                    // a switch closing onto a nonlinear load (a 555 output into an LED) can
                    // need more than ITL4 iterations: retries resume from the last iterate.
                    // Everything else retries from the accepted point as before.
                    resume = matches!(f, NrFail::Iterations) && e.ck.switched && failed_tries <= 6;
                    if !resume {
                        e.ck.x.copy_from_slice(&self.x_acc);
                    }
                    let old = self.delta;
                    self.delta /= 8.0;
                    self.order = 1;
                    hit_limit = false;
                    if self.delta <= self.delmin {
                        if old > self.delmin {
                            self.delta = self.delmin;
                        } else {
                            let err = e.nr_error(
                                f,
                                "transient",
                                Some(self.time + old),
                                vec!["timestep reduction".into()],
                            );
                            return Err(match err {
                                SimError::NoConvergence { report, .. } => {
                                    SimError::TimestepTooSmall {
                                        time: self.time,
                                        step: old,
                                        report,
                                    }
                                }
                                other => other,
                            });
                        }
                    }
                    continue;
                }
                Ok(_) => {
                    // truncation error
                    let mut newdelta = 2.0 * self.delta;
                    for d in &e.devices {
                        newdelta = newdelta.min(d.trunc(&e.ck));
                    }
                    if newdelta > 0.9 * self.delta || self.delta <= 2.0 * self.delmin {
                        if self.order == 1 && self.max_order > 1 {
                            e.ck.order = 2;
                            let mut nd2 = 2.0 * self.delta;
                            for d in &e.devices {
                                nd2 = nd2.min(d.trunc(&e.ck));
                            }
                            if nd2 <= 1.05 * self.delta {
                                e.ck.order = 1;
                            } else {
                                self.order = 2;
                                newdelta = nd2;
                            }
                        }
                        // accept
                        self.time = e.ck.time;
                        for d in e.devices.iter_mut() {
                            d.accept(&e.ck);
                        }
                        // transmission lines report wave-front corners as they appear
                        let mut dynamic = Vec::new();
                        for d in &e.devices {
                            if matches!(d, Device::Tline(_)) {
                                d.breakpoints(self.time, f64::INFINITY, &mut dynamic);
                            }
                        }
                        for b in dynamic {
                            self.add_breakpoint(b);
                        }
                        e.rotate_states();
                        let ck = &mut e.ck;
                        ck.delta_old = [
                            self.delta,
                            ck.delta_old[0],
                            ck.delta_old[1],
                            ck.delta_old[2],
                        ];
                        self.x_acc.copy_from_slice(&ck.x);
                        self.accepted += 1;
                        self.first = false;
                        self.delta = newdelta.max(self.delmin * 2.0);
                        self.at_bp = hit_bp;
                        if hit_bp {
                            self.order = 1;
                        }
                        let _ = hit_limit;
                        return Ok(self.time);
                    }
                    // reject
                    self.rejected += 1;
                    resume = false;
                    e.ck.x.copy_from_slice(&self.x_acc);
                    self.delta = newdelta;
                    hit_limit = false;
                    if self.delta <= self.delmin {
                        self.delta = self.delmin * 2.0;
                    }
                }
            }
        }
    }
}

impl Stepper {
    /// After a zero-length re-solve: restart at first order with a small step.
    pub(crate) fn discontinuity_after_resolve(&mut self) {
        self.at_bp = true;
        self.order = 1;
        self.first = false;
        self.delta = (self.delta.min(self.tmax) / 10.0).max(self.delmin * 2.0);
    }
}
