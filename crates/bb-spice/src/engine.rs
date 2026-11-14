//! The MNA engine: builds devices from a [`Circuit`], owns the solver state, runs Newton-Raphson
//! and the operating-point strategy (direct → dynamic gmin stepping → source stepping).

use crate::devices::bsrc::{Bsrc, Input};
use crate::devices::switch::{SwCtrl, Switch};
use crate::devices::*;
use crate::elab::{Circuit, ElementKind, InstParams, Value, eval_dyn};
use crate::error::{ConvergenceReport, Diagnostic, SimError};
use crate::expr::{Compiled, Expr, Leaf, RtVar};
use crate::matrix::{Pattern, Slot};
use std::collections::HashMap;

/// A parameter that depends on `ext()`/temperature and must be re-evaluated when they change.
#[derive(Debug, Clone)]
pub(crate) struct DynBinding {
    pub dev: usize,
    pub param: String,
    pub expr: Expr,
    pub last: f64,
}

/// Why Newton failed.
#[derive(Debug, Clone)]
pub(crate) enum NrFail {
    Iterations,
    Singular(usize, usize),
    NonFinite,
}

/// Counters.
#[derive(Debug, Clone, Default)]
pub(crate) struct Counters {
    pub newton: u64,
    pub conv_aids: u64,
}

#[derive(Debug, Clone)]
pub struct Engine {
    pub(crate) ck: Ckt,
    pub(crate) devices: Vec<Device>,
    /// Unknown names (`v(out)`, `i(v1)`, `v(d1#anode)`), index 1..=n.
    pub(crate) names: Vec<String>,
    pub(crate) kinds: Vec<UKind>,
    /// Number of circuit (non-internal) nodes; unknowns 1..=n_nodes are `circuit.nodes[1..]`.
    pub(crate) n_nodes: usize,
    pub(crate) node_names: Vec<String>,
    node_index: HashMap<String, usize>,
    dev_index: HashMap<String, usize>,
    pub(crate) ext_names: Vec<String>,
    ext_index: HashMap<String, usize>,
    pub(crate) bindings: Vec<DynBinding>,
    diag: Vec<Slot>,
    pub(crate) pattern: Pattern,
    pub(crate) ics: Vec<(usize, f64)>,
    pub(crate) nodesets: Vec<(usize, f64)>,
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) counters: Counters,
    sol: Vec<f64>,
    x_old: Vec<f64>,
    /// Whether every device is linear (no Newton needed beyond one solve).
    pub(crate) linear: bool,
    /// Whether any device stores charge/flux.
    pub(crate) reactive: bool,
    /// Whether any source varies with time.
    pub(crate) time_varying: bool,
    /// Report from the last failed Newton attempt.
    last_fail: ConvergenceReport,
    /// Whether the next factorisation must re-order (new analysis).
    pub(crate) reorder: bool,
}

fn inst<'a>(params: &'a InstParams, key: &str) -> Option<&'a Value> {
    params.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

impl Engine {
    /// Builds the engine (devices, unknowns, matrix pattern) for a circuit.
    pub fn new(c: &Circuit) -> Result<Engine, SimError> {
        Engine::with_ext(c, &HashMap::new())
    }

    /// Like [`Engine::new`] with initial external-variable values.
    pub fn with_ext(c: &Circuit, ext_values: &HashMap<String, f64>) -> Result<Engine, SimError> {
        let mut b = Builder {
            c,
            ext_names: Vec::new(),
            ext_index: HashMap::new(),
            ext_values,
            bindings: Vec::new(),
            warnings: Vec::new(),
        };
        let mut devices = Vec::with_capacity(c.elements.len());
        let mut couplings = Vec::new();
        for el in &c.elements {
            if let ElementKind::Coupling { l1, l2, k } = &el.kind {
                couplings.push((el, l1.clone(), l2.clone(), *k));
                continue;
            }
            let idx = devices.len();
            let d = b.device(el, idx)?;
            devices.push(d);
        }
        let dev_index: HashMap<String, usize> = devices
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name().to_string(), i))
            .collect();
        // mutual inductances
        for (el, l1, l2, k) in couplings {
            let find = |n: &str| -> Result<usize, SimError> {
                match dev_index.get(n) {
                    Some(&i) if matches!(devices[i], Device::Inductor(_)) => Ok(i),
                    _ => Err(SimError::elab(
                        &el.span,
                        format!("`{}`: `{n}` is not an inductor", el.name),
                    )),
                }
            };
            let (i1, i2) = (find(&l1)?, find(&l2)?);
            let (la, lb) = match (&devices[i1], &devices[i2]) {
                (Device::Inductor(a), Device::Inductor(b)) => (a.inductance(), b.inductance()),
                _ => unreachable!(),
            };
            let m = k * (la * lb).sqrt();
            if let Device::Inductor(a) = &mut devices[i1] {
                a.mutuals.push((l2.clone(), m, 0, Slot::GROUND));
            }
            if let Device::Inductor(bb) = &mut devices[i2] {
                bb.mutuals.push((l1.clone(), m, 0, Slot::GROUND));
            }
        }

        // unknowns: circuit nodes first
        let n_nodes = c.nodes.len() - 1;
        let mut names: Vec<String> = vec![String::new()];
        let mut kinds = vec![UKind::Node];
        for n in &c.nodes[1..] {
            names.push(format!("v({n})"));
            kinds.push(UKind::Node);
        }
        let empty = HashMap::new();
        let mut s = Setup {
            n: n_nodes,
            names: &mut names,
            kinds: &mut kinds,
            n_states: 0,
            pattern: None,
            branches: &empty,
            opts: &c.options,
        };
        for d in devices.iter_mut() {
            d.setup_unknowns(&mut s)?;
        }
        let n = s.n;
        let n_states = s.n_states;
        let mut branches = HashMap::new();
        for d in &devices {
            if let Some(br) = d.branch() {
                branches.insert(d.name().to_string(), br);
            }
        }
        // fill mutual branch indices
        for d in devices.iter_mut() {
            if let Device::Inductor(l) = d {
                for m in l.mutuals.iter_mut() {
                    m.2 = branches[&m.0];
                }
            }
        }
        let mut pattern = Pattern::new(n);
        let diag: Vec<Slot> = (0..=n)
            .map(|i| {
                if i == 0 {
                    Slot::GROUND
                } else {
                    pattern.slot(i, i)
                }
            })
            .collect();
        {
            let mut s = Setup {
                n,
                names: &mut names,
                kinds: &mut kinds,
                n_states,
                pattern: Some(&mut pattern),
                branches: &branches,
                opts: &c.options,
            };
            for d in devices.iter_mut() {
                d.setup_matrix(&mut s).map_err(|e| match e {
                    SimError::Invalid(m) => {
                        let span = c
                            .elements
                            .iter()
                            .find(|el| m.contains(&format!("`{}`", el.name)))
                            .map(|el| el.span.clone())
                            .unwrap_or_default();
                        SimError::Elaboration { span, msg: m }
                    }
                    e => e,
                })?;
            }
        }
        // rename branch unknowns to i(name)
        for d in &devices {
            if let Some(br) = d.branch() {
                names[br] = format!("i({})", d.name());
            }
        }
        for (i, name) in names.iter_mut().enumerate().skip(n_nodes + 1) {
            if kinds[i] == UKind::Node && !name.starts_with("v(") {
                *name = format!("v({name})");
            }
        }
        let mut ck = Ckt::new(n, &pattern, n_states, c.options.clone());
        ck.ext = b
            .ext_names
            .iter()
            .map(|n| ext_values.get(n).copied().unwrap_or(0.0))
            .collect();
        let linear = !devices.iter().any(|d| d.is_nonlinear());
        let reactive = devices.iter().any(|d| d.is_reactive());
        let time_varying = devices.iter().any(|d| d.is_time_varying());
        let node_index: HashMap<String, usize> = c
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        let mut warnings = c.warnings.clone();
        warnings.extend(b.warnings);
        let ext_names = b.ext_names;
        let ext_index = b.ext_index;
        let bindings = b.bindings;
        let mut e = Engine {
            ck,
            devices,
            names,
            kinds,
            n_nodes,
            node_names: c.nodes.clone(),
            node_index,
            dev_index,
            ext_names,
            ext_index,
            bindings,
            diag,
            pattern,
            ics: c.ics.clone(),
            nodesets: c.nodesets.clone(),
            warnings,
            counters: Counters::default(),
            sol: vec![0.0; n + 1],
            x_old: vec![0.0; n + 1],
            linear,
            reactive,
            time_varying,
            last_fail: ConvergenceReport::default(),
            reorder: true,
        };
        e.set_temperature(c.options.temp);
        Ok(e)
    }

    pub fn n(&self) -> usize {
        self.ck.n
    }

    /// Unknown index of a circuit node (`0` = ground).
    pub fn node(&self, name: &str) -> Option<usize> {
        let l = name.to_ascii_lowercase();
        if l == "gnd" || l == "0" {
            return Some(0);
        }
        self.node_index.get(&l).copied()
    }

    pub fn device_index(&self, name: &str) -> Option<usize> {
        self.dev_index.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn device(&self, i: usize) -> &Device {
        &self.devices[i]
    }

    pub fn device_mut(&mut self, i: usize) -> &mut Device {
        &mut self.devices[i]
    }

    /// Current solution vector (index 0 = ground).
    pub fn solution(&self) -> &[f64] {
        &self.ck.x
    }

    pub fn ext_slot(&self, name: &str) -> Option<usize> {
        self.ext_index.get(name).copied()
    }

    /// Sets the circuit temperature (°C) and updates temperature-dependent parameters.
    pub fn set_temperature(&mut self, temp_c: f64) {
        self.ck.opts.temp = temp_c;
        self.ck.temp = temp_c + KELVIN;
        let opts = self.ck.opts.clone();
        for d in self.devices.iter_mut() {
            d.temperature(temp_c + KELVIN, &opts);
        }
        self.refresh_bindings(true);
    }

    /// Sets an external variable. Returns true if any element depends on it.
    pub fn set_ext(&mut self, name: &str, v: f64) -> bool {
        match self.ext_index.get(name) {
            Some(&i) => {
                self.ck.ext[i] = v;
                true
            }
            None => false,
        }
    }

    /// Re-evaluates ext/temperature-dependent parameters; returns true if anything changed.
    pub fn refresh_bindings(&mut self, force: bool) -> bool {
        let mut changed = false;
        let temp_c = self.ck.temp - KELVIN;
        for b in self.bindings.iter_mut() {
            let names = &self.ext_names;
            let ext = &self.ck.ext;
            let v = eval_dyn(
                &b.expr,
                &|n| names.iter().position(|x| x == n).map(|i| ext[i]),
                temp_c,
            );
            if force || v != b.last {
                b.last = v;
                self.devices[b.dev].set_param(&b.param, v);
                changed = true;
            }
        }
        changed
    }

    /// Clears and stamps the matrix and right-hand side at the current iterate.
    pub(crate) fn load(&mut self) {
        self.ck.mat.clear();
        self.ck.rhs.iter_mut().for_each(|v| *v = 0.0);
        self.ck.noncon = 0;
        self.ck.noncon_devs.clear();
        for (i, d) in self.devices.iter_mut().enumerate() {
            self.ck.cur_dev = i as u32;
            d.load(&mut self.ck);
        }
        let g = self.ck.diag_gmin;
        if g > 0.0 {
            for i in 1..=self.ck.n {
                if self.kinds[i] == UKind::Node {
                    let s = self.diag[i];
                    self.ck.add(s, g);
                }
            }
        }
        // .nodeset during the first phases, .ic during the transient operating point
        if self.ck.mode == Mode::Dc {
            const G: f64 = 1e10;
            if self.ck.tran_op && !self.ck.uic {
                for &(n, v) in &self.ics {
                    let s = self.diag[n];
                    self.ck.add(s, G);
                    self.ck.rhs[n] += G * v;
                }
            }
            if matches!(self.ck.init, Init::Jct | Init::Fix) {
                for &(n, v) in &self.nodesets {
                    let s = self.diag[n];
                    self.ck.add(s, G);
                    self.ck.rhs[n] += G * v;
                }
            }
        }
        self.ck.rhs[0] = 0.0;
    }

    fn conv_test(&self) -> bool {
        let o = &self.ck.opts;
        for i in 1..=self.ck.n {
            let (a, b) = (self.ck.x[i], self.x_old[i]);
            let tol = o.reltol * a.abs().max(b.abs())
                + if self.kinds[i] == UKind::Node {
                    o.vntol
                } else {
                    o.abstol
                };
            if (a - b).abs() > tol {
                return false;
            }
        }
        self.devices.iter().all(|d| d.conv_ok(&self.ck))
    }

    fn fail_report(&self, iters: usize) -> ConvergenceReport {
        let o = &self.ck.opts;
        let mut worst: Vec<(f64, usize)> = (1..=self.ck.n)
            .map(|i| {
                let (a, b) = (self.ck.x[i], self.x_old[i]);
                let tol = o.reltol * a.abs().max(b.abs())
                    + if self.kinds[i] == UKind::Node {
                        o.vntol
                    } else {
                        o.abstol
                    };
                ((a - b).abs() / tol, i)
            })
            .filter(|(r, _)| *r > 1.0 || r.is_nan())
            .collect();
        worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut elements: Vec<String> = self
            .ck
            .noncon_devs
            .iter()
            .map(|&i| self.devices[i as usize].name().to_string())
            .collect();
        for d in &self.devices {
            if !d.conv_ok(&self.ck) && !elements.iter().any(|e| e == d.name()) && elements.len() < 8
            {
                elements.push(d.name().to_string());
            }
        }
        ConvergenceReport {
            unknowns: worst
                .iter()
                .take(5)
                .map(|&(_, i)| {
                    (
                        self.display_name(i),
                        self.ck.x[i],
                        self.ck.x[i] - self.x_old[i],
                    )
                })
                .collect(),
            elements,
            iterations: iters,
            aids_tried: Vec::new(),
        }
    }

    /// Unknown name for messages: `V(out)`, `I(V1)`.
    pub(crate) fn display_name(&self, i: usize) -> String {
        let n = &self.names[i];
        if let Some(rest) = n.strip_prefix("v(") {
            format!("V({rest}")
        } else if let Some(rest) = n.strip_prefix("i(") {
            format!("I({rest}")
        } else {
            n.clone()
        }
    }

    /// Newton-Raphson at the current mode. On success `ck.x` holds the solution.
    pub(crate) fn newton(&mut self, max_iter: usize) -> Result<usize, NrFail> {
        let mut iter = 0;
        if self.reorder {
            self.ck.mat.invalidate_ordering();
            self.reorder = false;
        }
        loop {
            self.load();
            iter += 1;
            self.counters.newton += 1;
            if let Err(s) = self.ck.mat.factor() {
                self.last_fail = self.fail_report(iter);
                return Err(NrFail::Singular(s.row, s.col));
            }
            self.sol.copy_from_slice(&self.ck.rhs);
            self.ck.mat.solve(&mut self.sol);
            if self.sol.iter().any(|v| !v.is_finite()) {
                self.last_fail = self.fail_report(iter);
                return Err(NrFail::NonFinite);
            }
            std::mem::swap(&mut self.x_old, &mut self.ck.x);
            self.ck.x.copy_from_slice(&self.sol);
            let converged = if self.ck.noncon == 0 && (iter > 1 || self.linear) {
                self.conv_test()
            } else {
                false
            };
            match self.ck.init {
                Init::Float => {
                    if converged {
                        return Ok(iter);
                    }
                }
                Init::Jct => self.ck.init = Init::Fix,
                Init::Fix => {
                    if self.ck.noncon == 0 {
                        self.ck.init = Init::Float;
                    }
                }
                Init::Tran | Init::Pred => self.ck.init = Init::Float,
            }
            if iter >= max_iter {
                self.last_fail = self.fail_report(iter);
                return Err(NrFail::Iterations);
            }
        }
    }

    fn singular_error(&self, row: usize, col: usize) -> SimError {
        let r = self.display_name(row);
        let c = self.display_name(col);
        let detail = if row <= self.n_nodes || col <= self.n_nodes {
            let node = if col <= self.n_nodes { &c } else { &r };
            format!(
                "{node} has no DC path to ground or is only connected to current sources/capacitors (also check for loops of voltage sources/inductors)"
            )
        } else {
            format!(
                "check {r} / {c}: a loop of voltage sources or inductors, or a current-controlled source with an invalid controller"
            )
        };
        SimError::Singular { detail }
    }

    pub(crate) fn nr_error(
        &self,
        f: NrFail,
        analysis: &str,
        time: Option<f64>,
        aids: Vec<String>,
    ) -> SimError {
        match f {
            NrFail::Singular(r, c) => self.singular_error(r, c),
            NrFail::NonFinite | NrFail::Iterations => {
                let mut report = self.last_fail.clone();
                report.aids_tried = aids;
                SimError::NoConvergence {
                    analysis: analysis.to_string(),
                    time,
                    report,
                }
            }
        }
    }

    /// DC operating point at the current mode settings (`tran_op`/`uic` flags respected).
    /// Tries plain Newton, then dynamic gmin stepping, then source stepping.
    pub(crate) fn solve_op(&mut self, analysis: &str) -> Result<(), SimError> {
        self.ck.mode = Mode::Dc;
        self.ck.srcfact = 1.0;
        self.ck.diag_gmin = 0.0;
        let itl1 = self.ck.opts.itl1;
        let mut aids = Vec::new();
        let x_start = self.ck.x.clone();
        if !self.ck.opts.noopiter {
            self.ck.init = Init::Jct;
            match self.newton(itl1) {
                Ok(_) => return Ok(()),
                Err(NrFail::Singular(r, c)) => return Err(self.singular_error(r, c)),
                Err(_) => {}
            }
        }
        if self.ck.opts.gmin_steps > 0 {
            aids.push("gmin stepping".to_string());
            self.counters.conv_aids += 1;
            self.ck.x.copy_from_slice(&x_start);
            if self.gmin_stepping().is_ok() {
                self.warnings.push(Diagnostic::info(
                    Default::default(),
                    format!("{analysis}: converged with gmin stepping"),
                ));
                return Ok(());
            }
        }
        if self.ck.opts.src_steps > 0 {
            aids.push("source stepping".to_string());
            self.counters.conv_aids += 1;
            self.ck.x.iter_mut().for_each(|v| *v = 0.0);
            if self.source_stepping().is_ok() {
                self.warnings.push(Diagnostic::info(
                    Default::default(),
                    format!("{analysis}: converged with source stepping"),
                ));
                return Ok(());
            }
        }
        self.ck.srcfact = 1.0;
        self.ck.diag_gmin = 0.0;
        let mut report = self.last_fail.clone();
        report.aids_tried = aids;
        Err(SimError::NoConvergence {
            analysis: analysis.to_string(),
            time: None,
            report,
        })
    }

    fn gmin_stepping(&mut self) -> Result<(), NrFail> {
        let itl2 = self.ck.opts.itl2.max(10);
        let gtarget = self.ck.opts.gmin.max(1e-15);
        let mut factor: f64 = 10.0;
        let mut old_gmin = 1e-2;
        self.ck.diag_gmin = old_gmin / factor;
        self.ck.init = Init::Jct;
        let mut saved_x = self.ck.x.clone();
        let mut saved_st = self.ck.st[0].clone();
        loop {
            match self.newton(itl2) {
                Ok(iters) => {
                    self.ck.init = Init::Float;
                    if self.ck.diag_gmin <= gtarget {
                        break;
                    }
                    saved_x.copy_from_slice(&self.ck.x);
                    saved_st.copy_from_slice(&self.ck.st[0]);
                    if iters <= itl2 / 4 {
                        factor = (factor * factor.sqrt()).min(10.0);
                    }
                    if iters > 3 * itl2 / 4 {
                        factor = factor.sqrt();
                    }
                    old_gmin = self.ck.diag_gmin;
                    if self.ck.diag_gmin < factor * gtarget {
                        self.ck.diag_gmin = gtarget;
                    } else {
                        self.ck.diag_gmin /= factor;
                    }
                }
                Err(NrFail::Singular(r, c)) if self.ck.diag_gmin == 0.0 => {
                    return Err(NrFail::Singular(r, c));
                }
                Err(e) => {
                    if factor < 1.00005 {
                        self.ck.diag_gmin = 0.0;
                        return Err(e);
                    }
                    factor = factor.sqrt().sqrt();
                    self.ck.diag_gmin = old_gmin / factor;
                    self.ck.x.copy_from_slice(&saved_x);
                    self.ck.st[0].copy_from_slice(&saved_st);
                    self.ck.init = Init::Float;
                }
            }
        }
        self.ck.diag_gmin = 0.0;
        let itl1 = self.ck.opts.itl1;
        self.newton(itl1).map(|_| ())
    }

    fn source_stepping(&mut self) -> Result<(), NrFail> {
        let itl2 = self.ck.opts.itl2.max(10);
        self.ck.srcfact = 0.0;
        self.ck.init = Init::Jct;
        if self.newton(itl2).is_err() {
            // zero sources with gmin help
            self.ck.x.iter_mut().for_each(|v| *v = 0.0);
            let r = self.gmin_stepping();
            r?;
        }
        let mut saved_x = self.ck.x.clone();
        let mut saved_st = self.ck.st[0].clone();
        let mut raise = 1e-3;
        let mut conv_fact = 0.0;
        self.ck.srcfact = raise;
        self.ck.init = Init::Float;
        let mut tries = 0;
        while conv_fact < 1.0 {
            tries += 1;
            if tries > 2000 {
                return Err(NrFail::Iterations);
            }
            match self.newton(itl2) {
                Ok(iters) => {
                    conv_fact = self.ck.srcfact;
                    saved_x.copy_from_slice(&self.ck.x);
                    saved_st.copy_from_slice(&self.ck.st[0]);
                    if iters <= itl2 / 4 {
                        raise *= 1.5;
                    }
                    if iters > 3 * itl2 / 4 {
                        raise *= 0.5;
                    }
                }
                Err(e) => {
                    if self.ck.srcfact - conv_fact < 1e-8 {
                        self.ck.srcfact = 1.0;
                        return Err(e);
                    }
                    raise /= 10.0;
                    self.ck.x.copy_from_slice(&saved_x);
                    self.ck.st[0].copy_from_slice(&saved_st);
                }
            }
            self.ck.srcfact = (conv_fact + raise).min(1.0);
            self.ck.init = Init::Float;
            if conv_fact >= 1.0 {
                break;
            }
        }
        self.ck.srcfact = 1.0;
        Ok(())
    }

    /// Value of a node voltage in the current solution.
    pub fn voltage(&self, node: usize) -> f64 {
        self.ck.x[node]
    }

    /// Main current of a device in the current solution.
    pub fn device_current(&self, i: usize) -> Option<f64> {
        self.devices[i].current(&self.ck)
    }

    /// Rotates the state history after an accepted time point (state0 is kept as a copy).
    pub(crate) fn rotate_states(&mut self) {
        self.ck.st.rotate_right(1);
        let (a, b) = self.ck.st.split_at_mut(1);
        a[0].copy_from_slice(&b[0]);
    }
}

/// Builds devices from elements.
struct Builder<'a> {
    c: &'a Circuit,
    ext_names: Vec<String>,
    ext_index: HashMap<String, usize>,
    ext_values: &'a HashMap<String, f64>,
    bindings: Vec<DynBinding>,
    warnings: Vec<Diagnostic>,
}

impl Builder<'_> {
    fn ext(&mut self, name: &str) -> usize {
        if let Some(&i) = self.ext_index.get(name) {
            return i;
        }
        let i = self.ext_names.len();
        self.ext_names.push(name.to_string());
        self.ext_index.insert(name.to_string(), i);
        i
    }

    /// Evaluates a value, recording a binding when it is dynamic.
    fn val(&mut self, v: &Value, dev: usize, param: &str) -> f64 {
        match v {
            Value::Const(x) => *x,
            Value::Dyn(e) => {
                let mut names = Vec::new();
                e.visit(&mut |n| {
                    if let Expr::Ext(name) = n {
                        names.push(name.clone());
                    }
                });
                for n in &names {
                    self.ext(n);
                }
                let ev = self.ext_values;
                let x = eval_dyn(e, &|n| ev.get(n).copied(), self.c.options.temp);
                self.bindings.push(DynBinding {
                    dev,
                    param: param.to_string(),
                    expr: (**e).clone(),
                    last: x,
                });
                x
            }
        }
    }

    fn opt(&mut self, p: &InstParams, key: &str, dev: usize) -> Option<f64> {
        inst(p, key).cloned().map(|v| self.val(&v, dev, key))
    }

    fn model_params(&mut self, m: &crate::elab::ModelInst, dev: usize) -> ParamMap {
        let mut out = Vec::new();
        for (k, v) in &m.params {
            let x = self.val(v, dev, k);
            out.push((k.clone(), x));
        }
        ParamMap::new(out)
    }

    fn warn_unknown(&mut self, el: &crate::elab::Element, kind: &str, unknown: Vec<String>) {
        if !unknown.is_empty() {
            self.warnings.push(Diagnostic::warning(
                el.span.clone(),
                format!(
                    "`{}`: unknown {kind} parameter(s) ignored: {}",
                    el.name,
                    unknown.join(", ")
                ),
            ));
        }
    }

    fn device(&mut self, el: &crate::elab::Element, idx: usize) -> Result<Device, SimError> {
        let n = &el.nodes;
        let name = el.name.clone();
        let o = &self.c.options;
        Ok(match &el.kind {
            ElementKind::Resistor { r, params, model } => {
                let r = self.val(r, idx, "r");
                let mp = model
                    .as_ref()
                    .map(|m| self.model_params(m, idx))
                    .unwrap_or_default();
                let tc1 = self
                    .opt(params, "tc1", idx)
                    .or(self.opt(params, "tc", idx))
                    .or(mp.get(&["tc1"]))
                    .unwrap_or(0.0);
                let tc2 = self
                    .opt(params, "tc2", idx)
                    .or(self.opt(params, "tc.1", idx))
                    .or(mp.get(&["tc2"]))
                    .unwrap_or(0.0);
                let temp = self.opt(params, "temp", idx);
                let dtemp = self.opt(params, "dtemp", idx).unwrap_or(0.0);
                let m = self.opt(params, "m", idx).unwrap_or(1.0);
                let noisy = self.opt(params, "noisy", idx).unwrap_or(1.0) != 0.0;
                Device::Resistor(passive::Resistor::new(
                    name, n[0], n[1], r, tc1, tc2, temp, dtemp, m, noisy,
                ))
            }
            ElementKind::Capacitor { c, params, model } => {
                let mut cv = self.val(c, idx, "c");
                let mp = model
                    .as_ref()
                    .map(|m| self.model_params(m, idx))
                    .unwrap_or_default();
                if let Some(cm) = mp.get(&["c", "cap"]) {
                    if cv == 0.0 {
                        cv = cm;
                    }
                }
                let tc1 = self
                    .opt(params, "tc1", idx)
                    .or(mp.get(&["tc1"]))
                    .unwrap_or(0.0);
                let tc2 = self
                    .opt(params, "tc2", idx)
                    .or(mp.get(&["tc2"]))
                    .unwrap_or(0.0);
                let temp = self.opt(params, "temp", idx);
                let dtemp = self.opt(params, "dtemp", idx).unwrap_or(0.0);
                let m = self.opt(params, "m", idx).unwrap_or(1.0);
                let ic = self.opt(params, "ic", idx);
                Device::Capacitor(passive::Capacitor::new(
                    name, n[0], n[1], cv, tc1, tc2, temp, dtemp, m, ic,
                ))
            }
            ElementKind::Inductor { l, params, model } => {
                let lv = self.val(l, idx, "l");
                let mp = model
                    .as_ref()
                    .map(|m| self.model_params(m, idx))
                    .unwrap_or_default();
                let tc1 = self
                    .opt(params, "tc1", idx)
                    .or(mp.get(&["tc1"]))
                    .unwrap_or(0.0);
                let tc2 = self
                    .opt(params, "tc2", idx)
                    .or(mp.get(&["tc2"]))
                    .unwrap_or(0.0);
                let temp = self.opt(params, "temp", idx);
                let dtemp = self.opt(params, "dtemp", idx).unwrap_or(0.0);
                let m = self.opt(params, "m", idx).unwrap_or(1.0);
                let ic = self.opt(params, "ic", idx);
                Device::Inductor(passive::Inductor::new(
                    name, n[0], n[1], lv, tc1, tc2, temp, dtemp, m, ic,
                ))
            }
            ElementKind::Coupling { .. } => unreachable!("couplings handled by the engine"),
            ElementKind::Vsource(src) | ElementKind::Isource(src) => {
                let dc_given = src.dc_given || src.wave.is_none();
                let dc = self.val(&src.dc, idx, "dc");
                if matches!(el.kind, ElementKind::Vsource(_)) {
                    Device::Vsrc(sources::Vsrc::new(name, n[0], n[1], src, dc, dc_given))
                } else {
                    Device::Isrc(sources::Isrc::new(name, n[0], n[1], src, dc, dc_given))
                }
            }
            ElementKind::Vcvs { gain } => Device::Vcvs(controlled::Vcvs::new(
                name,
                [n[0], n[1], n[2], n[3]],
                self.val(gain, idx, "gain"),
            )),
            ElementKind::Vccs { gm } => Device::Vccs(controlled::Vccs::new(
                name,
                [n[0], n[1], n[2], n[3]],
                self.val(gm, idx, "gain"),
            )),
            ElementKind::Cccs { ctrl, gain } => Device::Cccs(controlled::Cccs::new(
                name,
                n[0],
                n[1],
                ctrl.clone(),
                self.val(gain, idx, "gain"),
            )),
            ElementKind::Ccvs { ctrl, r } => Device::Ccvs(controlled::Ccvs::new(
                name,
                n[0],
                n[1],
                ctrl.clone(),
                self.val(r, idx, "gain"),
            )),
            ElementKind::Behavioral { is_current, expr } => {
                let compiled = Compiled::compile(expr)
                    .map_err(|m| SimError::elab(&el.span, format!("`{}`: {m}", el.name)))?;
                let mut inputs = Vec::new();
                for leaf in compiled.leaves.clone() {
                    inputs.push(match leaf {
                        Leaf::Volt(a, b) => {
                            let ia = self.c.node(&a).ok_or_else(|| {
                                SimError::elab(
                                    &el.span,
                                    format!("`{}`: unknown node `{a}` in expression", el.name),
                                )
                            })?;
                            let ib = match b {
                                Some(b) => self.c.node(&b).ok_or_else(|| {
                                    SimError::elab(
                                        &el.span,
                                        format!("`{}`: unknown node `{b}` in expression", el.name),
                                    )
                                })?,
                                None => 0,
                            };
                            Input::Diff(ia, ib)
                        }
                        Leaf::Curr(c) => Input::Curr(c),
                        Leaf::Rt(RtVar::Time) => Input::Time,
                        Leaf::Rt(RtVar::Temper) => Input::Temp,
                        Leaf::Rt(RtVar::Hertz) => Input::Hertz,
                        Leaf::Ext(x) => Input::Ext(self.ext(&x)),
                    });
                }
                Device::Bsrc(Bsrc::new(name, n[0], n[1], *is_current, compiled, inputs))
            }
            ElementKind::Switch { model, init_on }
            | ElementKind::CSwitch { model, init_on, .. } => {
                let mp = self.model_params(model, idx);
                let (ctrl, current) = match &el.kind {
                    ElementKind::CSwitch { ctrl, .. } => (SwCtrl::Curr(ctrl.clone()), true),
                    _ => (SwCtrl::Volt(n[2], n[3]), false),
                };
                let (mut sw, warn) = Switch::new(name, n[0], n[1], ctrl, &mp, current, o.gmin);
                sw.init_on = *init_on;
                self.warn_unknown(el, "switch model", warn);
                Device::Switch(sw)
            }
            ElementKind::Diode { model, params, off } => {
                let mp = self.model_params(model, idx);
                let area = self.opt(params, "area", idx).unwrap_or(1.0)
                    * self.opt(params, "m", idx).unwrap_or(1.0);
                let ic = self.opt(params, "ic", idx);
                let temp = self.opt(params, "temp", idx);
                let dtemp = self.opt(params, "dtemp", idx).unwrap_or(0.0);
                let (d, warn) =
                    diode::Diode::new(name, n[0], n[1], mp, area, *off, ic, temp, dtemp);
                self.warn_unknown(el, "diode model", warn);
                Device::Diode(d)
            }
            ElementKind::Bjt { model, params, off } => {
                let mp = self.model_params(model, idx);
                let area = self.opt(params, "area", idx).unwrap_or(1.0)
                    * self.opt(params, "m", idx).unwrap_or(1.0);
                let ic = (self.opt(params, "ic", idx), self.opt(params, "ic.1", idx));
                let temp = self.opt(params, "temp", idx);
                let dtemp = self.opt(params, "dtemp", idx).unwrap_or(0.0);
                let pnp = model.kind == crate::netlist::ModelKind::Pnp;
                let sub = n.get(3).copied().unwrap_or(0);
                let (d, warn) = bjt::Bjt::new(
                    name,
                    [n[0], n[1], n[2], sub],
                    pnp,
                    mp,
                    area,
                    *off,
                    ic,
                    temp,
                    dtemp,
                );
                self.warn_unknown(el, "BJT model", warn);
                Device::Bjt(d)
            }
            ElementKind::Mosfet { model, params, off } => {
                let mp = self.model_params(model, idx);
                let mut ip = Vec::new();
                for (k, v) in params {
                    let x = self.val(v, idx, k);
                    ip.push((k.clone(), x));
                }
                let pmos = model.kind == crate::netlist::ModelKind::Pmos;
                let (d, warn) = mos1::Mos1::new(
                    name,
                    [n[0], n[1], n[2], n[3]],
                    pmos,
                    mp,
                    ParamMap::new(ip),
                    *off,
                    o,
                );
                self.warn_unknown(el, "MOSFET model", warn);
                Device::Mos1(d)
            }
            ElementKind::Jfet { model, params, off } => {
                let mp = self.model_params(model, idx);
                let area = self.opt(params, "area", idx).unwrap_or(1.0)
                    * self.opt(params, "m", idx).unwrap_or(1.0);
                let ic = (self.opt(params, "ic", idx), self.opt(params, "ic.1", idx));
                let temp = self.opt(params, "temp", idx);
                let pchan = model.kind == crate::netlist::ModelKind::Pjf;
                let (d, warn) =
                    jfet::Jfet::new(name, [n[0], n[1], n[2]], pchan, mp, area, *off, ic, temp);
                self.warn_unknown(el, "JFET model", warn);
                Device::Jfet(d)
            }
            ElementKind::Tline { z0, td, ic } => Device::Tline(tline::Tline::new(
                name,
                [n[0], n[1], n[2], n[3]],
                *z0,
                *td,
                *ic,
            )),
            ElementKind::Port { drive } => {
                Device::Port(sources::Port::new(name, n[0], n[1], *drive, o.port_leak))
            }
        })
    }
}
