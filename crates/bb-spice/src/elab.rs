//! Elaboration: [`Netlist`] → flat, typed [`Circuit`].
//!
//! Subcircuits are flattened (nodes `x1.mid`, elements `x1.r1`), parameters are evaluated with
//! subcircuit scoping, user functions inlined, models resolved (subcircuit-local first, then
//! global, then registered libraries) and Monte Carlo tolerances applied. Expressions that still
//! depend on `ext()` or the temperature stay symbolic ([`Value::Dyn`]) so a co-simulation
//! [`crate::Session`] can update them cheaply.

use crate::error::{Diagnostic, SimError, Span, suggest};
use crate::expr::{Expr, Func, RtVar};
use crate::netlist::*;
use bb_core::PinDrive;
use std::collections::HashMap;
use std::sync::Arc;

/// A possibly external-variable-dependent numeric value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Const(f64),
    /// Depends on `ext()` and/or `temper`; re-evaluated when those change.
    Dyn(Box<Expr>),
}

impl Value {
    pub fn constant(&self) -> Option<f64> {
        match self {
            Value::Const(v) => Some(*v),
            Value::Dyn(_) => None,
        }
    }
    /// Evaluates with external variables and temperature.
    pub fn eval(&self, env: &dyn Fn(&str) -> Option<f64>, temp: f64) -> f64 {
        match self {
            Value::Const(v) => *v,
            Value::Dyn(e) => eval_dyn(e, env, temp),
        }
    }
    pub fn scale(self, k: f64) -> Value {
        match self {
            Value::Const(v) => Value::Const(v * k),
            Value::Dyn(e) => Value::Dyn(Box::new(Expr::Binary(
                crate::expr::BinOp::Mul,
                e,
                Box::new(Expr::Num(k)),
            ))),
        }
    }
}

/// Evaluates an expression whose only free leaves are `ext()` and `temper`.
pub(crate) fn eval_dyn(e: &Expr, ext: &dyn Fn(&str) -> Option<f64>, temp: f64) -> f64 {
    let r = e.rewrite(&mut |n| {
        Ok(match n {
            Expr::Ext(name) => Some(Expr::Num(ext(name).unwrap_or(0.0))),
            Expr::Rt(RtVar::Temper) => Some(Expr::Num(temp)),
            Expr::Rt(_) => Some(Expr::Num(0.0)),
            _ => None,
        })
    });
    r.ok()
        .and_then(|e| e.eval_with(&|_| None).ok())
        .unwrap_or(f64::NAN)
}

/// Resolved source waveform (missing optional arguments stay `None` and take SPICE defaults
/// that depend on the transient step/stop time).
#[derive(Debug, Clone, PartialEq)]
pub enum Wave {
    Pulse {
        v1: f64,
        v2: f64,
        td: f64,
        tr: Option<f64>,
        tf: Option<f64>,
        pw: Option<f64>,
        per: Option<f64>,
        np: Option<f64>,
    },
    Sin {
        vo: f64,
        va: f64,
        freq: Option<f64>,
        td: f64,
        theta: f64,
        phase: f64,
    },
    Exp {
        v1: f64,
        v2: f64,
        td1: f64,
        tau1: Option<f64>,
        td2: Option<f64>,
        tau2: Option<f64>,
    },
    Pwl {
        pts: Vec<(f64, f64)>,
        repeat: Option<f64>,
        delay: f64,
    },
    Sffm {
        vo: f64,
        va: f64,
        fc: Option<f64>,
        mdi: f64,
        fs: Option<f64>,
    },
    Am {
        va: f64,
        vo: f64,
        mf: f64,
        fc: f64,
        td: f64,
    },
}

/// Independent source data.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub dc: Value,
    /// Whether a DC value was given explicitly (otherwise `.op` uses the waveform at t=0).
    pub dc_given: bool,
    pub ac_mag: f64,
    pub ac_phase_deg: f64,
    pub wave: Option<Wave>,
}

/// A model with evaluated parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInst {
    pub name: String,
    pub kind: ModelKind,
    pub params: Vec<(String, Value)>,
    pub span: Span,
}

impl ModelInst {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.params
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

/// Instance parameters (`area`, `w`, `l`, `ic`, `temp`, …).
pub type InstParams = Vec<(String, Value)>;

/// Element after elaboration. Node indices refer to [`Circuit::nodes`] (0 = ground).
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: String,
    pub span: Span,
    pub nodes: Vec<usize>,
    pub kind: ElementKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElementKind {
    Resistor {
        r: Value,
        params: InstParams,
        model: Option<Arc<ModelInst>>,
    },
    Capacitor {
        c: Value,
        params: InstParams,
        model: Option<Arc<ModelInst>>,
    },
    Inductor {
        l: Value,
        params: InstParams,
        model: Option<Arc<ModelInst>>,
    },
    /// Mutual coupling between two inductors (flattened names).
    Coupling {
        l1: String,
        l2: String,
        k: f64,
    },
    Vsource(Source),
    Isource(Source),
    /// E: nodes p n cp cn
    Vcvs {
        gain: Value,
    },
    /// G: nodes p n cp cn
    Vccs {
        gm: Value,
    },
    /// F: nodes p n
    Cccs {
        ctrl: String,
        gain: Value,
    },
    /// H: nodes p n
    Ccvs {
        ctrl: String,
        r: Value,
    },
    /// B source (and behavioural E/G/R): resolved expression.
    Behavioral {
        is_current: bool,
        expr: Expr,
    },
    /// S: nodes p n cp cn
    Switch {
        model: Arc<ModelInst>,
        init_on: Option<bool>,
    },
    /// W: nodes p n
    CSwitch {
        ctrl: String,
        model: Arc<ModelInst>,
        init_on: Option<bool>,
    },
    Diode {
        model: Arc<ModelInst>,
        params: InstParams,
        off: bool,
    },
    /// nodes c b e [s]
    Bjt {
        model: Arc<ModelInst>,
        params: InstParams,
        off: bool,
    },
    /// nodes d g s b
    Mosfet {
        model: Arc<ModelInst>,
        params: InstParams,
        off: bool,
    },
    /// nodes d g s
    Jfet {
        model: Arc<ModelInst>,
        params: InstParams,
        off: bool,
    },
    /// nodes p1+ p1- p2+ p2-
    Tline {
        z0: f64,
        td: f64,
        ic: [f64; 4],
    },
    Port {
        drive: PinDrive,
    },
}

impl ElementKind {
    /// SPICE letter for messages.
    pub fn letter(&self) -> char {
        match self {
            ElementKind::Resistor { .. } => 'R',
            ElementKind::Capacitor { .. } => 'C',
            ElementKind::Inductor { .. } => 'L',
            ElementKind::Coupling { .. } => 'K',
            ElementKind::Vsource(_) => 'V',
            ElementKind::Isource(_) => 'I',
            ElementKind::Vcvs { .. } => 'E',
            ElementKind::Vccs { .. } => 'G',
            ElementKind::Cccs { .. } => 'F',
            ElementKind::Ccvs { .. } => 'H',
            ElementKind::Behavioral { .. } => 'B',
            ElementKind::Switch { .. } => 'S',
            ElementKind::CSwitch { .. } => 'W',
            ElementKind::Diode { .. } => 'D',
            ElementKind::Bjt { .. } => 'Q',
            ElementKind::Mosfet { .. } => 'M',
            ElementKind::Jfet { .. } => 'J',
            ElementKind::Tline { .. } => 'T',
            ElementKind::Port { .. } => 'P',
        }
    }
}

/// Integration method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Method {
    #[default]
    Trap,
    Gear2,
    BackwardEuler,
}

/// Simulator options (`.options`).
#[derive(Debug, Clone, PartialEq)]
pub struct SimOptions {
    pub reltol: f64,
    pub abstol: f64,
    pub vntol: f64,
    pub chgtol: f64,
    pub trtol: f64,
    pub gmin: f64,
    /// DC iteration limit.
    pub itl1: usize,
    /// Gmin/source stepping iteration limit per step.
    pub itl2: usize,
    /// Transient iteration limit per timepoint.
    pub itl4: usize,
    pub method: Method,
    pub tnom: f64,
    pub temp: f64,
    pub gmin_steps: usize,
    pub src_steps: usize,
    /// Skip plain Newton and go straight to gmin stepping.
    pub noopiter: bool,
    /// Default MOSFET L and W.
    pub defl: f64,
    pub defw: f64,
    /// Maximum transient step (overrides the `.tran` default when set).
    pub tmax: Option<f64>,
    /// Leakage conductance added by high-impedance ports (S).
    pub port_leak: f64,
}

impl Default for SimOptions {
    fn default() -> Self {
        SimOptions {
            reltol: 1e-3,
            abstol: 1e-12,
            vntol: 1e-6,
            chgtol: 1e-14,
            trtol: 7.0,
            gmin: 1e-12,
            itl1: 100,
            itl2: 50,
            itl4: 10,
            method: Method::Trap,
            tnom: 27.0,
            temp: 27.0,
            gmin_steps: 10,
            src_steps: 10,
            noopiter: false,
            defl: 100e-6,
            defw: 100e-6,
            tmax: None,
            port_leak: 1e-12,
        }
    }
}

impl SimOptions {
    pub(crate) fn from_netlist(nl: &Netlist, warnings: &mut Vec<Diagnostic>) -> SimOptions {
        let mut o = SimOptions::default();
        for (k, v, span) in &nl.options {
            let set = |dst: &mut f64| {
                if let Some(v) = v {
                    *dst = *v;
                }
            };
            match k.as_str() {
                "reltol" => set(&mut o.reltol),
                "abstol" => set(&mut o.abstol),
                "vntol" => set(&mut o.vntol),
                "chgtol" => set(&mut o.chgtol),
                "trtol" => set(&mut o.trtol),
                "gmin" => set(&mut o.gmin),
                "tnom" => set(&mut o.tnom),
                "temp" => set(&mut o.temp),
                "defl" => set(&mut o.defl),
                "defw" => set(&mut o.defw),
                "itl1" => o.itl1 = v.map(|v| v as usize).unwrap_or(o.itl1),
                "itl2" => o.itl2 = v.map(|v| v as usize).unwrap_or(o.itl2),
                "itl4" => o.itl4 = v.map(|v| v as usize).unwrap_or(o.itl4),
                "gminsteps" => o.gmin_steps = v.map(|v| v as usize).unwrap_or(o.gmin_steps),
                "srcsteps" => o.src_steps = v.map(|v| v as usize).unwrap_or(o.src_steps),
                "tmax" | "maxstep" => o.tmax = *v,
                "noopiter" => o.noopiter = true,
                "method=trap" | "method=trapezoidal" => o.method = Method::Trap,
                "method=gear" | "method=gear2" => o.method = Method::Gear2,
                "method=euler" | "method=be" => o.method = Method::BackwardEuler,
                "method" => {}
                "maxord" => {
                    if v == &Some(1.0) {
                        o.method = Method::BackwardEuler;
                    }
                }
                k if k.starts_with("method=") => {
                    warnings.push(Diagnostic::warning(
                        span.clone(),
                        format!("unknown integration method `{}`", &k[7..]),
                    ));
                }
                // accepted, no effect
                "post" | "nopage" | "nomod" | "noacct" | "acct" | "list" | "node" | "opts"
                | "numdgt" | "savecurrents" | "filetype=ascii" | "filetype=binary" | "interp"
                | "klu" | "sparse" | "rshunt" | "cshunt" | "pivtol" | "pivrel" | "itl3"
                | "itl5" | "itl6" | "trytocompact" | "xmu" | "warn" | "scale" | "keepopinfo"
                | "seed" => {}
                _ => warnings.push(Diagnostic::warning(
                    span.clone(),
                    format!("unknown option `{k}` ignored"),
                )),
            }
        }
        if let Some(&t) = nl.temps.first() {
            o.temp = t;
        }
        o
    }
}

/// A flat, elaborated circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct Circuit {
    pub title: String,
    /// Node names; index 0 is ground (`"0"`).
    pub nodes: Vec<String>,
    pub elements: Vec<Element>,
    pub options: SimOptions,
    pub ics: Vec<(usize, f64)>,
    pub nodesets: Vec<(usize, f64)>,
    pub warnings: Vec<Diagnostic>,
    node_index: HashMap<String, usize>,
}

impl Circuit {
    pub fn node(&self, name: &str) -> Option<usize> {
        let l = name.to_ascii_lowercase();
        if l == "gnd" {
            return Some(0);
        }
        self.node_index.get(&l).copied()
    }
    pub fn element(&self, name: &str) -> Option<&Element> {
        let l = name.to_ascii_lowercase();
        self.elements.iter().find(|e| e.name == l)
    }
}

// ---------------------------------------------------------------------------------------------
// Monte Carlo random source (xoshiro256**; stable across versions by construction)
// ---------------------------------------------------------------------------------------------

/// Deterministic PRNG for Monte Carlo (xoshiro256** seeded through SplitMix64).
#[derive(Debug, Clone)]
pub struct McRng {
    s: [u64; 4],
    spare: Option<f64>,
}

impl McRng {
    pub fn new(seed: u64) -> Self {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E3779B97F4A7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
            x ^ (x >> 31)
        };
        McRng {
            s: [next(), next(), next(), next()],
            spare: None,
        }
    }
    pub fn next_u64(&mut self) -> u64 {
        let r = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        r
    }
    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        loop {
            let u = 2.0 * self.uniform() - 1.0;
            let v = 2.0 * self.uniform() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare = Some(v * m);
                return u * m;
            }
        }
    }
    /// Relative deviation for a tolerance under a distribution.
    pub fn deviation(&mut self, tol: f64, dist: Dist) -> f64 {
        match dist {
            Dist::Uniform => tol * (2.0 * self.uniform() - 1.0),
            Dist::Gauss => (tol / 3.0 * self.normal()).clamp(-tol, tol),
        }
    }
}

/// Monte Carlo context for one elaboration.
#[derive(Debug, Clone)]
pub struct McCtx {
    pub rng: McRng,
    pub tol: Option<f64>,
    pub dist: Dist,
}

/// Elaboration inputs beyond the netlist.
#[derive(Debug, Clone, Default)]
pub struct ElabOptions {
    /// Parameter overrides (`.step param`, `.dc param`).
    pub params: HashMap<String, f64>,
    /// Element value overrides (`.step R1 …`): flattened element name → value.
    pub values: HashMap<String, f64>,
    /// Temperature override (°C).
    pub temp: Option<f64>,
    pub mc: Option<McCtx>,
}

// ---------------------------------------------------------------------------------------------
// Function inlining
// ---------------------------------------------------------------------------------------------

pub(crate) fn inline_funcs(
    e: &Expr,
    funcs: &HashMap<String, FuncDef>,
    depth: usize,
) -> Result<Expr, String> {
    if depth > 32 {
        return Err("function definitions are recursive".into());
    }
    e.rewrite(&mut |n| {
        if let Expr::UserCall(name, args) = n
            && let Some(f) = funcs.get(name)
        {
            if f.args.len() != args.len() {
                return Err(format!(
                    "function `{name}` expects {} argument(s), got {}",
                    f.args.len(),
                    args.len()
                ));
            }
            let body = f.body.rewrite(&mut |b| {
                Ok(match b {
                    Expr::Ident(id) => f.args.iter().position(|a| a == id).map(|i| args[i].clone()),
                    _ => None,
                })
            })?;
            return inline_funcs(&body, funcs, depth + 1).map(Some);
        }
        Ok(None)
    })
}

// ---------------------------------------------------------------------------------------------
// Elaborator
// ---------------------------------------------------------------------------------------------

/// Parameter/function environment of one scope instance.
#[derive(Clone)]
struct Env<'a> {
    params: HashMap<String, f64>,
    funcs: HashMap<String, FuncDef>,
    parent: Option<&'a Env<'a>>,
}

impl<'a> Env<'a> {
    fn lookup(&self, n: &str) -> Option<f64> {
        self.params
            .get(n)
            .copied()
            .or_else(|| self.parent.and_then(|p| p.lookup(n)))
    }
    fn all_funcs(&self) -> HashMap<String, FuncDef> {
        let mut m = self.parent.map(|p| p.all_funcs()).unwrap_or_default();
        for (k, v) in &self.funcs {
            m.insert(k.clone(), v.clone());
        }
        m
    }
}

/// Scope chain for model/subckt lookup.
struct ScopeChain<'a> {
    scope: &'a Scope,
    parent: Option<&'a ScopeChain<'a>>,
    /// Env for models defined in this scope.
    env: Env<'a>,
    /// Instance prefix of this scope (`x1.`), used to cache per-instance models.
    prefix: String,
}

struct Elab<'n> {
    nl: &'n Netlist,
    opts: ElabOptions,
    elements: Vec<Element>,
    node_names: Vec<String>,
    node_index: HashMap<String, usize>,
    warnings: Vec<Diagnostic>,
    model_cache: HashMap<String, Arc<ModelInst>>,
    globals: Vec<String>,
}

/// Elaborates a netlist.
pub fn elaborate(nl: &Netlist, opts: ElabOptions) -> Result<Circuit, SimError> {
    let mut warnings = nl.warnings.clone();
    let mut options = SimOptions::from_netlist(nl, &mut warnings);
    if let Some(t) = opts.temp {
        options.temp = t;
    }
    let mut el = Elab {
        nl,
        opts,
        elements: Vec::new(),
        node_names: vec!["0".to_string()],
        node_index: HashMap::from([("0".to_string(), 0)]),
        warnings: Vec::new(),
        model_cache: HashMap::new(),
        globals: nl.globals.clone(),
    };
    // Global env: library params, then top-level params, with overrides.
    let mut funcs = nl.library_funcs();
    for f in &nl.top.funcs {
        funcs.insert(f.name.clone(), f.clone());
    }
    let mut genv = Env {
        params: HashMap::new(),
        funcs,
        parent: None,
    };
    let mut defs: Vec<(String, Expr, Span)> = nl.library_params();
    defs.extend(nl.top.params.iter().cloned());
    el.eval_params(&mut genv, &defs)?;
    let chain = ScopeChain {
        scope: &nl.top,
        parent: None,
        env: genv.clone(),
        prefix: String::new(),
    };
    el.instantiate(&nl.top, &chain, &genv, "", &HashMap::new(), 0)?;

    let mut ics = Vec::new();
    let mut nodesets = Vec::new();
    for (list, out, what) in [
        (&nl.ics, &mut ics, ".ic"),
        (&nl.nodesets, &mut nodesets, ".nodeset"),
    ] {
        for (node, e, span) in list.iter() {
            let v = el.const_expr(e, &genv, span, "value")?;
            match el.node_index.get(node) {
                Some(&i) => out.push((i, v)),
                None => el.warnings.push(Diagnostic::warning(
                    span.clone(),
                    format!("{what}: unknown node `{node}`"),
                )),
            }
        }
    }
    warnings.extend(el.warnings);
    Ok(Circuit {
        title: nl.title.clone(),
        nodes: el.node_names,
        elements: el.elements,
        options,
        ics,
        nodesets,
        warnings,
        node_index: el.node_index,
    })
}

impl<'n> Elab<'n> {
    fn eval_params(
        &mut self,
        env: &mut Env,
        defs: &[(String, Expr, Span)],
    ) -> Result<(), SimError> {
        // Evaluate in order; allow forward references by retrying unresolved ones.
        let mut pending: Vec<&(String, Expr, Span)> = Vec::new();
        for d in defs {
            if let Some(v) = self.opts.params.get(&d.0)
                && env.parent.is_none()
            {
                env.params.insert(d.0.clone(), *v);
                continue;
            }
            pending.push(d);
        }
        // overrides for params not defined in the netlist still apply at top level
        if env.parent.is_none() {
            for (k, v) in &self.opts.params {
                env.params.insert(k.clone(), *v);
            }
        }
        for _round in 0..8 {
            let mut next = Vec::new();
            let mut progressed = false;
            for d in pending {
                if env.parent.is_none() && self.opts.params.contains_key(&d.0) {
                    continue;
                }
                match self.try_const(&d.1, env) {
                    Ok(v) => {
                        env.params.insert(d.0.clone(), v);
                        progressed = true;
                    }
                    Err(_) => next.push(d),
                }
            }
            pending = next;
            if pending.is_empty() || !progressed {
                break;
            }
        }
        if let Some(d) = pending.first() {
            let msg = self.try_const(&d.1, env).err().unwrap_or_default();
            return Err(SimError::elab(&d.2, format!(".param {}: {msg}", d.0)));
        }
        Ok(())
    }

    /// Resolves params/functions/random functions; keeps runtime leaves.
    fn resolve(
        &mut self,
        e: &Expr,
        env: &Env,
        nodes: &dyn Fn(&str) -> String,
        elems: &dyn Fn(&str) -> String,
    ) -> Result<Expr, String> {
        let funcs = env.all_funcs();
        let e = inline_funcs(e, &funcs, 0)?;
        let mc = &mut self.opts.mc;
        let r = e.rewrite(&mut |n| {
            Ok(match n {
                Expr::Ident(id) => match env.lookup(id) {
                    Some(v) => Some(Expr::Num(v)),
                    None => match id.as_str() {
                        "pi" => Some(Expr::Num(std::f64::consts::PI)),
                        "temp" => Some(Expr::Rt(RtVar::Temper)),
                        "e" => Some(Expr::Num(std::f64::consts::E)),
                        "boltz" => Some(Expr::Num(1.380649e-23)),
                        "echarge" => Some(Expr::Num(1.602176634e-19)),
                        _ => {
                            let names: Vec<&str> = env.params.keys().map(|s| s.as_str()).collect();
                            return Err(format!(
                                "undefined parameter `{id}`{}",
                                suggest(id, names)
                            ));
                        }
                    },
                },
                Expr::UserCall(name, _) => {
                    let names: Vec<&str> = funcs.keys().map(|s| s.as_str()).collect();
                    return Err(format!("unknown function `{name}`{}", suggest(name, names)));
                }
                Expr::Volt(a, b) => Some(Expr::Volt(nodes(a), b.as_deref().map(nodes))),
                Expr::Curr(a) => Some(Expr::Curr(elems(a))),
                Expr::Call(f, args) if f.is_random() => {
                    let a: Option<Vec<f64>> = args.iter().map(|x| x.as_num()).collect();
                    let Some(a) = a else {
                        return Err(format!("{}() arguments must be constant", f.name()));
                    };
                    let nom = a[0];
                    let v = match mc {
                        None => nom,
                        Some(mc) => {
                            let sig = |s: Option<&f64>| *s.unwrap_or(&1.0);
                            match f {
                                // agauss(nom, abs_var, sigma): abs_var is `sigma` sigmas
                                Func::Agauss => nom + a[1] / sig(a.get(2)) * mc.rng.normal(),
                                Func::Gauss => nom * (1.0 + a[1] / sig(a.get(2)) * mc.rng.normal()),
                                Func::Aunif => nom + a[1] * (2.0 * mc.rng.uniform() - 1.0),
                                Func::Unif | Func::Mc => {
                                    nom * (1.0 + a[1] * (2.0 * mc.rng.uniform() - 1.0))
                                }
                                _ => nom,
                            }
                        }
                    };
                    Some(Expr::Num(v))
                }
                _ => None,
            })
        })?;
        Ok(r.fold())
    }

    fn try_const(&mut self, e: &Expr, env: &Env) -> Result<f64, String> {
        let id = |s: &str| s.to_string();
        let r = self.resolve(e, env, &id, &id)?;
        match r {
            Expr::Num(v) => Ok(v),
            other => {
                if other.has_probes() {
                    Err("node voltages/currents are not allowed here".into())
                } else if other.has_ext() {
                    Err("ext() is not allowed here".into())
                } else {
                    other.eval_with(&|_| None)
                }
            }
        }
    }

    fn const_expr(
        &mut self,
        e: &Expr,
        env: &Env,
        span: &Span,
        what: &str,
    ) -> Result<f64, SimError> {
        self.try_const(e, env)
            .map_err(|m| SimError::elab(span, format!("{what}: {m}")))
    }

    /// Resolves a value that may depend on `ext()`/temperature.
    fn value(&mut self, e: &Expr, env: &Env, span: &Span, what: &str) -> Result<Value, SimError> {
        let id = |s: &str| s.to_string();
        let r = self
            .resolve(e, env, &id, &id)
            .map_err(|m| SimError::elab(span, format!("{what}: {m}")))?;
        match r {
            Expr::Num(v) => Ok(Value::Const(v)),
            other if other.has_probes() => Err(SimError::elab(
                span,
                format!("{what}: node voltages/currents are not allowed here (use a B source)"),
            )),
            other if other.is_runtime() => Ok(Value::Dyn(Box::new(other))),
            other => other
                .eval_with(&|_| None)
                .map(Value::Const)
                .map_err(|m| SimError::elab(span, format!("{what}: {m}"))),
        }
    }

    fn node_id(&mut self, name: &str) -> usize {
        if let Some(&i) = self.node_index.get(name) {
            return i;
        }
        let i = self.node_names.len();
        self.node_names.push(name.to_string());
        self.node_index.insert(name.to_string(), i);
        i
    }

    fn find_model<'c>(
        &mut self,
        name: &str,
        chain: &'c ScopeChain<'c>,
        span: &Span,
    ) -> Result<Arc<ModelInst>, SimError> {
        // innermost scope first
        let mut c = Some(chain);
        while let Some(sc) = c {
            if let Some(m) = sc.scope.find_model(name) {
                let key = format!("{}|{name}", sc.prefix);
                if let Some(m) = self.model_cache.get(&key) {
                    return Ok(m.clone());
                }
                let mut params = Vec::new();
                for (k, e) in &m.params {
                    let v = self.value(
                        e,
                        &sc.env,
                        &m.span,
                        &format!("model {} parameter {k}", m.name),
                    )?;
                    params.push((k.clone(), v));
                }
                let mi = Arc::new(ModelInst {
                    name: m.name.clone(),
                    kind: m.kind,
                    params,
                    span: m.span.clone(),
                });
                self.model_cache.insert(key, mi.clone());
                return Ok(mi);
            }
            c = sc.parent;
        }
        // libraries
        if let Some(m) = self
            .nl
            .libraries
            .iter()
            .rev()
            .find_map(|l| l.scope.find_model(name))
        {
            let key = format!("lib|{name}");
            if let Some(m) = self.model_cache.get(&key) {
                return Ok(m.clone());
            }
            let genv = Env {
                params: HashMap::new(),
                funcs: self.nl.library_funcs(),
                parent: None,
            };
            let mut params = Vec::new();
            for (k, e) in &m.params {
                let v = self.value(
                    e,
                    &genv,
                    &m.span,
                    &format!("model {} parameter {k}", m.name),
                )?;
                params.push((k.clone(), v));
            }
            let mi = Arc::new(ModelInst {
                name: m.name.clone(),
                kind: m.kind,
                params,
                span: m.span.clone(),
            });
            self.model_cache.insert(key, mi.clone());
            return Ok(mi);
        }
        let mut names = self.nl.model_names();
        let mut c = Some(chain);
        while let Some(sc) = c {
            names.extend(sc.scope.models.iter().map(|m| m.name.clone()));
            c = sc.parent;
        }
        Err(SimError::elab(
            span,
            format!(
                "unknown model `{name}`{}",
                suggest(name, names.iter().map(|s| s.as_str()))
            ),
        ))
    }

    fn find_subckt<'c>(&self, name: &str, chain: &'c ScopeChain<'c>) -> Option<Arc<Scope>> {
        let mut c = Some(chain);
        while let Some(sc) = c {
            if let Some(s) = sc.scope.find_subckt(name) {
                return Some(s.clone());
            }
            c = sc.parent;
        }
        self.nl.lookup_global_subckt(name)
    }

    #[allow(clippy::too_many_arguments)]
    fn instantiate<'c>(
        &mut self,
        scope: &Scope,
        chain: &'c ScopeChain<'c>,
        env: &Env,
        prefix: &str,
        port_map: &HashMap<String, String>,
        depth: usize,
    ) -> Result<(), SimError> {
        let local_elems: Vec<String> = scope.elements.iter().map(|e| e.name.clone()).collect();
        let globals = self.globals.clone();
        let map_node = |n: &str| -> String {
            if n == "0" || n == "gnd" {
                return "0".into();
            }
            if let Some(p) = port_map.get(n) {
                return p.clone();
            }
            if prefix.is_empty() || globals.iter().any(|g| g == n) {
                return n.to_string();
            }
            format!("{prefix}{n}")
        };
        let map_elem = |n: &str| -> String {
            if !prefix.is_empty() && local_elems.iter().any(|e| e == n) {
                format!("{prefix}{n}")
            } else {
                n.to_string()
            }
        };
        for card in &scope.elements {
            let name = format!("{prefix}{}", card.name);
            let span = &card.span;
            let nodes_s: Vec<String> = card.nodes.iter().map(|n| map_node(n)).collect();
            let p = |k: &str| card.param(k);
            macro_rules! val {
                ($e:expr, $what:expr) => {
                    self.value($e, env, span, &format!("{} {}", card.name, $what))?
                };
            }
            macro_rules! cst {
                ($e:expr, $what:expr) => {
                    self.const_expr($e, env, span, &format!("{} {}", card.name, $what))?
                };
            }
            let inst_params = |this: &mut Self, skip: &[&str]| -> Result<InstParams, SimError> {
                let mut out = Vec::new();
                for (k, e) in &card.params {
                    if skip.contains(&k.as_str()) {
                        continue;
                    }
                    let v = this.value(e, env, span, &format!("{} {k}", card.name))?;
                    out.push((k.clone(), v));
                }
                Ok(out)
            };
            let kind = match &card.kind {
                ElemKind::Subckt {
                    name: sname,
                    text_params,
                } => {
                    if depth >= 64 {
                        return Err(SimError::elab(
                            span,
                            "subcircuit nesting too deep (recursive definition?)",
                        ));
                    }
                    let Some(sub) = self.find_subckt(sname, chain) else {
                        let mut names = self.nl.subckt_names();
                        names.extend(scope.subckts.iter().map(|s| s.name.clone()));
                        return Err(SimError::elab(
                            span,
                            format!(
                                "unknown subcircuit `{sname}` used by `{}`{}; define it with .subckt, .include it, or register a library (Netlist::add_library)",
                                card.name,
                                suggest(sname, names.iter().map(|s| s.as_str()))
                            ),
                        ));
                    };
                    if let Some((k, v)) = text_params.first() {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}`: parameter `{k}={v}` is not a numeric expression",
                                card.name
                            ),
                        ));
                    }
                    if sub.ports.len() != card.nodes.len() {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}` connects {} nodes but subcircuit `{sname}` has {} ports",
                                card.name,
                                card.nodes.len(),
                                sub.ports.len()
                            ),
                        ));
                    }
                    // instance env: defaults overridden by instance params (evaluated in parent env)
                    let mut ienv = Env {
                        params: HashMap::new(),
                        funcs: HashMap::new(),
                        parent: Some(env),
                    };
                    for f in &sub.funcs {
                        ienv.funcs.insert(f.name.clone(), f.clone());
                    }
                    for (k, e) in &sub.defaults {
                        if card.param(k).is_none() {
                            let v = self.const_expr(
                                e,
                                &ienv,
                                &sub.span,
                                &format!("subcircuit {sname} parameter {k}"),
                            )?;
                            ienv.params.insert(k.clone(), v);
                        }
                    }
                    for (k, e) in &card.params {
                        if !sub.defaults.iter().any(|(d, _)| d == k) {
                            self.warnings.push(Diagnostic::warning(
                                span.clone(),
                                format!(
                                    "`{}`: subcircuit `{sname}` has no parameter `{k}`",
                                    card.name
                                ),
                            ));
                        }
                        let v = cst!(e, k);
                        ienv.params.insert(k.clone(), v);
                    }
                    self.eval_params(&mut ienv, &sub.params)?;
                    let pm: HashMap<String, String> = sub
                        .ports
                        .iter()
                        .cloned()
                        .zip(nodes_s.iter().cloned())
                        .collect();
                    let new_prefix = format!("{name}.");
                    let sub_chain = ScopeChain {
                        scope: &sub,
                        parent: Some(chain),
                        env: ienv.clone(),
                        prefix: new_prefix.clone(),
                    };
                    self.instantiate(&sub, &sub_chain, &ienv, &new_prefix, &pm, depth + 1)?;
                    continue;
                }
                ElemKind::Resistor { value, model }
                | ElemKind::Capacitor { value, model }
                | ElemKind::Inductor { value, model } => {
                    let letter = card.name.chars().next().unwrap_or('r');
                    // A lone identifier may be a parameter reference instead of a model.
                    let (value, model) = match (value, model) {
                        (None, Some(m))
                            if env.lookup(m).is_some()
                                || self.nl.lookup_global_model(m).is_none()
                                    && !chain.scope.models.iter().any(|x| &x.name == m) =>
                        {
                            (Some(Expr::Ident(m.clone())), None)
                        }
                        (v, m) => (v.clone(), m.clone()),
                    };
                    let model = match &model {
                        Some(m) => Some(self.find_model(m, chain, span)?),
                        None => None,
                    };
                    let Some(value) = value else {
                        return Err(SimError::elab(
                            span,
                            format!("`{}` needs a value", card.name),
                        ));
                    };
                    let funcs = env.all_funcs();
                    let resolved =
                        inline_funcs(&value, &funcs, 0).map_err(|m| SimError::elab(span, m))?;
                    let resolved = self
                        .resolve(&resolved, env, &|n| map_node(n), &|n| map_elem(n))
                        .map_err(|m| SimError::elab(span, format!("{}: {m}", card.name)))?;
                    let mut params = inst_params(self, &["tol"])?;
                    let mut v = if resolved.has_probes()
                        || (letter == 'r' && resolved_uses_time(&resolved))
                    {
                        if letter != 'r' {
                            return Err(SimError::elab(
                                span,
                                format!(
                                    "`{}`: value depends on node voltages/currents; use a B source",
                                    card.name
                                ),
                            ));
                        }
                        // behavioural resistor → B source I = V(a,b)/R
                        let vab = Expr::Volt(nodes_s[0].clone(), Some(nodes_s[1].clone()));
                        let expr = Expr::Binary(
                            crate::expr::BinOp::Div,
                            Box::new(vab),
                            Box::new(resolved),
                        );
                        self.push(
                            name,
                            span,
                            &nodes_s[..2],
                            ElementKind::Behavioral {
                                is_current: true,
                                expr,
                            },
                        );
                        continue;
                    } else if resolved.is_runtime() {
                        Value::Dyn(Box::new(resolved))
                    } else {
                        Value::Const(
                            resolved
                                .eval_with(&|_| None)
                                .map_err(|m| SimError::elab(span, m))?,
                        )
                    };
                    if let Some(ov) = self.opts.values.get(&name) {
                        v = Value::Const(*ov);
                    }
                    v = self.apply_tolerance(v, card, env, span, true)?;
                    params.retain(|(k, _)| k != "tol");
                    match letter {
                        'r' => ElementKind::Resistor {
                            r: v,
                            params,
                            model,
                        },
                        'c' => ElementKind::Capacitor {
                            c: v,
                            params,
                            model,
                        },
                        _ => ElementKind::Inductor {
                            l: v,
                            params,
                            model,
                        },
                    }
                }
                ElemKind::Coupling { inductors, k } => {
                    let kv = cst!(k, "coupling");
                    if kv.is_nan() || kv.abs() > 1.0 {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}`: coupling coefficient must be within [-1, 1]",
                                card.name
                            ),
                        ));
                    }
                    let names: Vec<String> = inductors.iter().map(|l| map_elem(l)).collect();
                    // pairwise couplings
                    for i in 0..names.len() {
                        for j in i + 1..names.len() {
                            let nm = if names.len() == 2 {
                                name.clone()
                            } else {
                                format!("{name}#{i}{j}")
                            };
                            self.elements.push(Element {
                                name: nm,
                                span: span.clone(),
                                nodes: vec![],
                                kind: ElementKind::Coupling {
                                    l1: names[i].clone(),
                                    l2: names[j].clone(),
                                    k: kv,
                                },
                            });
                        }
                    }
                    continue;
                }
                ElemKind::Vsource(s) | ElemKind::Isource(s) => {
                    let src = self.source(s, env, span, &card.name)?;
                    let mut src = src;
                    if let Some(ov) = self.opts.values.get(&name) {
                        src.dc = Value::Const(*ov);
                    }
                    if matches!(card.kind, ElemKind::Vsource(_)) {
                        ElementKind::Vsource(src)
                    } else {
                        ElementKind::Isource(src)
                    }
                }
                ElemKind::Vcvs(c) | ElemKind::Vccs(c) | ElemKind::Cccs(c) | ElemKind::Ccvs(c) => {
                    let is_e = matches!(card.kind, ElemKind::Vcvs(_));
                    let is_g = matches!(card.kind, ElemKind::Vccs(_));
                    let is_f = matches!(card.kind, ElemKind::Cccs(_));
                    let voltage_out = is_e || matches!(card.kind, ElemKind::Ccvs(_));
                    match c {
                        Control::Linear {
                            ctrl_nodes,
                            ctrl_src,
                            gain,
                        } => {
                            let g = val!(gain, "gain");
                            if is_e || is_g {
                                let mut ns = nodes_s.clone();
                                ns.extend(ctrl_nodes.iter().map(|n| map_node(n)));
                                let kind = if is_e {
                                    ElementKind::Vcvs { gain: g }
                                } else {
                                    ElementKind::Vccs { gm: g }
                                };
                                self.push(name, span, &ns, kind);
                                continue;
                            }
                            let ctrl = map_elem(ctrl_src.as_deref().unwrap_or(""));
                            if is_f {
                                ElementKind::Cccs { ctrl, gain: g }
                            } else {
                                ElementKind::Ccvs { ctrl, r: g }
                            }
                        }
                        Control::Poly {
                            dims,
                            ctrl_nodes,
                            ctrl_srcs,
                            coeffs,
                        } => {
                            let mut vars: Vec<Expr> = Vec::new();
                            if is_e || is_g {
                                for k in 0..*dims {
                                    let a = map_node(&ctrl_nodes[2 * k]);
                                    let b = map_node(&ctrl_nodes[2 * k + 1]);
                                    vars.push(Expr::Volt(a, Some(b)));
                                }
                            } else {
                                for s in ctrl_srcs {
                                    vars.push(Expr::Curr(map_elem(s)));
                                }
                            }
                            let cs: Vec<f64> = coeffs
                                .iter()
                                .map(|c| self.const_expr(c, env, span, "POLY coefficient"))
                                .collect::<Result<_, _>>()?;
                            let expr = poly_expr(&vars, &cs);
                            ElementKind::Behavioral {
                                is_current: !voltage_out,
                                expr,
                            }
                        }
                        Control::Value(e) => {
                            let expr = self
                                .resolve(e, env, &|n| map_node(n), &|n| map_elem(n))
                                .map_err(|m| SimError::elab(span, format!("{}: {m}", card.name)))?;
                            ElementKind::Behavioral {
                                is_current: !voltage_out,
                                expr,
                            }
                        }
                        Control::Table { input, points } => {
                            let inp = self
                                .resolve(input, env, &|n| map_node(n), &|n| map_elem(n))
                                .map_err(|m| SimError::elab(span, format!("{}: {m}", card.name)))?;
                            let mut args = vec![inp];
                            for (x, y) in points {
                                args.push(Expr::Num(cst!(x, "TABLE x")));
                                args.push(Expr::Num(cst!(y, "TABLE y")));
                            }
                            ElementKind::Behavioral {
                                is_current: !voltage_out,
                                expr: Expr::Call(Func::Table, args),
                            }
                        }
                    }
                }
                ElemKind::Behavioral { is_current, expr } => {
                    let expr = self
                        .resolve(expr, env, &|n| map_node(n), &|n| map_elem(n))
                        .map_err(|m| SimError::elab(span, format!("{}: {m}", card.name)))?;
                    ElementKind::Behavioral {
                        is_current: *is_current,
                        expr,
                    }
                }
                ElemKind::Switch { model, init_on } => {
                    let m = self.find_model(model, chain, span)?;
                    if m.kind != ModelKind::Sw {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}` needs an SW model, `{}` is {}",
                                card.name,
                                m.name,
                                m.kind.keyword()
                            ),
                        ));
                    }
                    ElementKind::Switch {
                        model: m,
                        init_on: *init_on,
                    }
                }
                ElemKind::CSwitch {
                    ctrl,
                    model,
                    init_on,
                } => {
                    let m = self.find_model(model, chain, span)?;
                    if m.kind != ModelKind::Csw {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}` needs a CSW model, `{}` is {}",
                                card.name,
                                m.name,
                                m.kind.keyword()
                            ),
                        ));
                    }
                    ElementKind::CSwitch {
                        ctrl: map_elem(ctrl),
                        model: m,
                        init_on: *init_on,
                    }
                }
                ElemKind::Diode { model, area, off }
                | ElemKind::Jfet { model, area, off }
                | ElemKind::Bjt { model, area, off } => {
                    let m = self.find_model(model, chain, span)?;
                    let mut params = inst_params(self, &[])?;
                    if let Some(a) = area {
                        let v = val!(a, "area");
                        params.push(("area".into(), v));
                    }
                    let (ok, want) = match &card.kind {
                        ElemKind::Diode { .. } => (m.kind == ModelKind::D, "a D"),
                        ElemKind::Jfet { .. } => (
                            matches!(m.kind, ModelKind::Njf | ModelKind::Pjf),
                            "an NJF/PJF",
                        ),
                        _ => (
                            matches!(m.kind, ModelKind::Npn | ModelKind::Pnp),
                            "an NPN/PNP",
                        ),
                    };
                    if !ok {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}` needs {want} model, `{}` is {}",
                                card.name,
                                m.name,
                                m.kind.keyword()
                            ),
                        ));
                    }
                    if let Some(tol) = card.param("tol") {
                        let _ = tol; // tolerances on semiconductors apply to area
                    }
                    match &card.kind {
                        ElemKind::Diode { .. } => ElementKind::Diode {
                            model: m,
                            params,
                            off: *off,
                        },
                        ElemKind::Jfet { .. } => ElementKind::Jfet {
                            model: m,
                            params,
                            off: *off,
                        },
                        _ => ElementKind::Bjt {
                            model: m,
                            params,
                            off: *off,
                        },
                    }
                }
                ElemKind::Mosfet { model, off } => {
                    let m = self.find_model(model, chain, span)?;
                    if !matches!(m.kind, ModelKind::Nmos | ModelKind::Pmos) {
                        return Err(SimError::elab(
                            span,
                            format!(
                                "`{}` needs an NMOS/PMOS model, `{}` is {}",
                                card.name,
                                m.name,
                                m.kind.keyword()
                            ),
                        ));
                    }
                    if let Some(l) = m.get("level").and_then(|v| v.constant())
                        && l != 1.0
                    {
                        self.warnings.push(Diagnostic::warning(
                                span.clone(),
                                format!("`{}`: MOSFET level {l} is simulated with the level-1 (Shichman-Hodges) model", card.name),
                            ));
                    }
                    let params = inst_params(self, &[])?;
                    ElementKind::Mosfet {
                        model: m,
                        params,
                        off: *off,
                    }
                }
                ElemKind::Tline => {
                    let z0 = p("z0")
                        .or(p("zo"))
                        .map(|e| self.const_expr(e, env, span, "Z0"))
                        .transpose()?
                        .unwrap_or(50.0);
                    let td = if let Some(td) = p("td") {
                        cst!(td, "TD")
                    } else {
                        let f = cst!(p("f").unwrap(), "F");
                        let nl = p("nl")
                            .map(|e| self.const_expr(e, env, span, "NL"))
                            .transpose()?
                            .unwrap_or(0.25);
                        nl / f
                    };
                    if td.is_nan() || z0.is_nan() || td <= 0.0 || z0 <= 0.0 {
                        return Err(SimError::elab(
                            span,
                            format!("`{}`: Z0 and TD must be positive", card.name),
                        ));
                    }
                    let mut ic = [0.0; 4];
                    for (i, k) in ["ic", "ic.1", "ic.2", "ic.3"].iter().enumerate() {
                        if let Some(e) = p(k) {
                            ic[i] = cst!(e, "IC");
                        }
                    }
                    ElementKind::Tline { z0, td, ic }
                }
                ElemKind::Port { drive } => ElementKind::Port { drive: *drive },
            };
            self.push(name, span, &nodes_s, kind);
        }
        Ok(())
    }

    fn push(&mut self, name: String, span: &Span, nodes: &[String], kind: ElementKind) {
        let ids: Vec<usize> = nodes.iter().map(|n| self.node_id(n)).collect();
        self.elements.push(Element {
            name,
            span: span.clone(),
            nodes: ids,
            kind,
        });
    }

    fn apply_tolerance(
        &mut self,
        v: Value,
        card: &ElementCard,
        env: &Env,
        span: &Span,
        passive: bool,
    ) -> Result<Value, SimError> {
        let Some(mc) = self.opts.mc.as_ref() else {
            return Ok(v);
        };
        let tol = match card.param("tol") {
            Some(e) => Some(self.const_expr(e, env, span, "tol")?),
            None if passive => mc.tol,
            None => None,
        };
        let Some(tol) = tol else { return Ok(v) };
        let mc = self.opts.mc.as_mut().unwrap();
        let dev = mc.rng.deviation(tol, mc.dist);
        Ok(v.scale(1.0 + dev))
    }

    fn source(
        &mut self,
        s: &SourceSpec,
        env: &Env,
        span: &Span,
        name: &str,
    ) -> Result<Source, SimError> {
        let dc = match &s.dc {
            Some(e) => self.value(e, env, span, &format!("{name} value"))?,
            None => Value::Const(0.0),
        };
        let ac_mag = match &s.ac_mag {
            Some(e) => self.const_expr(e, env, span, "AC magnitude")?,
            None => 0.0,
        };
        let ac_phase_deg = match &s.ac_phase {
            Some(e) => self.const_expr(e, env, span, "AC phase")?,
            None => 0.0,
        };
        let wave = match &s.wave {
            None => None,
            Some(w) => {
                let args = |this: &mut Self, a: &[Expr]| -> Result<Vec<f64>, SimError> {
                    a.iter()
                        .map(|e| this.const_expr(e, env, span, &format!("{name} {}", w.keyword())))
                        .collect()
                };
                Some(match w {
                    Waveform::Pulse(a) => {
                        let a = args(self, a)?;
                        let g = |i: usize| a.get(i).copied();
                        Wave::Pulse {
                            v1: a[0],
                            v2: a[1],
                            td: g(2).unwrap_or(0.0),
                            tr: g(3),
                            tf: g(4),
                            pw: g(5),
                            per: g(6),
                            np: g(7),
                        }
                    }
                    Waveform::Sin(a) => {
                        let a = args(self, a)?;
                        let g = |i: usize| a.get(i).copied();
                        Wave::Sin {
                            vo: a[0],
                            va: a[1],
                            freq: g(2),
                            td: g(3).unwrap_or(0.0),
                            theta: g(4).unwrap_or(0.0),
                            phase: g(5).unwrap_or(0.0),
                        }
                    }
                    Waveform::Exp(a) => {
                        let a = args(self, a)?;
                        let g = |i: usize| a.get(i).copied();
                        Wave::Exp {
                            v1: a[0],
                            v2: a[1],
                            td1: g(2).unwrap_or(0.0),
                            tau1: g(3),
                            td2: g(4),
                            tau2: g(5),
                        }
                    }
                    Waveform::Sffm(a) => {
                        let a = args(self, a)?;
                        let g = |i: usize| a.get(i).copied();
                        Wave::Sffm {
                            vo: a[0],
                            va: a[1],
                            fc: g(2),
                            mdi: g(3).unwrap_or(0.0),
                            fs: g(4),
                        }
                    }
                    Waveform::Am(a) => {
                        let a = args(self, a)?;
                        let g = |i: usize| a.get(i).copied();
                        Wave::Am {
                            va: a[0],
                            vo: a[1],
                            mf: g(2).unwrap_or(0.0),
                            fc: g(3).unwrap_or(0.0),
                            td: g(4).unwrap_or(0.0),
                        }
                    }
                    Waveform::Pwl {
                        points,
                        repeat,
                        delay,
                    } => {
                        let a = args(self, points)?;
                        let pts: Vec<(f64, f64)> = a.chunks(2).map(|c| (c[0], c[1])).collect();
                        for w in pts.windows(2) {
                            if w[1].0 < w[0].0 {
                                return Err(SimError::elab(
                                    span,
                                    format!("{name} PWL: time values must not decrease"),
                                ));
                            }
                        }
                        let repeat = repeat
                            .as_ref()
                            .map(|e| self.const_expr(e, env, span, "PWL r"))
                            .transpose()?;
                        let delay = delay
                            .as_ref()
                            .map(|e| self.const_expr(e, env, span, "PWL td"))
                            .transpose()?
                            .unwrap_or(0.0);
                        Wave::Pwl { pts, repeat, delay }
                    }
                })
            }
        };
        // a waveform's t=0 value is the DC value when no DC value is given
        Ok(Source {
            dc,
            dc_given: s.dc.is_some(),
            ac_mag,
            ac_phase_deg,
            wave,
        })
    }
}

fn resolved_uses_time(e: &Expr) -> bool {
    let mut t = false;
    e.visit(&mut |n| {
        if matches!(n, Expr::Rt(RtVar::Time)) {
            t = true;
        }
    });
    t
}

/// SPICE2 `POLY(n)` polynomial as an expression. Terms are ordered by total degree, then
/// lexicographically descending exponent vectors (x1², x1·x2, x2², …).
pub(crate) fn poly_expr(vars: &[Expr], coeffs: &[f64]) -> Expr {
    use crate::expr::BinOp::*;
    let n = vars.len();
    // Single coefficient for POLY(1) means the linear gain (SPICE2 convention).
    if n == 1 && coeffs.len() == 1 {
        return Expr::Binary(
            Mul,
            Box::new(Expr::Num(coeffs[0])),
            Box::new(vars[0].clone()),
        );
    }
    let mut terms: Vec<Vec<u32>> = Vec::new();
    let mut deg = 0u32;
    while terms.len() < coeffs.len() {
        let mut level = Vec::new();
        exps(n, deg, &mut vec![0; n], 0, &mut level);
        level.sort_by(|a, b| b.cmp(a));
        terms.extend(level);
        deg += 1;
        if deg > 16 {
            break;
        }
    }
    let mut sum: Option<Expr> = None;
    for (c, e) in coeffs.iter().zip(terms.iter()) {
        if *c == 0.0 {
            continue;
        }
        let mut t = Expr::Num(*c);
        for (v, &p) in vars.iter().zip(e.iter()) {
            for _ in 0..p {
                t = Expr::Binary(Mul, Box::new(t), Box::new(v.clone()));
            }
        }
        sum = Some(match sum {
            None => t,
            Some(s) => Expr::Binary(Add, Box::new(s), Box::new(t)),
        });
    }
    sum.unwrap_or(Expr::Num(0.0))
}

fn exps(n: usize, deg: u32, cur: &mut Vec<u32>, i: usize, out: &mut Vec<Vec<u32>>) {
    if i == n - 1 {
        cur[i] = deg;
        out.push(cur.clone());
        return;
    }
    for d in 0..=deg {
        cur[i] = d;
        exps(n, deg - d, cur, i + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elab(s: &str) -> Circuit {
        let nl = Netlist::parse(s).unwrap_or_else(|e| panic!("{e}"));
        elaborate(&nl, ElabOptions::default()).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn flattening_names_nodes() {
        let c = elab(
            "t
.subckt div in out params: r=1k
R1 in out {r}
R2 out 0 {r*2}
.ends
X1 a b div r=2k
V1 a 0 1
",
        );
        assert!(c.node("b").is_some());
        let r2 = c.element("x1.r2").unwrap();
        match &r2.kind {
            ElementKind::Resistor { r, .. } => assert_eq!(r, &Value::Const(4000.0)),
            k => panic!("{k:?}"),
        }
        assert_eq!(c.nodes[r2.nodes[0]], "b");
    }

    #[test]
    fn nested_internal_nodes() {
        let c = elab(
            "t
.subckt inner a
R1 a mid 1
R2 mid 0 1
.ends
.subckt outer a
X1 a inner
.ends
X5 n outer
V1 n 0 1
",
        );
        assert!(c.node("x5.x1.mid").is_some(), "{:?}", c.nodes);
        assert!(c.element("x5.x1.r2").is_some());
    }

    #[test]
    fn unknown_subckt_is_clean_error() {
        let nl = Netlist::parse("t\nXJ1 VCC 0 A0 CAPSOIL vwc=42\n").unwrap();
        let e = elaborate(&nl, ElabOptions::default()).unwrap_err();
        let msg = e.to_string();
        assert!(
            msg.contains("unknown subcircuit `capsoil`") && msg.contains("line 2"),
            "{msg}"
        );
        // registering a library resolves it
        let mut nl = nl;
        let lib = Library::parse(".subckt capsoil vcc gnd out params: vwc=30\nR1 vcc out {10k*vwc/100}\nR2 out gnd 10k\n.ends\n").unwrap();
        nl.add_library(Arc::new(lib));
        let c = elaborate(&nl, ElabOptions::default()).unwrap();
        assert!(c.element("xj1.r1").is_some());
    }

    #[test]
    fn unknown_model_suggests() {
        let nl = Netlist::parse("t\nD1 a 0 LED_AMBR\n.model LED_AMBER D(Is=1e-19)\n").unwrap();
        let e = elaborate(&nl, ElabOptions::default())
            .unwrap_err()
            .to_string();
        assert!(e.contains("did you mean `led_amber`"), "{e}");
    }

    #[test]
    fn param_overrides_and_ext() {
        let nl = Netlist::parse("t\n.param lux=180 RL={350k*pwr(lux,-0.7)}\nR1 a 0 {RL}\nR2 a 0 {350k*pwr(max(ext(\"J2.lux\"),0.01),-0.7)}\n").unwrap();
        let c = elaborate(
            &nl,
            ElabOptions {
                params: HashMap::from([("lux".into(), 1.0)]),
                ..Default::default()
            },
        )
        .unwrap();
        match &c.elements[0].kind {
            ElementKind::Resistor { r, .. } => {
                assert!((r.constant().unwrap() - 350e3).abs() < 1e-6)
            }
            k => panic!("{k:?}"),
        }
        match &c.elements[1].kind {
            ElementKind::Resistor {
                r: Value::Dyn(e), ..
            } => {
                let v = eval_dyn(e, &|n| (n == "J2.lux").then_some(1.0), 27.0);
                assert!((v - 350e3).abs() < 1e-6);
            }
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn poly_ordering() {
        let x = Expr::Ident("x".into());
        let y = Expr::Ident("y".into());
        // p0 + p1 x + p2 y + p3 x² + p4 xy + p5 y²
        let e = poly_expr(&[x, y], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let v = e
            .eval_with(&|n| Some(if n == "x" { 2.0 } else { 3.0 }))
            .unwrap();
        assert_eq!(v, 1.0 + 4.0 + 9.0 + 16.0 + 30.0 + 54.0);
    }

    #[test]
    fn mc_tolerance_is_seeded() {
        let nl = Netlist::parse("t\nR1 a 0 10k\nR2 a 0 10k tol=10%\nC1 a 0 1u\n").unwrap();
        let run = |seed| {
            let mc = McCtx {
                rng: McRng::new(seed),
                tol: Some(0.05),
                dist: Dist::Gauss,
            };
            let c = elaborate(
                &nl,
                ElabOptions {
                    mc: Some(mc),
                    ..Default::default()
                },
            )
            .unwrap();
            c.elements
                .iter()
                .map(|e| match &e.kind {
                    ElementKind::Resistor { r, .. } => r.constant().unwrap(),
                    ElementKind::Capacitor { c, .. } => c.constant().unwrap(),
                    _ => 0.0,
                })
                .collect::<Vec<f64>>()
        };
        let a = run(7);
        assert_eq!(a, run(7));
        assert_ne!(a, run(8));
        assert!((a[0] / 10e3 - 1.0).abs() <= 0.05);
        assert!((a[1] / 10e3 - 1.0).abs() <= 0.10);
    }
}
