//! E-series preferred values and resistor colour bands (WB-5: "resistor value with colour bands
//! updating live").

use crate::part::ESeries;
use serde::{Deserialize, Serialize};

const E6: [f64; 6] = [1.0, 1.5, 2.2, 3.3, 4.7, 6.8];
const E12: [f64; 12] = [1.0, 1.2, 1.5, 1.8, 2.2, 2.7, 3.3, 3.9, 4.7, 5.6, 6.8, 8.2];
const E24: [f64; 24] = [
    1.0, 1.1, 1.2, 1.3, 1.5, 1.6, 1.8, 2.0, 2.2, 2.4, 2.7, 3.0, 3.3, 3.6, 3.9, 4.3, 4.7, 5.1, 5.6,
    6.2, 6.8, 7.5, 8.2, 9.1,
];
const E96: [f64; 96] = [
    1.00, 1.02, 1.05, 1.07, 1.10, 1.13, 1.15, 1.18, 1.21, 1.24, 1.27, 1.30, 1.33, 1.37, 1.40, 1.43,
    1.47, 1.50, 1.54, 1.58, 1.62, 1.65, 1.69, 1.74, 1.78, 1.82, 1.87, 1.91, 1.96, 2.00, 2.05, 2.10,
    2.15, 2.21, 2.26, 2.32, 2.37, 2.43, 2.49, 2.55, 2.61, 2.67, 2.74, 2.80, 2.87, 2.94, 3.01, 3.09,
    3.16, 3.24, 3.32, 3.40, 3.48, 3.57, 3.65, 3.74, 3.83, 3.92, 4.02, 4.12, 4.22, 4.32, 4.42, 4.53,
    4.64, 4.75, 4.87, 4.99, 5.11, 5.23, 5.36, 5.49, 5.62, 5.76, 5.90, 6.04, 6.19, 6.34, 6.49, 6.65,
    6.81, 6.98, 7.15, 7.32, 7.50, 7.68, 7.87, 8.06, 8.25, 8.45, 8.66, 8.87, 9.09, 9.31, 9.53, 9.76,
];

pub fn mantissas(s: ESeries) -> &'static [f64] {
    match s {
        ESeries::E6 => &E6,
        ESeries::E12 => &E12,
        ESeries::E24 => &E24,
        ESeries::E96 => &E96,
    }
}

/// Nearest preferred value (log distance).
pub fn nearest(value: f64, s: ESeries) -> f64 {
    if value <= 0.0 || !value.is_finite() {
        return mantissas(s)[0];
    }
    let dec = value.log10().floor();
    let scale = 10f64.powf(dec);
    let m = value / scale;
    let mut best = (f64::MAX, 1.0);
    for &c in mantissas(s).iter().chain([10.0].iter()) {
        let d = (m.ln() - c.ln()).abs();
        if d < best.0 {
            best = (d, c);
        }
    }
    round_sig(best.1 * scale, 3)
}

/// All preferred values between `lo` and `hi` (for the UI picker).
pub fn values(s: ESeries, lo: f64, hi: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut dec = lo.max(1e-15).log10().floor() as i32;
    while 10f64.powi(dec) <= hi * 1.0001 {
        for &m in mantissas(s) {
            let v = round_sig(m * 10f64.powi(dec), 3);
            if v >= lo * 0.9999 && v <= hi * 1.0001 {
                out.push(v);
            }
        }
        dec += 1;
    }
    out
}

fn round_sig(v: f64, sig: i32) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let p = sig - 1 - v.abs().log10().floor() as i32;
    let f = 10f64.powi(p);
    (v * f).round() / f
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandColor {
    Black,
    Brown,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Violet,
    Grey,
    White,
    Gold,
    Silver,
}

impl BandColor {
    const DIGITS: [BandColor; 10] = [
        BandColor::Black,
        BandColor::Brown,
        BandColor::Red,
        BandColor::Orange,
        BandColor::Yellow,
        BandColor::Green,
        BandColor::Blue,
        BandColor::Violet,
        BandColor::Grey,
        BandColor::White,
    ];
    /// sRGB hex for drawing.
    pub fn hex(self) -> &'static str {
        match self {
            BandColor::Black => "#1A1A1A",
            BandColor::Brown => "#7A4A21",
            BandColor::Red => "#D0312D",
            BandColor::Orange => "#F08A24",
            BandColor::Yellow => "#F2D43D",
            BandColor::Green => "#3A9D4A",
            BandColor::Blue => "#2F6FD0",
            BandColor::Violet => "#8A4FC8",
            BandColor::Grey => "#8C8C8C",
            BandColor::White => "#F4F4F4",
            BandColor::Gold => "#C8A24A",
            BandColor::Silver => "#C0C0C0",
        }
    }
}

/// Colour bands for a resistor: 4 bands (2 digits) for tolerance ≥ 2 %, 5 bands (3 digits)
/// for 1 %. `None` for values that cannot be coded (≤ 0, ≥ 100 GΩ).
pub fn color_bands(ohms: f64, tolerance_pct: f64) -> Option<Vec<BandColor>> {
    if !(ohms > 0.0 && ohms < 1e11) {
        return None;
    }
    let digits = if tolerance_pct < 1.5 { 3 } else { 2 };
    let v = round_sig(ohms, digits);
    let exp = v.log10().floor() as i32 - (digits - 1);
    let sig = (v / 10f64.powi(exp)).round() as i64;
    let mut out = Vec::new();
    let s = format!("{sig:0width$}", width = digits as usize);
    for c in s.chars() {
        out.push(BandColor::DIGITS[c.to_digit(10)? as usize]);
    }
    out.push(match exp {
        -2 => BandColor::Silver,
        -1 => BandColor::Gold,
        e @ 0..=9 => BandColor::DIGITS[e as usize],
        _ => return None,
    });
    out.push(match tolerance_pct {
        t if t < 1.5 => BandColor::Brown,
        t if t < 3.0 => BandColor::Red,
        t if t < 7.5 => BandColor::Gold,
        _ => BandColor::Silver,
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use BandColor::*;

    #[test]
    fn nearest_values() {
        assert_eq!(nearest(4600.0, ESeries::E24), 4700.0);
        assert_eq!(nearest(215.0, ESeries::E12), 220.0);
        assert_eq!(nearest(9.7e3, ESeries::E6), 10e3);
        assert_eq!(nearest(10_040.0, ESeries::E96), 10_000.0);
        let v = values(ESeries::E12, 100.0, 1000.0);
        assert_eq!(v.first(), Some(&100.0));
        assert_eq!(v.last(), Some(&1000.0));
        assert_eq!(v.len(), 13);
    }

    #[test]
    fn bands() {
        assert_eq!(
            color_bands(220.0, 5.0).unwrap(),
            vec![Red, Red, Brown, Gold]
        );
        assert_eq!(
            color_bands(10e3, 5.0).unwrap(),
            vec![Brown, Black, Orange, Gold]
        );
        assert_eq!(
            color_bands(4.7, 10.0).unwrap(),
            vec![Yellow, Violet, Gold, Silver]
        );
        assert_eq!(
            color_bands(330.0, 5.0).unwrap(),
            vec![Orange, Orange, Brown, Gold]
        );
        assert_eq!(
            color_bands(10.2e3, 1.0).unwrap(),
            vec![Brown, Black, Red, Red, Brown]
        );
        assert!(color_bands(0.0, 5.0).is_none());
    }
}
