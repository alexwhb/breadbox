//! Syntax-level netlist model shared by the text parser and [`crate::CircuitBuilder`].
//!
//! A [`Netlist`] keeps elements, models, parameters and subcircuit definitions *unflattened*,
//! with expression-valued parameters, so `.step`/`.mc`/`.dc param` can re-elaborate it cheaply
//! and [`Netlist::to_spice_text`] can round-trip it.

mod parse;
pub(crate) mod write;

use crate::error::{Diagnostic, ParseError, Span};
use crate::expr::Expr;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use parse::ParseOptions;

/// Which kind of value an element parameter holds.
pub type ParamList = Vec<(String, Expr)>;

/// Independent source waveform.
#[derive(Debug, Clone, PartialEq)]
pub enum Waveform {
    /// `PULSE(v1 v2 td tr tf pw per [np])`
    Pulse(Vec<Expr>),
    /// `SIN(vo va freq td theta phase)`
    Sin(Vec<Expr>),
    /// `EXP(v1 v2 td1 tau1 td2 tau2)`
    Exp(Vec<Expr>),
    /// `PWL(t1 v1 t2 v2 …) [r=t] [td=t]`
    Pwl {
        points: Vec<Expr>,
        repeat: Option<Expr>,
        delay: Option<Expr>,
    },
    /// `SFFM(vo va fc mdi fs)`
    Sffm(Vec<Expr>),
    /// `AM(va vo mf fc td)`
    Am(Vec<Expr>),
}

impl Waveform {
    pub fn keyword(&self) -> &'static str {
        match self {
            Waveform::Pulse(_) => "PULSE",
            Waveform::Sin(_) => "SIN",
            Waveform::Exp(_) => "EXP",
            Waveform::Pwl { .. } => "PWL",
            Waveform::Sffm(_) => "SFFM",
            Waveform::Am(_) => "AM",
        }
    }
}

/// Independent V/I source specification.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceSpec {
    pub dc: Option<Expr>,
    pub ac_mag: Option<Expr>,
    pub ac_phase: Option<Expr>,
    pub wave: Option<Waveform>,
}

/// How a controlled source is controlled.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Linear: E/G use the two controlling nodes, F/H the controlling V source.
    Linear {
        ctrl_nodes: Vec<String>,
        ctrl_src: Option<String>,
        gain: Expr,
    },
    /// `POLY(n)`; controlling node pairs (E/G) or sources (F/H) and coefficients.
    Poly {
        dims: usize,
        ctrl_nodes: Vec<String>,
        ctrl_srcs: Vec<String>,
        coeffs: Vec<Expr>,
    },
    /// `VALUE={expr}` / `vol=` / `cur=` (behavioural).
    Value(Expr),
    /// `TABLE {expr} = (x1,y1) (x2,y2) …`
    Table {
        input: Expr,
        points: Vec<(Expr, Expr)>,
    },
}

/// Element-specific syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum ElemKind {
    Resistor {
        value: Option<Expr>,
        model: Option<String>,
    },
    Capacitor {
        value: Option<Expr>,
        model: Option<String>,
    },
    Inductor {
        value: Option<Expr>,
        model: Option<String>,
    },
    /// `K1 L1 L2 k`
    Coupling {
        inductors: Vec<String>,
        k: Expr,
    },
    Vsource(SourceSpec),
    Isource(SourceSpec),
    /// E (voltage-controlled voltage source)
    Vcvs(Control),
    /// F (current-controlled current source)
    Cccs(Control),
    /// G (voltage-controlled current source)
    Vccs(Control),
    /// H (current-controlled voltage source)
    Ccvs(Control),
    /// B source: `V=expr` (`is_current == false`) or `I=expr`.
    Behavioral {
        is_current: bool,
        expr: Expr,
    },
    /// S: voltage-controlled switch (nodes: p n cp cn)
    Switch {
        model: String,
        init_on: Option<bool>,
    },
    /// W: current-controlled switch (nodes: p n)
    CSwitch {
        ctrl: String,
        model: String,
        init_on: Option<bool>,
    },
    Diode {
        model: String,
        area: Option<Expr>,
        off: bool,
    },
    /// Q (nodes c b e [s]); `model` resolved during parsing heuristics
    Bjt {
        model: String,
        area: Option<Expr>,
        off: bool,
    },
    Mosfet {
        model: String,
        off: bool,
    },
    Jfet {
        model: String,
        area: Option<Expr>,
        off: bool,
    },
    /// T (lossless line; nodes p1+ p1- p2+ p2-)
    Tline,
    /// X (subcircuit instance)
    Subckt {
        name: String,
    },
    /// Breadbox co-simulation port (an MCU pin or device output driving `nodes[0]`
    /// against `nodes[1]`). Exported to `.cir` as an equivalent V+R / I source.
    Port {
        drive: bb_core::PinDrive,
    },
}

/// One element card (`R1 a b 10k tc1=…`).
#[derive(Debug, Clone, PartialEq)]
pub struct ElementCard {
    /// Lower-case name including the type letter (`r1`, `xu1`).
    pub name: String,
    pub nodes: Vec<String>,
    pub kind: ElemKind,
    /// Named instance parameters (`tc1`, `ic`, `w`, `l`, `m`, `temp`, `tol`, subckt params …).
    pub params: ParamList,
    pub span: Span,
}

impl ElementCard {
    pub fn param(&self, key: &str) -> Option<&Expr> {
        self.params
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

/// Model type keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    D,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    Njf,
    Pjf,
    Sw,
    Csw,
    R,
    C,
    L,
}

impl ModelKind {
    pub fn from_keyword(s: &str) -> Option<Self> {
        Some(match s {
            "d" => ModelKind::D,
            "npn" => ModelKind::Npn,
            "pnp" => ModelKind::Pnp,
            "nmos" => ModelKind::Nmos,
            "pmos" => ModelKind::Pmos,
            "njf" => ModelKind::Njf,
            "pjf" => ModelKind::Pjf,
            "sw" => ModelKind::Sw,
            "csw" => ModelKind::Csw,
            "r" | "res" => ModelKind::R,
            "c" | "cap" => ModelKind::C,
            "l" | "ind" => ModelKind::L,
            _ => return None,
        })
    }
    pub fn keyword(self) -> &'static str {
        match self {
            ModelKind::D => "D",
            ModelKind::Npn => "NPN",
            ModelKind::Pnp => "PNP",
            ModelKind::Nmos => "NMOS",
            ModelKind::Pmos => "PMOS",
            ModelKind::Njf => "NJF",
            ModelKind::Pjf => "PJF",
            ModelKind::Sw => "SW",
            ModelKind::Csw => "CSW",
            ModelKind::R => "R",
            ModelKind::C => "C",
            ModelKind::L => "L",
        }
    }
}

/// `.model name type(params)`
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCard {
    pub name: String,
    pub kind: ModelKind,
    pub params: ParamList,
    pub span: Span,
}

/// `.func name(args) = expr`
#[derive(Debug, Clone, PartialEq)]
pub struct FuncDef {
    pub name: String,
    pub args: Vec<String>,
    pub body: Expr,
    pub span: Span,
}

/// A subcircuit body (or the top level).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scope {
    pub name: String,
    pub ports: Vec<String>,
    /// Default instance parameters (`params: a=1`).
    pub defaults: ParamList,
    /// `.param` definitions in order.
    pub params: Vec<(String, Expr, Span)>,
    pub funcs: Vec<FuncDef>,
    pub elements: Vec<ElementCard>,
    pub models: Vec<ModelCard>,
    pub subckts: Vec<Arc<Scope>>,
    pub span: Span,
}

impl Scope {
    pub fn find_subckt(&self, name: &str) -> Option<&Arc<Scope>> {
        self.subckts.iter().find(|s| s.name == name)
    }
    pub fn find_model(&self, name: &str) -> Option<&ModelCard> {
        self.models.iter().rev().find(|m| m.name == name)
    }
}

/// Subcircuit/model library that callers can register programmatically
/// ([`Netlist::add_library`]), e.g. Breadbox-provided parts like `CAPSOIL`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Library {
    pub scope: Scope,
}

impl Library {
    /// Parses library text (`.subckt` / `.model` / `.param` / `.func` cards; no title line).
    pub fn parse(text: &str) -> Result<Library, ParseError> {
        let opts = ParseOptions {
            first_line_is_title: false,
            ..ParseOptions::default()
        };
        let nl = Netlist::parse_with(text, &opts)?;
        Ok(Library { scope: nl.top })
    }
}

/// Sweep spacing.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepKind {
    /// Linear with increment.
    Lin { step: f64 },
    /// Linear with a number of points (`.ac lin`).
    LinPoints(usize),
    /// Points per decade.
    Dec(usize),
    /// Points per octave.
    Oct(usize),
    /// Explicit list (start/stop ignored).
    List(Vec<f64>),
}

/// What a DC sweep / `.step` varies.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepVar {
    /// An independent V or I source's DC value (or, if no such source exists, an element value
    /// like `R1` or a `.param` — resolved at run time).
    Name(String),
    /// Explicit `.param` (`param x` syntax).
    Param(String),
    /// Circuit temperature in °C.
    Temp,
}

impl SweepVar {
    pub fn label(&self) -> String {
        match self {
            SweepVar::Name(n) | SweepVar::Param(n) => n.clone(),
            SweepVar::Temp => "temp".into(),
        }
    }
}

/// One sweep dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    pub var: SweepVar,
    pub start: f64,
    pub stop: f64,
    pub kind: SweepKind,
}

impl Sweep {
    /// Expands to the list of sweep values.
    pub fn values(&self) -> Vec<f64> {
        sweep_values(self.start, self.stop, &self.kind)
    }
}

pub(crate) fn sweep_values(start: f64, stop: f64, kind: &SweepKind) -> Vec<f64> {
    let mut v = Vec::new();
    match kind {
        SweepKind::List(l) => v.extend_from_slice(l),
        SweepKind::Lin { step } => {
            if *step == 0.0 || (stop - start) * step < 0.0 {
                v.push(start);
            } else {
                let n = ((stop - start) / step + 1e-9).floor() as usize;
                for i in 0..=n.min(10_000_000) {
                    v.push(start + i as f64 * step);
                }
                if let Some(&last) = v.last() {
                    if (last - stop).abs() > 1e-9 * step.abs() && (stop - last) * step > 0.0 {
                        v.push(stop);
                    }
                }
            }
        }
        SweepKind::LinPoints(n) => {
            let n = (*n).max(1);
            if n == 1 {
                v.push(start);
            } else {
                for i in 0..n {
                    v.push(start + (stop - start) * i as f64 / (n - 1) as f64);
                }
            }
        }
        SweepKind::Dec(p) | SweepKind::Oct(p) => {
            let base: f64 = if matches!(kind, SweepKind::Dec(_)) {
                10.0
            } else {
                2.0
            };
            if start <= 0.0 || stop <= 0.0 || *p == 0 {
                v.push(start);
            } else {
                let ratio = base.powf(1.0 / *p as f64);
                let n = ((stop / start).ln() / ratio.ln() + 1e-9).floor() as usize;
                for i in 0..=n.min(10_000_000) {
                    v.push(start * ratio.powi(i as i32));
                }
                if let Some(&last) = v.last() {
                    if last < stop * (1.0 - 1e-9) {
                        v.push(stop);
                    }
                }
            }
        }
    }
    v
}

/// Transient analysis parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct TranSpec {
    pub tstep: f64,
    pub tstop: f64,
    pub tstart: f64,
    pub tmax: Option<f64>,
    pub uic: bool,
}

/// Output of `.tf` / `.noise`: a node voltage (difference) or a source current.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputVar {
    Voltage(String, Option<String>),
    Current(String),
}

impl OutputVar {
    pub fn label(&self) -> String {
        match self {
            OutputVar::Voltage(p, None) => format!("v({p})"),
            OutputVar::Voltage(p, Some(n)) => format!("v({p},{n})"),
            OutputVar::Current(s) => format!("i({s})"),
        }
    }
}

/// An analysis request.
#[derive(Debug, Clone, PartialEq)]
pub enum Analysis {
    /// `.op`
    Op,
    /// `.dc` — first sweep is the inner (fastest) loop.
    Dc(Vec<Sweep>),
    /// `.tran`
    Tran(TranSpec),
    /// `.ac` — sweep over frequency (`start`/`stop` in Hz).
    Ac(Sweep),
    /// `.noise v(out[,ref]) src …`
    Noise {
        output: OutputVar,
        input: String,
        sweep: Sweep,
        pts_per_summary: usize,
    },
    /// `.tf outvar insrc`
    Tf { output: OutputVar, input: String },
}

impl Analysis {
    pub fn keyword(&self) -> &'static str {
        match self {
            Analysis::Op => "op",
            Analysis::Dc(_) => "dc",
            Analysis::Tran(_) => "tran",
            Analysis::Ac(_) => "ac",
            Analysis::Noise { .. } => "noise",
            Analysis::Tf { .. } => "tf",
        }
    }
}

/// `.step` (one nested loop level; the first `.step` line is the outer loop).
#[derive(Debug, Clone, PartialEq)]
pub struct StepSpec {
    pub var: SweepVar,
    pub values: Vec<f64>,
    pub span: Span,
}

/// Monte Carlo distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dist {
    /// Gaussian with σ = tol/3 (so ±tol is the 3σ band), clipped to ±tol.
    #[default]
    Gauss,
    /// Uniform in ±tol.
    Uniform,
}

/// `.mc runs [tol=5%] [seed=7] [dist=gauss|uniform] [analysis=op|dc|tran|ac]`.
#[derive(Debug, Clone, PartialEq)]
pub struct McSpec {
    pub runs: usize,
    /// Global tolerance applied to R, C and L values without their own `tol=`.
    pub tol: Option<f64>,
    pub seed: u64,
    pub dist: Dist,
    /// Which analyses to repeat (default: `.op` only).
    pub analyses: Vec<String>,
    pub span: Span,
}

/// `.meas` statement (see [`crate::meas`]).
#[derive(Debug, Clone, PartialEq)]
pub struct MeasSpec {
    pub name: String,
    /// `tran`, `ac`, `dc`.
    pub analysis: String,
    pub kind: crate::meas::MeasKind,
    pub span: Span,
}

/// A parsed netlist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Netlist {
    pub title: String,
    pub top: Scope,
    pub analyses: Vec<(Analysis, Span)>,
    pub steps: Vec<StepSpec>,
    pub mc: Option<McSpec>,
    pub measures: Vec<MeasSpec>,
    /// `.options` key/values (`None` for flags).
    pub options: Vec<(String, Option<f64>, Span)>,
    /// `.temp` values (more than one = temperature sweep).
    pub temps: Vec<f64>,
    pub ics: Vec<(String, Expr, Span)>,
    pub nodesets: Vec<(String, Expr, Span)>,
    pub globals: Vec<String>,
    /// `.save` / `.probe` vectors (empty = save everything).
    pub saves: Vec<String>,
    pub warnings: Vec<Diagnostic>,
    pub libraries: Vec<Arc<Library>>,
    /// Directory used to resolve `.include` paths.
    pub base_dir: Option<PathBuf>,
}

impl Netlist {
    /// Parses SPICE3/ngspice-compatible netlist text. The first line is the title.
    /// `.include`/`.lib` paths are resolved relative to the current directory.
    ///
    /// ```
    /// use bb_spice::Netlist;
    /// let nl = Netlist::parse("divider\nV1 in 0 5\nR1 in out 1k\nR2 out 0 1k\n.op\n.end\n").unwrap();
    /// assert_eq!(nl.top.elements.len(), 3);
    /// ```
    pub fn parse(text: &str) -> Result<Netlist, ParseError> {
        Netlist::parse_with(text, &ParseOptions::default())
    }

    /// Parses with explicit options (include directory, file name for spans, libraries).
    pub fn parse_with(text: &str, opts: &ParseOptions) -> Result<Netlist, ParseError> {
        parse::parse_netlist(text, opts)
    }

    /// Reads and parses a file; includes resolve relative to its directory.
    pub fn parse_file(path: impl AsRef<std::path::Path>) -> Result<Netlist, ParseError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            ParseError::new(Span::none(), format!("cannot read {}: {e}", path.display()))
        })?;
        let opts = ParseOptions {
            file_name: Some(
                path.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ),
            base_dir: path.parent().map(|p| p.to_path_buf()),
            ..ParseOptions::default()
        };
        Netlist::parse_with(&text, &opts)
    }

    /// Registers a library of subcircuits/models used to resolve `X` instances and model names
    /// that the netlist itself does not define.
    pub fn add_library(&mut self, lib: Arc<Library>) {
        self.libraries.push(lib);
    }

    /// Value of an `.options` entry.
    pub fn option(&self, key: &str) -> Option<f64> {
        self.options
            .iter()
            .rev()
            .find(|(k, _, _)| k == key)
            .and_then(|(_, v, _)| *v)
    }

    /// True if an `.options` flag is present.
    pub fn has_option(&self, key: &str) -> bool {
        self.options.iter().any(|(k, _, _)| k == key)
    }

    /// Top-level `.param` names and their (nominal) expressions.
    pub fn params(&self) -> impl Iterator<Item = (&str, &Expr)> {
        self.top.params.iter().map(|(n, e, _)| (n.as_str(), e))
    }

    /// Element by name (top level).
    pub fn element(&self, name: &str) -> Option<&ElementCard> {
        let name = name.to_ascii_lowercase();
        self.top.elements.iter().find(|e| e.name == name)
    }

    /// Mutable element by name (top level).
    pub fn element_mut(&mut self, name: &str) -> Option<&mut ElementCard> {
        let name = name.to_ascii_lowercase();
        self.top.elements.iter_mut().find(|e| e.name == name)
    }

    /// Sets (or adds) a top-level `.param`.
    pub fn set_param(&mut self, name: &str, value: Expr) {
        let name = name.to_ascii_lowercase();
        if let Some(p) = self.top.params.iter_mut().find(|p| p.0 == name) {
            p.1 = value;
        } else {
            self.top.params.push((name, value, Span::none()));
        }
    }

    /// Writes ngspice-compatible netlist text (see [`crate::export`]).
    pub fn to_spice_text(&self) -> String {
        write::netlist_to_text(self)
    }

    pub(crate) fn subckt_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.top.subckts.iter().map(|s| s.name.clone()).collect();
        for l in &self.libraries {
            v.extend(l.scope.subckts.iter().map(|s| s.name.clone()));
        }
        v
    }

    pub(crate) fn lookup_global_subckt(&self, name: &str) -> Option<Arc<Scope>> {
        self.top.find_subckt(name).cloned().or_else(|| {
            self.libraries
                .iter()
                .rev()
                .find_map(|l| l.scope.find_subckt(name).cloned())
        })
    }

    pub(crate) fn lookup_global_model(&self, name: &str) -> Option<&ModelCard> {
        self.top.find_model(name).or_else(|| {
            self.libraries
                .iter()
                .rev()
                .find_map(|l| l.scope.find_model(name))
        })
    }

    /// All model names visible at top level (for suggestions).
    pub(crate) fn model_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.top.models.iter().map(|m| m.name.clone()).collect();
        for l in &self.libraries {
            v.extend(l.scope.models.iter().map(|m| m.name.clone()));
        }
        v
    }

    /// Library-level params and funcs (merged into the global scope during elaboration).
    pub(crate) fn library_params(&self) -> Vec<(String, Expr, Span)> {
        let mut v = Vec::new();
        for l in &self.libraries {
            v.extend(l.scope.params.iter().cloned());
        }
        v
    }

    pub(crate) fn library_funcs(&self) -> HashMap<String, FuncDef> {
        let mut m = HashMap::new();
        for l in &self.libraries {
            for f in &l.scope.funcs {
                m.insert(f.name.clone(), f.clone());
            }
        }
        m
    }
}
