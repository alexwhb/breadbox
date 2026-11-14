//! Breadbox analog engine: SPICE netlist parser, MNA solver, device models and analyses.

pub mod elab;
pub mod error;
pub mod expr;
pub mod matrix;
pub mod meas;
pub mod netlist;
pub mod results;

pub use elab::{Circuit, Method, SimOptions};
pub use error::{ConvergenceReport, Diagnostic, ParseError, Severity, SimError, Span};
pub use netlist::{Analysis, Library, Netlist, ParseOptions};
pub use results::{Distribution, McResults, Plot, PlotKind, Results, Run, Vector};
