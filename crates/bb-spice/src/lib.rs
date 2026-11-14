//! # bb-spice — Breadbox's analog engine
//!
//! A SPICE3/ngspice-compatible circuit simulator written for Breadbox: netlist parser, modified
//! nodal analysis with sparse LU, SPICE device models, the classic analyses, and an incremental
//! transient [`Session`] for mixed-signal co-simulation with real firmware.
//!
//! ## Pro mode: parse and run a netlist
//!
//! ```
//! use bb_spice::{Netlist, PlotKind};
//!
//! let text = "rc filter
//! V1 in 0 DC 0 AC 1 PULSE(0 5 0 1u 1u 1m 2m)
//! R1 in out {rval}
//! C1 out 0 100n
//! .param rval=1k
//! .op
//! .ac dec 10 10 100k
//! .tran 1u 3m
//! .step param rval list 1k 2.2k
//! .meas tran vmax max v(out)
//! .end
//! ";
//! let results = Netlist::parse(text)?.run()?;
//! assert_eq!(results.runs.len(), 2); // one run per .step value
//! let ac = results.plot(PlotKind::Ac).unwrap();
//! let fc_gain = ac.vector("v(out)").unwrap().complex_at(10); // 100 Hz … 1.59 kHz corner
//! assert!(fc_gain.abs() < 1.0);
//! assert!(results.measure("vmax").unwrap() > 4.9);
//! println!("{}", results.summary()); // "2 steps · 3 ms solve · 0 convergence warnings"
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Co-simulation: drive a circuit from an MCU
//!
//! ```
//! use bb_spice::{CircuitBuilder, ModelKind, Session};
//! use bb_core::PinDrive;
//!
//! let mut b = CircuitBuilder::new("night light");
//! let d9 = b.port("d9", "d9", "0", PinDrive::HighZ);           // MCU pin
//! b.resistor("2", "d9", "n1", 220.0);
//! b.model("led_amber", ModelKind::D, &[("is", 1e-19), ("n", 1.8), ("rs", 6.0)]);
//! b.diode("1", "n1", "0", "led_amber");
//! b.vsource_dc("cc", "vcc", "0", 5.0);
//! b.resistor_expr("ldr", "vcc", "a1", "350k*pwr(max(ext(\"J2.lux\"),0.01),-0.7)")?;
//! b.resistor("1", "a1", "0", 10e3);
//! let mut s = Session::new(&b.build())?;
//! let a1 = s.node("a1").unwrap();
//! s.set_ext("J2.lux", 180.0);                        // world → circuit
//! s.set_port(d9, PinDrive::Thevenin { v: 5.0, r: 25.0 }); // firmware drives the pin
//! s.advance_to(1e-3)?;                               // 1 ms of simulated time
//! let adc = s.voltage_at(a1);                        // ADC sample
//! assert!((adc - 2.598).abs() < 0.01);
//! assert!(s.current("d1").unwrap() > 5e-3);         // LED on
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## What is supported
//!
//! * **Syntax** (SPICE-1): title line, `*`/`;`/`$` comments, `+` continuation, case-insensitive
//!   names, SI suffixes (`f p n u µ m k meg g t mil`), `{expr}` and `'expr'`, `.param`,
//!   `.func`, `.model`, `.subckt`/`.ends` with `params:` and nesting, `.include`, `.lib`,
//!   `.global`, `.temp`, `.options`, `.ic`, `.nodeset`, `.end`; `.control` blocks are skipped.
//!   Errors carry file/line/column ([`ParseError`]). See [`netlist`] and [`expr`].
//! * **Elements** (SPICE-2): R C L K, V/I (DC, AC, PULSE, SIN, EXP, PWL with `r=`/`td=`, SFFM,
//!   AM), E F G H (linear, `POLY(n)`, `VALUE=`, `TABLE`), B sources, S/W switches with
//!   hysteresis, D, Q (Gummel-Poon), M (level 1 + Meyer; other levels fall back with a
//!   warning), J, T (lossless), X, and co-simulation ports. Models follow ngspice's equations
//!   (`src/devices/`), including temperature scaling.
//! * **Analyses** (SPICE-3): `.op`, `.dc` (source, element, `.param`, temperature; lin/dec/oct/
//!   list; two nested), `.tran` (LTE-controlled trapezoidal / Gear-2 / backward Euler,
//!   breakpoints, `uic`, `tmax`), `.ac`, `.noise`, `.tf`, `.step` (nested), `.mc`
//!   (Breadbox: `.mc runs [tol=5%] [seed=n] [dist=gauss|uniform] [analysis=op|dc|tran|ac]`),
//!   multi-value `.temp`, `.meas`.
//! * **Convergence** (SPICE-4): junction/FET limiting, dynamic gmin stepping, source stepping,
//!   timestep cuts; failures name the unknowns and elements ([`ConvergenceReport`]).
//! * **Solver** (SPICE-5): Markowitz sparse LU with symbolic reuse, dense LU for tiny systems
//!   ([`matrix`] explains why it is hand-written).
//! * **Accuracy** (SPICE-6): `tests/reference/` holds 47 circuits compared with ngspice golden
//!   data (`scripts/regen-spice-golden.sh`).
//! * **Co-simulation** (SPICE-7): [`Session`].
//! * **Export** (SPICE-8): [`Netlist::to_spice_text`], [`export`] (CSV, ASCII rawfile).
//!
//! ## Known differences from ngspice
//!
//! * `pwr(x, y)` is `|x|^y` everywhere (ngspice's B sources use `sgn(x)·|x|^y`; use `pwrs`).
//! * ngspice 47 silently treats `AM(...)` as a DC source; bb-spice implements the AM waveform as
//!   documented in the ngspice manual.
//! * `.save` is accepted but every node voltage, branch current and two-terminal element current
//!   is kept.
//! * Switches shorten the time step to land on their threshold crossing, so switching instants
//!   are more precise than ngspice's at the same `.tran` step.

pub(crate) mod analysis;
pub mod builder;
pub(crate) mod devices;
pub mod elab;
pub(crate) mod engine;
pub mod error;
pub mod export;
pub mod expr;
pub mod matrix;
pub mod meas;
pub mod netlist;
pub mod results;
mod run;
pub mod session;
pub(crate) mod tran;

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
