//! Running a netlist's analyses: `.step` / multi-`.temp` loops, `.mc`, `.meas`.

use crate::analysis::{self, SweepTarget};
use crate::devices::{Dev, Device};
use crate::elab::{self, Circuit, ElabOptions, ElementKind, McCtx, McRng};
use crate::engine::Engine;
use crate::error::{Diagnostic, SimError};
use crate::netlist::{Analysis, MeasSpec, Netlist, SweepKind, SweepVar};
use crate::results::{McResults, MeasResult, Plot, PlotKind, Results, Run, SolveStats};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Cooperative cancellation for long runs (Pro mode "Run analyses" in a background thread).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Options for [`Netlist::run_with`].
#[derive(Clone, Default)]
pub struct RunOptions {
    pub cancel: Option<CancelToken>,
    /// Progress callback (0.0 – 1.0).
    pub progress: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    /// Skip the `.mc` runs.
    pub skip_mc: bool,
    /// Only run these analyses (by keyword: `op`, `dc`, `tran`, `ac`, `noise`, `tf`); empty = all.
    pub only: Vec<String>,
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("skip_mc", &self.skip_mc)
            .field("only", &self.only)
            .finish()
    }
}

#[derive(Default)]
struct Acc {
    newton: u64,
    steps: u64,
    rejected: u64,
    warnings: u64,
    diags: Vec<Diagnostic>,
}

impl Acc {
    fn absorb(&mut self, e: &Engine) {
        self.newton += e.counters.newton;
        self.warnings += e.counters.conv_aids;
        for w in &e.warnings {
            if !self.diags.contains(w) {
                self.diags.push(w.clone());
            }
        }
    }
}

impl Netlist {
    /// Elaborates with nominal parameters.
    pub fn elaborate(&self) -> Result<Circuit, SimError> {
        elab::elaborate(self, ElabOptions::default())
    }

    /// Runs every analysis in the netlist (with `.step`, multi-value `.temp`, `.mc`, `.meas`).
    ///
    /// ```
    /// use bb_spice::{Netlist, PlotKind};
    /// let nl = Netlist::parse("divider\nV1 in 0 5\nR1 in out 1k\nR2 out 0 1k\n.op\n").unwrap();
    /// let r = nl.run().unwrap();
    /// assert!((r.op("v(out)").unwrap() - 2.5).abs() < 1e-9);
    /// ```
    pub fn run(&self) -> Result<Results, SimError> {
        self.run_with(&RunOptions::default())
    }

    /// Runs a single analysis at nominal parameters (ignores `.step`/`.mc`).
    pub fn run_analysis(&self, a: &Analysis) -> Result<Vec<Plot>, SimError> {
        let c = self.elaborate()?;
        let mut acc = Acc::default();
        run_one(
            self,
            &c,
            a,
            &mut acc,
            &RunOptions::default(),
            &ElabOptions::default(),
        )
    }

    /// Runs with options (cancellation, progress, subset of analyses).
    pub fn run_with(&self, opts: &RunOptions) -> Result<Results, SimError> {
        let start = Instant::now();
        let mut acc = Acc::default();
        let mut results = Results {
            title: self.title.clone(),
            ..Default::default()
        };
        // step combinations (first .step = outer loop), multi .temp as an extra outer loop
        let mut dims: Vec<(SweepVar, Vec<f64>)> = Vec::new();
        if self.temps.len() > 1 {
            dims.push((SweepVar::Temp, self.temps.clone()));
        }
        for s in &self.steps {
            dims.push((s.var.clone(), s.values.clone()));
        }
        let combos = cartesian(&dims);
        let total = combos.len().max(1)
            + self
                .mc
                .as_ref()
                .map(|m| if opts.skip_mc { 0 } else { m.runs })
                .unwrap_or(0);
        let mut done = 0usize;
        for combo in &combos {
            check_cancel(opts)?;
            let eo = self.elab_options(combo)?;
            let c = elab::elaborate(self, eo.clone())?;
            for w in &c.warnings {
                if !acc.diags.contains(w) {
                    acc.diags.push(w.clone());
                }
            }
            let mut run = Run {
                label: label(combo),
                params: combo.iter().map(|(v, x)| (v.label(), *x)).collect(),
                ..Default::default()
            };
            for (a, _) in &self.analyses {
                if !opts.only.is_empty() && !opts.only.iter().any(|o| o == a.keyword()) {
                    continue;
                }
                let plots = run_one(self, &c, a, &mut acc, opts, &eo)?;
                run.plots.extend(plots);
            }
            run.measures = self.measures_for(&run.plots);
            results.runs.push(run);
            done += 1;
            progress(opts, done, total);
        }
        if let Some(mc) = &self.mc {
            if !opts.skip_mc {
                let mut mcr = McResults {
                    runs: Vec::new(),
                    seed: mc.seed,
                };
                let wanted: Vec<String> = if mc.analyses.is_empty() {
                    vec!["op".into()]
                } else {
                    mc.analyses.clone()
                };
                for k in 0..mc.runs {
                    check_cancel(opts)?;
                    let seed = mc
                        .seed
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        .wrapping_add(k as u64 + 1);
                    let eo = ElabOptions {
                        mc: Some(McCtx {
                            rng: McRng::new(seed),
                            tol: mc.tol,
                            dist: mc.dist,
                        }),
                        ..Default::default()
                    };
                    let c = elab::elaborate(self, eo.clone())?;
                    let mut run = Run {
                        label: format!("mc run {}", k + 1),
                        ..Default::default()
                    };
                    for w in &wanted {
                        let a = if w == "op" {
                            Some(Analysis::Op)
                        } else {
                            self.analyses
                                .iter()
                                .map(|(a, _)| a)
                                .find(|a| a.keyword() == w)
                                .cloned()
                        };
                        let Some(a) = a else {
                            return Err(SimError::elab(
                                &mc.span,
                                format!(".mc analysis={w}: no such analysis in the netlist"),
                            ));
                        };
                        run.plots
                            .extend(run_one(self, &c, &a, &mut acc, opts, &eo)?);
                    }
                    run.measures = self.measures_for(&run.plots);
                    mcr.runs.push(run);
                    done += 1;
                    progress(opts, done, total);
                }
                results.mc = Some(mcr);
            }
        }
        results.stats = SolveStats {
            elapsed: start.elapsed(),
            newton_iterations: acc.newton,
            timesteps: acc.steps,
            rejected_timesteps: acc.rejected,
            convergence_warnings: acc.warnings,
            runs: combos.len(),
        };
        results.warnings = acc.diags;
        Ok(results)
    }

    fn elab_options(&self, combo: &[(SweepVar, f64)]) -> Result<ElabOptions, SimError> {
        let mut eo = ElabOptions::default();
        for (var, v) in combo {
            match var {
                SweepVar::Temp => eo.temp = Some(*v),
                SweepVar::Param(p) => {
                    eo.params.insert(p.clone(), *v);
                }
                SweepVar::Name(n) => {
                    if self.top.elements.iter().any(|e| &e.name == n) {
                        eo.values.insert(n.clone(), *v);
                    } else {
                        eo.params.insert(n.clone(), *v);
                    }
                }
            }
        }
        Ok(eo)
    }

    fn measures_for(&self, plots: &[Plot]) -> Vec<MeasResult> {
        let mut out: Vec<MeasResult> = Vec::new();
        let params = crate::netlist::top_params(self);
        for m in &self.measures {
            out.push(MeasResult {
                name: m.name.clone(),
                value: eval_meas(m, plots, &params, &out),
            });
        }
        out
    }
}

fn eval_meas(
    m: &MeasSpec,
    plots: &[Plot],
    params: &HashMap<String, f64>,
    prev: &[MeasResult],
) -> Result<f64, String> {
    let kind = match m.analysis.as_str() {
        "tran" => PlotKind::Tran,
        "ac" => PlotKind::Ac,
        "dc" => PlotKind::Dc,
        "op" => PlotKind::Op,
        "noise" => PlotKind::Noise,
        _ => PlotKind::Tran,
    };
    let lookup = |n: &str| -> Option<f64> {
        prev.iter()
            .rev()
            .find(|r| r.name == n)
            .and_then(|r| r.value.clone().ok())
            .or_else(|| params.get(n).copied())
    };
    if let crate::meas::MeasKind::Param(e) = &m.kind {
        return e.eval_with(&lookup);
    }
    let plot = plots
        .iter()
        .find(|p| p.kind == kind)
        .ok_or_else(|| format!("no {} results for `.meas {}`", m.analysis, m.name))?;
    crate::meas::measure(plot, &m.kind, &lookup)
}

fn check_cancel(opts: &RunOptions) -> Result<(), SimError> {
    if opts.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
        Err(SimError::Cancelled)
    } else {
        Ok(())
    }
}

fn progress(opts: &RunOptions, done: usize, total: usize) {
    if let Some(p) = &opts.progress {
        p(done as f32 / total.max(1) as f32);
    }
}

fn label(combo: &[(SweepVar, f64)]) -> String {
    combo
        .iter()
        .map(|(v, x)| {
            format!(
                "{}={}",
                v.label(),
                bb_core::si::format_sig(*x, "", 4).replace(' ', "")
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cartesian(dims: &[(SweepVar, Vec<f64>)]) -> Vec<Vec<(SweepVar, f64)>> {
    let mut out: Vec<Vec<(SweepVar, f64)>> = vec![Vec::new()];
    for (var, vals) in dims {
        let mut next = Vec::new();
        for prefix in &out {
            for &v in vals {
                let mut p = prefix.clone();
                p.push((var.clone(), v));
                next.push(p);
            }
        }
        out = next;
    }
    out
}

fn sweep_target(e: &Engine, c: &Circuit, var: &SweepVar) -> Option<(SweepTarget, String)> {
    match var {
        SweepVar::Temp => Some((SweepTarget::Temp, "temp-sweep".into())),
        SweepVar::Param(_) => None,
        SweepVar::Name(n) => {
            let i = e.device_index(n)?;
            match &e.devices[i] {
                Device::Vsrc(_) => Some((SweepTarget::Source(i), "v-sweep".into())),
                Device::Isrc(_) => Some((SweepTarget::Source(i), "i-sweep".into())),
                Device::Resistor(_) => Some((SweepTarget::Param(i, "r"), "res-sweep".into())),
                Device::Capacitor(_) => Some((SweepTarget::Param(i, "c"), "cap-sweep".into())),
                Device::Inductor(_) => Some((SweepTarget::Param(i, "l"), "ind-sweep".into())),
                _ => {
                    let _ = c;
                    None
                }
            }
        }
    }
}

fn run_one(
    nl: &Netlist,
    c: &Circuit,
    a: &Analysis,
    acc: &mut Acc,
    opts: &RunOptions,
    eo: &ElabOptions,
) -> Result<Vec<Plot>, SimError> {
    let title = nl.title.clone();
    let mut e = Engine::new(c)?;
    let cancel = || opts.cancel.as_ref().is_some_and(|c| c.is_cancelled());
    let r = match a {
        Analysis::Op => analysis::op(&mut e, &title).map(|p| vec![p]),
        Analysis::Tran(spec) => analysis::tran(&mut e, &title, spec, &cancel).map(|(p, st)| {
            acc.steps += st.accepted;
            acc.rejected += st.rejected;
            if st.cuts > 0 {
                acc.warnings += 1;
            }
            vec![p]
        }),
        Analysis::Ac(sw) => analysis::ac(&mut e, &title, sw).map(|p| vec![p]),
        Analysis::Noise {
            output,
            input,
            sweep,
            ..
        } => analysis::noise(&mut e, &title, output, input, sweep).map(|(a, b)| vec![a, b]),
        Analysis::Tf { output, input } => {
            analysis::tf(&mut e, &title, output, input).map(|p| vec![p])
        }
        Analysis::Dc(sweeps) => {
            // In-engine targets when possible; parameter sweeps re-elaborate per point.
            let targets: Vec<Option<(SweepTarget, String)>> =
                sweeps.iter().map(|s| sweep_target(&e, c, &s.var)).collect();
            if targets.iter().all(|t| t.is_some()) {
                let list: Vec<(SweepTarget, Vec<f64>, String)> = targets
                    .into_iter()
                    .zip(sweeps)
                    .map(|(t, s)| {
                        let (t, n) = t.unwrap();
                        (t, s.values(), n)
                    })
                    .collect();
                analysis::dc_sweep(&mut e, &title, &list).map(|p| vec![p])
            } else {
                dc_param_sweep(nl, c, sweeps, eo, acc).map(|p| vec![p])
            }
        }
    };
    acc.absorb(&e);
    r
}

/// DC sweep where at least one variable is a `.param`: re-elaborate per point.
fn dc_param_sweep(
    nl: &Netlist,
    c0: &Circuit,
    sweeps: &[crate::netlist::Sweep],
    eo: &ElabOptions,
    acc: &mut Acc,
) -> Result<Plot, SimError> {
    let inner = &sweeps[0];
    let outer = sweeps.get(1);
    let outer_vals: Vec<f64> = outer.map(|s| s.values()).unwrap_or_else(|| vec![f64::NAN]);
    let mut scale = Vec::new();
    let mut data: Vec<Vec<f64>> = Vec::new();
    let mut outputs: Option<analysis::Outputs> = None;
    let mut prev: Option<(Vec<f64>, Vec<f64>)> = None;
    let name_of = |s: &crate::netlist::Sweep| -> String {
        match &s.var {
            SweepVar::Param(p) | SweepVar::Name(p) => p.clone(),
            SweepVar::Temp => "temp-sweep".into(),
        }
    };
    let _ = c0;
    for ov in outer_vals {
        for iv in inner.values() {
            let mut eo2 = eo.clone();
            for (s, v) in [(Some(inner), iv), (outer, ov)] {
                let Some(s) = s else { continue };
                match &s.var {
                    SweepVar::Temp => eo2.temp = Some(v),
                    SweepVar::Param(p) => {
                        eo2.params.insert(p.clone(), v);
                    }
                    SweepVar::Name(n) => {
                        if nl.top.elements.iter().any(|e| &e.name == n) {
                            eo2.values.insert(n.clone(), v);
                        } else {
                            eo2.params.insert(n.clone(), v);
                        }
                    }
                }
            }
            let c = elab::elaborate(nl, eo2)?;
            let mut e = Engine::new(&c)?;
            let first = match &prev {
                Some((x, st)) if x.len() == e.ck.x.len() && st.len() == e.ck.st[0].len() => {
                    e.ck.x.copy_from_slice(x);
                    e.ck.st[0].copy_from_slice(st);
                    false
                }
                _ => true,
            };
            e.reorder = true;
            analysis::dc_point(&mut e, first)?;
            let o = outputs.get_or_insert_with(|| analysis::Outputs::new(&e));
            if data.is_empty() {
                data = vec![Vec::new(); o.names.len()];
            }
            o.sample(&e, &mut data);
            scale.push(iv);
            prev = Some((e.ck.x.clone(), e.ck.st[0].clone()));
            acc.absorb(&e);
        }
    }
    let o = outputs.ok_or_else(|| SimError::invalid("empty DC sweep"))?;
    let q = if matches!(inner.kind, SweepKind::List(_)) {
        crate::results::Quantity::Other
    } else {
        crate::results::Quantity::Other
    };
    Ok(o.into_plot(
        PlotKind::Dc,
        &nl.title,
        Some(crate::results::Vector::real(name_of(inner), q, scale)),
        data,
    ))
}

/// Helper used by the Pro screen: element kinds that can be swept in `.dc`.
pub fn sweepable(c: &Circuit, name: &str) -> bool {
    c.element(name).is_some_and(|e| {
        matches!(
            e.kind,
            ElementKind::Vsource(_) | ElementKind::Isource(_) | ElementKind::Resistor { .. }
        )
    })
}
