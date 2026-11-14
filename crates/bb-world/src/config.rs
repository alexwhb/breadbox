//! Serialized world settings (the `"world"` object in `project.json`).

use crate::sun::{DateTime, Location};
use bb_core::Pos;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn default_true() -> bool {
    true
}
fn one() -> f64 {
    1.0
}

/// Everything about the world a project saves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldConfig {
    /// Seed for every random process in the world and the sensors.
    pub seed: u64,
    /// World seconds per simulated second (WORLD-5 time-warp; 1 = real time).
    pub time_warp: f64,
    /// "Sensor noise & ADC quantization" (WB-9 / WORLD-6).
    pub noise: bool,
    pub air: AirConfig,
    pub light: LightConfig,
    pub pots: Vec<PotConfig>,
    pub objects: Vec<ObjectConfig>,
    pub timelines: Vec<Timeline>,
    pub scripts: Vec<Script>,
    /// User variables (readable by devices and scripts).
    pub vars: BTreeMap<String, f64>,
}

impl Default for WorldConfig {
    fn default() -> Self {
        WorldConfig {
            seed: 1,
            time_warp: 1.0,
            noise: true,
            air: AirConfig::default(),
            light: LightConfig::default(),
            pots: Vec::new(),
            objects: Vec::new(),
            timelines: Vec::new(),
            scripts: Vec::new(),
            vars: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AirConfig {
    /// Air temperature (°C) — the daily mean when the diurnal cycle is on.
    pub temp_c: f64,
    /// Relative humidity, %.
    pub rh: f64,
    pub pressure_pa: f64,
    /// Follow a day/night temperature + humidity cycle (coupled to the light's clock).
    pub diurnal: bool,
    /// Peak-to-mean temperature swing (°C) of the diurnal cycle.
    pub temp_swing_c: f64,
    /// Peak-to-mean humidity swing (%RH), opposite in phase to temperature.
    pub rh_swing: f64,
}

impl Default for AirConfig {
    fn default() -> Self {
        AirConfig {
            temp_c: 21.0,
            rh: 45.0,
            pressure_pa: 101_325.0,
            diurnal: false,
            temp_swing_c: 5.0,
            rh_swing: 15.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LightMode {
    /// Ambient lux slider.
    #[default]
    Manual,
    /// Day/night cycle from the sun at `location`.
    Solar,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightConfig {
    pub mode: LightMode,
    /// Manual ambient illuminance, lux.
    pub ambient_lux: f64,
    pub location: Location,
    /// Local date/time at world time 0.
    pub start: DateTime,
    /// 0 = clear sky, 1 = overcast.
    pub cloud_cover: f64,
    /// Fraction of outdoor daylight reaching an indoor bench (window light).
    pub indoor_fraction: f64,
    /// Bench is indoors (applies `indoor_fraction` in solar mode).
    #[serde(default = "default_true")]
    pub indoor: bool,
}

impl Default for LightConfig {
    fn default() -> Self {
        LightConfig {
            mode: LightMode::Manual,
            ambient_lux: 180.0,
            location: Location::default(),
            start: DateTime::default(),
            cloud_cover: 0.2,
            indoor_fraction: 0.01,
            indoor: true,
        }
    }
}

/// A pot of soil (WORLD-1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PotConfig {
    pub id: String,
    pub name: String,
    pub indoor: bool,
    /// Volumetric water content now, % (0–100).
    pub vwc: f64,
    /// Water held against gravity, %.
    pub field_capacity: f64,
    /// All pores full, %.
    pub saturation: f64,
    /// Soil volume in millilitres (watering 10 % of it raises VWC by 10 points).
    pub soil_volume_ml: f64,
    /// Evaporation on/off (WB-9 checkbox).
    pub evaporation: bool,
    /// Multiplier on the evaporation model (1 = design calibration ≈ 1.4 %/h at 21 °C).
    #[serde(default = "one")]
    pub evaporation_scale: f64,
    /// Excess water above field capacity drains with this rate constant (1/h).
    pub drainage_per_h: f64,
    /// Bench position of the pot (metres).
    pub pos: Pos,
}

impl Default for PotConfig {
    fn default() -> Self {
        PotConfig {
            id: "pot1".into(),
            name: "Pot 1".into(),
            indoor: true,
            vwc: 42.0,
            field_capacity: 45.0,
            saturation: 60.0,
            soil_volume_ml: 1000.0,
            evaporation: true,
            evaporation_scale: 1.0,
            drainage_per_h: 0.8,
            pos: Pos::new(0.20, 0.16),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    /// A draggable hand (warm, reflects ultrasound).
    #[default]
    Hand,
    /// A person walking by (warm).
    Person,
    /// A wall or box (reflects ultrasound, not warm).
    Wall,
    /// A permanent magnet (hall / reed sensors).
    Magnet,
}

/// A movable thing in the world (WORLD-4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectConfig {
    pub id: String,
    pub kind: ObjectKind,
    pub pos: Pos,
    /// Size (m); distance sensors measure to the surface.
    pub radius: f64,
    /// Magnet strength: flux density (mT) at 1 cm.
    pub strength_mt: f64,
    /// Present on the bench (hidden objects are ignored).
    pub present: bool,
}

impl Default for ObjectConfig {
    fn default() -> Self {
        ObjectConfig {
            id: "hand".into(),
            kind: ObjectKind::Hand,
            pos: Pos::new(0.3, 0.1),
            radius: 0.04,
            strength_mt: 50.0,
            present: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    #[default]
    Linear,
    Step,
    /// Smoothstep between keys.
    Smooth,
}

/// One keyframe: world time (s) → value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub t: f64,
    pub value: f64,
}

/// A per-variable keyframe timeline (WORLD-5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Variable key (`pot1.vwc`, `light.ambient_lux`, `air.temp_c`, `hand.x`, …).
    pub var: String,
    pub keys: Vec<Keyframe>,
    #[serde(default)]
    pub interp: Interp,
    /// Loop period in world seconds.
    #[serde(default)]
    pub repeat: Option<f64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// An expression script, e.g. `moisture = 45 - 0.8*t_hours` (WORLD-5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub source: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}
