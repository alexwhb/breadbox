//! Electrical and design rule checks (WB-12): `ERC 1 warning · DRC clean`.
//!
//! ERC (on the logical circuit): unconnected required pins, floating inputs, output
//! conflicts, LEDs without current limiting, missing ground, supply shorts, over-voltage on
//! low-voltage pins (e.g. 5 V into an ESP-01). DRC (on the bench): leads not plugged in,
//! two leads in one hole, parts whose own leads are shorted by a breadboard strip.

use crate::connectivity::{Connectivity, Net, TermRef};
use crate::library::Library;
use crate::part::{PartDef, PinRole};
use crate::project::{Endpoint, Project};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ErcSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErcItem {
    pub severity: ErcSeverity,
    /// Stable rule id (`led_no_resistor`, `supply_short`, …).
    pub code: String,
    pub message: String,
    /// Part ids involved (for click-to-select).
    pub parts: Vec<String>,
    pub net: Option<String>,
}

fn item(sev: ErcSeverity, code: &str, message: String, parts: Vec<String>, net: Option<&Net>) -> ErcItem {
    ErcItem {
        severity: sev,
        code: code.into(),
        message,
        parts,
        net: net.map(|n| if n.name == "0" { "GND".into() } else { n.name.clone() }),
    }
}

struct Ctx<'a> {
    project: &'a Project,
    defs: HashMap<&'a str, Arc<PartDef>>,
}

impl<'a> Ctx<'a> {
    fn new(project: &'a Project, lib: &Library) -> Ctx<'a> {
        let defs = project
            .parts
            .iter()
            .filter_map(|p| lib.get(&p.part).map(|d| (p.id.as_str(), d.clone())))
            .collect();
        Ctx { project, defs }
    }
    fn def(&self, part: &str) -> Option<&Arc<PartDef>> {
        self.defs.get(part)
    }
    fn reference(&self, part: &str) -> String {
        self.project
            .part(part)
            .map(|p| p.reference.clone())
            .unwrap_or_else(|| part.into())
    }
    fn pin_label(&self, t: &TermRef) -> String {
        let pl = self
            .def(&t.part)
            .and_then(|d| d.pin(&t.pin).map(|p| p.label().to_string()))
            .unwrap_or_else(|| t.pin.clone());
        format!("{}.{}", self.reference(&t.part), pl)
    }
    fn role(&self, t: &TermRef) -> Option<PinRole> {
        self.def(&t.part)?.pin(&t.pin).map(|p| p.role)
    }
    fn is_mcu_pin(&self, t: &TermRef) -> bool {
        self.def(&t.part)
            .and_then(|d| d.pin(&t.pin))
            .is_some_and(|p| p.mcu.is_some())
    }
    /// Nominal voltage a supply / MCU pin drives.
    fn drive_voltage(&self, t: &TermRef) -> Option<f64> {
        let d = self.def(&t.part)?;
        let p = self.project.part(&t.part)?;
        let pin = d.pin(&t.pin)?;
        if pin.mcu.is_some() {
            return Some(match d.model.mcu.as_ref().map(|m| m.core.as_str()) {
                Some("avr") => 5.0,
                _ => 3.3,
            });
        }
        if pin.role != PinRole::PowerOut {
            return None;
        }
        for key in ["voltage", "emf"] {
            if let Some(v) = Project::prop_f64(d, p, key) {
                return Some(v);
            }
        }
        match d.id.as_str() {
            "reg_7805" => return Some(5.0),
            "ams1117_3v3" => return Some(3.3),
            _ => {}
        }
        let l = pin.label().to_ascii_uppercase().replace("3V3", "3.3V");
        l.trim_end_matches('V')
            .trim_start_matches('+')
            .parse()
            .ok()
            .or(Some(5.0))
    }
    fn is_board(&self, part: &str) -> bool {
        self.def(part).is_some_and(|d| d.is_mcu())
    }
    fn is_led(&self, part: &str) -> bool {
        self.def(part).is_some_and(|d| {
            d.model.spice.iter().any(|l| l.to_ascii_uppercase().contains(" LED_"))
        })
    }
    /// Parts that limit current in series (anything with resistance or a transistor).
    fn limits_current(&self, part: &str) -> bool {
        self.def(part).is_some_and(|d| {
            !d.is_mcu()
                && !self.is_led(part)
                && !d.pins.iter().any(|p| p.role == PinRole::PowerOut)
                && (d.model.spice.iter().any(|l| {
                    let first = l.trim_start_matches(|c| c != '$').to_string();
                    first.starts_with("$(el:R") || first.starts_with("$(el:Q") || first.starts_with("$(el:M") || first.starts_with("$(el:X")
                }) || d.model.behaviour.is_some())
        })
    }
}

/// Runs the electrical rule check.
pub fn erc(project: &Project, lib: &Library, conn: &Connectivity) -> Vec<ErcItem> {
    use ErcSeverity::*;
    let cx = Ctx::new(project, lib);
    let mut out = Vec::new();
    let real_pins = |net: &Net| -> Vec<TermRef> {
        net.pins
            .iter()
            .filter(|t| cx.def(&t.part).is_some_and(|d| !d.has_tag("symbol") && !d.has_tag("instrument")))
            .cloned()
            .collect()
    };

    let any_parts = project.parts.iter().any(|p| {
        cx.def(&p.id)
            .is_some_and(|d| !d.pins.is_empty() && !d.is_breadboard() && !d.has_tag("symbol"))
    });

    // missing ground
    if any_parts && conn.ground().is_none() {
        let powered = project.parts.iter().any(|p| {
            cx.def(&p.id)
                .is_some_and(|d| d.pins.iter().any(|x| x.role == PinRole::PowerOut))
        });
        if powered {
            out.push(item(
                Error,
                "missing_ground",
                "No ground: connect a supply's − / GND to the circuit".into(),
                vec![],
                None,
            ));
        }
    }

    for net in &conn.nets {
        let pins = real_pins(net);
        // unconnected required / supply pins
        if pins.len() == 1 {
            let t = &pins[0];
            if let (Some(d), Some(role)) = (cx.def(&t.part), cx.role(t)) {
                let required = d.pin(&t.pin).is_some_and(|p| p.required);
                let supply = matches!(role, PinRole::PowerIn | PinRole::Ground)
                    && !d.is_mcu()
                    && d.model.behaviour.is_some();
                if (required || supply) && !cx.is_board(&t.part) {
                    out.push(item(
                        Warning,
                        "unconnected_pin",
                        format!("{} is not connected", cx.pin_label(t)),
                        vec![t.part.clone()],
                        Some(net),
                    ));
                }
            }
        }
        if pins.len() < 2 {
            continue;
        }
        // floating inputs: only inputs on the net
        let inputs: Vec<&TermRef> = pins
            .iter()
            .filter(|t| cx.role(t) == Some(PinRole::Input) && !cx.is_mcu_pin(t))
            .collect();
        let drivers = pins.iter().any(|t| {
            cx.is_mcu_pin(t)
                || !matches!(cx.role(t), Some(PinRole::Input) | Some(PinRole::Nc))
        });
        if !inputs.is_empty() && !drivers && net.name != "0" {
            out.push(item(
                Warning,
                "floating_input",
                format!(
                    "{} floats: nothing drives it (add a pull-up/down or connect an output)",
                    inputs.iter().map(|t| cx.pin_label(t)).collect::<Vec<_>>().join(", ")
                ),
                inputs.iter().map(|t| t.part.clone()).collect(),
                Some(net),
            ));
        }
        // output conflicts (push-pull outputs of different parts)
        let outputs: Vec<&TermRef> = pins
            .iter()
            .filter(|t| cx.role(t) == Some(PinRole::Output))
            .collect();
        let distinct: BTreeSet<&str> = outputs.iter().map(|t| t.part.as_str()).collect();
        if distinct.len() > 1 {
            out.push(item(
                Error,
                "output_conflict",
                format!(
                    "Outputs fight each other: {}",
                    outputs.iter().map(|t| cx.pin_label(t)).collect::<Vec<_>>().join(" ↔ ")
                ),
                distinct.iter().map(|s| s.to_string()).collect(),
                Some(net),
            ));
        }
        // supply shorts / conflicts
        let supplies: Vec<(&TermRef, f64)> = pins
            .iter()
            .filter(|t| cx.role(t) == Some(PinRole::PowerOut))
            .filter_map(|t| cx.drive_voltage(t).map(|v| (t, v)))
            .collect();
        if net.name == "0" || net.is_ground {
            for (t, _) in &supplies {
                out.push(item(
                    Error,
                    "supply_short",
                    format!("{} is shorted to ground", cx.pin_label(t)),
                    vec![t.part.clone()],
                    Some(net),
                ));
            }
        } else {
            let parts: BTreeSet<&str> = supplies.iter().map(|(t, _)| t.part.as_str()).collect();
            let vmin = supplies.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
            let vmax = supplies.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max);
            if parts.len() > 1 && vmax - vmin > 0.25 {
                out.push(item(
                    Error,
                    "supply_short",
                    format!(
                        "Different supplies shorted together: {}",
                        supplies
                            .iter()
                            .map(|(t, v)| format!("{} ({v} V)", cx.pin_label(t)))
                            .collect::<Vec<_>>()
                            .join(" ↔ ")
                    ),
                    parts.iter().map(|s| s.to_string()).collect(),
                    Some(net),
                ));
            }
        }
        // over-voltage into low-voltage pins
        let vdrive = pins
            .iter()
            .filter_map(|t| cx.drive_voltage(t).map(|v| (t, v)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((src, v)) = vdrive {
            for t in &pins {
                let maxv = cx.def(&t.part).and_then(|d| d.pin(&t.pin)).and_then(|p| p.max_voltage);
                if let Some(m) = maxv
                    && v > m + 0.05
                    && t.part != src.part
                {
                    let dmax = if m < 3.7 { "3.3 V" } else { "low-voltage" };
                    out.push(item(
                        Warning,
                        "overvoltage",
                        format!(
                            "{} is {dmax} tolerant, driven by {} V {} — add a divider",
                            cx.pin_label(t),
                            v,
                            cx.pin_label(src).split('.').nth(1).unwrap_or("")
                        ),
                        vec![t.part.clone(), src.part.clone()],
                        Some(net),
                    ));
                }
            }
        }
    }

    // LEDs without current limiting
    for p in &project.parts {
        if !cx.is_led(&p.id) {
            continue;
        }
        let Some(d) = cx.def(&p.id) else { continue };
        let mut sides: Vec<(bool, bool)> = Vec::new(); // (driven by supply/mcu/ground, has limiter)
        for pin in &d.pins {
            let Some(nid) = conn.net_of(&p.id, &pin.id) else {
                continue;
            };
            let net = conn.net(nid);
            let others: Vec<&TermRef> = net.pins.iter().filter(|t| t.part != p.id).collect();
            let hard = net.name == "0"
                || others.iter().any(|t| {
                    cx.is_mcu_pin(t) || matches!(cx.role(t), Some(PinRole::PowerOut) | Some(PinRole::Ground))
                });
            let limiter = others.iter().any(|t| cx.limits_current(&t.part));
            sides.push((hard, limiter));
        }
        let all_hard = sides.len() >= 2 && sides.iter().all(|s| s.0);
        let any_limit = sides.iter().any(|s| s.1);
        if all_hard && !any_limit {
            out.push(item(
                Error,
                "led_no_resistor",
                format!("{} has no current-limiting resistor — it will burn out", p.reference),
                vec![p.id.clone()],
                None,
            ));
        }
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.code.cmp(&b.code)));
    out.dedup();
    out
}

/// Runs the bench design rule check.
pub fn drc(project: &Project, lib: &Library, conn: &Connectivity) -> Vec<ErcItem> {
    use ErcSeverity::*;
    let cx = Ctx::new(project, lib);
    let mut out = Vec::new();
    let mut inserted: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut hole_use: BTreeMap<&TermRef, Vec<&TermRef>> = BTreeMap::new();
    for (lead, socket) in &conn.insertions {
        inserted.entry(&lead.part).or_default().insert(&lead.pin);
        hole_use.entry(socket).or_default().push(lead);
    }
    let wired: BTreeSet<(String, String)> = project
        .wires
        .iter()
        .flat_map(|w| [&w.a, &w.b])
        .filter_map(|e| match e {
            Endpoint::Pin { part, pin } => Some((part.clone(), pin.clone())),
            Endpoint::Free { .. } => None,
        })
        .collect();
    for p in &project.parts {
        let Some(d) = cx.def(&p.id) else { continue };
        if p.bench.is_none() || d.bench.sockets || d.bench.off_board || d.bench.hidden || d.pins.is_empty() {
            continue;
        }
        let ins = inserted.get(p.id.as_str());
        let loose: Vec<&str> = d
            .pins
            .iter()
            .filter(|x| x.role != PinRole::Nc)
            .filter(|x| !ins.is_some_and(|s| s.contains(x.id.as_str())))
            .filter(|x| !wired.contains(&(p.id.clone(), x.id.clone())))
            .map(|x| x.id.as_str())
            .collect();
        let n = d.pins.iter().filter(|x| x.role != PinRole::Nc).count();
        if loose.len() == n {
            out.push(item(Warning, "not_plugged", format!("{} is not plugged into anything", p.reference), vec![p.id.clone()], None));
        } else if !loose.is_empty() {
            out.push(item(
                Warning,
                "loose_lead",
                format!("{}: {} of {} leads not in a hole", p.reference, loose.len(), n),
                vec![p.id.clone()],
                None,
            ));
        }
        // own leads shorted by a strip (not bonded inside the part)
        let pins: Vec<&str> = d.pins.iter().map(|x| x.id.as_str()).collect();
        for i in 0..pins.len() {
            for j in i + 1..pins.len() {
                let bonded = d.bonds.iter().any(|b| b.iter().any(|x| x == pins[i]) && b.iter().any(|x| x == pins[j]));
                if !bonded
                    && ins.is_some_and(|s| s.contains(pins[i]) && s.contains(pins[j]))
                    && conn.connected_on_bench((&p.id, pins[i]), (&p.id, pins[j]))
                {
                    out.push(item(
                        Warning,
                        "shorted_leads",
                        format!(
                            "{}: leads {} and {} are in the same breadboard strip (shorted)",
                            p.reference,
                            d.pin(pins[i]).map(|x| x.label()).unwrap_or(""),
                            d.pin(pins[j]).map(|x| x.label()).unwrap_or("")
                        ),
                        vec![p.id.clone()],
                        None,
                    ));
                }
            }
        }
    }
    for (hole, leads) in hole_use {
        let parts: BTreeSet<&str> = leads.iter().map(|l| l.part.as_str()).collect();
        if leads.len() > 1 {
            out.push(item(
                Error,
                "hole_conflict",
                format!("Two leads in hole {} ({})", hole.pin, parts.iter().map(|p| cx.reference(p)).collect::<Vec<_>>().join(", ")),
                parts.iter().map(|s| s.to_string()).collect(),
                None,
            ));
        }
    }
    out
}

/// Status-strip text: `ERC 1 warning`, `ERC clean`, `DRC 2 errors · 1 warning`.
pub fn summary(prefix: &str, items: &[ErcItem]) -> String {
    let e = items.iter().filter(|i| i.severity == ErcSeverity::Error).count();
    let w = items.len() - e;
    let plural = |n: usize, s: &str| format!("{n} {s}{}", if n == 1 { "" } else { "s" });
    match (e, w) {
        (0, 0) => format!("{prefix} clean"),
        (0, w) => format!("{prefix} {}", plural(w, "warning")),
        (e, 0) => format!("{prefix} {}", plural(e, "error")),
        (e, w) => format!("{prefix} {} · {}", plural(e, "error"), plural(w, "warning")),
    }
}

#[cfg(test)]
mod tests;
