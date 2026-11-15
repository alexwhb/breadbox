//! Placement and propagation: positions in metres on the floor plan, walls, and log-distance
//! path loss → RSSI (`RF-1`).
//!
//! `PL(d) = PL(d0) + 10·n·log10(d/d0) + Σ wall losses`, `d0 = 1 m`, with `PL(d0)` the free-space
//! loss at the carrier frequency. Defaults are chosen so the garden-node bench reads the design's
//! `-54 dBm (3.2 m, 1 wall)` from the default 16 dBm access point.

use serde::{Deserialize, Serialize};

/// A point on the floor plan, metres.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Pos {
    pub x: f64,
    pub y: f64,
}

impl Pos {
    pub const fn new(x: f64, y: f64) -> Self {
        Pos { x, y }
    }
    pub fn dist(self, o: Pos) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// A wall segment with its attenuation (dB per crossing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub a: Pos,
    pub b: Pos,
    pub loss_db: f64,
    #[serde(default)]
    pub label: String,
}

impl Wall {
    /// An interior wall (15 dB at 2.4 GHz, plasterboard + studs + furniture).
    pub fn interior(a: Pos, b: Pos) -> Self {
        Wall {
            a,
            b,
            loss_db: 15.0,
            label: "interior wall".into(),
        }
    }
    /// An exterior / concrete wall (25 dB).
    pub fn exterior(a: Pos, b: Pos) -> Self {
        Wall {
            a,
            b,
            loss_db: 25.0,
            label: "exterior wall".into(),
        }
    }

    fn crosses(&self, p: Pos, q: Pos) -> bool {
        fn orient(a: Pos, b: Pos, c: Pos) -> f64 {
            (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
        }
        let d1 = orient(self.a, self.b, p);
        let d2 = orient(self.a, self.b, q);
        let d3 = orient(p, q, self.a);
        let d4 = orient(p, q, self.b);
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    }
}

/// Floor plan: walls shared by every radio technology.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Geometry {
    pub walls: Vec<Wall>,
}

/// Result of a link-budget computation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub distance_m: f64,
    pub walls: u32,
    pub wall_loss_db: f64,
    pub path_loss_db: f64,
    pub rssi_dbm: f64,
}

impl Geometry {
    /// Walls crossed by the straight line between `a` and `b`: (count, total dB).
    pub fn walls_between(&self, a: Pos, b: Pos) -> (u32, f64) {
        self.walls
            .iter()
            .filter(|w| w.crosses(a, b))
            .fold((0, 0.0), |(n, db), w| (n + 1, db + w.loss_db))
    }

    /// Link budget from a transmitter at `a` to a receiver at `b`.
    pub fn link(&self, model: &PathLoss, tx_dbm: f64, a: Pos, b: Pos) -> Link {
        let d = a.dist(b);
        let (walls, wall_loss_db) = self.walls_between(a, b);
        let pl = model.loss_db(d) + wall_loss_db;
        Link {
            distance_m: d,
            walls,
            wall_loss_db,
            path_loss_db: pl,
            rssi_dbm: tx_dbm - pl,
        }
    }
}

/// Log-distance path loss model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathLoss {
    /// Carrier frequency (Hz), used for the 1 m free-space reference loss.
    pub freq_hz: f64,
    /// Path-loss exponent `n` (2 = free space, ~3 indoors).
    pub exponent: f64,
}

impl PathLoss {
    /// 2.4 GHz Wi-Fi indoors (channel 6, n = 3).
    pub const WIFI_INDOOR: PathLoss = PathLoss {
        freq_hz: 2.437e9,
        exponent: 3.0,
    };
    /// 2.4 GHz BLE in a room (n = 2.2).
    pub const BLE_ROOM: PathLoss = PathLoss {
        freq_hz: 2.44e9,
        exponent: 2.2,
    };

    /// Free-space path loss at 1 m (dB).
    pub fn pl0_db(&self) -> f64 {
        let lambda = 299_792_458.0 / self.freq_hz;
        20.0 * (4.0 * std::f64::consts::PI / lambda).log10()
    }

    /// Path loss at `d` metres (distances under 10 cm are clamped).
    pub fn loss_db(&self, d: f64) -> f64 {
        self.pl0_db() + 10.0 * self.exponent * d.max(0.1).log10()
    }
}

/// 2.4 GHz channel centre frequency (channels 1–13; 14 = 2484 MHz).
pub fn wifi_channel_hz(ch: u8) -> f64 {
    match ch {
        14 => 2.484e9,
        c => 2.407e9 + 5e6 * c.clamp(1, 13) as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_rssi_at_3_2_m_through_one_wall() {
        let g = Geometry {
            walls: vec![Wall::interior(Pos::new(1.5, -5.0), Pos::new(1.5, 5.0))],
        };
        let l = g.link(
            &PathLoss::WIFI_INDOOR,
            16.0,
            Pos::new(0.0, 0.0),
            Pos::new(3.2, 0.0),
        );
        assert_eq!(l.walls, 1);
        assert!((l.distance_m - 3.2).abs() < 1e-12);
        assert_eq!(l.rssi_dbm.round(), -54.0, "{l:?}");
        // Free-space reference: 40.2 dB at 2.437 GHz, 1 m.
        assert!((PathLoss::WIFI_INDOOR.pl0_db() - 40.18).abs() < 0.05);
        // Doubling the distance costs 10·n·log10(2) ≈ 9 dB; parallel walls are not crossed.
        let far = g.link(
            &PathLoss::WIFI_INDOOR,
            16.0,
            Pos::new(0.0, 0.0),
            Pos::new(6.4, 0.0),
        );
        assert!((l.rssi_dbm - far.rssi_dbm - 9.03).abs() < 0.01);
        let (n, _) = g.walls_between(Pos::new(0.0, 0.0), Pos::new(1.0, 3.0));
        assert_eq!(n, 0);
    }
}
