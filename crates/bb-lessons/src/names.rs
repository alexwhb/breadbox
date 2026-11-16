//! Friendly names for what's on the bench, so goals and diagnoses read like a person talking
//! about the breadboard: `USB +5 V`, `R1's lead in column 5`, `D1 (red LED)`, `the top + rail`,
//! `UNO pin D13`, `the meter's mA jack` — never `PS1.VBUS` / `R1.1`.
//!
//! Everything is derived from the parts library (part kind, pin roles and labels, properties)
//! and from where things sit on the breadboard (`Connectivity::insertions`), so every lesson —
//! including lesson packs — gets the same wording.

use bb_circuit::breadboard::{self, Hole, Rail};
use bb_circuit::connectivity::{Connectivity, TermRef};
use bb_circuit::part::{Layout, PartDef, PinRole};
use bb_circuit::{Library, Project};
use std::collections::HashMap;
use std::sync::Arc;

/// Names things on one circuit.
pub struct Namer<'a> {
    pub project: &'a Project,
    pub lib: &'a Library,
    pub conn: &'a Connectivity,
    /// lead → the hole / header socket it is plugged into
    sockets: HashMap<TermRef, TermRef>,
}

/// `+5V` → `+5 V`, `3V3` stays.
fn spaced_unit(label: &str) -> String {
    let b = label.as_bytes();
    if b.len() >= 2 && b[b.len() - 1] == b'V' && b[b.len() - 2].is_ascii_digit() {
        format!("{} V", &label[..label.len() - 1])
    } else {
        label.to_string()
    }
}

/// `Light sensor (LDR)` → `light sensor`; acronyms stay upper case.
fn kind_words(name: &str) -> String {
    let base = name.split(" (").next().unwrap_or(name).trim();
    let mut c = base.chars();
    match (c.next(), c.next()) {
        (Some(a), Some(b)) if a.is_uppercase() && b.is_lowercase() => {
            format!("{}{}", a.to_lowercase(), &base[a.len_utf8()..])
        }
        _ => base.to_string(),
    }
}

impl<'a> Namer<'a> {
    pub fn new(project: &'a Project, lib: &'a Library, conn: &'a Connectivity) -> Namer<'a> {
        let sockets = conn
            .insertions
            .iter()
            .map(|(lead, hole)| (lead.clone(), hole.clone()))
            .collect();
        Namer {
            project,
            lib,
            conn,
            sockets,
        }
    }

    pub fn def(&self, part: &str) -> Option<Arc<PartDef>> {
        let p = self.project.part(part)?;
        self.lib.get(&p.part).cloned()
    }

    fn def_id(&self, part: &str) -> String {
        self.project
            .part(part)
            .map(|p| p.part.clone())
            .unwrap_or_default()
    }

    /// `R1` (the reference designator, or the id in capitals).
    pub fn reference(&self, part: &str) -> String {
        self.project
            .part(part)
            .map(|p| p.reference.clone())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| part.to_ascii_uppercase())
    }

    /// The part is a breadboard.
    pub fn is_breadboard(&self, part: &str) -> bool {
        self.def(part).is_some_and(|d| d.is_breadboard())
    }

    /// A meter (the Handheld DMM or the built-in one).
    pub fn is_meter(&self, part: &str) -> bool {
        let id = self.def_id(part);
        id.starts_with("dmm")
    }

    fn is_supply(&self, part: &str) -> bool {
        self.def(part).is_some_and(|d| {
            d.bench.off_board && d.pins.iter().any(|p| p.role == PinRole::PowerOut)
        }) && !self.def(part).is_some_and(|d| d.is_mcu())
    }

    fn is_board(&self, part: &str) -> bool {
        self.def(part)
            .is_some_and(|d| d.is_mcu() || matches!(d.bench.layout, Some(Layout::Board(_))))
    }

    /// Short name of a supply / board: `USB`, `battery`, `UNO`, `ESP32`.
    fn short_kind(&self, part: &str) -> String {
        let id = self.def_id(part);
        match id.as_str() {
            "usb_power" => "USB".into(),
            "battery_pack" => "battery".into(),
            "uno" => "UNO".into(),
            "esp32_devkit" => "ESP32".into(),
            "pico" => "Pico".into(),
            _ => self.reference(part),
        }
    }

    /// A string property's value (`red` for an LED's colour).
    fn text_prop(&self, part: &str, key: &str) -> Option<String> {
        let p = self.project.part(part)?;
        let d = self.def(part)?;
        Project::prop(&d, p, key).map(|v| v.as_text())
    }

    fn num_prop(&self, part: &str, key: &str) -> Option<f64> {
        let p = self.project.part(part)?;
        let d = self.def(part)?;
        Project::prop_f64(&d, p, key)
    }

    /// `D1 (red LED)`, `R1 (220 Ω)`, `C1 (100 µF)`, `the USB supply`, `the meter`, `the UNO`.
    pub fn part(&self, part: &str) -> String {
        let r = self.reference(part);
        let Some(d) = self.def(part) else {
            return r;
        };
        match d.id.as_str() {
            "led" => match self.text_prop(part, "color") {
                Some(c) if !c.is_empty() => format!("{r} ({c} LED)"),
                _ => format!("{r} (LED)"),
            },
            "resistor" => match self.num_prop(part, "resistance") {
                Some(v) => format!("{r} ({})", bb_core::si::format(v, "Ω")),
                None => r,
            },
            "capacitor" | "electrolytic" | "cap_electrolytic" => {
                match self.num_prop(part, "capacitance") {
                    Some(v) => format!("{r} ({})", bb_core::si::format(v, "F")),
                    None => r,
                }
            }
            _ if self.is_meter(part) => "the meter".into(),
            _ if self.is_supply(part) => format!("the {} supply", self.short_kind(part)),
            _ if self.is_board(part) => format!("the {}", self.short_kind(part)),
            _ => format!("{r} ({})", kind_words(&d.name)),
        }
    }

    /// `D1`, `R1`, `the meter`, `the UNO` — for sentences that name the part again.
    pub fn part_short(&self, part: &str) -> String {
        if self.is_meter(part) {
            "the meter".into()
        } else if self.is_supply(part) {
            format!("the {} supply", self.short_kind(part))
        } else if self.is_board(part) {
            format!("the {}", self.short_kind(part))
        } else {
            self.reference(part)
        }
    }

    /// The hole / socket a lead is plugged into.
    pub fn socket(&self, part: &str, pin: &str) -> Option<&TermRef> {
        self.sockets.get(&TermRef::new(part, pin))
    }

    /// Strip of a breadboard hole: (breadboard, strip key, hole).
    pub fn strip_of(&self, hole: &TermRef) -> Option<(String, String, Hole)> {
        let d = self.def(&hole.part)?;
        let Some(Layout::Breadboard { rail_holes, .. }) = d.bench.layout else {
            return None;
        };
        let split = self
            .project
            .part(&hole.part)
            .and_then(|p| Project::prop(&d, p, "split_rails"))
            .is_some_and(|v| matches!(v, bb_circuit::PropValue::Bool(true)));
        let h = breadboard::parse_hole(&hole.pin)?;
        Some((
            hole.part.clone(),
            breadboard::strip_key(&h, rail_holes, split),
            h,
        ))
    }

    /// Every hole of the strip `hole` belongs to (in board order).
    pub fn strip_holes(&self, hole: &TermRef) -> Vec<TermRef> {
        let Some((bb, key, _)) = self.strip_of(hole) else {
            return Vec::new();
        };
        let Some(d) = self.def(&bb) else {
            return Vec::new();
        };
        d.pins
            .iter()
            .map(|p| TermRef::new(&bb, &p.id))
            .filter(|t| self.strip_of(t).is_some_and(|(_, k, _)| k == key))
            .collect()
    }

    /// What is plugged into a strip: leads inserted into its holes and wire ends on them.
    pub fn occupants(&self, hole: &TermRef) -> Vec<Occupant> {
        let holes = self.strip_holes(hole);
        let mut out = Vec::new();
        for (lead, h) in &self.conn.insertions {
            if holes.contains(h) {
                out.push(Occupant::Lead {
                    lead: lead.clone(),
                    hole: h.clone(),
                });
            }
        }
        for w in &self.project.wires {
            for (end, e) in [(0u8, &w.a), (1, &w.b)] {
                if let bb_circuit::project::Endpoint::Pin { part, pin } = e {
                    let t = TermRef::new(part, pin);
                    if holes.contains(&t) {
                        out.push(Occupant::WireEnd {
                            wire: w.id.clone(),
                            end,
                            hole: t,
                        });
                    }
                }
            }
        }
        out
    }

    /// Holes of the strip that nothing is plugged into.
    pub fn free_holes(&self, hole: &TermRef) -> Vec<TermRef> {
        let used: Vec<TermRef> = self
            .occupants(hole)
            .into_iter()
            .map(|o| o.hole().clone())
            .collect();
        self.strip_holes(hole)
            .into_iter()
            .filter(|h| !used.contains(h))
            .collect()
    }

    /// `column 5`, `the top + rail`; with `rows` the half too (`column 5 (rows f–j)`).
    pub fn place(&self, hole: &TermRef) -> String {
        match breadboard::parse_hole(&hole.pin) {
            Some(Hole::Strip { column, .. }) => format!("column {column}"),
            Some(Hole::Rail { rail, .. }) => rail_name(rail).into(),
            None => self.pin(&hole.part, &hole.pin),
        }
    }

    /// `a5–e5` for a column strip, `the top + rail` for a rail.
    pub fn strip_range(&self, hole: &TermRef) -> String {
        match breadboard::parse_hole(&hole.pin) {
            Some(Hole::Strip { row, column }) => {
                if breadboard::ROWS.iter().position(|&r| r == row).unwrap_or(0) < 5 {
                    format!("a{column}–e{column}")
                } else {
                    format!("f{column}–j{column}")
                }
            }
            Some(Hole::Rail { rail, .. }) => rail_name(rail).into(),
            None => hole.pin.clone(),
        }
    }

    /// Where a pin is on the bench in a few words: `column 5`, `the top + rail`, `UNO pin D13`.
    pub fn where_is(&self, part: &str, pin: &str) -> String {
        if self.is_breadboard(part) {
            return self.place(&TermRef::new(part, pin));
        }
        match self.socket(part, pin) {
            Some(h) if self.is_breadboard(&h.part) => self.place(h),
            Some(h) => self.pin(&h.part, &h.pin),
            None => self.pin(part, pin),
        }
    }

    /// A pin in words: `USB +5 V`, `USB GND`, `R1's lead in column 5`, `D1's long leg (+)`,
    /// `UNO pin D13`, `the meter's mA jack`, `hole a5`, `C1's + leg`.
    pub fn pin(&self, part: &str, pin: &str) -> String {
        let r = self.reference(part);
        let Some(d) = self.def(part) else {
            return if pin.chars().all(|c| c.is_ascii_digit()) {
                format!("{r}'s lead {pin}")
            } else {
                format!("{r}'s {pin} pin")
            };
        };
        if d.is_breadboard() {
            return match breadboard::parse_hole(pin) {
                Some(Hole::Rail { rail, .. }) => rail_name(rail).into(),
                _ => format!("hole {pin}"),
            };
        }
        let pd = d.pin(pin);
        let label = pd.map(|p| p.label().to_string()).unwrap_or(pin.into());
        let role = pd.map(|p| p.role).unwrap_or_default();
        if self.is_meter(part) {
            return format!("the meter's {label} jack");
        }
        if self.is_supply(part) {
            let label = if role == PinRole::Ground {
                "GND".to_string()
            } else {
                spaced_unit(&label)
            };
            return format!("{} {label}", self.short_kind(part));
        }
        if self.is_board(part) {
            let kind = self.short_kind(part);
            return match role {
                PinRole::PowerOut | PinRole::Ground | PinRole::PowerIn => {
                    format!("{kind} {}", spaced_unit(&label))
                }
                _ => format!("{kind} pin {label}"),
            };
        }
        let at = self.socket(part, pin).and_then(|h| {
            self.is_breadboard(&h.part)
                .then(|| match breadboard::parse_hole(&h.pin) {
                    Some(Hole::Strip { column, .. }) => format!(" in column {column}"),
                    Some(Hole::Rail { rail, .. }) => format!(" on {}", rail_name(rail)),
                    None => String::new(),
                })
        });
        match d.id.as_str() {
            "led" => {
                let leg = if pin == "a" {
                    "long leg (+)"
                } else {
                    "short leg (−)"
                };
                format!("{r}'s {leg}")
            }
            _ if pin == "plus" || pin == "minus" => {
                let leg = if pin == "plus" { "+ leg" } else { "− leg" };
                format!("{r}'s {leg}{}", at.unwrap_or_default())
            }
            _ if d.pins.len() == 2 && pd.is_some_and(|p| p.role == PinRole::Passive) => match at {
                Some(at) => format!("{r}'s lead{at}"),
                None => format!("{r}'s lead {label}"),
            },
            _ => format!("{r}'s {label} pin"),
        }
    }

    /// A pin given as `part.pin`.
    pub fn pin_str(&self, s: &str) -> String {
        let (p, n) = split(s);
        self.pin(p, n)
    }

    /// A wire in words: `the red jumper from column 10 to the top − rail`.
    pub fn wire(&self, id: &str) -> String {
        let Some(w) = self.project.wire(id) else {
            return "a jumper".into();
        };
        let end = |e: &bb_circuit::project::Endpoint| match e {
            bb_circuit::project::Endpoint::Pin { part, pin } => self.where_is(part, pin),
            bb_circuit::project::Endpoint::Free { .. } => "nowhere".into(),
        };
        format!("the {} jumper from {} to {}", w.color, end(&w.a), end(&w.b))
    }

    /// `the red jumper`.
    pub fn wire_short(&self, id: &str) -> String {
        self.project
            .wire(id)
            .map(|w| format!("the {} jumper", w.color))
            .unwrap_or_else(|| "a jumper".into())
    }

    /// Component pins on a net (breadboard holes excluded).
    pub fn net_pins(&self, part: &str, pin: &str) -> Vec<TermRef> {
        self.conn
            .net_of(part, pin)
            .map(|n| self.conn.net(n).pins.clone())
            .unwrap_or_default()
    }
}

/// Something plugged into a breadboard strip.
#[derive(Debug, Clone, PartialEq)]
pub enum Occupant {
    Lead {
        lead: TermRef,
        hole: TermRef,
    },
    WireEnd {
        wire: String,
        end: u8,
        hole: TermRef,
    },
}

impl Occupant {
    pub fn hole(&self) -> &TermRef {
        match self {
            Occupant::Lead { hole, .. } | Occupant::WireEnd { hole, .. } => hole,
        }
    }
}

pub fn rail_name(rail: Rail) -> &'static str {
    match rail {
        Rail::TopPlus => "the top + rail",
        Rail::TopMinus => "the top − rail",
        Rail::BottomPlus => "the bottom + rail",
        Rail::BottomMinus => "the bottom − rail",
    }
}

/// `r1.1` → (`r1`, `1`).
pub fn split(pin: &str) -> (&str, &str) {
    pin.split_once('.').unwrap_or((pin, ""))
}

/// A current for people: `13.6 mA`, `0.30 mA`, `1.20 A`; below 1 µA it reads `0 mA`.
pub fn current(amps: f64) -> String {
    let a = amps.abs();
    if !a.is_finite() || a < 1e-6 {
        "0 mA".into()
    } else if a < 1.0 {
        let ma = a * 1e3;
        if ma >= 10.0 {
            format!("{ma:.1} mA")
        } else {
            format!("{ma:.2} mA")
        }
    } else {
        format!("{a:.2} A")
    }
}

/// A voltage for people: `4.98 V`, `450 mV`; below 1 mV it reads `0 V`.
pub fn voltage(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let a = v.abs();
    if a < 1e-3 {
        "0 V".into()
    } else if a < 1.0 {
        format!("{:.0} mV", v * 1e3)
    } else if a < 10.0 {
        format!("{v:.2} V")
    } else {
        format!("{v:.1} V")
    }
}

/// A frequency for people: `0.5 Hz`, `490 Hz`, `1.6 kHz`.
pub fn hz(f: f64) -> String {
    if !f.is_finite() {
        return "—".into();
    }
    if f.abs() >= 1000.0 {
        return bb_core::si::format_sig(f, "Hz", 3);
    }
    let s = if f.abs() >= 100.0 {
        format!("{f:.0}")
    } else {
        format!("{f:.2}")
    };
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    format!("{s} Hz")
}

/// A value with its unit, with a floor so that numerical dust reads as zero (`0 aA` never
/// shows): currents and voltages via [`current`] / [`voltage`], others SI-formatted.
pub fn value(v: f64, unit: &str) -> String {
    match unit {
        "A" => current(v),
        "V" => voltage(v),
        _ if !v.is_finite() => "—".into(),
        _ if v.abs() < 1e-12 => format!("0 {unit}").trim().to_string(),
        _ => bb_core::si::format(v, unit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_have_a_floor() {
        assert_eq!(current(1e-18), "0 mA");
        assert_eq!(current(0.0136), "13.6 mA");
        assert_eq!(current(0.0003), "0.30 mA");
        assert_eq!(current(1.2), "1.20 A");
        assert_eq!(voltage(4e-9), "0 V");
        assert_eq!(voltage(0.45), "450 mV");
        assert_eq!(voltage(4.98), "4.98 V");
        assert_eq!(value(1e-20, "Ω"), "0 Ω");
        assert_eq!(spaced_unit("+5V"), "+5 V");
        assert_eq!(hz(0.5), "0.5 Hz");
        assert_eq!(hz(490.2), "490 Hz");
        assert_eq!(hz(5.0), "5 Hz");
        assert_eq!(hz(1591.5), "1.59 kHz");
        assert_eq!(spaced_unit("3V3"), "3V3");
        assert_eq!(kind_words("Light sensor (LDR)"), "light sensor");
        assert_eq!(
            kind_words("TMP36 temperature sensor"),
            "TMP36 temperature sensor"
        );
    }
}
