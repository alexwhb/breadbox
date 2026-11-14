//! Analysis results: plots of named vectors (like ngspice's plots), grouped into runs
//! (one per `.step` point), plus Monte Carlo distributions and solve statistics.

use crate::error::Diagnostic;
use crate::matrix::C64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Kind of analysis a plot came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlotKind {
    Op,
    Dc,
    Tran,
    Ac,
    /// `.noise` spectral densities (V/√Hz or A/√Hz).
    Noise,
    /// `.noise` integrated totals.
    NoiseTotal,
    Tf,
}

impl PlotKind {
    /// ngspice plot name.
    pub fn plot_name(self) -> &'static str {
        match self {
            PlotKind::Op => "Operating Point",
            PlotKind::Dc => "DC transfer characteristic",
            PlotKind::Tran => "Transient Analysis",
            PlotKind::Ac => "AC Analysis",
            PlotKind::Noise => "Noise Spectral Density Curves",
            PlotKind::NoiseTotal => "Integrated Noise",
            PlotKind::Tf => "Transfer Function",
        }
    }

    pub fn from_plot_name(s: &str) -> Option<PlotKind> {
        let l = s.to_ascii_lowercase();
        Some(if l.starts_with("operating point") {
            PlotKind::Op
        } else if l.starts_with("dc transfer") {
            PlotKind::Dc
        } else if l.starts_with("transient") {
            PlotKind::Tran
        } else if l.starts_with("ac analysis") {
            PlotKind::Ac
        } else if l.starts_with("noise spectral") {
            PlotKind::Noise
        } else if l.starts_with("integrated noise") {
            PlotKind::NoiseTotal
        } else if l.starts_with("transfer function") {
            PlotKind::Tf
        } else {
            return None;
        })
    }
}

/// Physical quantity of a vector (rawfile "type").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quantity {
    Voltage,
    Current,
    Time,
    Frequency,
    Temperature,
    /// Noise density (V²/Hz or V/√Hz).
    Noise,
    Impedance,
    Other,
}

impl Quantity {
    pub fn raw_type(self) -> &'static str {
        match self {
            Quantity::Voltage => "voltage",
            Quantity::Current => "current",
            Quantity::Time => "time",
            Quantity::Frequency => "frequency",
            Quantity::Temperature => "temp-sweep",
            Quantity::Noise => "voltage",
            Quantity::Impedance => "impedance",
            Quantity::Other => "notype",
        }
    }
    pub fn from_raw_type(s: &str) -> Quantity {
        match s {
            "voltage" => Quantity::Voltage,
            "current" => Quantity::Current,
            "time" => Quantity::Time,
            "frequency" => Quantity::Frequency,
            "temp-sweep" => Quantity::Temperature,
            "impedance" => Quantity::Impedance,
            _ => Quantity::Other,
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            Quantity::Voltage | Quantity::Noise => "V",
            Quantity::Current => "A",
            Quantity::Time => "s",
            Quantity::Frequency => "Hz",
            Quantity::Temperature => "°C",
            Quantity::Impedance => "Ω",
            Quantity::Other => "",
        }
    }
}

/// Vector samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Data {
    Real(Vec<f64>),
    Complex(Vec<C64>),
}

/// A named vector (`v(out)`, `i(v1)`, `time`, `frequency`, …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vector {
    pub name: String,
    pub quantity: Quantity,
    pub data: Data,
}

impl Vector {
    pub fn real(name: impl Into<String>, quantity: Quantity, data: Vec<f64>) -> Self {
        Vector {
            name: name.into(),
            quantity,
            data: Data::Real(data),
        }
    }
    pub fn complex(name: impl Into<String>, quantity: Quantity, data: Vec<C64>) -> Self {
        Vector {
            name: name.into(),
            quantity,
            data: Data::Complex(data),
        }
    }
    pub fn len(&self) -> usize {
        match &self.data {
            Data::Real(v) => v.len(),
            Data::Complex(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn is_complex(&self) -> bool {
        matches!(self.data, Data::Complex(_))
    }
    /// Real part (or magnitude-free real value) at `i`.
    pub fn real_at(&self, i: usize) -> f64 {
        match &self.data {
            Data::Real(v) => v[i],
            Data::Complex(v) => v[i].re,
        }
    }
    pub fn complex_at(&self, i: usize) -> C64 {
        match &self.data {
            Data::Real(v) => C64::new(v[i], 0.0),
            Data::Complex(v) => v[i],
        }
    }
    /// Real samples (real part for complex vectors).
    pub fn reals(&self) -> Vec<f64> {
        match &self.data {
            Data::Real(v) => v.clone(),
            Data::Complex(v) => v.iter().map(|c| c.re).collect(),
        }
    }
    /// Real slice if the vector is real.
    pub fn as_real(&self) -> Option<&[f64]> {
        match &self.data {
            Data::Real(v) => Some(v),
            Data::Complex(_) => None,
        }
    }
    pub fn as_complex(&self) -> Option<&[C64]> {
        match &self.data {
            Data::Complex(v) => Some(v),
            Data::Real(_) => None,
        }
    }
}

/// One analysis result: a scale vector (time, frequency, sweep value; absent for `.op`/`.tf`)
/// followed by data vectors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plot {
    pub kind: PlotKind,
    pub title: String,
    /// True when `vectors[0]` is the scale.
    pub has_scale: bool,
    pub vectors: Vec<Vector>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

impl Plot {
    pub fn new(kind: PlotKind, title: impl Into<String>) -> Self {
        Plot {
            kind,
            title: title.into(),
            has_scale: false,
            vectors: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn kind_name(&self) -> &'static str {
        self.kind.plot_name()
    }

    /// Adds a vector (names are matched case-insensitively).
    pub fn push(&mut self, v: Vector) {
        self.index
            .insert(v.name.to_ascii_lowercase(), self.vectors.len());
        self.vectors.push(v);
    }

    /// Adds the scale vector; must be the first vector pushed.
    pub fn push_scale(&mut self, v: Vector) {
        debug_assert!(self.vectors.is_empty());
        self.has_scale = true;
        self.push(v);
    }

    fn rebuild_index(&mut self) {
        self.index = self
            .vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (v.name.to_ascii_lowercase(), i))
            .collect();
    }

    /// Vector by name: `v(out)`, `out` (same as `v(out)`), `i(v1)`, `time`, … (case-insensitive).
    pub fn vector(&self, name: &str) -> Option<&Vector> {
        let l = name.to_ascii_lowercase();
        if self.index.len() != self.vectors.len() {
            // deserialised plot: linear search
            return self.vectors.iter().find(|v| {
                v.name.eq_ignore_ascii_case(&l) || v.name.eq_ignore_ascii_case(&format!("v({l})"))
            });
        }
        self.index
            .get(&l)
            .or_else(|| self.index.get(&format!("v({l})")))
            .map(|&i| &self.vectors[i])
    }

    /// Real samples of a vector.
    pub fn real(&self, name: &str) -> Option<Vec<f64>> {
        self.vector(name).map(|v| v.reals())
    }

    /// Single value (for `.op`/`.tf` plots, or the last point of a sweep).
    pub fn value(&self, name: &str) -> Option<f64> {
        let v = self.vector(name)?;
        if v.is_empty() {
            None
        } else {
            Some(v.real_at(v.len() - 1))
        }
    }

    pub fn scale(&self) -> Option<&Vector> {
        if self.has_scale {
            self.vectors.first()
        } else {
            None
        }
    }

    pub fn scale_values(&self) -> Vec<f64> {
        match self.scale() {
            Some(s) => s.reals(),
            None => (0..self.len()).map(|i| i as f64).collect(),
        }
    }

    pub fn scale_at(&self, i: usize) -> f64 {
        match self.scale() {
            Some(s) => s.real_at(i),
            None => i as f64,
        }
    }

    /// Number of points.
    pub fn len(&self) -> usize {
        self.vectors.first().map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Names of all vectors.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.vectors.iter().map(|v| v.name.as_str())
    }

    /// Restores the lookup index after deserialisation.
    pub fn reindex(&mut self) {
        self.rebuild_index();
    }
}

/// Result of a `.meas`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasResult {
    pub name: String,
    pub value: Result<f64, String>,
}

/// Results of one pass over the analyses (one `.step` point, one temperature, one MC run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Run {
    /// Human-readable label, e.g. `r1v=4.7k` (empty without `.step`).
    pub label: String,
    /// Swept values for this run.
    pub params: Vec<(String, f64)>,
    pub plots: Vec<Plot>,
    pub measures: Vec<MeasResult>,
}

impl Run {
    /// First plot of a kind.
    pub fn plot(&self, kind: PlotKind) -> Option<&Plot> {
        self.plots.iter().find(|p| p.kind == kind)
    }
    pub fn measure(&self, name: &str) -> Option<f64> {
        self.measures
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .and_then(|m| m.value.clone().ok())
    }
}

/// Summary statistics of a Monte Carlo quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Distribution {
    pub name: String,
    pub values: Vec<f64>,
    pub mean: f64,
    /// Sample standard deviation.
    pub sigma: f64,
    pub min: f64,
    pub max: f64,
}

impl Distribution {
    pub fn from_values(name: impl Into<String>, values: Vec<f64>) -> Self {
        let n = values.len().max(1) as f64;
        let mean = values.iter().sum::<f64>() / n;
        let var = if values.len() > 1 {
            values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (values.len() - 1) as f64
        } else {
            0.0
        };
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Distribution {
            name: name.into(),
            values,
            mean,
            sigma: var.sqrt(),
            min,
            max,
        }
    }

    /// Histogram with `bins` equal-width bins over `[min, max]`: `(bin_start, bin_end, count)`.
    pub fn histogram(&self, bins: usize) -> Vec<(f64, f64, usize)> {
        let bins = bins.max(1);
        let w = (self.max - self.min) / bins as f64;
        let mut h: Vec<(f64, f64, usize)> = (0..bins)
            .map(|i| (self.min + w * i as f64, self.min + w * (i + 1) as f64, 0))
            .collect();
        for &v in &self.values {
            let i = if w > 0.0 {
                (((v - self.min) / w) as usize).min(bins - 1)
            } else {
                0
            };
            h[i].2 += 1;
        }
        h
    }

    /// Fraction of values above a threshold (e.g. firmware threshold marker).
    pub fn fraction_above(&self, threshold: f64) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        self.values.iter().filter(|&&v| v > threshold).count() as f64 / self.values.len() as f64
    }
}

/// Monte Carlo results: one [`Run`] per iteration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct McResults {
    pub runs: Vec<Run>,
    pub seed: u64,
}

impl McResults {
    /// Distribution of an operating-point vector (e.g. `v(a1)`) or a measurement name.
    pub fn distribution(&self, name: &str) -> Option<Distribution> {
        let mut vals = Vec::with_capacity(self.runs.len());
        for r in &self.runs {
            let v = r
                .measure(name)
                .or_else(|| r.plot(PlotKind::Op).and_then(|p| p.value(name)));
            vals.push(v?);
        }
        Some(Distribution::from_values(name, vals))
    }
}

/// Counters describing how hard the solver worked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SolveStats {
    /// Wall-clock solve time.
    pub elapsed: Duration,
    pub newton_iterations: u64,
    pub timesteps: u64,
    pub rejected_timesteps: u64,
    /// Number of times gmin or source stepping, or a timestep cut, was needed.
    pub convergence_warnings: u64,
    /// Number of analysis passes (`.step` points × temperatures).
    pub runs: usize,
}

/// Everything produced by [`crate::Netlist::run`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Results {
    pub title: String,
    pub runs: Vec<Run>,
    pub mc: Option<McResults>,
    pub stats: SolveStats,
    pub warnings: Vec<Diagnostic>,
}

impl Results {
    /// First plot of a kind in the first run.
    pub fn plot(&self, kind: PlotKind) -> Option<&Plot> {
        self.runs.first().and_then(|r| r.plot(kind))
    }
    /// All plots of a kind across runs (a `.step` family).
    pub fn family(&self, kind: PlotKind) -> Vec<(&Run, &Plot)> {
        self.runs
            .iter()
            .filter_map(|r| r.plot(kind).map(|p| (r, p)))
            .collect()
    }
    /// Operating-point value from the first run.
    pub fn op(&self, name: &str) -> Option<f64> {
        self.plot(PlotKind::Op).and_then(|p| p.value(name))
    }
    /// Measurement value from the first run.
    pub fn measure(&self, name: &str) -> Option<f64> {
        self.runs.first().and_then(|r| r.measure(name))
    }
    /// Header line like the Pro screen's: `3 steps · 1.82 s solve · 0 convergence warnings`.
    pub fn summary(&self) -> String {
        let n = self.stats.runs.max(self.runs.len());
        format!(
            "{n} step{} · {} solve · {} convergence warning{}",
            if n == 1 { "" } else { "s" },
            fmt_duration(self.stats.elapsed),
            self.stats.convergence_warnings,
            if self.stats.convergence_warnings == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s >= 1.0 {
        format!("{s:.2} s")
    } else {
        format!("{:.0} ms", s * 1e3)
    }
}
