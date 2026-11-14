//! Breadbox analog engine: SPICE netlist parser, MNA solver, device models and analyses.

pub(crate) mod analysis;
pub mod builder;
pub mod devices;
pub mod elab;
pub mod engine;
pub mod error;
pub mod export;
pub mod expr;
pub mod matrix;
pub mod meas;
pub mod netlist;
pub mod results;
mod run;
pub mod session;
pub mod tran;

pub use builder::{CircuitBuilder, ElementRef, SourceDef};
pub use elab::{Circuit, Method, SimOptions};
pub use error::{ConvergenceReport, Diagnostic, ParseError, Severity, SimError, Span};
pub use netlist::ModelKind;
pub use netlist::{Analysis, Library, Netlist, ParseOptions};
pub use results::{Distribution, McResults, Plot, PlotKind, Results, Run, Vector};
pub use run::{CancelToken, RunOptions};
pub use session::{
    Crossing, NodeId, PortId, Session, SessionOptions, SessionSnapshot, SessionStats, WatchId,
};
