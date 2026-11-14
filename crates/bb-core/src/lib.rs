//! Shared types for Breadbox: simulated time, SI units, pin drives and (later) the device contract.
//!
//! This crate is intentionally tiny and dependency-light; every other crate depends on it.
//! See `docs/ARCHITECTURE.md` §2–§3.

pub mod si;
mod time;

pub use time::SimTime;

use serde::{Deserialize, Serialize};

/// How something drives one node of the analog circuit (an MCU pin, a sensor output, …).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum PinDrive {
    /// Floating / input. The engine adds its own leakage so nodes never float numerically.
    #[default]
    HighZ,
    /// Push-pull output: ideal source `v` volts behind `r` ohms.
    Thevenin { v: f64, r: f64 },
    /// Weak pull (e.g. AVR internal pull-up ≈ 35 kΩ to Vcc).
    Pull { v: f64, r: f64 },
    /// Ideal current source pushing `i` amps into the node.
    Current { i: f64 },
}
