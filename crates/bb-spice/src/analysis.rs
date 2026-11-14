//! `.op`, `.dc`, `.tran`, `.ac`, `.noise` and `.tf` on an [`Engine`].

use crate::devices::{AcCtx, Dev, Device, Init, Mode, NoiseSrc};
use crate::elab::SimOptions;
use crate::engine::Engine;
use crate::error::SimError;
use crate::matrix::{C64, Matrix};
use crate::netlist::{OutputVar, Sweep, TranSpec};
use crate::results::{Plot, PlotKind, Quantity, Vector};
use crate::tran::Stepper;

/// What to record at each output point.
#[derive(Debug, Clone)]
pub(crate) struct Outputs {
    pub names: Vec<String>,
    pub quantities: Vec<Quantity>,
    src: Vec<Src>,
}

#[derive(Debug, Clone, Copy)]
enum Src {
    X(usize),
    Dev(usize),
}

impl Outputs {
    pub fn new(e: &Engine) -> Outputs {
        let mut o = Outputs {
            names: Vec::new(),
            quantities: Vec::new(),
            src: Vec::new(),
        };
        for (i, name) in e.node_names.iter().enumerate().take(e.n_nodes + 1).skip(1) {
            o.names.push(format!("v({name})"));
            o.quantities.push(Quantity::Voltage);
            o.src.push(Src::X(i));
        }
        for (k, d) in e.devices.iter().enumerate() {
            if let Some(b) = d.branch() {
                o.names.push(format!("i({})", d.name()));
                o.quantities.push(Quantity::Current);
                o.src.push(Src::X(b));
            } else if d.current(&e.ck).is_some() {
                o.names.push(format!("i({})", d.name()));
                o.quantities.push(Quantity::Current);
                o.src.push(Src::Dev(k));
            }
        }
        o
    }

    pub fn sample(&self, e: &Engine, out: &mut [Vec<f64>]) {
        for (k, s) in self.src.iter().enumerate() {
            let v = match *s {
                Src::X(i) => e.ck.x[i],
                Src::Dev(d) => e.devices[d].current(&e.ck).unwrap_or(0.0),
            };
            out[k].push(v);
        }
    }

    pub fn to_plot(
        &self,
        kind: PlotKind,
        title: &str,
        scale: Option<Vector>,
        data: Vec<Vec<f64>>,
    ) -> Plot {
        let mut p = Plot::new(kind, title);
        if let Some(s) = scale {
            p.push_scale(s);
        }
        for ((n, q), d) in self.names.iter().zip(&self.quantities).zip(data) {
            p.push(Vector::real(n.clone(), *q, d));
        }
        p
    }

    /// Unknown index for complex outputs (devices without a branch are skipped in AC).
    fn x_index(&self, k: usize) -> Option<usize> {
        match self.src[k] {
            Src::X(i) => Some(i),
            Src::Dev(_) => None,
        }
    }
}

/// `.op`
pub(crate) fn op(e: &mut Engine, title: &str) -> Result<Plot, SimError> {
    e.ck.tran_op = false;
    e.ck.uic = false;
    e.reorder = true;
    e.solve_op("operating point")?;
    let o = Outputs::new(e);
    let mut data = vec![Vec::new(); o.names.len()];
    o.sample(e, &mut data);
    Ok(o.to_plot(PlotKind::Op, title, None, data))
}

/// How a DC sweep variable is applied to an engine that already exists.
pub(crate) enum SweepTarget {
    /// Independent source (device index).
    Source(usize),
    /// Element parameter (device index, param name).
    Param(usize, &'static str),
    Temp,
}

/// DC sweep over in-engine targets (`inner` fastest). Parameter sweeps that need
/// re-elaboration are handled by the runner, which calls [`dc_point`] per point.
pub(crate) fn dc_sweep(
    e: &mut Engine,
    title: &str,
    sweeps: &[(SweepTarget, Vec<f64>, String)],
) -> Result<Plot, SimError> {
    e.ck.tran_op = false;
    e.ck.uic = false;
    e.reorder = true;
    let o = Outputs::new(e);
    let mut data = vec![Vec::new(); o.names.len()];
    let mut scale = Vec::new();
    let (inner, outer) = (&sweeps[0], sweeps.get(1));
    let outer_vals: Vec<f64> = outer.map(|s| s.1.clone()).unwrap_or_else(|| vec![f64::NAN]);
    let mut first = true;
    for ov in outer_vals {
        if let Some(o) = outer {
            apply_target(e, &o.0, ov);
        }
        for &v in &inner.1 {
            apply_target(e, &inner.0, v);
            dc_point(e, first)?;
            first = false;
            scale.push(v);
            o.sample(e, &mut data);
        }
    }
    let q = match inner.0 {
        SweepTarget::Temp => Quantity::Temperature,
        SweepTarget::Source(i) if matches!(e.devices[i], Device::Isrc(_)) => Quantity::Current,
        SweepTarget::Source(_) => Quantity::Voltage,
        SweepTarget::Param(..) => Quantity::Other,
    };
    Ok(o.to_plot(
        PlotKind::Dc,
        title,
        Some(Vector::real(inner.2.clone(), q, scale)),
        data,
    ))
}

pub(crate) fn apply_target(e: &mut Engine, t: &SweepTarget, v: f64) {
    match *t {
        SweepTarget::Source(i) => {
            e.devices[i].set_source_value(v);
        }
        SweepTarget::Param(i, p) => {
            e.devices[i].set_param(p, v);
        }
        SweepTarget::Temp => e.set_temperature(v),
    }
}

/// Solves one DC point, continuing from the previous solution when possible.
pub(crate) fn dc_point(e: &mut Engine, first: bool) -> Result<(), SimError> {
    e.ck.mode = Mode::Dc;
    if !first {
        let saved = e.ck.x.clone();
        let saved_st = e.ck.st[0].clone();
        e.ck.init = Init::Float;
        e.ck.srcfact = 1.0;
        e.ck.diag_gmin = 0.0;
        let itl = e.ck.opts.itl2.max(e.ck.opts.itl1);
        if e.newton(itl).is_ok() {
            return Ok(());
        }
        e.ck.x.copy_from_slice(&saved);
        e.ck.st[0].copy_from_slice(&saved_st);
    }
    e.solve_op("DC sweep")
}

/// Default maximum step for a `.tran` spec.
pub(crate) fn tran_tmax(spec: &TranSpec, opts: &SimOptions) -> f64 {
    let span = spec.tstop - spec.tstart;
    let default = if spec.tstep > 0.0 {
        spec.tstep.min(span / 50.0)
    } else {
        span / 50.0
    };
    let tmax = spec
        .tmax
        .or(opts.tmax)
        .filter(|t| *t > 0.0)
        .unwrap_or(default);
    tmax.max(f64::MIN_POSITIVE)
}

/// Initial state for a transient: operating point (with `.ic` clamps) or UIC.
pub(crate) fn tran_init(e: &mut Engine, uic: bool) -> Result<(), SimError> {
    e.ck.uic = uic;
    e.reorder = true;
    if uic {
        e.ck.x.iter_mut().for_each(|v| *v = 0.0);
        for &(n, v) in &e.ics.clone() {
            e.ck.x[n] = v;
        }
        // twice so mutual inductors see each other's initial currents
        for _ in 0..2 {
            for d in e.devices.iter_mut() {
                d.init_uic(&mut e.ck);
            }
        }
        e.ck.mode = Mode::Dc;
    } else {
        e.ck.tran_op = true;
        let r = e.solve_op("transient operating point");
        e.ck.tran_op = false;
        r?;
        for d in e.devices.iter_mut() {
            d.accept(&e.ck);
        }
    }
    Ok(())
}

/// `.tran`
pub(crate) fn tran(
    e: &mut Engine,
    title: &str,
    spec: &TranSpec,
    cancel: &dyn Fn() -> bool,
) -> Result<(Plot, Stepper), SimError> {
    let tmax = tran_tmax(spec, &e.ck.opts);
    let tstep = if spec.tstep > 0.0 { spec.tstep } else { tmax };
    for d in e.devices.iter_mut() {
        d.prepare_tran(tstep, spec.tstop);
    }
    tran_init(e, spec.uic)?;
    let o = Outputs::new(e);
    let mut data = vec![Vec::new(); o.names.len()];
    let mut times = Vec::new();
    let mut st = Stepper::new(e, 0.0, tstep, spec.tstop, tmax);
    if spec.uic {
        e.ck.time = 0.0;
    }
    if spec.tstart <= 0.0 {
        times.push(0.0);
        o.sample(e, &mut data);
    }
    if spec.tstart > 0.0 {
        st.add_breakpoint(spec.tstart);
    }
    let mut n = 0u64;
    while st.time < spec.tstop * (1.0 - 1e-12) {
        let t = st.step(e, spec.tstop)?;
        if t >= spec.tstart * (1.0 - 1e-12) {
            times.push(t);
            o.sample(e, &mut data);
        }
        n += 1;
        if n.is_multiple_of(256) && cancel() {
            return Err(SimError::Cancelled);
        }
        if n > 50_000_000 {
            return Err(SimError::invalid("transient: too many time points"));
        }
    }
    let scale = Vector::real("time", Quantity::Time, times);
    Ok((o.to_plot(PlotKind::Tran, title, Some(scale), data), st))
}

/// Small-signal matrix helper.
pub(crate) struct AcSystem {
    pub mat: Matrix<C64>,
    pub rhs: Vec<C64>,
}

impl AcSystem {
    pub fn new(e: &Engine) -> AcSystem {
        AcSystem {
            mat: Matrix::new(&e.pattern),
            rhs: vec![C64::default(); e.ck.n + 1],
        }
    }

    /// Loads the AC matrix at angular frequency `omega` and factors it.
    pub fn load(&mut self, e: &Engine, omega: f64) -> Result<(), SimError> {
        self.mat.clear();
        let mut ac = AcCtx {
            mat: &mut self.mat,
            rhs: &mut self.rhs,
            omega,
            x: &e.ck.x,
            st: &e.ck.st[0],
        };
        for d in &e.devices {
            d.ac_load(&mut ac);
        }
        // gmin on node diagonals keeps floating capacitor nodes solvable
        self.mat.factor().map_err(|s| SimError::Singular {
            detail: format!(
                "AC matrix singular at {} (f = {:.4e} Hz)",
                e.display_name(s.col),
                omega / std::f64::consts::TAU
            ),
        })
    }
}

/// Solves the operating point and records small-signal parameters.
pub(crate) fn small_signal_op(e: &mut Engine) -> Result<(), SimError> {
    e.ck.tran_op = false;
    e.ck.uic = false;
    e.reorder = true;
    e.solve_op("operating point")?;
    // one more load at the converged point so devices hold its small-signal values
    e.ck.init = Init::Float;
    e.load();
    Ok(())
}

/// `.ac`
pub(crate) fn ac(e: &mut Engine, title: &str, sweep: &Sweep) -> Result<Plot, SimError> {
    small_signal_op(e)?;
    let o = Outputs::new(e);
    let freqs = sweep.values();
    let mut sys = AcSystem::new(e);
    let mut data: Vec<Vec<C64>> = vec![Vec::new(); o.names.len()];
    let mut keep = vec![true; o.names.len()];
    for (k, _) in o.names.iter().enumerate() {
        keep[k] = o.x_index(k).is_some();
    }
    for &f in &freqs {
        let omega = std::f64::consts::TAU * f;
        sys.load(e, omega)?;
        sys.rhs.iter_mut().for_each(|v| *v = C64::default());
        for d in &e.devices {
            d.ac_excite(&mut sys.rhs);
        }
        sys.rhs[0] = C64::default();
        sys.mat.solve(&mut sys.rhs);
        for (k, column) in data.iter_mut().enumerate() {
            if let Some(i) = o.x_index(k) {
                column.push(sys.rhs[i]);
            }
        }
    }
    let mut p = Plot::new(PlotKind::Ac, title);
    p.push_scale(Vector::complex(
        "frequency",
        Quantity::Frequency,
        freqs.iter().map(|&f| C64::new(f, 0.0)).collect(),
    ));
    for (k, d) in data.into_iter().enumerate() {
        if keep[k] {
            p.push(Vector::complex(o.names[k].clone(), o.quantities[k], d));
        }
    }
    Ok(p)
}

fn output_unknowns(e: &Engine, out: &OutputVar) -> Result<(usize, usize), SimError> {
    match out {
        OutputVar::Voltage(p, n) => {
            let a = e.node(p).ok_or_else(|| SimError::Unknown {
                kind: "node",
                name: p.clone(),
            })?;
            let b = match n {
                Some(n) => e.node(n).ok_or_else(|| SimError::Unknown {
                    kind: "node",
                    name: n.clone(),
                })?,
                None => 0,
            };
            Ok((a, b))
        }
        OutputVar::Current(s) => {
            let i = e.device_index(s).ok_or_else(|| SimError::Unknown {
                kind: "source",
                name: s.clone(),
            })?;
            let b = e.devices[i]
                .branch()
                .ok_or_else(|| SimError::invalid(format!("i({s}) has no branch current")))?;
            Ok((b, 0))
        }
    }
}

fn input_source(e: &Engine, name: &str) -> Result<usize, SimError> {
    let i = e.device_index(name).ok_or_else(|| SimError::Unknown {
        kind: "input source",
        name: name.to_string(),
    })?;
    if !matches!(e.devices[i], Device::Vsrc(_) | Device::Isrc(_)) {
        return Err(SimError::invalid(format!(
            "`{name}` is not an independent V or I source"
        )));
    }
    Ok(i)
}

/// Unit excitation of an independent source into a right-hand side.
fn excite_unit<T: Copy + From<f64> + std::ops::AddAssign + std::ops::SubAssign>(
    e: &Engine,
    src: usize,
    rhs: &mut [T],
) {
    match &e.devices[src] {
        Device::Vsrc(v) => rhs[v.br] += T::from(1.0),
        Device::Isrc(i) => {
            rhs[i.p] -= T::from(1.0);
            rhs[i.n] += T::from(1.0);
        }
        _ => {}
    }
}

/// `.noise` — returns (spectral density plot, integrated totals plot).
pub(crate) fn noise(
    e: &mut Engine,
    title: &str,
    output: &OutputVar,
    input: &str,
    sweep: &Sweep,
) -> Result<(Plot, Plot), SimError> {
    small_signal_op(e)?;
    let (op_, on_) = output_unknowns(e, output)?;
    let src = input_source(e, input)?;
    let freqs = sweep.values();
    let mut sys = AcSystem::new(e);
    let mut onoise = Vec::new();
    let mut inoise = Vec::new();
    // per-source densities for power-law integration
    let mut per_src: Vec<Vec<f64>> = Vec::new();
    let mut gains: Vec<f64> = Vec::new();
    let mut srcs: Vec<NoiseSrc> = Vec::new();
    for &f in &freqs {
        let omega = std::f64::consts::TAU * f;
        sys.load(e, omega)?;
        // gain from the input source
        sys.rhs.iter_mut().for_each(|v| *v = C64::default());
        excite_unit(e, src, &mut sys.rhs);
        sys.rhs[0] = C64::default();
        sys.mat.solve(&mut sys.rhs);
        let gain = sys.rhs[op_]
            - if on_ != 0 {
                sys.rhs[on_]
            } else {
                C64::default()
            };
        // adjoint: transfer from every node pair to the output
        sys.rhs.iter_mut().for_each(|v| *v = C64::default());
        sys.rhs[op_] += C64::new(1.0, 0.0);
        if on_ != 0 {
            sys.rhs[on_] -= C64::new(1.0, 0.0);
        }
        sys.rhs[0] = C64::default();
        sys.mat.solve_transpose(&mut sys.rhs);
        srcs.clear();
        for d in &e.devices {
            d.noise(&e.ck, f, &mut srcs);
        }
        if per_src.is_empty() {
            per_src = vec![Vec::new(); srcs.len()];
        }
        let mut total = 0.0;
        for (k, s) in srcs.iter().enumerate() {
            let z = sys.rhs[s.p] - sys.rhs[s.n];
            let contrib = z.norm_sqr() * s.psd;
            total += contrib;
            if let Some(v) = per_src.get_mut(k) {
                v.push(contrib);
            }
        }
        let g2 = gain.norm_sqr();
        gains.push(g2);
        onoise.push(total.sqrt());
        inoise.push(if g2 > 0.0 {
            (total / g2).sqrt()
        } else {
            f64::INFINITY
        });
    }
    let mut p1 = Plot::new(PlotKind::Noise, title);
    p1.push_scale(Vector::real(
        "frequency",
        Quantity::Frequency,
        freqs.clone(),
    ));
    p1.push(Vector::real("onoise_spectrum", Quantity::Noise, onoise));
    p1.push(Vector::real("inoise_spectrum", Quantity::Noise, inoise));
    // integrated noise (power-law integration per source, like ngspice)
    let mut out_tot = 0.0;
    let mut in_tot = 0.0;
    for dens in &per_src {
        out_tot += powerlaw_integral(&freqs, dens);
        // input-referred: ngspice scales each interval by the gain at its upper end
        for i in 1..freqs.len().min(dens.len()) {
            let g = gains[i];
            if g > 0.0 {
                in_tot += powerlaw_integral(&freqs[i - 1..=i], &[dens[i - 1] / g, dens[i] / g]);
            }
        }
    }
    let mut p2 = Plot::new(PlotKind::NoiseTotal, title);
    p2.push(Vector::real(
        "onoise_total",
        Quantity::Noise,
        vec![out_tot.sqrt()],
    ));
    p2.push(Vector::real(
        "inoise_total",
        Quantity::Noise,
        vec![in_tot.sqrt()],
    ));
    Ok((p1, p2))
}

/// ∫ N(f) df assuming N varies as a power law between samples (ngspice `Nintegrate`).
pub(crate) fn powerlaw_integral(f: &[f64], n: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 1..f.len().min(n.len()) {
        let (f0, f1, n0, n1) = (f[i - 1], f[i], n[i - 1], n[i]);
        if f1 <= f0 {
            continue;
        }
        if n0 <= 0.0 || n1 <= 0.0 || f0 <= 0.0 {
            s += 0.5 * (n0 + n1) * (f1 - f0);
            continue;
        }
        let a = (n1 / n0).ln() / (f1 / f0).ln();
        if (a + 1.0).abs() < 1e-10 {
            s += n0 * f0 * (f1 / f0).ln();
        } else {
            s += n0 * f0 / (a + 1.0) * ((f1 / f0).powf(a + 1.0) - 1.0);
        }
    }
    s
}

/// `.tf`
pub(crate) fn tf(
    e: &mut Engine,
    title: &str,
    output: &OutputVar,
    input: &str,
) -> Result<Plot, SimError> {
    small_signal_op(e)?;
    let src = input_source(e, input)?;
    let (op_, on_) = output_unknowns(e, output)?;
    // linearised DC matrix at the operating point (reuse the AC machinery at ω = 0)
    let mut sys = AcSystem::new(e);
    sys.load(e, 0.0)?;
    let solve = |sys: &mut AcSystem, f: &dyn Fn(&mut [C64])| -> Vec<C64> {
        sys.rhs.iter_mut().for_each(|v| *v = C64::default());
        f(&mut sys.rhs);
        sys.rhs[0] = C64::default();
        sys.mat.solve(&mut sys.rhs);
        sys.rhs.clone()
    };
    let x = solve(&mut sys, &|r| excite_unit(e, src, r));
    let out = |x: &[C64]| x[op_].re - if on_ != 0 { x[on_].re } else { 0.0 };
    let tfv = out(&x);
    let zin = match &e.devices[src] {
        Device::Vsrc(v) => -1.0 / x[v.br].re,
        Device::Isrc(i) => x[i.n].re - x[i.p].re,
        _ => f64::NAN,
    };
    let zout = match output {
        OutputVar::Voltage(..) => {
            let y = solve(&mut sys, &|r| {
                r[op_] += C64::new(1.0, 0.0);
                if on_ != 0 {
                    r[on_] -= C64::new(1.0, 0.0);
                }
            });
            out(&y)
        }
        OutputVar::Current(s) => {
            // impedance seen by the output source branch: unit voltage in its branch row
            let i = e.device_index(s).unwrap_or(0);
            let br = e.devices[i].branch().unwrap_or(0);
            let y = solve(&mut sys, &|r| r[br] += C64::new(1.0, 0.0));
            1.0 / y[br].re
        }
    };
    let mut p = Plot::new(PlotKind::Tf, title);
    p.push(Vector::real(
        "transfer_function",
        Quantity::Other,
        vec![tfv],
    ));
    p.push(Vector::real(
        format!("output_impedance_at_{}", output.label()),
        Quantity::Impedance,
        vec![zout],
    ));
    p.push(Vector::real(
        format!("{input}#input_impedance"),
        Quantity::Impedance,
        vec![zin],
    ));
    Ok(p)
}
