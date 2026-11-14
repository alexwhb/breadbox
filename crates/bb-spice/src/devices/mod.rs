//! Device models and their MNA stamps.
//!
//! Every element is a [`Device`] variant implementing [`Dev`]: it allocates unknowns/matrix
//! slots/state at setup, stamps its linearised companion model in [`Dev::load`] (DC and
//! transient Newton iterations), its complex admittances in [`Dev::ac_load`], and optionally
//! limits Newton steps, estimates truncation error, reports breakpoints and noise sources.
//! The structure follows SPICE3/ngspice closely so results match ngspice within tolerance.

pub mod bjt;
pub mod bsrc;
pub mod controlled;
pub mod diode;
pub mod jfet;
pub mod mos1;
pub mod passive;
pub mod semi;
pub mod sources;
pub mod switch;
pub mod tline;

use crate::elab::{Method, SimOptions};
use crate::error::SimError;
use crate::matrix::{C64, Matrix, Pattern, Slot};

/// Boltzmann constant (J/K).
pub const KB: f64 = 1.380_649e-23;
/// Elementary charge (C).
pub const Q: f64 = 1.602_176_634e-19;
/// 0 °C in kelvin.
pub const KELVIN: f64 = 273.15;

/// Kind of unknown (for convergence tolerances and names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UKind {
    Node,
    Branch,
}

/// Analysis mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// DC operating point / DC sweep / transient initial operating point.
    Dc,
    /// Transient time point.
    Tran,
}

/// SPICE-style initialisation sub-mode of a Newton iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Init {
    /// First iteration: junctions start at their critical voltage (or 0 if `off`).
    Jct,
    /// `off` devices held at zero until the rest converges.
    Fix,
    /// Normal iteration.
    Float,
    /// First time point after the operating point.
    Tran,
    /// First iteration of a new time point (device-level predictor).
    Pred,
}

/// Circuit-wide solver state seen by devices.
#[derive(Debug, Clone)]
pub struct Ckt {
    pub n: usize,
    pub mat: Matrix<f64>,
    pub rhs: Vec<f64>,
    /// Current Newton iterate (indexed by unknown; `x[0]` = ground = 0).
    pub x: Vec<f64>,
    /// History states: `st[0]` current, `st[1]` previous accepted point, …
    pub st: [Vec<f64>; 4],
    pub mode: Mode,
    pub init: Init,
    /// Solving the transient's initial operating point (applies `.ic` clamps).
    pub tran_op: bool,
    pub uic: bool,
    pub time: f64,
    pub delta: f64,
    /// `delta_old[0]` = current step, `[1]` previous, …
    pub delta_old: [f64; 4],
    pub order: usize,
    pub method: Method,
    /// Integration coefficients for the current step (see [`Ckt::integrate`]).
    pub ag: [f64; 3],
    /// Junction gmin (`.options gmin`).
    pub gmin: f64,
    /// Extra conductance to ground on every node (gmin stepping).
    pub diag_gmin: f64,
    /// Source stepping factor (1 = full sources).
    pub srcfact: f64,
    pub noncon: usize,
    pub noncon_devs: Vec<u32>,
    pub cur_dev: u32,
    pub opts: SimOptions,
    /// Circuit temperature in kelvin.
    pub temp: f64,
    /// External variable values (indexed by the engine's ext table).
    pub ext: Vec<f64>,
    /// Max breakpoint-limited step a device asked for during this load (switches, T-lines).
    pub step_limit: f64,
}

impl Ckt {
    pub fn new(n: usize, pattern: &Pattern, n_states: usize, opts: SimOptions) -> Ckt {
        let temp = opts.temp + KELVIN;
        Ckt {
            n,
            mat: Matrix::new(pattern),
            rhs: vec![0.0; n + 1],
            x: vec![0.0; n + 1],
            st: [
                vec![0.0; n_states],
                vec![0.0; n_states],
                vec![0.0; n_states],
                vec![0.0; n_states],
            ],
            mode: Mode::Dc,
            init: Init::Jct,
            tran_op: false,
            uic: false,
            time: 0.0,
            delta: 0.0,
            delta_old: [0.0; 4],
            order: 1,
            method: opts.method,
            ag: [0.0; 3],
            gmin: opts.gmin,
            diag_gmin: 0.0,
            srcfact: 1.0,
            noncon: 0,
            noncon_devs: Vec::new(),
            cur_dev: 0,
            opts,
            temp,
            ext: Vec::new(),
            step_limit: f64::INFINITY,
        }
    }

    #[inline]
    pub fn add(&mut self, s: Slot, v: f64) {
        self.mat.vals[s.0 as usize] += v;
    }

    #[inline]
    pub fn add_rhs(&mut self, i: usize, v: f64) {
        self.rhs[i] += v;
    }

    /// Node/branch value from the current iterate.
    #[inline]
    pub fn v(&self, i: usize) -> f64 {
        self.x[i]
    }

    /// Records that the calling device has not converged (limiting was active, state changed).
    pub fn nonconverged(&mut self) {
        self.noncon += 1;
        if self.noncon_devs.len() < 16 && !self.noncon_devs.contains(&self.cur_dev) {
            self.noncon_devs.push(self.cur_dev);
        }
    }

    pub fn is_dc(&self) -> bool {
        self.mode == Mode::Dc
    }
    pub fn is_tran(&self) -> bool {
        self.mode == Mode::Tran
    }

    /// Thermal voltage kT/q at the circuit temperature.
    pub fn vt(&self) -> f64 {
        KB * self.temp / Q
    }

    /// Computes the integration coefficients for the current step (SPICE `NIcomCof`).
    pub fn compute_coeffs(&mut self) {
        let h = self.delta;
        self.ag = [0.0; 3];
        match (self.method, self.order) {
            (_, 1) | (Method::BackwardEuler, _) => {
                self.ag[0] = 1.0 / h;
                self.ag[1] = -1.0 / h;
            }
            (Method::Trap, _) => {
                // xmu = 0.5
                self.ag[0] = 2.0 / h;
                self.ag[1] = 1.0;
            }
            (Method::Gear2, _) => {
                let hp = self.delta_old[1];
                let r = h / hp;
                self.ag[0] = (1.0 + 2.0 * r) / (h * (1.0 + r));
                self.ag[1] = -(1.0 + r) / h;
                self.ag[2] = r * r / (h * (1.0 + r));
            }
        }
    }

    fn effective_order(&self) -> usize {
        if self.method == Method::BackwardEuler {
            1
        } else {
            self.order
        }
    }

    /// Integrates the charge in state `q` (its current is stored in state `q+1`), returning
    /// `(geq, i_now)` for incremental capacitance `cap` (SPICE `NIintegrate`). The device's
    /// companion model is `i(v) ≈ i_now + geq·(v − v_now)`.
    pub fn integrate(&mut self, q: usize, cap: f64) -> (f64, f64) {
        let ccap = match (self.method, self.effective_order()) {
            (_, 1) => self.ag[0] * self.st[0][q] + self.ag[1] * self.st[1][q],
            (Method::Trap, _) => {
                -self.st[1][q + 1] * self.ag[1] + self.ag[0] * (self.st[0][q] - self.st[1][q])
            }
            _ => {
                self.ag[0] * self.st[0][q] + self.ag[1] * self.st[1][q] + self.ag[2] * self.st[2][q]
            }
        };
        self.st[0][q + 1] = ccap;
        let geq = self.ag[0] * cap;
        (geq, ccap)
    }

    /// Local truncation error estimate for charge state `q` (SPICE `CKTterr`); returns the
    /// largest acceptable next step.
    pub fn trunc_q(&self, q: usize) -> f64 {
        let o = &self.opts;
        let order = self.effective_order();
        let volttol = o.abstol + o.reltol * self.st[0][q + 1].abs().max(self.st[1][q + 1].abs());
        let chargetol =
            o.reltol * self.st[0][q].abs().max(self.st[1][q].abs()).max(o.chgtol) / self.delta;
        let tol = volttol.max(chargetol);
        // divided differences over order+2 points
        let mut diff = [0.0f64; 4];
        let mut dt = [0.0f64; 4];
        for i in 0..=order + 1 {
            diff[i] = self.st[i][q];
        }
        for (i, d) in dt.iter_mut().enumerate().take(order + 1) {
            *d = self.delta_old[i];
        }
        let mut j = order + 1;
        loop {
            for i in 0..j {
                diff[i] = (diff[i] - diff[i + 1]) / dt[i];
            }
            j -= 1;
            if j == 0 {
                break;
            }
            for i in 0..j {
                dt[i] = dt[i + 1] + dt[i];
            }
        }
        let factor = match (self.method, order) {
            (_, 1) => 0.5,
            (Method::Trap, _) => 1.0 / 12.0,
            _ => 2.0 / 9.0,
        };
        let del = o.trtol * tol / o.abstol.max(factor * diff[0].abs());
        if order == 2 { del.sqrt() } else { del }
    }

    /// Device-level predictor for a stored state value (SPICE `DEVpred`).
    pub fn predict(&self, s: usize) -> f64 {
        let xfact = self.delta / self.delta_old[1];
        (1.0 + xfact) * self.st[1][s] - xfact * self.st[2][s]
    }
}

/// Small-signal (AC) stamping context.
pub struct AcCtx<'a> {
    pub mat: &'a mut Matrix<C64>,
    pub rhs: &'a mut [C64],
    pub omega: f64,
    /// Operating point.
    pub x: &'a [f64],
    pub st: &'a [f64],
}

impl AcCtx<'_> {
    #[inline]
    pub fn add(&mut self, s: Slot, v: C64) {
        self.mat.vals[s.0 as usize] += v;
    }
    #[inline]
    pub fn add_re(&mut self, s: Slot, v: f64) {
        self.mat.vals[s.0 as usize].re += v;
    }
    #[inline]
    pub fn add_im(&mut self, s: Slot, v: f64) {
        self.mat.vals[s.0 as usize].im += v;
    }
}

/// A noise source: current noise PSD (A²/Hz) injected between two unknowns.
#[derive(Debug, Clone)]
pub struct NoiseSrc {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub psd: f64,
}

/// Setup context: allocates unknowns, matrix slots and state slots.
pub struct Setup<'a> {
    pub n: usize,
    pub names: &'a mut Vec<String>,
    pub kinds: &'a mut Vec<UKind>,
    pub n_states: usize,
    pub pattern: Option<&'a mut Pattern>,
    /// element name → branch unknown carrying its current (V, L, E, H, voltage B, ports).
    pub branches: &'a std::collections::HashMap<String, usize>,
    pub opts: &'a SimOptions,
}

impl Setup<'_> {
    pub fn alloc(&mut self, name: String, kind: UKind) -> usize {
        self.n += 1;
        self.names.push(name);
        self.kinds.push(kind);
        self.n
    }
    pub fn states(&mut self, k: usize) -> usize {
        let s = self.n_states;
        self.n_states += k;
        s
    }
    pub fn slot(&mut self, r: usize, c: usize) -> Slot {
        match &mut self.pattern {
            Some(p) => p.slot(r, c),
            None => Slot::GROUND,
        }
    }
    /// Four slots of a conductance between `a` and `b`.
    pub fn g4(&mut self, a: usize, b: usize) -> G4 {
        G4 {
            aa: self.slot(a, a),
            ab: self.slot(a, b),
            ba: self.slot(b, a),
            bb: self.slot(b, b),
        }
    }
    pub fn branch_of(&self, name: &str) -> Option<usize> {
        self.branches.get(name).copied()
    }
}

/// Slots of a two-terminal conductance.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct G4 {
    pub aa: Slot,
    pub ab: Slot,
    pub ba: Slot,
    pub bb: Slot,
}

impl G4 {
    #[inline]
    pub fn stamp(&self, ck: &mut Ckt, g: f64) {
        ck.add(self.aa, g);
        ck.add(self.ab, -g);
        ck.add(self.ba, -g);
        ck.add(self.bb, g);
    }
    #[inline]
    pub fn stamp_ac(&self, ac: &mut AcCtx, y: C64) {
        ac.add(self.aa, y);
        ac.add(self.ab, -y);
        ac.add(self.ba, -y);
        ac.add(self.bb, y);
    }
}

/// Slots of a transconductance: current into `p`/out of `n` controlled by `v(cp) - v(cn)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gm4 {
    pub pcp: Slot,
    pub pcn: Slot,
    pub ncp: Slot,
    pub ncn: Slot,
}

impl Gm4 {
    pub fn new(s: &mut Setup, p: usize, n: usize, cp: usize, cn: usize) -> Gm4 {
        Gm4 {
            pcp: s.slot(p, cp),
            pcn: s.slot(p, cn),
            ncp: s.slot(n, cp),
            ncn: s.slot(n, cn),
        }
    }
    /// Current `gm·(v(cp)-v(cn))` flowing from `p` to `n` through the element.
    #[inline]
    pub fn stamp(&self, ck: &mut Ckt, gm: f64) {
        ck.add(self.pcp, gm);
        ck.add(self.pcn, -gm);
        ck.add(self.ncp, -gm);
        ck.add(self.ncn, gm);
    }
    #[inline]
    pub fn stamp_ac(&self, ac: &mut AcCtx, gm: C64) {
        ac.add(self.pcp, gm);
        ac.add(self.pcn, -gm);
        ac.add(self.ncp, -gm);
        ac.add(self.ncn, gm);
    }
}

/// Behaviour shared by all devices.
pub trait Dev {
    /// Element name (flattened, lower case).
    fn name(&self) -> &str;
    /// Pass 1: allocate internal nodes/branches and state slots.
    fn setup_unknowns(&mut self, _s: &mut Setup) -> Result<(), SimError> {
        Ok(())
    }
    /// Pass 2: allocate matrix slots (other devices' branches are known now).
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError>;
    /// Temperature-dependent parameters (called before each analysis).
    fn temperature(&mut self, _temp_k: f64, _opts: &SimOptions) {}
    /// Stamps the linearised model into `ck.mat`/`ck.rhs`.
    fn load(&mut self, ck: &mut Ckt);
    /// Stamps the small-signal model at the operating point stored in `ck.x`/`ck.st[0]`.
    fn ac_load(&self, _ac: &mut AcCtx) {}
    /// Device-level convergence test (current prediction checks).
    fn conv_ok(&self, _ck: &Ckt) -> bool {
        true
    }
    /// Largest acceptable next time step from truncation error.
    fn trunc(&self, _ck: &Ckt) -> f64 {
        f64::INFINITY
    }
    /// Called once a time point is accepted.
    fn accept(&mut self, _ck: &Ckt) {}
    /// Breakpoints (times where the device's behaviour is discontinuous) in `(t, t_end]`.
    fn breakpoints(&self, _t: f64, _t_end: f64, _out: &mut Vec<f64>) {}
    /// True if it holds charge/flux (needs time stepping).
    fn is_reactive(&self) -> bool {
        false
    }
    /// True if its stamp depends on the solution (needs Newton iterations).
    fn is_nonlinear(&self) -> bool {
        false
    }
    /// True if its value changes with time by itself (time-dependent source).
    fn is_time_varying(&self) -> bool {
        false
    }
    /// Main terminal current (into the first terminal), if meaningful.
    fn current(&self, _ck: &Ckt) -> Option<f64> {
        None
    }
    /// Noise current sources at the operating point.
    fn noise(&self, _ck: &Ckt, _freq: f64, _out: &mut Vec<NoiseSrc>) {}
    /// Changes a parameter at run time (Session::set_param / ext() updates).
    fn set_param(&mut self, _name: &str, _value: f64) -> bool {
        false
    }
    /// Initial state for UIC transients (capacitor charge from `ic=` or node voltages).
    fn init_uic(&mut self, _ck: &mut Ckt) {}
    /// Branch unknown index if this element has one.
    fn branch(&self) -> Option<usize> {
        None
    }
    /// Independent source value for AC/noise/TF excitation: (index pair or branch, value).
    fn set_source_value(&mut self, _v: f64) -> bool {
        false
    }
    /// AC excitation stamped into the AC right-hand side.
    fn ac_excite(&self, _rhs: &mut [C64]) {}
    /// Transient step/stop times (waveform defaults).
    fn prepare_tran(&mut self, _tstep: f64, _tstop: f64) {}
}

macro_rules! devices {
    ($($v:ident($t:ty)),* $(,)?) => {
        /// All device implementations.
        #[derive(Debug, Clone)]
        pub enum Device { $($v($t)),* }

        macro_rules! dispatch {
            ($self:expr, $d:ident => $body:expr) => {
                match $self { $(Device::$v($d) => $body),* }
            };
        }

        impl Dev for Device {
            fn name(&self) -> &str { dispatch!(self, d => d.name()) }
            fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> { dispatch!(self, d => d.setup_unknowns(s)) }
            fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> { dispatch!(self, d => d.setup_matrix(s)) }
            fn temperature(&mut self, t: f64, o: &SimOptions) { dispatch!(self, d => d.temperature(t, o)) }
            fn load(&mut self, ck: &mut Ckt) { dispatch!(self, d => d.load(ck)) }
            fn ac_load(&self, ac: &mut AcCtx) { dispatch!(self, d => d.ac_load(ac)) }
            fn conv_ok(&self, ck: &Ckt) -> bool { dispatch!(self, d => d.conv_ok(ck)) }
            fn trunc(&self, ck: &Ckt) -> f64 { dispatch!(self, d => d.trunc(ck)) }
            fn accept(&mut self, ck: &Ckt) { dispatch!(self, d => d.accept(ck)) }
            fn breakpoints(&self, t: f64, e: f64, out: &mut Vec<f64>) { dispatch!(self, d => d.breakpoints(t, e, out)) }
            fn is_reactive(&self) -> bool { dispatch!(self, d => d.is_reactive()) }
            fn is_nonlinear(&self) -> bool { dispatch!(self, d => d.is_nonlinear()) }
            fn is_time_varying(&self) -> bool { dispatch!(self, d => d.is_time_varying()) }
            fn current(&self, ck: &Ckt) -> Option<f64> { dispatch!(self, d => d.current(ck)) }
            fn noise(&self, ck: &Ckt, f: f64, out: &mut Vec<NoiseSrc>) { dispatch!(self, d => d.noise(ck, f, out)) }
            fn set_param(&mut self, n: &str, v: f64) -> bool { dispatch!(self, d => d.set_param(n, v)) }
            fn init_uic(&mut self, ck: &mut Ckt) { dispatch!(self, d => d.init_uic(ck)) }
            fn branch(&self) -> Option<usize> { dispatch!(self, d => d.branch()) }
            fn set_source_value(&mut self, v: f64) -> bool { dispatch!(self, d => d.set_source_value(v)) }
            fn ac_excite(&self, rhs: &mut [C64]) { dispatch!(self, d => d.ac_excite(rhs)) }
            fn prepare_tran(&mut self, a: f64, b: f64) { dispatch!(self, d => d.prepare_tran(a, b)) }
        }
    };
}

devices! {
    Resistor(passive::Resistor),
    Capacitor(passive::Capacitor),
    Inductor(passive::Inductor),
    Vsrc(sources::Vsrc),
    Isrc(sources::Isrc),
    Port(sources::Port),
    Vcvs(controlled::Vcvs),
    Vccs(controlled::Vccs),
    Cccs(controlled::Cccs),
    Ccvs(controlled::Ccvs),
    Bsrc(bsrc::Bsrc),
    Switch(switch::Switch),
    Diode(diode::Diode),
    Bjt(bjt::Bjt),
    Mos1(mos1::Mos1),
    Jfet(jfet::Jfet),
    Tline(tline::Tline),
}

/// Evaluated model/instance parameters with alias lookup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamMap {
    pub vals: Vec<(String, f64)>,
}

impl ParamMap {
    pub fn new(vals: Vec<(String, f64)>) -> Self {
        ParamMap { vals }
    }
    /// First present key among aliases.
    pub fn get(&self, keys: &[&str]) -> Option<f64> {
        for k in keys {
            if let Some((_, v)) = self.vals.iter().rev().find(|(n, _)| n == k) {
                return Some(*v);
            }
        }
        None
    }
    pub fn or(&self, keys: &[&str], default: f64) -> f64 {
        self.get(keys).unwrap_or(default)
    }
    pub fn has(&self, keys: &[&str]) -> bool {
        self.get(keys).is_some()
    }
    pub fn set(&mut self, key: &str, v: f64) {
        if let Some(e) = self.vals.iter_mut().rev().find(|(n, _)| n == key) {
            e.1 = v;
        } else {
            self.vals.push((key.to_string(), v));
        }
    }
    /// Keys not in `known` (for "unknown parameter" warnings).
    pub fn unknown(&self, known: &[&str]) -> Vec<String> {
        const IGNORED: [&str; 6] = ["level", "version", "kf", "af", "tnom", "mfg"];
        self.vals
            .iter()
            .map(|(k, _)| k)
            .filter(|k| !known.contains(&k.as_str()) && !IGNORED.contains(&k.as_str()))
            .cloned()
            .collect()
    }
}
