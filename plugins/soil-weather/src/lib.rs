//! Soil & weather pack (PLUG-5, WORLD-5): a world model that takes over the pots'
//! evaporation and adds weather.
//!
//! * Evaporation from temperature, humidity, light and wetness (≈ 1.2 %VWC/h indoors at
//!   21 °C / 45 %RH), 1.6× outdoors (wind) — `weather.evap_rate_pct_h`.
//! * Rain events for outdoor pots: a Poisson process (mean every 72 h, 1–4 h long,
//!   1–8 mm/h), watering the pot through its surface; runoff above saturation —
//!   `weather.rain_mm_h`, `weather.raining`.
//! * Sun position for the world's location and clock (NOAA approximation) —
//!   `sun.azimuth_deg`, `weather.sun_elevation_deg`, `weather.daylight`.

use bb_plugin_sdk::prelude::*;
use std::f64::consts::PI;

struct Pot {
    id: String,
    indoor: bool,
    saturation: f64,
    /// Surface area (cm²) from the soil volume (a pot as tall as it is wide).
    area_cm2: f64,
}

struct Weather {
    pots: Vec<Pot>,
    lat: f64,
    lon: f64,
    utc_offset_h: f64,
    start_doy: f64,
    start_hour: f64,
    mean_rain_interval_h: f64,
    next_rain_h: f64,
    rain_left_h: f64,
    rain_mm_h: f64,
    elapsed_h: f64,
    any_outdoor: bool,
}

fn day_of_year(month: f64, day: f64) -> f64 {
    let cum = [
        0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0,
    ];
    cum[((month as usize).clamp(1, 12)) - 1] + day
}

impl Weather {
    /// (elevation°, azimuth°) for the current world time.
    fn sun(&self, ctx: &Ctx) -> (f64, f64) {
        let hours = self.start_hour + ctx.world_secs() / 3600.0;
        let doy = self.start_doy + (hours / 24.0).floor();
        let local_h = hours.rem_euclid(24.0);
        let g = 2.0 * PI / 365.0 * (doy - 1.0 + (local_h - 12.0) / 24.0);
        let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
            + 0.000907 * (2.0 * g).sin()
            - 0.002697 * (3.0 * g).cos()
            + 0.00148 * (3.0 * g).sin();
        let eqt = 229.18
            * (0.000075 + 0.001868 * g.cos()
                - 0.032077 * g.sin()
                - 0.014615 * (2.0 * g).cos()
                - 0.040849 * (2.0 * g).sin());
        let tst = local_h * 60.0 + eqt + 4.0 * self.lon - 60.0 * self.utc_offset_h;
        let ha = (tst / 4.0 - 180.0).to_radians();
        let lat = self.lat.to_radians();
        let cos_z = lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos();
        let zen = cos_z.clamp(-1.0, 1.0).acos();
        let elev = 90.0 - zen.to_degrees();
        let az = {
            let s = -(ha.sin()) * decl.cos() / zen.sin().max(1e-9);
            let c = (decl.sin() - lat.sin() * cos_z) / (lat.cos() * zen.sin()).max(1e-9);
            (s.atan2(c).to_degrees() + 360.0) % 360.0
        };
        (elev, az)
    }

    fn schedule_rain(&mut self, ctx: &Ctx) {
        // exponential waiting time
        let u = ctx.rng().max(1e-9);
        self.next_rain_h = self.elapsed_h - self.mean_rain_interval_h * u.ln();
    }
}

impl WorldModel for Weather {
    fn init(cfg: &Config) -> Result<Self, String> {
        let w = &cfg.world;
        let mut pots = Vec::new();
        for p in w
            .get("pots")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default()
        {
            let vol = p
                .get("soil_volume_ml")
                .and_then(|v| v.as_f64())
                .unwrap_or(1000.0);
            // a cylinder with h = d: V = π d³ / 4 → d = (4V/π)^(1/3)
            let d = (4.0 * vol / PI).cbrt();
            pots.push(Pot {
                id: p
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pot1")
                    .to_string(),
                indoor: p.get("indoor").and_then(|v| v.as_bool()).unwrap_or(true),
                saturation: p.get("saturation").and_then(|v| v.as_f64()).unwrap_or(60.0),
                area_cm2: PI * d * d / 4.0,
            });
        }
        let light = w.get("light").cloned().unwrap_or_default();
        let loc = light.get("location").cloned().unwrap_or_default();
        let start = light.get("start").cloned().unwrap_or_default();
        let f = |v: &Value, k: &str, d: f64| v.get(k).and_then(|x| x.as_f64()).unwrap_or(d);
        let any_outdoor = pots.iter().any(|p| !p.indoor);
        Ok(Weather {
            pots,
            lat: f(&loc, "lat", 42.1946),
            lon: f(&loc, "lon", -122.7095),
            utc_offset_h: f(&loc, "utc_offset_h", -8.0),
            start_doy: day_of_year(f(&start, "month", 6.0), f(&start, "day", 21.0)),
            start_hour: f(&start, "hour", 8.0),
            mean_rain_interval_h: cfg.spec_f64("mean_rain_interval_h").unwrap_or(72.0),
            next_rain_h: f64::INFINITY,
            rain_left_h: 0.0,
            rain_mm_h: 0.0,
            elapsed_h: 0.0,
            any_outdoor,
        })
    }

    fn step(&mut self, ctx: &Ctx, dt_world_s: f64) {
        let dt_h = dt_world_s / 3600.0;
        if self.next_rain_h.is_infinite() && self.any_outdoor {
            self.schedule_rain(ctx);
        }
        self.elapsed_h += dt_h;
        let temp = ctx.world_get("air.temp_c").unwrap_or(21.0);
        let rh = ctx.world_get("air.rh").unwrap_or(45.0);
        let lux = ctx.world_get("light.ambient_lux").unwrap_or(180.0);
        // sun
        let (elev, az) = self.sun(ctx);
        ctx.world_set("sun.azimuth_deg", az);
        ctx.world_set("weather.sun_elevation_deg", elev);
        ctx.world_set("weather.daylight", if elev > -0.833 { 1.0 } else { 0.0 });
        // rain
        if self.rain_left_h <= 0.0 && self.elapsed_h >= self.next_rain_h {
            self.rain_left_h = 1.0 + 3.0 * ctx.rng();
            self.rain_mm_h = 1.0 + 7.0 * ctx.rng();
            ctx.info(&format!(
                "rain: {:.1} mm/h for {:.1} h",
                self.rain_mm_h, self.rain_left_h
            ));
        }
        let raining = self.rain_left_h > 0.0;
        if raining {
            self.rain_left_h -= dt_h;
            if self.rain_left_h <= 0.0 {
                self.rain_mm_h = 0.0;
                self.schedule_rain(ctx);
            }
        }
        ctx.world_set(
            "weather.rain_mm_h",
            if raining { self.rain_mm_h } else { 0.0 },
        );
        ctx.world_set("weather.raining", raining as u8 as f64);
        // pots
        let mut shown_rate = 0.0;
        for p in &self.pots {
            let Some(vwc) = ctx.world_get(&format!("{}.vwc", p.id)) else {
                continue;
            };
            let on = ctx
                .world_get(&format!("{}.evaporation", p.id))
                .unwrap_or(1.0)
                > 0.5;
            let fc = ctx
                .world_get(&format!("{}.field_capacity", p.id))
                .unwrap_or(45.0);
            let wet = (vwc / fc.max(1.0)).clamp(0.0, 1.0);
            let dryness = ((100.0 - rh) / 55.0).clamp(0.0, 2.0);
            let heat = (1.0 + 0.03 * (temp - 21.0)).max(0.0);
            let light = 0.85 + 0.15 * (lux / 20_000.0).min(1.0);
            let wind = if p.indoor { 1.0 } else { 1.6 };
            let rate = if on {
                1.5 * heat * dryness * light * wet * wind
            } else {
                0.0
            };
            shown_rate = rate;
            let mut new = vwc - rate * dt_h;
            if raining && !p.indoor {
                // mm of rain over the surface → ml → % of the soil volume
                let ml = self.rain_mm_h * dt_h * p.area_cm2 / 10.0;
                ctx.water(&p.id, ml);
                new = ctx.world_get(&format!("{}.vwc", p.id)).unwrap_or(new) - rate * dt_h;
            }
            new = new.clamp(0.0, p.saturation);
            ctx.world_set(&format!("{}.vwc", p.id), new);
        }
        ctx.world_set("weather.evap_rate_pct_h", shown_rate);
    }
}

export_world_model!(Weather);
