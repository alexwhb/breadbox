//! # bb-sim — Breadbox's mixed-signal co-simulation engine
//!
//! Builds a runnable [`Bench`] from a `bb-circuit` project — analog `bb_spice::Session`,
//! cycle-accurate AVRs running real firmware and API-level host boards (ESP32 / Pico, see
//! [`host`]) behind one [`mcu::McuCore`] trait, behavioural devices, the `bb-world` world, instruments — and
//! advances them together (SIM-1…SIM-7). [`SimHandle`] runs a bench on its own thread for the
//! app: commands in, a lock-free telemetry snapshot (≤ 60 Hz) and bounded serial/log/audio
//! streams out.
//!
//! ```no_run
//! use bb_circuit::{Library, Project};
//! use bb_sim::{Bench, BenchOptions};
//! use bb_core::SimTime;
//! let lib = Library::builtin();
//! let project = Project::load("examples/garden-node")?;
//! let mut opts = BenchOptions::default();
//! opts.firmware.insert("u1".into(), bb_sim::firmware::load_image("examples/fixtures/garden-node.uno.hex")?);
//! let mut bench = Bench::new(project, lib, opts)?;
//! bench.run_for(SimTime::from_secs(3))?;
//! for line in bench.serial("u1").unwrap().data_lines() {
//!     println!("{} {}", line.time, line.text);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod bench;
pub mod ctx;
pub mod damage;
pub mod devices;
pub mod firmware;
pub mod handle;
pub mod host;
pub mod instruments;
pub mod mcu;
mod sched;
pub mod serial;
pub mod telemetry;

pub use bench::{Bench, BenchOptions, BenchStats, Service};
pub use devices::{DeviceRegistry, Factory};
pub use handle::{Command, DebugCommand, SimHandle, Speed};
pub use host::{HostMcu, RadioService};
pub use instruments::{MeterReading, ProbeReading, ScopeFrame, Trigger};
pub use serial::{LineKind, SerialLine};
pub use telemetry::{McuTelemetry, PartTelemetry, PowerReport, RunStatus, Telemetry};

use bb_core::{DeviceEvent, SimTime};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SimError {
    #[error("cannot build the bench: {0}")]
    Build(String),
    #[error("analog solver: {0}")]
    Analog(String),
    #[error("no part `{0}` on the bench")]
    NotFound(String),
    #[error("firmware: {0}")]
    Firmware(String),
}

/// Something that happened on the bench (device events with their source part).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimEvent {
    pub time: SimTime,
    /// Designator (`D1`) or `world`.
    pub source: String,
    pub event: DeviceEvent,
}
