//! The behavioural-device contract (ARCHITECTURE §3).
//!
//! Everything on the bench that is not a plain SPICE element — sensors, LEDs with visuals,
//! displays, motors, radios, instruments, plugin parts — implements [`Device`]. The co-simulator
//! (`bb-sim`) owns time: it calls the device's hooks at exact simulated instants and hands it a
//! [`DeviceCtx`] through which the device reads its pins, drives them, schedules wake-ups and
//! reads (or, with permission, writes) the world.
//!
//! Pins are indices into the part definition's pin list (`bb-circuit` `PartDef::pins`).

use crate::rng::Rng;
use crate::visual::VisualState;
use crate::world::{WorldMut, WorldView};
use crate::{PinDrive, SimTime};
use serde::{Deserialize, Serialize};

/// What a device can see and do while one of its hooks runs.
pub trait DeviceCtx {
    /// Current simulated time (the instant the hook is called for).
    fn now(&self) -> SimTime;
    /// Voltage of `pin` against ground at [`DeviceCtx::now`].
    fn pin_voltage(&self, pin: usize) -> f64;
    /// Current flowing *into* the device through `pin` (from the device's own drive on it).
    fn pin_current(&self, pin: usize) -> f64;
    /// Current through one of this part's SPICE elements (local name from the part's model
    /// template, e.g. `"D"` for an LED's diode), into its first terminal.
    fn element_current(&self, element: &str) -> Option<f64>;
    /// Charge (A·s) that has flowed through a SPICE element since the bench started. Exact
    /// for piecewise-constant currents; call [`DeviceCtx::track_element`] first (in
    /// `on_start`). Average current = Δcharge / Δt.
    fn element_charge(&self, element: &str) -> Option<f64>;
    /// Starts integrating an element's current (see [`DeviceCtx::element_charge`]).
    fn track_element(&mut self, element: &str);
    /// Energy (J) dissipated in a tracked element since the bench started (average power =
    /// Δenergy / Δt).
    fn element_energy(&self, _element: &str) -> Option<f64> {
        None
    }
    /// Changes how the device drives `pin` from now on.
    fn set_drive(&mut self, pin: usize, drive: PinDrive);
    /// Sets an analog external variable of this part (`ext("<ref>.<name>")` in its SPICE
    /// template), e.g. an LDR's illuminance.
    fn set_ext(&mut self, name: &str, value: f64);
    /// Schedules [`Device::on_wake`] at `at` (≥ now) with `token`.
    fn wake_at(&mut self, at: SimTime, token: u64);
    /// Cancels every pending wake-up with `token`.
    fn cancel_wake(&mut self, token: u64);
    /// Schmitt-trigger edge notifications on `pin` (falls below `lo` / rises above `hi`) →
    /// [`Device::on_edge`] at the interpolated crossing time.
    fn watch(&mut self, pin: usize, lo: f64, hi: f64);
    /// Calls [`Device::on_tick`] every `period` of simulated time (`SimTime::ZERO` stops it).
    fn tick_every(&mut self, period: SimTime);
    /// Read-only world access.
    fn world(&self) -> &dyn WorldView;
    /// Mutable world access, for devices allowed to change it (pumps, heaters, lamps).
    fn world_mut(&mut self) -> Option<&mut dyn WorldMut>;
    /// Per-device deterministic random numbers (seeded from the bench seed + part id).
    fn rng(&mut self) -> &mut Rng;
    /// Reports something to the user / the engine.
    fn emit(&mut self, ev: DeviceEvent);
}

/// A simulated behavioural part.
pub trait Device: Send {
    /// Behaviour id (`"led"`, `"dht22"`, …).
    fn kind(&self) -> &str;
    /// Called once when the bench starts (set initial drives, watches, ticks here).
    fn on_start(&mut self, _ctx: &mut dyn DeviceCtx) {}
    /// A wake-up scheduled with [`DeviceCtx::wake_at`] is due.
    fn on_wake(&mut self, _ctx: &mut dyn DeviceCtx, _token: u64) {}
    /// A watched pin crossed a threshold (`high` = rose above `hi`).
    fn on_edge(&mut self, _ctx: &mut dyn DeviceCtx, _pin: usize, _high: bool) {}
    /// Periodic tick requested with [`DeviceCtx::tick_every`].
    fn on_tick(&mut self, _ctx: &mut dyn DeviceCtx) {}
    /// The user interacted with the part on the bench (button, knob, switch).
    fn on_input(&mut self, _ctx: &mut dyn DeviceCtx, _input: &UserInput) {}
    /// A part property changed while running. Return `false` if the device must be rebuilt.
    fn set_param(&mut self, _ctx: &mut dyn DeviceCtx, _key: &str, _value: f64) -> bool {
        false
    }
    /// What the UI should draw.
    fn visual(&self) -> VisualState;
    /// Live readings for the inspector (`("angle", 87.0)`, `("rpm", 4200.0)`).
    fn readings(&self) -> Vec<(String, f64)> {
        Vec::new()
    }
    fn save_state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn load_state(&mut self, _s: &serde_json::Value) {}
}

/// Interaction with a part on the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UserInput {
    /// Push button / momentary contact held (`true`) or released.
    Press(bool),
    /// Toggle a switch (slide switch, tilt, DIP position `index`).
    Toggle { index: usize },
    /// Set a switch to a definite state.
    SetSwitch { index: usize, on: bool },
    /// Knob / slider position 0…1 (potentiometer, trimmer).
    Position(f64),
    /// A named numeric control (e.g. `"distance_cm"` on a test fixture).
    Value { key: String, value: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

/// Short sounds the UI plays (and lessons can check).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Sound {
    /// Relay armature click.
    Click,
    /// Electrolytic capacitor popping / part blowing.
    Pop,
    /// A beep with a pitch and duration (continuity tester, active buzzer burst).
    Beep { freq_hz: f64, secs: f64 },
}

/// A block of mono PCM audio rendered from a simulated waveform (buzzers, speakers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioChunk {
    /// Simulated time of the first sample.
    pub start: SimTime,
    pub sample_rate: u32,
    /// Samples in −1…1.
    pub samples: Vec<f32>,
}

/// Something a device reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeviceEvent {
    /// A line for the event log (and the serial monitor's system lines).
    Log {
        level: LogLevel,
        text: String,
    },
    /// A serial-monitor annotation (`channel` = `"wifi"`, `"mqtt"`, `"ble"`, …; RF-8).
    Annotation {
        channel: String,
        text: String,
    },
    /// The part is overstressed but still working.
    Stress {
        reason: String,
    },
    /// The part was destroyed (stays damaged until repaired).
    Damage {
        reason: String,
    },
    Sound(Sound),
    Audio(AudioChunk),
    /// An electrical-rule finding discovered while running (e.g. 5 V on a 3.3 V input).
    Erc {
        warning: bool,
        message: String,
    },
}

/// A context that does nothing — for unit-testing devices' pure logic and as a default.
pub struct NullCtx {
    pub now: SimTime,
    pub rng: Rng,
    pub events: Vec<DeviceEvent>,
    pub world: crate::world::StaticWorld,
}

impl Default for NullCtx {
    fn default() -> Self {
        NullCtx {
            now: SimTime::ZERO,
            rng: Rng::new(1),
            events: Vec::new(),
            world: crate::world::StaticWorld::default(),
        }
    }
}

impl DeviceCtx for NullCtx {
    fn now(&self) -> SimTime {
        self.now
    }
    fn pin_voltage(&self, _pin: usize) -> f64 {
        0.0
    }
    fn pin_current(&self, _pin: usize) -> f64 {
        0.0
    }
    fn element_current(&self, _element: &str) -> Option<f64> {
        None
    }
    fn element_charge(&self, _element: &str) -> Option<f64> {
        None
    }
    fn track_element(&mut self, _element: &str) {}
    fn set_drive(&mut self, _pin: usize, _drive: PinDrive) {}
    fn set_ext(&mut self, _name: &str, _value: f64) {}
    fn wake_at(&mut self, _at: SimTime, _token: u64) {}
    fn cancel_wake(&mut self, _token: u64) {}
    fn watch(&mut self, _pin: usize, _lo: f64, _hi: f64) {}
    fn tick_every(&mut self, _period: SimTime) {}
    fn world(&self) -> &dyn WorldView {
        &self.world
    }
    fn world_mut(&mut self) -> Option<&mut dyn WorldMut> {
        Some(&mut self.world)
    }
    fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }
    fn emit(&mut self, ev: DeviceEvent) {
        self.events.push(ev);
    }
}
