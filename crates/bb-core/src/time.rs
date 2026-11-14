use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// Simulated time in **picoseconds** since simulation start (u64 ≈ 213 days of range).
///
/// One AVR cycle at 16 MHz is exactly 62 500 ps, so cycle-exact scheduling never rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct SimTime(pub u64);

impl SimTime {
    pub const ZERO: SimTime = SimTime(0);
    pub const MAX: SimTime = SimTime(u64::MAX);
    pub const PS_PER_SEC: u64 = 1_000_000_000_000;

    pub const fn from_ps(ps: u64) -> Self {
        SimTime(ps)
    }
    pub const fn from_ns(ns: u64) -> Self {
        SimTime(ns * 1_000)
    }
    pub const fn from_us(us: u64) -> Self {
        SimTime(us * 1_000_000)
    }
    pub const fn from_ms(ms: u64) -> Self {
        SimTime(ms * 1_000_000_000)
    }
    pub const fn from_secs(s: u64) -> Self {
        SimTime(s * Self::PS_PER_SEC)
    }
    /// Rounds to the nearest picosecond; negative / NaN inputs clamp to zero.
    pub fn from_secs_f64(s: f64) -> Self {
        if s.is_nan() || s <= 0.0 {
            return SimTime::ZERO;
        }
        let ps = (s * Self::PS_PER_SEC as f64).round();
        if ps >= u64::MAX as f64 { SimTime::MAX } else { SimTime(ps as u64) }
    }
    /// Time of `cycles` clock cycles at `hz` (exact when `hz` divides 1e12).
    pub fn from_cycles(cycles: u64, hz: u64) -> Self {
        SimTime(((cycles as u128 * Self::PS_PER_SEC as u128) / hz as u128) as u64)
    }
    /// Number of whole clock cycles at `hz` elapsed by this time.
    pub fn to_cycles(self, hz: u64) -> u64 {
        ((self.0 as u128 * hz as u128) / Self::PS_PER_SEC as u128) as u64
    }
    pub const fn as_ps(self) -> u64 {
        self.0
    }
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / Self::PS_PER_SEC as f64
    }
    pub fn as_millis(self) -> u64 {
        self.0 / 1_000_000_000
    }
    pub fn saturating_sub(self, rhs: SimTime) -> SimTime {
        SimTime(self.0.saturating_sub(rhs.0))
    }
}

impl Add for SimTime {
    type Output = SimTime;
    fn add(self, rhs: SimTime) -> SimTime {
        SimTime(self.0.saturating_add(rhs.0))
    }
}
impl AddAssign for SimTime {
    fn add_assign(&mut self, rhs: SimTime) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}
impl Sub for SimTime {
    type Output = SimTime;
    fn sub(self, rhs: SimTime) -> SimTime {
        SimTime(self.0.saturating_sub(rhs.0))
    }
}

/// Formats as the design's sim clock: `mm:ss.mmm` (or `h:mm:ss.mmm` past an hour).
impl fmt::Display for SimTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_ms = self.as_millis();
        let ms = total_ms % 1000;
        let s = (total_ms / 1000) % 60;
        let m = (total_ms / 60_000) % 60;
        let h = total_ms / 3_600_000;
        if h > 0 { write!(f, "{h}:{m:02}:{s:02}.{ms:03}") } else { write!(f, "{m:02}:{s:02}.{ms:03}") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avr_cycle_is_exact() {
        assert_eq!(SimTime::from_cycles(1, 16_000_000).as_ps(), 62_500);
        assert_eq!(SimTime::from_cycles(16_000_000, 16_000_000), SimTime::from_secs(1));
        assert_eq!(SimTime::from_secs(1).to_cycles(16_000_000), 16_000_000);
    }

    #[test]
    fn display_matches_design_clock() {
        assert_eq!(SimTime::from_ms(12_500).to_string(), "00:12.500");
        assert_eq!(SimTime::from_secs(3_725).to_string(), "1:02:05.000");
    }

    #[test]
    fn f64_round_trip() {
        let t = SimTime::from_secs_f64(2.041e-3);
        assert!((t.as_secs_f64() - 2.041e-3).abs() < 1e-15);
        assert_eq!(SimTime::from_secs_f64(-1.0), SimTime::ZERO);
    }
}
