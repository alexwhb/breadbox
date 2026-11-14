//! Physical world models for Breadbox (SPEC §8, `WORLD-1`…`WORLD-6`).
//!
//! * Pots of soil with volumetric water content dynamics: evaporation driven by temperature,
//!   humidity and light, drainage above field capacity, watering events ([`World::water`]).
//! * Light: a manual ambient slider or a day/night cycle from the sun's elevation at a
//!   location/date with cloud cover ([`sun`]); bench light sources (LEDs, lamps) add lux to
//!   nearby sensors with inverse-square falloff.
//! * Air temperature + humidity with an optional diurnal cycle; heat sources (parts
//!   dissipating power) warm their surroundings.
//! * Objects (hand, person, wall, magnet) for distance / motion / hall sensors.
//! * Programmable world: keyframe timelines and expression scripts
//!   (`moisture = 45 - 0.8*t_hours`, evaluated with bb-spice's expression engine), world-model
//!   plugins ([`WorldModel`]), time-warp independent of circuit speed.
//! * Global sensor-noise switch; everything is deterministic (no hidden randomness).
//!
//! [`World`] implements [`bb_core::WorldView`] / [`bb_core::WorldMut`]; `bb-sim` steps it with
//! [`World::step`] at simulated times.

pub mod config;
pub mod sun;

pub use config::{
    AirConfig, Interp, Keyframe, LightConfig, LightMode, ObjectConfig, ObjectKind, PotConfig,
    Script, Timeline, WorldConfig,
};
pub use sun::{DateTime, Location, daylight_lux, solar_elevation};

use bb_core::{Pos, SimTime, WorldMut, WorldView};
use bb_spice::expr::Expr;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum WorldError {
    #[error("script `{script}`: {msg}")]
    Script { script: String, msg: String },
    #[error("unknown world variable `{0}`")]
    UnknownVar(String),
}

/// A plugin world model (WORLD-5): stepped after the built-in models with world-time `dt`.
pub trait WorldModel: Send {
    fn name(&self) -> &str;
    fn step(&mut self, world: &mut dyn WorldMut, dt_world_s: f64);
}

/// A slider / checkbox the World panel shows (WB-9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// Variable key to `set`.
    pub key: String,
    pub label: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    /// Named stops (`Bone dry`, `Field capacity`, `Saturated`).
    pub stops: Vec<(String, f64)>,
    /// Logarithmic slider.
    pub log: bool,
}

/// Live values for the UI (part of the sim telemetry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorldSnapshot {
    pub world_secs: f64,
    pub local_time: Option<DateTime>,
    pub time_warp: f64,
    pub temp_c: f64,
    pub rh: f64,
    pub pressure_pa: f64,
    pub ambient_lux: f64,
    pub sun_elevation_deg: f64,
    pub noise: bool,
    pub solar: bool,
    pub diurnal: bool,
    pub pots: Vec<PotSnapshot>,
    pub objects: Vec<(String, Pos)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PotSnapshot {
    pub id: String,
    pub name: String,
    pub vwc: f64,
    /// Current evaporation rate, % VWC per world hour.
    pub evaporation_pct_h: f64,
    pub evaporation: bool,
    pub field_capacity: f64,
    pub saturation: f64,
}

/// A world event worth logging ("watered pot1 (+100 ml)").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldEvent {
    pub time: SimTime,
    pub text: String,
}

struct CompiledScript {
    #[allow(dead_code)]
    source: String,
    target: String,
    expr: Expr,
}

/// The simulated world.
pub struct World {
    cfg: WorldConfig,
    time: SimTime,
    world_secs: f64,
    temp_c: f64,
    rh: f64,
    ambient_lux: f64,
    sun_elev: f64,
    evap_rate: Vec<f64>,
    heat: BTreeMap<String, (Pos, f64)>,
    lights: BTreeMap<String, (Pos, f64)>,
    moved_at: BTreeMap<String, f64>,
    scripts: Vec<CompiledScript>,
    /// Variables currently driven by a timeline/script (sliders are overridden).
    driven: BTreeMap<String, f64>,
    models: Vec<Box<dyn WorldModel>>,
    events: Vec<WorldEvent>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("time", &self.time)
            .field("world_secs", &self.world_secs)
            .finish_non_exhaustive()
    }
}

const ALIASES: [(&str, &str); 9] = [
    ("lux", "light.ambient_lux"),
    ("light", "light.ambient_lux"),
    ("temp", "air.temp_c"),
    ("temperature", "air.temp_c"),
    ("humidity", "air.rh"),
    ("rh", "air.rh"),
    ("pressure", "air.pressure_pa"),
    ("clouds", "light.cloud_cover"),
    ("warp", "world.time_warp"),
];

impl World {
    pub fn new(cfg: WorldConfig) -> Result<World, WorldError> {
        let mut w = World {
            time: SimTime::ZERO,
            world_secs: 0.0,
            temp_c: cfg.air.temp_c,
            rh: cfg.air.rh,
            ambient_lux: cfg.light.ambient_lux,
            sun_elev: 0.0,
            evap_rate: vec![0.0; cfg.pots.len()],
            heat: BTreeMap::new(),
            lights: BTreeMap::new(),
            moved_at: BTreeMap::new(),
            scripts: Vec::new(),
            driven: BTreeMap::new(),
            models: Vec::new(),
            events: Vec::new(),
            cfg,
        };
        w.compile_scripts()?;
        w.apply_programs();
        w.update_derived();
        Ok(w)
    }

    /// The current settings (slider values, pots' moisture, …) for saving into the project.
    pub fn config(&self) -> &WorldConfig {
        &self.cfg
    }

    pub fn add_model(&mut self, m: Box<dyn WorldModel>) {
        self.models.push(m);
    }

    /// Replaces the scripts (validates them first).
    pub fn set_scripts(&mut self, scripts: Vec<Script>) -> Result<(), WorldError> {
        let old = std::mem::replace(&mut self.cfg.scripts, scripts);
        if let Err(e) = self.compile_scripts() {
            self.cfg.scripts = old;
            self.compile_scripts().ok();
            return Err(e);
        }
        Ok(())
    }

    pub fn set_timelines(&mut self, timelines: Vec<Timeline>) {
        self.cfg.timelines = timelines;
    }

    fn compile_scripts(&mut self) -> Result<(), WorldError> {
        let mut out = Vec::new();
        for s in self.cfg.scripts.iter().filter(|s| s.enabled) {
            for stmt in s.source.split([';', '\n']) {
                let stmt = stmt.trim();
                if stmt.is_empty() || stmt.starts_with('#') {
                    continue;
                }
                let (lhs, rhs) = stmt.split_once('=').ok_or_else(|| WorldError::Script {
                    script: stmt.into(),
                    msg: "expected `variable = expression`".into(),
                })?;
                let target = self.resolve_name(lhs.trim());
                let expr = Expr::parse(rhs.trim()).map_err(|e| WorldError::Script {
                    script: stmt.into(),
                    msg: e.to_string(),
                })?;
                out.push(CompiledScript {
                    source: stmt.into(),
                    target,
                    expr,
                });
            }
        }
        self.scripts = out;
        Ok(())
    }

    /// Maps a script / alias name to a variable key: `moisture` → first pot's `vwc`,
    /// `lux` → `light.ambient_lux`, `pot1_vwc` → `pot1.vwc`.
    pub fn resolve_name(&self, name: &str) -> String {
        let n = name.trim();
        if n.contains('.') {
            return n.to_string();
        }
        if matches!(n, "moisture" | "soil" | "vwc")
            && let Some(p) = self.cfg.pots.first()
        {
            return format!("{}.vwc", p.id);
        }
        if let Some((_, k)) = ALIASES.iter().find(|(a, _)| *a == n) {
            return k.to_string();
        }
        if let Some((a, b)) = n.split_once('_') {
            let cand = format!("{a}.{b}");
            if self.var(&cand).is_some() {
                return cand;
            }
        }
        n.to_string()
    }

    // ------------------------------------------------------------------ time

    pub fn time(&self) -> SimTime {
        self.time
    }

    /// Local date/time of the world clock (solar mode clock; also used by the diurnal cycle).
    pub fn local_time(&self) -> DateTime {
        self.cfg.light.start.plus_secs(self.world_secs)
    }

    /// Advances the world to simulated time `now`.
    pub fn step(&mut self, now: SimTime) {
        if now <= self.time {
            return;
        }
        let dt_sim = (now - self.time).as_secs_f64();
        let dt_w = dt_sim * self.cfg.time_warp.max(0.0);
        self.time = now;
        self.world_secs += dt_w;
        self.apply_programs();
        self.update_derived();
        self.step_pots(dt_w);
        if !self.models.is_empty() {
            let mut models = std::mem::take(&mut self.models);
            for m in models.iter_mut() {
                m.step(self, dt_w);
            }
            models.append(&mut self.models);
            self.models = models;
            self.update_derived();
        }
    }

    fn apply_programs(&mut self) {
        self.driven.clear();
        let t = self.world_secs;
        let timelines: Vec<(String, f64)> = self
            .cfg
            .timelines
            .iter()
            .filter(|tl| tl.enabled && !tl.keys.is_empty())
            .map(|tl| (tl.var.clone(), eval_timeline(tl, t)))
            .collect();
        for (k, v) in timelines {
            self.set_base(&k, v);
            self.driven.insert(k, v);
        }
        if self.scripts.is_empty() {
            return;
        }
        let scripts = std::mem::take(&mut self.scripts);
        for s in &scripts {
            let hour = self.local_time().hour;
            let val = s.expr.eval_with(&|name: &str| match name {
                "t" | "t_s" => Some(t),
                "t_min" => Some(t / 60.0),
                "t_hours" | "t_h" => Some(t / 3600.0),
                "t_days" => Some(t / 86400.0),
                "hour" => Some(hour),
                _ => {
                    let key = self.resolve_name(name);
                    self.var(&key)
                }
            });
            if let Ok(v) = val
                && v.is_finite()
            {
                let key = s.target.clone();
                self.set_base(&key, v);
                self.driven.insert(key, v);
            }
        }
        self.scripts = scripts;
    }

    fn update_derived(&mut self) {
        let lt = self.local_time();
        self.sun_elev = solar_elevation(&self.cfg.light.location, &lt);
        let phase = 2.0 * std::f64::consts::PI * (lt.hour - 15.0) / 24.0;
        let a = &self.cfg.air;
        if a.diurnal {
            self.temp_c = a.temp_c + a.temp_swing_c * phase.cos();
            self.rh = (a.rh - a.rh_swing * phase.cos()).clamp(1.0, 100.0);
        } else {
            self.temp_c = a.temp_c;
            self.rh = a.rh.clamp(0.0, 100.0);
        }
        let l = &self.cfg.light;
        self.ambient_lux = match l.mode {
            LightMode::Manual => l.ambient_lux.max(0.0),
            LightMode::Solar => {
                let out = daylight_lux(self.sun_elev, l.cloud_cover);
                if l.indoor {
                    out * l.indoor_fraction
                } else {
                    out
                }
            }
        };
        if self.evap_rate.len() != self.cfg.pots.len() {
            self.evap_rate.resize(self.cfg.pots.len(), 0.0);
        }
        for i in 0..self.cfg.pots.len() {
            self.evap_rate[i] = self.evaporation_rate(i);
        }
    }

    /// Evaporation rate of pot `i` in % VWC per world hour (0 when evaporation is off).
    fn evaporation_rate(&self, i: usize) -> f64 {
        let p = &self.cfg.pots[i];
        if !p.evaporation {
            return 0.0;
        }
        let dryness = (1.0 - self.rh / 100.0).max(0.0) / 0.55;
        let light = 1.0 + self.lux_at(p.pos) / 20_000.0;
        let wet = if p.vwc >= p.field_capacity {
            1.0
        } else {
            (p.vwc / p.field_capacity.max(1e-6)).max(0.0).sqrt()
        };
        let wind = if p.indoor { 1.0 } else { 2.5 };
        (0.6 + 0.04 * self.temp_c.max(0.0)) * dryness * light * wet * wind * p.evaporation_scale
    }

    fn step_pots(&mut self, dt_w: f64) {
        let dt_h = dt_w / 3600.0;
        if dt_h <= 0.0 {
            return;
        }
        for i in 0..self.cfg.pots.len() {
            let key = format!("{}.vwc", self.cfg.pots[i].id);
            if self.driven.contains_key(&key) {
                continue;
            }
            let rate = self.evap_rate[i];
            let p = &mut self.cfg.pots[i];
            p.vwc -= rate * dt_h;
            if p.vwc > p.field_capacity {
                let excess = p.vwc - p.field_capacity;
                p.vwc = p.field_capacity + excess * (-p.drainage_per_h * dt_h).exp();
            }
            p.vwc = p.vwc.clamp(0.0, p.saturation.max(p.field_capacity));
        }
    }

    // ------------------------------------------------------------------ edits

    /// Sets a variable from the UI or a command (`pot1.vwc`, `light.ambient_lux`, `air.temp_c`,
    /// `air.rh`, `light.cloud_cover`, `world.time_warp`, `hand.x`, `world.noise` (0/1),
    /// `light.solar` (0/1), `air.diurnal` (0/1), `pot1.evaporation` (0/1), or a user var).
    pub fn set(&mut self, key: &str, value: f64) {
        let key = self.resolve_name(key);
        self.set_base(&key, value);
        self.update_derived();
    }

    fn set_base(&mut self, key: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        let on = value != 0.0;
        match key {
            "air.temp_c" => self.cfg.air.temp_c = value,
            "air.rh" => self.cfg.air.rh = value.clamp(0.0, 100.0),
            "air.pressure_pa" => self.cfg.air.pressure_pa = value.max(0.0),
            "air.diurnal" => self.cfg.air.diurnal = on,
            "light.ambient_lux" => self.cfg.light.ambient_lux = value.max(0.0),
            "light.cloud_cover" => self.cfg.light.cloud_cover = value.clamp(0.0, 1.0),
            "light.solar" => {
                self.cfg.light.mode = if on {
                    LightMode::Solar
                } else {
                    LightMode::Manual
                }
            }
            "world.time_warp" => self.cfg.time_warp = value.clamp(0.0, 1e6),
            "world.noise" => self.cfg.noise = on,
            _ => {
                if let Some((obj, field)) = key.split_once('.') {
                    if let Some(p) = self.cfg.pots.iter_mut().find(|p| p.id == obj) {
                        match field {
                            "vwc" => p.vwc = value.clamp(0.0, 100.0),
                            "evaporation" => p.evaporation = on,
                            "field_capacity" => p.field_capacity = value.clamp(0.0, 100.0),
                            _ => {
                                self.cfg.vars.insert(key.to_string(), value);
                            }
                        }
                        return;
                    }
                    if let Some(o) = self.cfg.objects.iter_mut().find(|o| o.id == obj) {
                        let moved = match field {
                            "x" => {
                                let m = o.pos.x != value;
                                o.pos.x = value;
                                m
                            }
                            "y" => {
                                let m = o.pos.y != value;
                                o.pos.y = value;
                                m
                            }
                            "present" => {
                                o.present = on;
                                true
                            }
                            "strength_mt" => {
                                o.strength_mt = value;
                                false
                            }
                            _ => false,
                        };
                        if moved {
                            self.moved_at.insert(obj.to_string(), self.world_secs);
                        }
                        return;
                    }
                }
                self.cfg.vars.insert(key.to_string(), value);
            }
        }
    }

    /// Waters a pot ("Water the pot", a scheduled event, or a pump part).
    pub fn water(&mut self, pot: &str, ml: f64) {
        let t = self.time;
        if let Some(p) = self.cfg.pots.iter_mut().find(|p| p.id == pot) {
            let d = ml.max(0.0) / p.soil_volume_ml.max(1.0) * 100.0;
            p.vwc = (p.vwc + d).min(p.saturation.max(p.field_capacity));
            let text = format!("watered {} (+{:.0} ml → {:.0} % VWC)", p.name, ml, p.vwc);
            self.events.push(WorldEvent { time: t, text });
        }
    }

    /// World events since the last call.
    pub fn take_events(&mut self) -> Vec<WorldEvent> {
        std::mem::take(&mut self.events)
    }

    /// Slider/checkbox definitions for the World panel.
    pub fn controls(&self) -> Vec<Control> {
        let mut v = Vec::new();
        for p in &self.cfg.pots {
            v.push(Control {
                key: format!("{}.vwc", p.id),
                label: "Soil moisture".into(),
                unit: "% VWC".into(),
                min: 0.0,
                max: p.saturation.max(p.field_capacity),
                stops: vec![
                    ("Bone dry".into(), 5.0),
                    ("Field capacity".into(), p.field_capacity),
                    ("Saturated".into(), p.saturation),
                ],
                log: false,
            });
        }
        v.push(Control {
            key: "light.ambient_lux".into(),
            label: "Ambient light".into(),
            unit: "lx".into(),
            min: 0.0,
            max: 1000.0,
            stops: vec![
                ("Night".into(), 1.0),
                ("Living room".into(), 180.0),
                ("Bright office".into(), 1000.0),
            ],
            log: false,
        });
        v.push(Control {
            key: "air.temp_c".into(),
            label: "Air temperature".into(),
            unit: "°C".into(),
            min: -10.0,
            max: 45.0,
            stops: vec![],
            log: false,
        });
        v.push(Control {
            key: "air.rh".into(),
            label: "Humidity".into(),
            unit: "%RH".into(),
            min: 0.0,
            max: 100.0,
            stops: vec![],
            log: false,
        });
        v
    }

    /// Live values for telemetry.
    pub fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            world_secs: self.world_secs,
            local_time: Some(self.local_time()),
            time_warp: self.cfg.time_warp,
            temp_c: self.temp_c,
            rh: self.rh,
            pressure_pa: self.cfg.air.pressure_pa,
            ambient_lux: self.ambient_lux,
            sun_elevation_deg: self.sun_elev,
            noise: self.cfg.noise,
            solar: self.cfg.light.mode == LightMode::Solar,
            diurnal: self.cfg.air.diurnal,
            pots: self
                .cfg
                .pots
                .iter()
                .enumerate()
                .map(|(i, p)| PotSnapshot {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    vwc: p.vwc,
                    evaporation_pct_h: self.evap_rate.get(i).copied().unwrap_or(0.0),
                    evaporation: p.evaporation,
                    field_capacity: p.field_capacity,
                    saturation: p.saturation,
                })
                .collect(),
            objects: self
                .cfg
                .objects
                .iter()
                .map(|o| (o.id.clone(), o.pos))
                .collect(),
        }
    }

    /// Bench light sources (id → position, candela) currently registered.
    pub fn light_sources(&self) -> impl Iterator<Item = (&String, &(Pos, f64))> {
        self.lights.iter()
    }
}

fn eval_timeline(tl: &Timeline, t: f64) -> f64 {
    let mut keys = tl.keys.clone();
    keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    let t = match tl.repeat {
        Some(p) if p > 0.0 => t.rem_euclid(p),
        _ => t,
    };
    if t <= keys[0].t {
        return keys[0].value;
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t < b.t {
            let span = (b.t - a.t).max(1e-12);
            let mut f = ((t - a.t) / span).clamp(0.0, 1.0);
            match tl.interp {
                Interp::Linear => {}
                Interp::Step => f = 0.0,
                Interp::Smooth => f = f * f * (3.0 - 2.0 * f),
            }
            return a.value + (b.value - a.value) * f;
        }
    }
    keys[keys.len() - 1].value
}

impl WorldView for World {
    fn time(&self) -> SimTime {
        self.time
    }
    fn world_secs(&self) -> f64 {
        self.world_secs
    }
    fn var(&self, key: &str) -> Option<f64> {
        Some(match key {
            "air.temp_c" => self.temp_c,
            "air.rh" => self.rh,
            "air.pressure_pa" => self.cfg.air.pressure_pa,
            "air.diurnal" => self.cfg.air.diurnal as u8 as f64,
            "light.ambient_lux" => self.ambient_lux,
            "light.cloud_cover" => self.cfg.light.cloud_cover,
            "light.solar" => (self.cfg.light.mode == LightMode::Solar) as u8 as f64,
            "sun.elevation_deg" => self.sun_elev,
            "world.time_warp" => self.cfg.time_warp,
            "world.hour" => self.local_time().hour,
            "world.secs" => self.world_secs,
            "world.noise" => self.cfg.noise as u8 as f64,
            _ => {
                if let Some((obj, field)) = key.split_once('.') {
                    if let Some((i, p)) =
                        self.cfg.pots.iter().enumerate().find(|(_, p)| p.id == obj)
                    {
                        return match field {
                            "vwc" => Some(p.vwc),
                            "evaporation" => Some(p.evaporation as u8 as f64),
                            "evap_rate" => self.evap_rate.get(i).copied(),
                            "field_capacity" => Some(p.field_capacity),
                            "saturation" => Some(p.saturation),
                            "soil_temp_c" => Some(self.temp_c - 1.0),
                            _ => self.cfg.vars.get(key).copied(),
                        };
                    }
                    if let Some(o) = self.cfg.objects.iter().find(|o| o.id == obj) {
                        return match field {
                            "x" => Some(o.pos.x),
                            "y" => Some(o.pos.y),
                            "present" => Some(o.present as u8 as f64),
                            _ => self.cfg.vars.get(key).copied(),
                        };
                    }
                }
                return self.cfg.vars.get(key).copied();
            }
        })
    }
    fn soil_vwc(&self, pot: &str) -> Option<f64> {
        self.cfg.pots.iter().find(|p| p.id == pot).map(|p| p.vwc)
    }
    fn lux_at(&self, pos: Pos) -> f64 {
        let mut lux = self.ambient_lux;
        for (p, cd) in self.lights.values() {
            let d = p.dist(pos).max(0.003);
            lux += cd.max(0.0) / (d * d);
        }
        lux
    }
    fn temperature_at(&self, pos: Pos) -> f64 {
        let mut t = self.temp_c;
        for (p, w) in self.heat.values() {
            let d = p.dist(pos).max(0.005);
            t += w.max(0.0) * 25.0 * (0.005 / d);
        }
        t
    }
    fn humidity(&self) -> f64 {
        self.rh
    }
    fn pressure(&self) -> f64 {
        self.cfg.air.pressure_pa
    }
    fn distance(&self, pos: Pos, dir_deg: f64, half_angle_deg: f64, max_m: f64) -> Option<f64> {
        let dir = dir_deg.to_radians();
        let (dx, dy) = (dir.cos(), dir.sin());
        let mut best: Option<f64> = None;
        for o in self.cfg.objects.iter().filter(|o| o.present) {
            let (vx, vy) = (o.pos.x - pos.x, o.pos.y - pos.y);
            let d = (vx * vx + vy * vy).sqrt();
            if d < 1e-9 {
                continue;
            }
            let cosang = (vx * dx + vy * dy) / d;
            let ang = cosang.clamp(-1.0, 1.0).acos().to_degrees();
            let ang_size = (o.radius / d).clamp(0.0, 1.0).asin().to_degrees();
            if ang - ang_size > half_angle_deg {
                continue;
            }
            let surf = (d - o.radius).max(0.0);
            if surf <= max_m && best.is_none_or(|b| surf < b) {
                best = Some(surf);
            }
        }
        best
    }
    fn motion(&self, pos: Pos, range_m: f64, window_s: f64) -> bool {
        self.cfg.objects.iter().any(|o| {
            o.present
                && matches!(o.kind, ObjectKind::Hand | ObjectKind::Person)
                && o.pos.dist(pos) <= range_m
                && self
                    .moved_at
                    .get(&o.id)
                    .is_some_and(|t| self.world_secs - t <= window_s)
        })
    }
    fn magnetic_field(&self, pos: Pos) -> f64 {
        self.cfg
            .objects
            .iter()
            .filter(|o| o.present && o.kind == ObjectKind::Magnet)
            .map(|o| {
                let d = o.pos.dist(pos).max(0.002);
                o.strength_mt * (0.01 / d).powi(3)
            })
            .sum()
    }
    fn noise_enabled(&self) -> bool {
        self.cfg.noise
    }
}

impl WorldMut for World {
    fn set_var(&mut self, key: &str, value: f64) {
        self.set(key, value);
    }
    fn water(&mut self, pot: &str, ml: f64) {
        World::water(self, pot, ml);
    }
    fn set_heat_source(&mut self, id: &str, pos: Pos, watts: f64) {
        if watts <= 0.0 {
            self.heat.remove(id);
        } else {
            self.heat.insert(id.to_string(), (pos, watts));
        }
    }
    fn set_light_source(&mut self, id: &str, pos: Pos, candela: f64) {
        if candela <= 0.0 {
            self.lights.remove(id);
        } else {
            self.lights.insert(id.to_string(), (pos, candela));
        }
    }
}

#[cfg(test)]
mod tests;
