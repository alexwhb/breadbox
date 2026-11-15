//! Footprints from the parts library's footprint names (`PRO-7`). KiCad-style names
//! (`Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P10.16mm_Horizontal`, `Package_DIP:DIP-8_W7.62mm`,
//! `LED_SMD:LED_0805_2012Metric`, …) are turned into pads with the right pitch, drill and shape;
//! boards, modules and anything without a known pattern use the part's bench lead positions
//! (0.1" grid = 2.54 mm), which is how the real parts are laid out. Pads map to the part's pins
//! in order (pad `n` = pin `n-1`), so every footprint is complete for its part.

use crate::part::PartDef;
use serde::{Deserialize, Serialize};

/// 0.1 inch in mm.
pub const PITCH: f64 = 2.54;

/// Pad shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadShape {
    Round,
    Rect,
    Oval,
}

/// One pad (footprint-local mm, y down).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    /// Part pin id it connects.
    pub pin: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub shape: PadShape,
    /// Plated hole diameter (THT); `None` = SMD on the top layer.
    pub drill: Option<f64>,
}

impl Pad {
    pub fn is_smd(&self) -> bool {
        self.drill.is_none()
    }
    /// Radius of the pad's bounding circle (clearance checks treat pads as their outline).
    pub fn radius(&self) -> f64 {
        self.w.max(self.h) / 2.0
    }
}

/// A silkscreen line (footprint-local mm).
pub type Silk = ((f64, f64), (f64, f64));

/// A footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Footprint {
    /// Library name (`Package_DIP:DIP-8_W7.62mm`).
    pub name: String,
    pub pads: Vec<Pad>,
    /// Body outline on the silkscreen.
    pub silk: Vec<Silk>,
    /// Courtyard `(x0, y0, x1, y1)`.
    pub bounds: (f64, f64, f64, f64),
}

/// A number after `key` in a footprint name (`P10.16mm` → 10.16).
fn num_after(name: &str, key: &str) -> Option<f64> {
    let i = name.find(key)? + key.len();
    let s: String = name[i..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    s.parse().ok()
}

fn tht(pin: &str, x: f64, y: f64, d: f64, drill: f64, shape: PadShape) -> Pad {
    Pad {
        pin: pin.into(),
        x,
        y,
        w: d,
        h: d,
        shape,
        drill: Some(drill),
    }
}

fn smd(pin: &str, x: f64, y: f64, w: f64, h: f64) -> Pad {
    Pad {
        pin: pin.into(),
        x,
        y,
        w,
        h,
        shape: PadShape::Rect,
        drill: None,
    }
}

fn rect_silk(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Silk> {
    vec![
        ((x0, y0), (x1, y0)),
        ((x1, y0), (x1, y1)),
        ((x1, y1), (x0, y1)),
        ((x0, y1), (x0, y0)),
    ]
}

/// The footprint of a part (`smd` picks the SMD alternative when the part has one).
pub fn footprint(def: &PartDef, smd_variant: bool) -> Footprint {
    let pins: Vec<String> = def.pins.iter().map(|p| p.id.clone()).collect();
    let pin = |i: usize| pins.get(i).cloned().unwrap_or_else(|| format!("{}", i + 1));
    let name = match (&def.pcb.smd, smd_variant) {
        (Some(s), true) => s.clone(),
        _ => def.pcb.footprint.clone(),
    };
    let lname = name.to_ascii_lowercase();
    let mut pads: Vec<Pad> = Vec::new();
    let mut silk: Vec<Silk> = Vec::new();
    let n = pins.len();
    // ---- SMD packages
    if lname.contains("_0805") || lname.contains("_0603") || lname.contains("_1206") {
        let (dx, w, h) = if lname.contains("0603") {
            (0.8, 0.9, 0.95)
        } else if lname.contains("1206") {
            (1.5, 1.15, 1.8)
        } else {
            (0.95, 1.0, 1.45)
        };
        pads.push(smd(&pin(0), -dx, 0.0, w, h));
        pads.push(smd(&pin(1), dx, 0.0, w, h));
        silk.extend(rect_silk(
            -dx - w / 2.0 - 0.3,
            -h / 2.0 - 0.3,
            dx + w / 2.0 + 0.3,
            h / 2.0 + 0.3,
        ));
    } else if lname.contains("sot-23") && !lname.contains("sot-223") {
        pads.push(smd(&pin(0), -0.95, 1.1, 0.6, 1.0));
        pads.push(smd(&pin(1), 0.95, 1.1, 0.6, 1.0));
        pads.push(smd(&pin(2), 0.0, -1.1, 0.6, 1.0));
        silk.extend(rect_silk(-1.6, -0.7, 1.6, 0.7));
    } else if lname.contains("sod-123") {
        pads.push(smd(&pin(0), -1.65, 0.0, 0.9, 1.2));
        pads.push(smd(&pin(1), 1.65, 0.0, 0.9, 1.2));
        silk.extend(rect_silk(-1.4, -0.9, 1.4, 0.9));
    } else if lname.contains("sot-223") {
        for i in 0..3.min(n) {
            pads.push(smd(&pin(i), -2.3 + 2.3 * i as f64, 3.15, 1.0, 2.0));
        }
        if n > 3 {
            pads.push(smd(&pin(3), 0.0, -3.15, 3.25, 2.0));
        }
        silk.extend(rect_silk(-3.3, -1.8, 3.3, 1.8));
    } else if lname.contains("plcc4") || lname.contains("ws2812") {
        for (i, (x, y)) in [(-2.45, -1.6), (-2.45, 1.6), (2.45, 1.6), (2.45, -1.6)]
            .iter()
            .enumerate()
            .take(n.max(1))
        {
            pads.push(smd(&pin(i), *x, *y, 1.5, 1.0));
        }
        silk.extend(rect_silk(-2.6, -2.6, 2.6, 2.6));
    // ---- THT two-terminal parts with a pitch in the name
    } else if (lname.contains("r_axial")
        || lname.contains("d_do-")
        || lname.contains("c_disc")
        || lname.contains("cp_radial")
        || lname.contains("l_radial")
        || lname.contains("r_ldr")
        || lname.contains("buzzer"))
        && n >= 2
    {
        let pitch = num_after(&name, "_P")
            .or_else(|| num_after(&name, "RM"))
            .unwrap_or(PITCH * 4.0);
        let (d, drill) = if lname.contains("d_do-41") {
            (2.0, 1.0)
        } else {
            (1.6, 0.8)
        };
        pads.push(tht(&pin(0), 0.0, 0.0, d, drill, PadShape::Rect));
        pads.push(tht(&pin(1), pitch, 0.0, d, drill, PadShape::Round));
        let body = if lname.contains("_l") {
            num_after(&lname, "_l").unwrap_or(pitch * 0.6)
        } else {
            pitch * 0.6
        };
        let dia = num_after(&lname, "_d").unwrap_or(2.5).max(1.6);
        let x0 = (pitch - body) / 2.0;
        if lname.contains("radial") || lname.contains("disc") || lname.contains("buzzer") {
            let r = (dia / 2.0).max(pitch / 2.0 + 1.0);
            let c = (pitch / 2.0, 0.0);
            for k in 0..24 {
                let a0 = k as f64 / 24.0 * std::f64::consts::TAU;
                let a1 = (k + 1) as f64 / 24.0 * std::f64::consts::TAU;
                silk.push((
                    (c.0 + r * a0.cos(), c.1 + r * a0.sin()),
                    (c.0 + r * a1.cos(), c.1 + r * a1.sin()),
                ));
            }
        } else {
            silk.extend(rect_silk(x0, -dia / 2.0, x0 + body, dia / 2.0));
        }
    } else if lname.contains("led_d5.0mm") || lname.contains("led_d3.0mm") {
        let pitch = if n > 2 { 1.27 } else { PITCH };
        for i in 0..n {
            let shape = if i == 0 {
                PadShape::Rect
            } else {
                PadShape::Round
            };
            pads.push(tht(&pin(i), pitch * i as f64, 0.0, 1.8, 0.9, shape));
        }
        let c = (pitch * (n as f64 - 1.0) / 2.0, 0.0);
        let r = 2.9;
        for k in 0..24 {
            let a0 = k as f64 / 24.0 * std::f64::consts::TAU;
            let a1 = (k + 1) as f64 / 24.0 * std::f64::consts::TAU;
            silk.push((
                (c.0 + r * a0.cos(), c.1 + r * a0.sin()),
                (c.0 + r * a1.cos(), c.1 + r * a1.sin()),
            ));
        }
    } else if lname.contains("to-92") {
        for i in 0..n.min(3) {
            pads.push(tht(
                &pin(i),
                1.27 * i as f64,
                0.0,
                1.05,
                0.75,
                PadShape::Oval,
            ));
        }
        silk.extend(rect_silk(-0.9, -1.9, 3.44, 1.1));
    } else if lname.contains("to-220") {
        for i in 0..n.min(3) {
            let mut p = tht(&pin(i), PITCH * i as f64, 0.0, 1.9, 1.1, PadShape::Oval);
            p.h = 2.4;
            pads.push(p);
        }
        silk.extend(rect_silk(-2.5, -3.2, 7.6, 1.6));
    } else if let Some(count) = num_after(&name, "DIP-")
        .or_else(|| (lname.contains("sw_dip") && n >= 2).then_some((n.max(2) * 2) as f64))
    {
        // two rows, 7.62 mm apart, 2.54 mm pitch; pin 1 top-left, counter-clockwise
        let count = (count as usize).max(n).max(2);
        let half = count / 2;
        let w = num_after(&name, "_W").unwrap_or(7.62);
        for i in 0..n.min(count) {
            let (x, y) = if i < half {
                (PITCH * i as f64, w)
            } else {
                (PITCH * (count - 1 - i) as f64, 0.0)
            };
            let shape = if i == 0 {
                PadShape::Rect
            } else {
                PadShape::Oval
            };
            pads.push(tht(&pin(i), x, y, 1.6, 0.8, shape));
        }
        silk.extend(rect_silk(
            -1.27,
            1.0,
            PITCH * (half as f64 - 1.0) + 1.27,
            w - 1.0,
        ));
    } else if lname.contains("pinheader") || lname.contains("pin_header") {
        for i in 0..n {
            let shape = if i == 0 {
                PadShape::Rect
            } else {
                PadShape::Oval
            };
            pads.push(tht(&pin(i), PITCH * i as f64, 0.0, 1.7, 1.0, shape));
        }
        silk.extend(rect_silk(
            -1.33,
            -1.33,
            PITCH * (n as f64 - 1.0) + 1.33,
            1.33,
        ));
    } else if lname.contains("sw_push_6mm") && n >= 2 {
        let at = [(0.0, 0.0), (0.0, 4.5), (6.5, 0.0), (6.5, 4.5)];
        for i in 0..n.min(4) {
            pads.push(tht(&pin(i), at[i].0, at[i].1, 2.0, 1.1, PadShape::Round));
        }
        silk.extend(rect_silk(0.25, -0.75, 6.25, 5.25));
    }
    // ---- everything else: the bench lead geometry (boards, modules, displays, sensors)
    if pads.is_empty() {
        let leads = &def.bench.leads;
        if !leads.is_empty() {
            for (i, l) in leads.iter().enumerate() {
                let shape = if i == 0 {
                    PadShape::Rect
                } else {
                    PadShape::Round
                };
                pads.push(tht(&l.pin, l.x * PITCH, l.y * PITCH, 1.7, 1.0, shape));
            }
        } else {
            for i in 0..n {
                let shape = if i == 0 {
                    PadShape::Rect
                } else {
                    PadShape::Oval
                };
                pads.push(tht(&pin(i), PITCH * i as f64, 0.0, 1.7, 1.0, shape));
            }
        }
        let (mut x0, mut y0, mut x1, mut y1) = bbox(&pads);
        // boards keep their real outline
        let (w, h) = def.bench.size;
        if def.is_mcu() && w > 0.0 {
            x0 = x0.min(0.0);
            y0 = y0.min(0.0);
            x1 = x1.max(w * PITCH);
            y1 = y1.max(h * PITCH);
        }
        silk.extend(rect_silk(x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0));
    }
    let (x0, y0, x1, y1) = bbox(&pads);
    let (mut bx0, mut by0, mut bx1, mut by1) = (x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0);
    for (a, b) in &silk {
        for p in [a, b] {
            bx0 = bx0.min(p.0 - 0.25);
            by0 = by0.min(p.1 - 0.25);
            bx1 = bx1.max(p.0 + 0.25);
            by1 = by1.max(p.1 + 0.25);
        }
    }
    Footprint {
        name,
        pads,
        silk,
        bounds: (bx0, by0, bx1, by1),
    }
}

fn bbox(pads: &[Pad]) -> (f64, f64, f64, f64) {
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in pads {
        b.0 = b.0.min(p.x - p.w / 2.0);
        b.1 = b.1.min(p.y - p.h / 2.0);
        b.2 = b.2.max(p.x + p.w / 2.0);
        b.3 = b.3.max(p.y + p.h / 2.0);
    }
    if !b.0.is_finite() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn every_part_with_a_footprint_gets_complete_pads() {
        let lib = Library::builtin();
        let mut seen = 0;
        for def in lib.parts() {
            if def.pcb.footprint.is_empty() {
                continue;
            }
            seen += 1;
            for smd in [false, true] {
                if smd && def.pcb.smd.is_none() {
                    continue;
                }
                let f = footprint(def, smd);
                assert!(!f.pads.is_empty(), "{}: {}", def.id, f.name);
                for pin in &def.pins {
                    if def.is_breadboard() {
                        continue;
                    }
                    assert!(
                        f.pads.iter().any(|p| p.pin == pin.id) || f.pads.len() < def.pins.len(),
                        "{}: pin {} has no pad in {}",
                        def.id,
                        pin.id,
                        f.name
                    );
                }
                // no two pads on top of each other
                for (i, a) in f.pads.iter().enumerate() {
                    for b in &f.pads[i + 1..] {
                        let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                        assert!(d > 0.5, "{}: pads {} and {} overlap", def.id, a.pin, b.pin);
                    }
                }
            }
        }
        assert!(seen > 40, "{seen}");
        let r = lib.get("resistor").unwrap();
        let f = footprint(r, false);
        assert!((f.pads[1].x - 10.16).abs() < 1e-9, "DIN0207 pitch 10.16 mm");
        assert!(f.pads[0].drill.is_some());
        let f = footprint(r, true);
        assert!(f.pads.iter().all(|p| p.is_smd()), "0805");
        let uno = footprint(lib.get("uno").unwrap(), false);
        assert!(uno.pads.iter().any(|p| p.pin == "D13"));
        assert!(uno.bounds.2 - uno.bounds.0 > 60.0, "UNO shield outline");
    }
}
