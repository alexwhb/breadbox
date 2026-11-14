//! Solar position and daylight illuminance (WORLD-2).

use serde::{Deserialize, Serialize};

/// A place on Earth (default: Ashland, OR — the design's day/night cycle location).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub name: String,
    /// Degrees north.
    pub lat: f64,
    /// Degrees east (west is negative).
    pub lon: f64,
    /// Local standard time offset from UTC in hours.
    pub utc_offset_h: f64,
}

impl Default for Location {
    fn default() -> Self {
        Location {
            name: "Ashland, OR".into(),
            lat: 42.1946,
            lon: -122.7095,
            utc_offset_h: -8.0,
        }
    }
}

/// A local calendar date and time of day.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Hours since local midnight (fractional).
    pub hour: f64,
}

impl Default for DateTime {
    fn default() -> Self {
        DateTime {
            year: 2026,
            month: 6,
            day: 21,
            hour: 8.0,
        }
    }
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

impl DateTime {
    /// Day of year, 1-based.
    pub fn day_of_year(&self) -> u32 {
        const CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let m = self.month.clamp(1, 12) as usize;
        let leap = if m > 2 && is_leap(self.year) { 1 } else { 0 };
        CUM[m - 1] + self.day.max(1) + leap
    }

    /// This date/time advanced by `secs` (day rollover handled; months/years are approximated
    /// by advancing the day of year).
    pub fn plus_secs(&self, secs: f64) -> DateTime {
        let total_h = self.hour + secs / 3600.0;
        let days = (total_h / 24.0).floor();
        let hour = total_h - days * 24.0;
        let mut doy = self.day_of_year() as i64 + days as i64;
        let mut year = self.year;
        loop {
            let len = if is_leap(year) { 366 } else { 365 };
            if doy > len {
                doy -= len;
                year += 1;
            } else if doy < 1 {
                year -= 1;
                doy += if is_leap(year) { 366 } else { 365 };
            } else {
                break;
            }
        }
        let (month, day) = month_day(year, doy as u32);
        DateTime {
            year,
            month,
            day,
            hour,
        }
    }
}

fn month_day(year: i32, doy: u32) -> (u32, u32) {
    let lens = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut d = doy;
    for (i, l) in lens.iter().enumerate() {
        if d <= *l {
            return (i as u32 + 1, d);
        }
        d -= l;
    }
    (12, 31)
}

/// Solar elevation in degrees at a local date/time (NOAA-style approximation, ±0.5°).
pub fn solar_elevation(loc: &Location, at: &DateTime) -> f64 {
    let n = at.day_of_year() as f64;
    let deg = std::f64::consts::PI / 180.0;
    let gamma = 2.0 * std::f64::consts::PI / 365.0 * (n - 1.0 + (at.hour - 12.0) / 24.0);
    let decl = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin()
        - 0.006758 * (2.0 * gamma).cos()
        + 0.000907 * (2.0 * gamma).sin()
        - 0.002697 * (3.0 * gamma).cos()
        + 0.00148 * (3.0 * gamma).sin();
    let eqtime = 229.18
        * (0.000075 + 0.001868 * gamma.cos()
            - 0.032077 * gamma.sin()
            - 0.014615 * (2.0 * gamma).cos()
            - 0.040849 * (2.0 * gamma).sin());
    let time_offset = eqtime + 4.0 * loc.lon - 60.0 * loc.utc_offset_h;
    let tst = at.hour * 60.0 + time_offset;
    let ha = (tst / 4.0 - 180.0) * deg;
    let lat = loc.lat * deg;
    let cos_zen = lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos();
    90.0 - cos_zen.clamp(-1.0, 1.0).acos() / deg
}

/// Outdoor horizontal illuminance (lux) for a solar elevation and cloud cover 0…1.
/// Clear-sky noon in summer ≈ 110 klx; civil twilight fades to ~3 lux at −6°; night 0.1 lux.
pub fn daylight_lux(elevation_deg: f64, cloud_cover: f64) -> f64 {
    let c = cloud_cover.clamp(0.0, 1.0);
    let cloud = 1.0 - 0.75 * c.powf(3.4);
    let night = 0.1;
    if elevation_deg > 0.0 {
        let s = (elevation_deg.to_radians()).sin();
        // direct + diffuse, smooth at the horizon
        let clear = 128_000.0 * s * (-0.2 / s.max(0.02)).exp() + 400.0 * s.sqrt().min(1.0);
        (clear * cloud).max(400.0 * cloud) + night
    } else {
        // twilight: ~400 lux at sunset decaying ~e-fold per degree
        400.0 * cloud * (elevation_deg * 0.82).exp() + night
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solar_noon_elevation_matches_geometry() {
        let loc = Location::default();
        // June solstice, solar noon near 13:10 PST (12:10 local mean time + DST ignored)
        let mut best: f64 = -90.0;
        for i in 0..24 * 60 {
            let e = solar_elevation(
                &loc,
                &DateTime {
                    hour: i as f64 / 60.0,
                    ..DateTime::default()
                },
            );
            best = best.max(e);
        }
        // 90 − 42.19 + 23.44 = 71.25°
        assert!((best - 71.25).abs() < 0.6, "{best}");
        let midnight = solar_elevation(
            &loc,
            &DateTime {
                hour: 0.5,
                ..DateTime::default()
            },
        );
        assert!(midnight < -15.0);
        // winter solstice noon is low
        let dec = DateTime {
            year: 2026,
            month: 12,
            day: 21,
            hour: 12.2,
        };
        let e = solar_elevation(&loc, &dec);
        assert!((e - (90.0 - 42.19 - 23.44)).abs() < 1.0, "{e}");
    }

    #[test]
    fn daylight_levels_are_plausible() {
        assert!(daylight_lux(70.0, 0.0) > 90_000.0);
        assert!(daylight_lux(70.0, 1.0) < daylight_lux(70.0, 0.0) * 0.3);
        assert!(daylight_lux(0.5, 0.0) > 300.0);
        assert!(daylight_lux(-6.0, 0.0) < 5.0);
        assert!(daylight_lux(-30.0, 0.0) < 0.2);
    }

    #[test]
    fn dates_roll_over() {
        let d = DateTime {
            year: 2026,
            month: 12,
            day: 31,
            hour: 23.0,
        };
        let n = d.plus_secs(2.0 * 3600.0);
        assert_eq!((n.year, n.month, n.day), (2027, 1, 1));
        assert!((n.hour - 1.0).abs() < 1e-9);
        assert_eq!(DateTime::default().day_of_year(), 172);
    }
}
