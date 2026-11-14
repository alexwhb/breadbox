//! 2-D placement transforms shared by the bench and the schematic (0.1" units, y down).

use serde::{Deserialize, Serialize};

/// Where a part sits in a view: position of its local origin, rotation (degrees, multiples of
/// 90 for snapping views), mirror across the local y axis (applied before rotation).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rot: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip: bool,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Placement {
    pub fn at(x: f64, y: f64) -> Placement {
        Placement {
            x,
            y,
            rot: 0.0,
            flip: false,
        }
    }
    pub fn rotated(mut self, deg: f64) -> Placement {
        self.rot = (self.rot + deg).rem_euclid(360.0);
        self
    }
    /// Local → view coordinates.
    pub fn apply(&self, lx: f64, ly: f64) -> (f64, f64) {
        let lx = if self.flip { -lx } else { lx };
        let r = self.rot.to_radians();
        let (s, c) = r.sin_cos();
        // snap tiny trig errors for right angles
        let snap = |v: f64| if v.abs() < 1e-12 { 0.0 } else { v };
        let (s, c) = (snap(s), snap(c));
        let x = lx * c - ly * s;
        let y = lx * s + ly * c;
        (self.x + x, self.y + y)
    }
    /// View → local coordinates.
    pub fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.x, y - self.y);
        let r = -self.rot.to_radians();
        let (s, c) = r.sin_cos();
        let lx = dx * c - dy * s;
        let ly = dx * s + dy * c;
        (if self.flip { -lx } else { lx }, ly)
    }
}

/// Points closer than this (0.1" units) are the same point (lead in hole, wire on anchor).
pub const SNAP: f64 = 0.26;

pub fn same_point(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < SNAP && (a.1 - b.1).abs() < SNAP
}

/// Spatial hash key for snapping.
pub fn cell(p: (f64, f64)) -> (i64, i64) {
    ((p.0 * 2.0).round() as i64, (p.1 * 2.0).round() as i64)
}

/// Axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}

/// Distance from point p to segment ab.
pub fn point_segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_round_trip() {
        let p = Placement {
            x: 10.0,
            y: 5.0,
            rot: 90.0,
            flip: true,
        };
        let (x, y) = p.apply(4.0, 0.0);
        assert_eq!((x, y), (10.0, 1.0));
        let (lx, ly) = p.inverse(x, y);
        assert!((lx - 4.0).abs() < 1e-12 && ly.abs() < 1e-12);
        assert!(same_point((1.0, 1.0), (1.2, 0.9)));
        assert!(!same_point((1.0, 1.0), (1.5, 1.0)));
        assert!((point_segment_distance((1.0, 1.0), (0.0, 0.0), (2.0, 0.0)) - 1.0).abs() < 1e-12);
    }
}
