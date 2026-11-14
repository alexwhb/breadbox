//! Incremental transient co-simulation ([`Session`], SPEC SPICE-7, ARCHITECTURE §4).
//!
//! A session owns one circuit and advances it in time under the control of a host (bb-sim's
//! scheduler). The host changes inputs between calls — port drives ([`PinDrive`]), external
//! variables (`ext("name")` in any expression), element parameters — and reads node voltages,
//! currents and threshold crossings.
//!
//! **How it stays cheap.** Internally the solver takes its own adaptive steps (LTE control,
//! breakpoints) and may run *ahead* of the requested time; `advance_to(t)` for a `t` inside the
//! last step just interpolates. So a 1 µs co-simulation quantum costs an interpolation, not a
//! Newton solve. When an input changes inside a step, the step is rewound by interpolation to
//! the change time and the circuit is re-solved there (a consistent "t⁺" solution: capacitor
//! voltages and inductor currents are continuous, everything else may jump). Circuits without
//! reactive elements or time-dependent sources are solved only when an input changes, and
//! reactive circuits that have settled stop stepping until something changes.
//!
//! ```
//! use bb_spice::{CircuitBuilder, Session};
//! use bb_core::PinDrive;
//!
//! let mut b = CircuitBuilder::new("pwm into RC");
//! let pin = b.port("d9", "pin", "0", PinDrive::HighZ);
//! b.resistor("1", "pin", "out", 1e3);
//! b.capacitor("1", "out", "0", 1e-6);
//! let nl = b.build();
//! let mut s = Session::new(&nl).unwrap();
//! let w = s.watch("out", 1.0, 2.0).unwrap();
//! s.set_port(pin, PinDrive::Thevenin { v: 5.0, r: 25.0 });
//! let crossings = s.advance_to(2e-3).unwrap();
//! assert_eq!(crossings.len(), 1);
//! assert!(crossings[0].rising && crossings[0].watch == w);
//! // τ = 1.025 ms: 2 V is reached at τ·ln(5/3)
//! assert!((crossings[0].time - 1.025e-3 * (5.0f64 / 3.0).ln()).abs() < 2e-6);
//! assert!((s.voltage("out").unwrap() - 5.0 * (1.0 - (-2e-3f64 / 1.025e-3).exp())).abs() < 5e-3);
//! ```

use crate::analysis;
use crate::devices::{Dev, Device, Init, Mode};
use crate::elab::{self, ElabOptions};
use crate::engine::Engine;
use crate::error::SimError;
use crate::netlist::{ElemKind, ElementCard, Netlist};
use crate::tran::Stepper;
use bb_core::PinDrive;
use std::collections::HashMap;

/// A co-simulation port (MCU pin or device output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId(pub usize);

/// A threshold watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WatchId(pub usize);

/// A circuit node resolved once for fast voltage reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

/// A Schmitt-trigger crossing of a watched node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossing {
    pub watch: WatchId,
    /// Interpolated crossing time (s).
    pub time: f64,
    /// `true` when the node rose through `hi`, `false` when it fell through `lo`.
    pub rising: bool,
}

/// Session settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    /// Largest internal time step (s). Smaller values resolve fast signals in circuits without
    /// charge storage; LTE control sets the step otherwise.
    pub max_step: f64,
    /// Integration method override (default: the netlist's `.options method`, trapezoidal).
    pub method: Option<crate::elab::Method>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            max_step: 50e-6,
            method: None,
        }
    }
}

/// Work counters (for performance tests and the UI's status line).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub steps: u64,
    pub rejected: u64,
    /// Re-solves after input changes.
    pub commits: u64,
    /// `advance_to` calls answered without solving (interpolation, settled or static circuit).
    pub free_advances: u64,
    pub newton_iterations: u64,
}

#[derive(Debug, Clone)]
struct Watch {
    node: usize,
    lo: f64,
    hi: f64,
    high: bool,
    last_v: f64,
    active: bool,
}

#[derive(Debug, Clone)]
struct PortInfo {
    name: String,
    dev: usize,
    drive: PinDrive,
}

/// Everything that changes while simulating (cloned by [`Session::snapshot`]).
#[derive(Debug, Clone)]
struct State {
    engine: Engine,
    stepper: Option<Stepper>,
    started: bool,
    dirty: bool,
    /// Visible time.
    t: f64,
    /// Previous accepted point (for interpolation inside the last step).
    t_prev: f64,
    x_prev: Vec<f64>,
    ports: Vec<PortInfo>,
    ext_values: HashMap<String, f64>,
    watches: Vec<Watch>,
    /// Accepted points beyond the visible time: (time, watched node values).
    trail: Vec<(f64, Vec<f64>)>,
    pending: Vec<Crossing>,
    quiet: u32,
    stats: SessionStats,
}

/// Saved session state; restore with [`Session::restore`].
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    state: Box<State>,
    generation: u64,
}

impl SessionSnapshot {
    /// Simulated time at which the snapshot was taken.
    pub fn time(&self) -> f64 {
        self.state.t
    }
}

/// An incremental transient simulation driven by a host.
#[derive(Debug, Clone)]
pub struct Session {
    netlist: Netlist,
    opts: SessionOptions,
    s: State,
    generation: u64,
}

impl Session {
    /// Creates a session for a netlist (its analyses are ignored; `.options`, `.ic`, `.temp`
    /// and `.nodeset` apply). Ports declared with [`crate::CircuitBuilder::port`] are available
    /// through [`Session::port`].
    pub fn new(netlist: &Netlist) -> Result<Session, SimError> {
        Session::with_options(netlist, SessionOptions::default())
    }

    pub fn with_options(netlist: &Netlist, opts: SessionOptions) -> Result<Session, SimError> {
        Session::with_ext(netlist, opts, &[])
    }

    /// Like [`Session::with_options`] with initial values for `ext()` variables, so the
    /// circuit is elaborated (and its first operating point solved) with them in place.
    pub fn with_ext(
        netlist: &Netlist,
        opts: SessionOptions,
        ext: &[(String, f64)],
    ) -> Result<Session, SimError> {
        if opts.max_step.is_nan() || opts.max_step <= 0.0 {
            return Err(SimError::invalid("max_step must be positive"));
        }
        let ext_values: HashMap<String, f64> = ext.iter().cloned().collect();
        let engine = build_engine(netlist, &opts, &ext_values)?;
        let ports = collect_ports(netlist, &engine);
        let n = engine.n();
        Ok(Session {
            netlist: netlist.clone(),
            opts,
            s: State {
                engine,
                stepper: None,
                started: false,
                dirty: false,
                t: 0.0,
                t_prev: 0.0,
                x_prev: vec![0.0; n + 1],
                ports,
                ext_values,
                watches: Vec::new(),
                trail: Vec::new(),
                pending: Vec::new(),
                quiet: 0,
                stats: SessionStats::default(),
            },
            generation: 0,
        })
    }

    /// Current (visible) simulation time in seconds.
    pub fn time(&self) -> f64 {
        self.s.t
    }

    pub fn stats(&self) -> SessionStats {
        let mut st = self.s.stats;
        st.newton_iterations = self.s.engine.counters.newton;
        st
    }

    /// The circuit this session simulates (including ports added later).
    pub fn netlist(&self) -> &Netlist {
        &self.netlist
    }

    /// True when nothing in the circuit can change until an input changes: the circuit has
    /// no charge storage and no time-dependent sources, or it has settled (several maximum
    /// steps without measurable change) and has no time-dependent sources. Co-simulators use
    /// this to lengthen their sync quantum safely.
    pub fn is_quiescent(&self) -> bool {
        if !self.s.started || self.s.dirty || !self.s.pending.is_empty() {
            return false;
        }
        self.is_static() || (self.s.quiet >= 3 && !self.s.engine.time_varying)
    }

    fn is_static(&self) -> bool {
        !self.s.engine.reactive && !self.s.engine.time_varying
    }

    fn ensure_started(&mut self) -> Result<(), SimError> {
        if self.s.started {
            return Ok(());
        }
        let st = &mut self.s;
        let e = &mut st.engine;
        e.refresh_bindings(true);
        for d in e.devices.iter_mut() {
            d.prepare_tran(self.opts.max_step, f64::INFINITY);
        }
        analysis::tran_init(e, false)?;
        // port drives set before the start are an event at t = 0
        st.dirty = false;
        for p in &st.ports {
            if let Device::Port(d) = &mut e.devices[p.dev]
                && d.drive != p.drive
            {
                d.drive = p.drive;
                st.dirty = true;
            }
        }
        if !(!e.reactive && !e.time_varying) {
            let mut stp = Stepper::new(
                e,
                0.0,
                self.opts.max_step,
                f64::INFINITY,
                self.opts.max_step,
            );
            stp.tstop = f64::INFINITY;
            st.stepper = Some(stp);
        }
        st.x_prev.copy_from_slice(&e.ck.x);
        st.t_prev = 0.0;
        st.started = true;
        for w in st.watches.iter_mut() {
            w.last_v = e.ck.x[w.node];
            w.high = w.last_v > w.hi;
        }
        Ok(())
    }

    /// Interpolated value of unknown `i` at the visible time.
    fn x_at(&self, i: usize) -> f64 {
        let s = &self.s;
        let x = &s.engine.ck.x;
        match &s.stepper {
            Some(stp) if stp.time > s.t && stp.time > s.t_prev => {
                let w = (s.t - s.t_prev) / (stp.time - s.t_prev);
                s.x_prev[i] + w * (x[i] - s.x_prev[i])
            }
            _ => x[i],
        }
    }

    // ---------------------------------------------------------------------------------------
    // Inputs
    // ---------------------------------------------------------------------------------------

    /// Port declared in the netlist/builder by name (`d9` for a port element `pd9` or `d9`).
    pub fn port(&self, name: &str) -> Option<PortId> {
        let l = name.to_ascii_lowercase();
        self.s
            .ports
            .iter()
            .position(|p| p.name == l || p.name.strip_prefix('p') == Some(l.as_str()))
            .map(PortId)
    }

    /// Adds a port driving `node_p` relative to `node_n` (both created if new). Prefer declaring
    /// ports before the first `advance_to`; adding one later rebuilds the solver and keeps node
    /// voltages and charges (transmission-line histories restart).
    pub fn add_port(&mut self, name: &str, node_p: &str, node_n: &str) -> Result<PortId, SimError> {
        let pname = port_element_name(name);
        if self.netlist.top.elements.iter().any(|e| e.name == pname) {
            return Err(SimError::invalid(format!("port `{name}` already exists")));
        }
        self.netlist.top.elements.push(ElementCard {
            name: pname.clone(),
            nodes: vec![node_p.to_ascii_lowercase(), node_n.to_ascii_lowercase()],
            kind: ElemKind::Port {
                drive: PinDrive::HighZ,
            },
            params: vec![],
            span: Default::default(),
        });
        self.rebuild()?;
        Ok(PortId(
            self.s
                .ports
                .iter()
                .position(|p| p.name == pname)
                .unwrap_or(0),
        ))
    }

    fn rebuild(&mut self) -> Result<(), SimError> {
        if self.s.started {
            // make the old engine's state describe exactly the visible time before transferring
            let t = self.s.t;
            self.rewind_to(t);
            self.s.x_prev.copy_from_slice(&self.s.engine.ck.x);
            self.s.t_prev = t;
        }
        let mut engine = build_engine(&self.netlist, &self.opts, &self.s.ext_values)?;
        // keep drives of existing ports
        let mut ports = collect_ports(&self.netlist, &engine);
        for p in ports.iter_mut() {
            if let Some(old) = self.s.ports.iter().find(|o| o.name == p.name) {
                p.drive = old.drive;
                if let Device::Port(d) = &mut engine.devices[p.dev] {
                    d.drive = old.drive;
                }
            }
        }
        let n = engine.n();
        if self.s.started {
            let old = &self.s.engine;
            // transfer unknowns by name and charges wholesale (ports add no state)
            for (i, name) in engine.names.clone().iter().enumerate().skip(1) {
                if let Some(j) = old.names.iter().position(|n| n == name) {
                    engine.ck.x[i] = old.ck.x[j];
                }
            }
            if engine.ck.st[0].len() == old.ck.st[0].len() {
                engine.ck.st = old.ck.st.clone();
            }
            engine.ck.delta_old = old.ck.delta_old;
            engine.ck.time = old.ck.time;
            engine.ck.mode = Mode::Tran;
            engine.reorder = true;
            for d in engine.devices.iter_mut() {
                d.prepare_tran(self.opts.max_step, f64::INFINITY);
                d.accept(&old.ck);
            }
            let reactive_or_timed = engine.reactive || engine.time_varying;
            let stepper = match self.s.stepper.take() {
                Some(mut stp) if reactive_or_timed => {
                    stp.x_acc = engine.ck.x.clone();
                    stp.reset_breakpoints(&engine);
                    Some(stp)
                }
                _ if reactive_or_timed => {
                    let mut stp = Stepper::new(
                        &mut engine,
                        self.s.t,
                        self.opts.max_step,
                        f64::INFINITY,
                        self.opts.max_step,
                    );
                    stp.tstop = f64::INFINITY;
                    Some(stp)
                }
                _ => None,
            };
            self.s.stepper = stepper;
            // watches refer to unknown indices: remap by node name
            for w in self.s.watches.iter_mut() {
                let name = &self.s.engine.names[w.node];
                w.node = engine.names.iter().position(|n| n == name).unwrap_or(0);
            }
            self.s.engine = engine;
            self.s.x_prev = self.s.engine.ck.x.clone();
            self.s.t_prev = self.s.t;
            self.s.trail.clear();
            self.s.dirty = true;
        } else {
            for w in self.s.watches.iter_mut() {
                let name = &self.s.engine.names[w.node];
                w.node = engine.names.iter().position(|n| n == name).unwrap_or(0);
            }
            self.s.engine = engine;
            self.s.x_prev = vec![0.0; n + 1];
        }
        self.s.ports = ports;
        self.generation += 1;
        Ok(())
    }

    /// Changes how a port drives its node. Takes effect at the current time (before the first
    /// `advance_to`, as an event at t = 0 after the initial operating point, which uses the
    /// drive declared in the netlist/builder).
    pub fn set_port(&mut self, id: PortId, drive: PinDrive) {
        let Some(p) = self.s.ports.get_mut(id.0) else {
            return;
        };
        if p.drive == drive {
            return;
        }
        p.drive = drive;
        // before the first solve the operating point uses the netlist's declared drives, and
        // changes become an event at t = 0 (applied in `ensure_started`)
        if self.s.started
            && let Device::Port(d) = &mut self.s.engine.devices[p.dev]
        {
            d.drive = drive;
        }
        self.s.dirty = true;
    }

    /// Current drive of a port.
    pub fn port_drive(&self, id: PortId) -> Option<PinDrive> {
        self.s.ports.get(id.0).map(|p| p.drive)
    }

    /// Sets an external variable used by `ext("name")` expressions. Unknown names are stored
    /// (and ignored by the circuit).
    pub fn set_ext(&mut self, name: &str, value: f64) {
        if self.s.ext_values.get(name) == Some(&value) {
            return;
        }
        self.s.ext_values.insert(name.to_string(), value);
        if self.s.engine.set_ext(name, value) {
            self.s.dirty = true;
        }
    }

    /// Value of an external variable.
    pub fn ext(&self, name: &str) -> Option<f64> {
        self.s.ext_values.get(name).copied()
    }

    /// Changes an element parameter (`r`, `c`, `l`, `dc`, `gain`, model parameters such as
    /// `is`, …). Takes effect at the current time.
    pub fn set_param(&mut self, element: &str, param: &str, value: f64) -> Result<(), SimError> {
        let i = self
            .s
            .engine
            .device_index(element)
            .ok_or_else(|| SimError::Unknown {
                kind: "element",
                name: element.to_string(),
            })?;
        if !self.s.engine.devices[i].set_param(&param.to_ascii_lowercase(), value) {
            return Err(SimError::invalid(format!(
                "`{element}` has no parameter `{param}`"
            )));
        }
        self.s.dirty = true;
        Ok(())
    }

    /// Sets the circuit temperature (°C).
    pub fn set_temperature(&mut self, celsius: f64) {
        self.s.engine.set_temperature(celsius);
        self.s.dirty = true;
    }

    // ---------------------------------------------------------------------------------------
    // Watches
    // ---------------------------------------------------------------------------------------

    /// Watches a node with Schmitt thresholds `lo < hi`: a rising crossing is reported when the
    /// voltage passes `hi` while low, a falling one when it passes `lo` while high.
    pub fn watch(&mut self, node: &str, lo: f64, hi: f64) -> Result<WatchId, SimError> {
        let n = self.s.engine.node(node).ok_or_else(|| SimError::Unknown {
            kind: "node",
            name: node.to_string(),
        })?;
        if n == 0 {
            return Err(SimError::invalid("cannot watch ground"));
        }
        if lo.is_nan() || hi.is_nan() || lo > hi {
            return Err(SimError::invalid("watch thresholds must satisfy lo <= hi"));
        }
        let v = if self.s.started { self.x_at(n) } else { 0.0 };
        self.s.watches.push(Watch {
            node: n,
            lo,
            hi,
            high: v > hi,
            last_v: v,
            active: true,
        });
        Ok(WatchId(self.s.watches.len() - 1))
    }

    /// Stops reporting a watch.
    pub fn unwatch(&mut self, id: WatchId) {
        if let Some(w) = self.s.watches.get_mut(id.0) {
            w.active = false;
        }
    }

    /// Current Schmitt state of a watch (`true` = high).
    pub fn watch_state(&self, id: WatchId) -> Option<bool> {
        self.s.watches.get(id.0).map(|w| w.high)
    }

    fn watched_values(&self) -> Vec<f64> {
        self.s
            .watches
            .iter()
            .map(|w| self.s.engine.ck.x[w.node])
            .collect()
    }

    /// Scans the visible trajectory up to `t1` for crossings. Stops at the first crossing when
    /// `first_only`, returning its time.
    fn scan(&mut self, t1: f64, first_only: bool, out: &mut Vec<Crossing>) -> Option<f64> {
        if self.s.watches.iter().all(|w| !w.active) {
            return None;
        }
        // fast path (the common 1 µs quantum): a single segment and no threshold crossed
        let t_vis = self.s.t;
        if !self.s.trail.iter().any(|(t, _)| *t > t_vis && *t < t1) {
            let mut crossed = false;
            for w in &self.s.watches {
                if !w.active {
                    continue;
                }
                let v = self.x_at_time(w.node, t1);
                if (w.high && v < w.lo) || (!w.high && v > w.hi) {
                    crossed = true;
                    break;
                }
            }
            if !crossed {
                for k in 0..self.s.watches.len() {
                    let n = self.s.watches[k].node;
                    let v = self.x_at_time(n, t1);
                    self.s.watches[k].last_v = v;
                }
                return None;
            }
        }
        // segment end points: trail entries < t1, then (t1, interpolated)
        let end_vals: Vec<f64> = self
            .s
            .watches
            .iter()
            .map(|w| w.node)
            .collect::<Vec<_>>()
            .iter()
            .map(|&n| self.x_at_time(n, t1))
            .collect();
        let mut points: Vec<(f64, Vec<f64>)> = self
            .s
            .trail
            .iter()
            .filter(|(t, _)| *t > self.s.t && *t < t1)
            .cloned()
            .collect();
        points.push((t1, end_vals));
        let mut t0 = self.s.t;
        let mut stop = None;
        'outer: for (tp, vals) in points {
            let mut seg_crossings: Vec<(f64, usize, bool)> = Vec::new();
            for (k, w) in self.s.watches.iter().enumerate() {
                if !w.active {
                    continue;
                }
                let (va, vb) = (w.last_v, vals[k]);
                let mut high = w.high;
                // up to two crossings per segment (a swing through both thresholds)
                let mut ta = t0;
                let mut a = va;
                for _ in 0..2 {
                    let thr = if high { w.lo } else { w.hi };
                    let crossed = if high {
                        vb < thr && a >= thr
                    } else {
                        vb > thr && a <= thr
                    };
                    if !crossed {
                        break;
                    }
                    let frac = if vb != a { (thr - a) / (vb - a) } else { 1.0 };
                    let tc = ta + (tp - ta) * frac.clamp(0.0, 1.0);
                    high = !high;
                    seg_crossings.push((tc, k, high));
                    ta = tc;
                    a = thr;
                }
            }
            seg_crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            if first_only && let Some(&(tc, _, _)) = seg_crossings.first() {
                // keep only crossings at the first time, then stop
                for (tcc, k, high) in seg_crossings.iter().copied() {
                    if tcc > tc {
                        break;
                    }
                    self.s.watches[k].high = high;
                    out.push(Crossing {
                        watch: WatchId(k),
                        time: tcc,
                        rising: high,
                    });
                }
                // values at tc
                let frac = if tp > t0 { (tc - t0) / (tp - t0) } else { 1.0 };
                for (k, w) in self.s.watches.iter_mut().enumerate() {
                    w.last_v += (vals[k] - w.last_v) * frac;
                }
                stop = Some(tc);
                break 'outer;
            }
            for (tc, k, high) in seg_crossings {
                self.s.watches[k].high = high;
                out.push(Crossing {
                    watch: WatchId(k),
                    time: tc,
                    rising: high,
                });
            }
            for (k, w) in self.s.watches.iter_mut().enumerate() {
                w.last_v = vals[k];
            }
            t0 = tp;
        }
        stop
    }

    /// Interpolated value of unknown `i` at time `t` within the last accepted step.
    fn x_at_time(&self, i: usize, t: f64) -> f64 {
        let s = &self.s;
        let x = &s.engine.ck.x;
        match &s.stepper {
            Some(stp) if stp.time > s.t_prev && t < stp.time => {
                let w = ((t - s.t_prev) / (stp.time - s.t_prev)).clamp(0.0, 1.0);
                s.x_prev[i] + w * (x[i] - s.x_prev[i])
            }
            _ => x[i],
        }
    }

    // ---------------------------------------------------------------------------------------
    // Time
    // ---------------------------------------------------------------------------------------

    /// Re-solves the circuit at the current time after input changes (normally done
    /// automatically by the next [`Session::advance_to`]). Returns crossings caused by the jump.
    pub fn op(&mut self) -> Result<Vec<Crossing>, SimError> {
        self.ensure_started()?;
        self.commit()?;
        Ok(std::mem::take(&mut self.s.pending))
    }

    fn commit(&mut self) -> Result<(), SimError> {
        if !self.s.dirty {
            return Ok(());
        }
        self.s.dirty = false;
        self.s.stats.commits += 1;
        self.s.engine.refresh_bindings(false);
        let t = self.s.t;
        if self.s.stepper.is_none() {
            // static circuit: plain operating point with the new inputs
            let e = &mut self.s.engine;
            e.ck.mode = Mode::Dc;
            e.ck.tran_op = false;
            e.ck.init = Init::Float;
            let itl = e.ck.opts.itl1;
            if e.newton(itl).is_err() {
                e.solve_op("operating point")?;
            }
        } else {
            self.rewind_to(t);
            let st = &mut self.s;
            let stp = st.stepper.as_mut().unwrap();
            stp.reset_breakpoints(&st.engine);
            stp.resolve(&mut st.engine)?;
            st.quiet = 0;
            st.trail.clear();
        }
        let st = &mut self.s;
        st.x_prev.copy_from_slice(&st.engine.ck.x);
        st.t_prev = t;
        // jump at time t: report watch transitions
        let vals = self.watched_values();
        let mut out = Vec::new();
        for (k, w) in self.s.watches.iter_mut().enumerate() {
            let v = vals[k];
            if w.active {
                if !w.high && v > w.hi {
                    w.high = true;
                    out.push(Crossing {
                        watch: WatchId(k),
                        time: t,
                        rising: true,
                    });
                } else if w.high && v < w.lo {
                    w.high = false;
                    out.push(Crossing {
                        watch: WatchId(k),
                        time: t,
                        rising: false,
                    });
                }
            }
            w.last_v = v;
        }
        self.s.pending.extend(out);
        Ok(())
    }

    /// Moves the integration point back to `t` (inside the last accepted step) by interpolation.
    fn rewind_to(&mut self, t: f64) {
        let st = &mut self.s;
        let Some(stp) = st.stepper.as_mut() else {
            return;
        };
        let tb = stp.time;
        if tb <= t * (1.0 + 1e-15) + 1e-21 {
            return;
        }
        let ta = st.t_prev;
        let e = &mut st.engine;
        let w = if tb > ta {
            ((t - ta) / (tb - ta)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        for i in 0..e.ck.x.len() {
            e.ck.x[i] = st.x_prev[i] + w * (e.ck.x[i] - st.x_prev[i]);
        }
        // after accepting tb: st[1] = state(tb), st[2] = state(ta)
        let n = e.ck.st[0].len();
        for k in 0..n {
            let v = e.ck.st[2][k] + w * (e.ck.st[1][k] - e.ck.st[2][k]);
            e.ck.st[1][k] = v;
            e.ck.st[0][k] = v;
        }
        if t - ta > 1e-18 {
            e.ck.delta_old[0] = t - ta;
        } else {
            // back at ta: drop the step entirely
            e.ck.st.rotate_left(1);
            let (a, b) = e.ck.st.split_at_mut(3);
            b[0].copy_from_slice(&a[2]);
            let (a, b) = e.ck.st.split_at_mut(1);
            a[0].copy_from_slice(&b[0]);
            e.ck.delta_old = [
                e.ck.delta_old[1],
                e.ck.delta_old[2],
                e.ck.delta_old[3],
                e.ck.delta_old[3],
            ];
        }
        for d in e.devices.iter_mut() {
            d.rewind(t, &e.ck.x);
        }
        e.ck.time = t;
        stp.time = t;
        stp.x_acc.copy_from_slice(&e.ck.x);
    }

    /// Advances to time `t` (seconds), returning the threshold crossings in `(previous time, t]`
    /// in time order (plus any caused by input changes at the previous time).
    pub fn advance_to(&mut self, t: f64) -> Result<Vec<Crossing>, SimError> {
        self.advance(t, false).map(|(c, _)| c)
    }

    /// Like [`Session::advance_to`] but stops at the first crossing (so the host can react
    /// exactly at that time). Returns the crossing(s) at that instant, or an empty vector if
    /// `t` was reached.
    pub fn advance_until_crossing(&mut self, t: f64) -> Result<Vec<Crossing>, SimError> {
        self.advance(t, true).map(|(c, _)| c)
    }

    fn advance(&mut self, t: f64, first_only: bool) -> Result<(Vec<Crossing>, f64), SimError> {
        if !t.is_finite() {
            return Err(SimError::invalid("advance_to: time must be finite"));
        }
        if t < self.s.t - 1e-15 * self.s.t.abs().max(1.0) {
            return Err(SimError::invalid(format!(
                "advance_to({t:.6e}) is before the current time {:.6e}; use snapshot/restore to go back",
                self.s.t
            )));
        }
        self.ensure_started()?;
        self.commit()?;
        let mut out = std::mem::take(&mut self.s.pending);
        if first_only && !out.is_empty() {
            return Ok((out, self.s.t));
        }
        if t <= self.s.t {
            return Ok((out, self.s.t));
        }
        if self.is_static() {
            self.s.t = t;
            self.s.stats.free_advances += 1;
            return Ok((out, t));
        }
        let settled = self.s.quiet >= 3 && !self.s.engine.time_varying;
        let stp_time = self.s.stepper.as_ref().map(|s| s.time).unwrap_or(t);
        if settled || stp_time >= t {
            if settled && stp_time < t {
                // quiescent: nothing moves, so jump the integration point too
                let st = &mut self.s;
                let stp = st.stepper.as_mut().unwrap();
                stp.time = t;
                st.engine.ck.time = t;
                st.t_prev = t;
                st.x_prev.copy_from_slice(&st.engine.ck.x);
            }
            self.s.stats.free_advances += 1;
        }
        while self.s.stepper.as_ref().is_some_and(|s| s.time < t) {
            let st = &mut self.s;
            let stp = st.stepper.as_mut().unwrap();
            st.x_prev.copy_from_slice(&st.engine.ck.x);
            st.t_prev = stp.time;
            let (acc0, rej0) = (stp.accepted, stp.rejected);
            stp.step(&mut st.engine, f64::INFINITY)?;
            st.stats.steps += stp.accepted - acc0;
            st.stats.rejected += stp.rejected - rej0;
            let tb = stp.time;
            let at_max = (tb - st.t_prev) >= 0.999 * stp.tmax;
            // settled detection: nothing moved measurably over several maximum-size steps
            let e = &st.engine;
            let o = &e.ck.opts;
            let quiet = (1..e.ck.x.len()).all(|i| {
                let d = (e.ck.x[i] - st.x_prev[i]).abs();
                let floor = if e.kinds[i] == crate::devices::UKind::Node {
                    o.vntol
                } else {
                    o.abstol
                };
                d <= 1e-9 * e.ck.x[i].abs() + 1e-2 * floor
            });
            st.quiet = if quiet && at_max { st.quiet + 1 } else { 0 };
            if !st.watches.is_empty() {
                let vals: Vec<f64> = st.watches.iter().map(|w| st.engine.ck.x[w.node]).collect();
                st.trail.push((tb, vals));
            }
            if st.quiet >= 3 && !st.engine.time_varying {
                // settled before reaching t: stop stepping
                let stp = st.stepper.as_mut().unwrap();
                if stp.time < t {
                    stp.time = t;
                    st.engine.ck.time = t;
                    st.t_prev = t;
                    st.x_prev.copy_from_slice(&st.engine.ck.x);
                }
            }
            if first_only {
                // look for the first crossing in this step before stepping further
                let t1 = self.s.stepper.as_ref().map(|s| s.time).unwrap_or(t).min(t);
                if let Some(tc) = self.scan(t1, true, &mut out) {
                    self.s.t = tc;
                    self.s.trail.retain(|(tt, _)| *tt > tc);
                    return Ok((out, tc));
                }
                self.s.t = t1;
                self.s.trail.retain(|(tt, _)| *tt > t1);
            }
        }
        let mut stop_at = t;
        if let Some(tc) = self.scan(t, first_only, &mut out) {
            stop_at = tc;
        }
        self.s.t = stop_at;
        let now = self.s.t;
        self.s.trail.retain(|(tt, _)| *tt > now);
        Ok((out, stop_at))
    }

    // ---------------------------------------------------------------------------------------
    // Outputs
    // ---------------------------------------------------------------------------------------

    /// Resolves a node name for repeated fast reads.
    pub fn node(&self, name: &str) -> Option<NodeId> {
        self.s.engine.node(name).map(NodeId)
    }

    /// Voltage of a node at the current time (`None` for unknown names). Pending input changes
    /// are not reflected until [`Session::op`] or the next `advance_to`.
    pub fn voltage(&self, name: &str) -> Option<f64> {
        self.node(name).map(|n| self.voltage_at(n))
    }

    /// Voltage of a resolved node.
    pub fn voltage_at(&self, n: NodeId) -> f64 {
        if n.0 == 0 || !self.s.started {
            return if n.0 == 0 {
                0.0
            } else {
                self.s.engine.ck.x.get(n.0).copied().unwrap_or(0.0)
            };
        }
        self.x_at(n.0)
    }

    /// Current of an element (into its first terminal; collector/drain for transistors; for V
    /// sources the SPICE branch current, positive into the + terminal).
    pub fn current(&self, element: &str) -> Option<f64> {
        let i = self.s.engine.device_index(element)?;
        let d = &self.s.engine.devices[i];
        if let Some(br) = d.branch() {
            return Some(self.x_at(br));
        }
        // devices whose current is a function of node voltages: evaluate at the interpolated point
        match d {
            Device::Resistor(r) => Some((self.x_at(r.a) - self.x_at(r.b)) * r.conductance()),
            Device::Port(p) => {
                let (g, i0) = p.norton();
                Some(i0 - g * (self.x_at(p.p) - self.x_at(p.n)))
            }
            _ => d.current(&self.s.engine.ck),
        }
    }

    /// Current delivered by a port into its node (positive when sourcing).
    pub fn port_current(&self, id: PortId) -> Option<f64> {
        let p = self.s.ports.get(id.0)?;
        let Device::Port(d) = &self.s.engine.devices[p.dev] else {
            return None;
        };
        let (g, i0) = d.norton();
        Some(i0 - g * (self.x_at(d.p) - self.x_at(d.n)))
    }

    /// Saves the complete simulation state.
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            state: Box::new(self.s.clone()),
            generation: self.generation,
        }
    }

    /// Restores a snapshot taken from this session (with the same ports).
    pub fn restore(&mut self, snap: &SessionSnapshot) -> Result<(), SimError> {
        if snap.generation != self.generation {
            return Err(SimError::invalid(
                "snapshot was taken before the circuit changed (ports added)",
            ));
        }
        self.s = (*snap.state).clone();
        Ok(())
    }

    /// ngspice-compatible netlist of the current circuit with port drives and `ext()` values
    /// frozen at their current values.
    pub fn to_spice_text(&self) -> String {
        let mut nl = self.netlist.clone();
        for p in &self.s.ports {
            if let Some(el) = nl.top.elements.iter_mut().find(|e| e.name == p.name) {
                el.kind = ElemKind::Port { drive: p.drive };
            }
        }
        crate::netlist::write::netlist_to_text_with(&nl, &self.s.ext_values)
    }
}

pub(crate) fn port_element_name(name: &str) -> String {
    let l = name.to_ascii_lowercase();
    if l.starts_with('p') {
        l
    } else {
        format!("p{l}")
    }
}

fn build_engine(
    nl: &Netlist,
    opts: &SessionOptions,
    ext: &HashMap<String, f64>,
) -> Result<Engine, SimError> {
    let mut c = elab::elaborate(nl, ElabOptions::default())?;
    if let Some(m) = opts.method {
        c.options.method = m;
    }
    let mut e = Engine::with_ext(&c, ext)?;
    e.ck.method = c.options.method;
    Ok(e)
}

fn collect_ports(nl: &Netlist, e: &Engine) -> Vec<PortInfo> {
    let mut v = Vec::new();
    for el in &nl.top.elements {
        if let ElemKind::Port { drive } = el.kind
            && let Some(dev) = e.device_index(&el.name)
        {
            v.push(PortInfo {
                name: el.name.clone(),
                dev,
                drive,
            });
        }
    }
    v
}

impl Stepper {
    /// Consistent re-solve at the current time after an input change: a tiny backward-Euler
    /// step (charges stay continuous, algebraic nodes jump), recorded as a breakpoint so the
    /// following steps restart at first order.
    pub fn resolve(&mut self, e: &mut Engine) -> Result<(), SimError> {
        let itl = e.ck.opts.itl4.max(10) * 3;
        let mut h = (self.tmax * 1e-6).clamp(1e-15, 1e-9);
        let x0 = e.ck.x.clone();
        for attempt in 0..4 {
            {
                let ck = &mut e.ck;
                ck.mode = Mode::Tran;
                ck.time = self.time;
                ck.delta = h;
                ck.delta_old[0] = h;
                ck.order = 1;
                ck.compute_coeffs();
                ck.init = Init::Pred;
            }
            match e.newton(itl) {
                Ok(_) => {
                    for d in e.devices.iter_mut() {
                        d.accept(&e.ck);
                    }
                    e.rotate_states();
                    let ck = &mut e.ck;
                    ck.delta_old = [h, ck.delta_old[0].max(h), ck.delta_old[1], ck.delta_old[2]];
                    self.x_acc.copy_from_slice(&ck.x);
                    self.discontinuity_after_resolve();
                    return Ok(());
                }
                Err(f) => {
                    e.ck.x.copy_from_slice(&x0);
                    if attempt == 3 {
                        return Err(e.nr_error(
                            f,
                            "co-simulation re-solve",
                            Some(self.time),
                            vec!["re-solve with larger steps".into()],
                        ));
                    }
                    h *= 100.0;
                }
            }
        }
        Ok(())
    }
}
