//! Error and diagnostic types.
//!
//! Parsing problems are [`ParseError`]s carrying a [`Span`] (file, 1-based line and column) so the
//! Pro-mode editor can underline them. Problems found while running analyses are [`SimError`]s;
//! convergence failures name the offending nodes and elements ([`ConvergenceReport`]).

use std::fmt;
use std::sync::Arc;
use thiserror::Error;

/// A location in netlist source text. Lines and columns are 1-based; `line == 0` means
/// "no location" (for programmatically built circuits).
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct Span {
    /// File name (`None` for the top-level text passed to [`crate::Netlist::parse`]).
    pub file: Option<Arc<str>>,
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn new(file: Option<Arc<str>>, line: u32, col: u32) -> Self {
        Span { file, line, col }
    }
    /// A span that points nowhere (programmatic elements).
    pub fn none() -> Self {
        Span::default()
    }
    pub fn is_none(&self) -> bool {
        self.line == 0
    }
    /// Same line, different column.
    pub fn with_col(&self, col: u32) -> Self {
        Span {
            file: self.file.clone(),
            line: self.line,
            col,
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            return write!(f, "<builder>");
        }
        match &self.file {
            Some(file) => write!(f, "{}:{}:{}", file, self.line, self.col),
            None => write!(f, "line {}:{}", self.line, self.col),
        }
    }
}

/// A syntax or semantic error in netlist text, with its location.
#[derive(Debug, Clone, Error, PartialEq)]
#[error("{span}: {msg}")]
pub struct ParseError {
    pub span: Span,
    pub msg: String,
}

impl ParseError {
    pub fn new(span: Span, msg: impl Into<String>) -> Self {
        ParseError {
            span,
            msg: msg.into(),
        }
    }
}

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Severity {
    Info,
    Warning,
}

/// A non-fatal message produced while parsing or simulating (unknown model parameter, level-3
/// MOSFET fallback, gmin stepping used, …).
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub span: Span,
    pub msg: String,
}

impl Diagnostic {
    pub fn warning(span: Span, msg: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            span,
            msg: msg.into(),
        }
    }
    pub fn info(span: Span, msg: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Info,
            span,
            msg: msg.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Info => "note",
            Severity::Warning => "warning",
        };
        if self.span.is_none() {
            write!(f, "{sev}: {}", self.msg)
        } else {
            write!(f, "{}: {sev}: {}", self.span, self.msg)
        }
    }
}

/// Details of a Newton-Raphson failure: which unknowns were still moving and which devices were
/// still being limited when the iteration budget ran out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvergenceReport {
    /// `(unknown name, last value, last update)` for the worst offenders, worst first.
    /// Names look like `V(out)`, `I(V1)` or `V(q1#base)` for internal nodes.
    pub unknowns: Vec<(String, f64, f64)>,
    /// Elements that reported non-convergence (junction limiting, switch chatter, …).
    pub elements: Vec<String>,
    /// Newton iterations performed in the final attempt.
    pub iterations: usize,
    /// Convergence aids that were tried (`"gmin stepping"`, `"source stepping"`).
    pub aids_tried: Vec<String>,
}

impl fmt::Display for ConvergenceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.unknowns.is_empty() {
            write!(f, "worst unknowns: ")?;
            for (i, (n, v, d)) in self.unknowns.iter().take(4).enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{n} = {} (Δ {})", fmt_num(*v), fmt_num(*d))?;
            }
        }
        if !self.elements.is_empty() {
            if !self.unknowns.is_empty() {
                write!(f, "; ")?;
            }
            write!(
                f,
                "non-converging elements: {}",
                self.elements
                    .iter()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        if !self.aids_tried.is_empty() {
            write!(f, "; tried {}", self.aids_tried.join(", "))?;
        }
        Ok(())
    }
}

fn fmt_num(v: f64) -> String {
    format!("{v:.4e}")
}

/// Errors raised while elaborating or simulating a circuit.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum SimError {
    /// The netlist parsed but cannot be turned into a circuit (unknown model, unknown subcircuit,
    /// bad parameter value, recursive subcircuit, …).
    #[error("{span}: {msg}")]
    Elaboration { span: Span, msg: String },
    /// The DC operating point (or a DC sweep point) did not converge.
    #[error("{analysis}: no convergence{}: {report}", at_time(*time))]
    NoConvergence {
        analysis: String,
        time: Option<f64>,
        report: ConvergenceReport,
    },
    /// The transient step shrank below the minimum step.
    #[error("transient: timestep too small at t = {time:.6e} s (step {step:.3e} s): {report}")]
    TimestepTooSmall {
        time: f64,
        step: f64,
        report: ConvergenceReport,
    },
    /// The MNA matrix is structurally or numerically singular.
    #[error("singular matrix: {detail}")]
    Singular { detail: String },
    /// A node, element or vector name that does not exist.
    #[error("unknown {kind} `{name}`")]
    Unknown { kind: &'static str, name: String },
    /// An argument was out of range or inconsistent.
    #[error("{0}")]
    Invalid(String),
    /// The run was cancelled through a [`crate::CancelToken`].
    #[error("cancelled")]
    Cancelled,
}

fn at_time(t: Option<f64>) -> String {
    match t {
        Some(t) => format!(" at t = {t:.6e} s"),
        None => String::new(),
    }
}

impl SimError {
    pub fn elab(span: &Span, msg: impl Into<String>) -> Self {
        SimError::Elaboration {
            span: span.clone(),
            msg: msg.into(),
        }
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        SimError::Invalid(msg.into())
    }
}

impl From<ParseError> for SimError {
    fn from(e: ParseError) -> Self {
        SimError::Elaboration {
            span: e.span,
            msg: e.msg,
        }
    }
}

/// Levenshtein distance, used to suggest names in error messages.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// `" (did you mean `x`?)"` if a candidate is close enough, else an empty string.
pub(crate) fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let d = edit_distance(name, c);
        if d <= 2.max(name.len() / 3) && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    match best {
        Some((_, c)) => format!(" (did you mean `{c}`?)"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_display() {
        assert_eq!(Span::new(None, 3, 7).to_string(), "line 3:7");
        assert_eq!(
            Span::new(Some("a.cir".into()), 3, 7).to_string(),
            "a.cir:3:7"
        );
    }

    #[test]
    fn suggestions() {
        assert_eq!(
            suggest("led_ambr", ["led_amber", "led_red"]),
            " (did you mean `led_amber`?)"
        );
        assert_eq!(suggest("zzzzzz", ["led_amber"]), "");
    }
}
