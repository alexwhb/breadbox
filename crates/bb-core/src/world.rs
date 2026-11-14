//! What devices may know about the simulated world ([`WorldView`]) and change in it
//! ([`WorldMut`]). Implemented by `bb-world`; positions are bench coordinates in metres.

use crate::SimTime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A position on the bench (metres, x right, y down — same orientation as the bench view).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Pos {
    pub x: f64,
    pub y: f64,
}

impl Pos {
    pub const fn new(x: f64, y: f64) -> Pos {
        Pos { x, y }
    }
    pub fn dist(self, o: Pos) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// Read access to the world.
///
/// Well-known variable names for [`WorldView::var`]: `air.temp_c`, `air.rh`,
/// `air.pressure_pa`, `light.ambient_lux`, `sun.elevation_deg`, `<pot>.vwc` (% VWC),
/// `<pot>.soil_temp_c`, `<object>.x`/`.y`, `sound.db`, `tilt.deg`, plus user variables.
pub trait WorldView {
    /// Simulated (circuit) time.
    fn time(&self) -> SimTime;
    /// World clock in seconds since the world started (sim time × time-warp).
    fn world_secs(&self) -> f64;
    fn var(&self, key: &str) -> Option<f64>;
    /// Soil volumetric water content of a pot, % (0–100).
    fn soil_vwc(&self, pot: &str) -> Option<f64>;
    /// Illuminance at a bench position (ambient + nearby bench light sources), lux.
    fn lux_at(&self, pos: Pos) -> f64;
    /// Air temperature at a bench position, including heat from nearby parts, °C.
    fn temperature_at(&self, pos: Pos) -> f64;
    /// Relative humidity, %.
    fn humidity(&self) -> f64;
    /// Barometric pressure, Pa.
    fn pressure(&self) -> f64;
    /// Distance from `pos` to the nearest object inside a cone of `half_angle_deg` around
    /// `dir_deg` (0° = +x, 90° = +y), up to `max_m`. HC-SR04-style sensors.
    fn distance(&self, pos: Pos, dir_deg: f64, half_angle_deg: f64, max_m: f64) -> Option<f64>;
    /// True if a warm object moved within `range_m` of `pos` during the last `window_s`
    /// seconds (PIR).
    fn motion(&self, pos: Pos, range_m: f64, window_s: f64) -> bool;
    /// Magnetic flux density at `pos` from magnet objects, millitesla.
    fn magnetic_field(&self, pos: Pos) -> f64;
    /// Global "sensor noise & ADC quantization" switch (WB-9, WORLD-6).
    fn noise_enabled(&self) -> bool;
}

/// Write access (pumps water pots, LEDs light the bench, resistors heat it).
pub trait WorldMut: WorldView {
    fn set_var(&mut self, key: &str, value: f64);
    /// Adds water to a pot (millilitres).
    fn water(&mut self, pot: &str, ml: f64);
    /// Declares (or updates) a heat source at `pos` dissipating `watts`; `id` is stable per part.
    fn set_heat_source(&mut self, id: &str, pos: Pos, watts: f64);
    /// Declares (or updates) a light source with luminous intensity `candela`.
    fn set_light_source(&mut self, id: &str, pos: Pos, candela: f64);
}

/// A fixed world for tests and for devices running without a world model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticWorld {
    pub time: SimTime,
    pub vars: BTreeMap<String, f64>,
    pub temp_c: f64,
    pub rh: f64,
    pub lux: f64,
    pub pressure_pa: f64,
    pub distance_m: Option<f64>,
    pub motion: bool,
    pub field_mt: f64,
    pub noise: bool,
    pub heat: BTreeMap<String, f64>,
    pub light: BTreeMap<String, f64>,
}

impl Default for StaticWorld {
    fn default() -> Self {
        StaticWorld {
            time: SimTime::ZERO,
            vars: BTreeMap::new(),
            temp_c: 21.0,
            rh: 45.0,
            lux: 180.0,
            pressure_pa: 101_325.0,
            distance_m: None,
            motion: false,
            field_mt: 0.0,
            noise: false,
            heat: BTreeMap::new(),
            light: BTreeMap::new(),
        }
    }
}

impl WorldView for StaticWorld {
    fn time(&self) -> SimTime {
        self.time
    }
    fn world_secs(&self) -> f64 {
        self.time.as_secs_f64()
    }
    fn var(&self, key: &str) -> Option<f64> {
        match key {
            "air.temp_c" => Some(self.temp_c),
            "air.rh" => Some(self.rh),
            "air.pressure_pa" => Some(self.pressure_pa),
            "light.ambient_lux" => Some(self.lux),
            _ => self.vars.get(key).copied(),
        }
    }
    fn soil_vwc(&self, pot: &str) -> Option<f64> {
        self.vars.get(&format!("{pot}.vwc")).copied()
    }
    fn lux_at(&self, _pos: Pos) -> f64 {
        self.lux
    }
    fn temperature_at(&self, _pos: Pos) -> f64 {
        self.temp_c
    }
    fn humidity(&self) -> f64 {
        self.rh
    }
    fn pressure(&self) -> f64 {
        self.pressure_pa
    }
    fn distance(&self, _pos: Pos, _dir: f64, _half: f64, max_m: f64) -> Option<f64> {
        self.distance_m.filter(|d| *d <= max_m)
    }
    fn motion(&self, _pos: Pos, _range_m: f64, _window_s: f64) -> bool {
        self.motion
    }
    fn magnetic_field(&self, _pos: Pos) -> f64 {
        self.field_mt
    }
    fn noise_enabled(&self) -> bool {
        self.noise
    }
}

impl WorldMut for StaticWorld {
    fn set_var(&mut self, key: &str, value: f64) {
        self.vars.insert(key.to_string(), value);
    }
    fn water(&mut self, pot: &str, ml: f64) {
        let k = format!("{pot}.vwc");
        let v = self.vars.get(&k).copied().unwrap_or(0.0);
        self.vars.insert(k, (v + ml * 0.05).min(100.0));
    }
    fn set_heat_source(&mut self, id: &str, _pos: Pos, watts: f64) {
        self.heat.insert(id.to_string(), watts);
    }
    fn set_light_source(&mut self, id: &str, _pos: Pos, candela: f64) {
        self.light.insert(id.to_string(), candela);
    }
}
