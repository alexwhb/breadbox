//! Shared types for Breadbox: simulated time, SI units, pin drives, the [`Device`] contract,
//! visual states, device events, the world view, and a deterministic RNG.
//!
//! This crate is intentionally tiny and dependency-light; every other crate depends on it.
//! See `docs/ARCHITECTURE.md` §2–§3.

pub mod device;
pub mod rng;
pub mod si;
mod time;
pub mod visual;
pub mod world;

pub use device::{AudioChunk, Device, DeviceCtx, DeviceEvent, LogLevel, NullCtx, Sound, UserInput};
pub use rng::Rng;
pub use time::SimTime;
pub use visual::{Health, Look, Rgb, VisualState};
pub use world::{Pos, StaticWorld, WorldMut, WorldView};

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

impl PinDrive {
    /// Norton equivalent `(conductance, current into the node at 0 V)`.
    pub fn norton(&self) -> (f64, f64) {
        match *self {
            PinDrive::HighZ => (0.0, 0.0),
            PinDrive::Thevenin { v, r } | PinDrive::Pull { v, r } => {
                let g = 1.0 / r.max(1e-6);
                (g, v * g)
            }
            PinDrive::Current { i } => (0.0, i),
        }
    }

    /// Current this drive pushes into a node sitting at `v_node`.
    pub fn current_into(&self, v_node: f64) -> f64 {
        let (g, i0) = self.norton();
        i0 - g * v_node
    }
}

/// Voltage of a node that is connected only to drives (no other circuit elements): the
/// closed-form solution of a single node with Norton sources. `leak` (siemens to ground) keeps
/// an all-high-Z node defined (it then reads 0 V).
pub fn resolve_drives<'a>(drives: impl IntoIterator<Item = &'a PinDrive>, leak: f64) -> f64 {
    let mut g = leak.max(0.0);
    let mut i = 0.0;
    for d in drives {
        let (gd, id) = d.norton();
        g += gd;
        i += id;
    }
    if g <= 0.0 { 0.0 } else { i / g }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drives_resolve_in_closed_form() {
        let a = PinDrive::Thevenin { v: 5.0, r: 25.0 };
        let b = PinDrive::Pull { v: 0.0, r: 25.0 };
        assert!((resolve_drives([&a, &b], 0.0) - 2.5).abs() < 1e-12);
        let z = PinDrive::HighZ;
        assert_eq!(resolve_drives([&z], 1e-12), 0.0);
        let pu = PinDrive::Pull { v: 5.0, r: 35e3 };
        assert!((resolve_drives([&pu, &z], 1e-12) - 5.0).abs() < 1e-6);
        assert!((a.current_into(4.5) - 0.02).abs() < 1e-12);
    }
}
