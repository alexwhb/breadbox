//! Pro → Analysis "synced from schematic" (`PRO-2`): the netlist comes from the shared circuit
//! through the bb-circuit netlister, with two Pro touches:
//!
//! * world values the parts read (`lux`, `vwc`, …) become `.param`s, so `.dc lux …` /
//!   `.step` sweeps act on the LDR / soil sensor instead of a frozen number;
//! * the MCU subcircuit replays the outputs measured on the live bench (a duty probe → a PWM
//!   `PULSE` at the measured frequency and duty, a logic probe → a DC level), so the transient
//!   shows what the firmware is doing (`PRO-5`).
//!
//! Edits that only change a resistor / capacitor / inductor value round-trip into the circuit;
//! anything else detaches the netlist (with a Re-sync button).

use crate::circuit::CircuitDoc;
use anyhow::Result;
use bb_circuit::project::{Endpoint, Quantity};
use bb_circuit::{Project, PropValue};
use bb_sim::Telemetry;
use std::collections::HashMap;

/// World keys the netlister may read, with unique sentinel values.
const SENTINELS: [(&str, f64); 4] = [
    ("lux", 271_828.25),
    ("vwc", 314_159.25),
    ("temp", 161_803.25),
    ("rh", 141_421.25),
];

/// The synced netlist text.
pub fn synced_netlist(doc: &CircuitDoc, tel: Option<&Telemetry>) -> Result<String> {
    let project = doc.project();
    let mut probe = project.clone();
    let w = &mut probe.world;
    w.light.ambient_lux = SENTINELS[0].1;
    for p in w.pots.iter_mut() {
        p.vwc = SENTINELS[1].1;
    }
    w.air.temp_c = SENTINELS[2].1;
    w.air.rh = SENTINELS[3].1;
    let text = bb_circuit::spice_io::pro_display_text(&probe, &doc.lib)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let actual = [
        project.world.light.ambient_lux,
        project.world.pots.first().map(|p| p.vwc).unwrap_or(40.0),
        project.world.air.temp_c,
        project.world.air.rh,
    ];
    let mut text = text;
    let mut params = Vec::new();
    for ((key, s), v) in SENTINELS.iter().zip(actual) {
        let lit = bb_spice::expr::fmt_g(*s);
        if !text.contains(&lit) {
            continue;
        }
        // `vwc=314159.25` → `vwc={vwc}`; inside an expression → the bare name
        text = text.replace(&format!("={lit}"), &format!("={{{key}}}"));
        text = text.replace(&lit, key);
        params.push(format!("{key}={}", bb_spice::expr::fmt_g(v)));
    }
    if !params.is_empty() {
        let card = format!(".param {}\n", params.join(" "));
        text = match text.split_once('\n') {
            Some((title, rest)) => format!("{title}\n{card}{rest}"),
            None => format!("{text}\n{card}"),
        };
    }
    // Breadbox-only cards (`*!bb .step …`) are plain cards in the editor; bb-spice runs them
    let text = text.replace("*!bb ", "");
    // disabled cards (`*off .mc …`) are comments the netlister drops: keep them so the sidebar
    // can turn them back on; a circuit without any analysis gets an operating point
    let mut extra: Vec<String> = project
        .spice
        .directives
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('*') && is_card_line(l))
        .map(str::to_string)
        .collect();
    if project.spice.directives.trim().is_empty() && !text.lines().any(is_card_line) {
        extra.push(".op".into());
    }
    let text = if extra.is_empty() {
        text
    } else {
        let cards = extra.join("\n") + "\n";
        match text.rfind("\n.end") {
            Some(i) => format!("{}\n{cards}{}", &text[..i], &text[i + 1..]),
            None => format!("{text}{cards}"),
        }
    };
    Ok(replay_mcu(&text, project, tel))
}

/// Analysis / control cards the Analyses sidebar edits (also when disabled: `*off .mc …`).
/// They round-trip into `project.spice.directives`; everything else in the synced text comes
/// from the circuit.
pub fn is_card_line(line: &str) -> bool {
    let t = line.trim();
    let t = t.strip_prefix("*off").map(str::trim_start).unwrap_or(t);
    let word = t
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        word.as_str(),
        ".tran"
            | ".ac"
            | ".dc"
            | ".op"
            | ".noise"
            | ".tf"
            | ".step"
            | ".mc"
            | ".temp"
            | ".meas"
            | ".measure"
            | ".options"
            | ".option"
            | ".four"
    )
}

/// Plot choices for a synced circuit: the PWM-driven node and its LED current, the probed
/// voltage a light sweep moves (`.dc lux`), probes first in the `.op` table.
pub fn profile_for(doc: &CircuitDoc) -> super::model::Profile {
    let p = doc.project();
    let conn = bb_circuit::Connectivity::compute(p, &doc.lib);
    let node = |e: &Endpoint| match e {
        Endpoint::Pin { part, pin } => conn
            .net_of(part, pin)
            .map(|n| conn.net(n).name.to_ascii_lowercase()),
        _ => None,
    };
    let mut prof = super::model::Profile::default();
    // tran: the duty-probed node + the LED it drives
    if let Some(pr) = p.probes.iter().find(|x| x.quantity == Quantity::Duty)
        && let Some(n) = node(&pr.target)
    {
        prof.tran_traces.push(format!("v({n})"));
        let led = p.parts.iter().find(|x| {
            doc.lib
                .get(&x.part)
                .is_some_and(|d| d.bench.art == "led_5mm")
        });
        if let Some(l) = led {
            prof.tran_traces
                .push(format!("i({})", l.reference.to_ascii_lowercase()));
        }
        prof.tran_title = Some(pr.label.to_lowercase());
    }
    // dc / mc: the voltage probe on an LDR's net (light sweep), else the first one
    let ldr_nets: Vec<String> = p
        .parts
        .iter()
        .filter(|x| x.part == "ldr")
        .flat_map(|x| {
            ["1", "2"].into_iter().filter_map(|pin| {
                conn.net_of(&x.id, pin)
                    .map(|n| conn.net(n).name.to_ascii_lowercase())
            })
        })
        .collect();
    let vprobes: Vec<String> = p
        .probes
        .iter()
        .filter(|x| x.quantity == Quantity::Voltage)
        .filter_map(|x| node(&x.target))
        .collect();
    let pick = vprobes
        .iter()
        .find(|n| ldr_nets.contains(n))
        .or(vprobes.first())
        .cloned();
    if let Some(n) = pick {
        prof.dc_vector = Some(format!("v({n})"));
        prof.dc_title = Some(format!("V({}) vs light", n.to_ascii_uppercase()));
        prof.mc_vector = Some(format!("v({n})"));
    }
    prof.op_rows = std::iter::once("V(VCC)".to_string())
        .chain(
            vprobes
                .iter()
                .map(|n| format!("V({})", n.to_ascii_uppercase())),
        )
        .collect();
    prof
}

/// Replaces the netlister's high-Z MCU stand-in with outputs replayed from the bench.
fn replay_mcu(text: &str, project: &Project, tel: Option<&Telemetry>) -> String {
    // nodes probed with duty / logic → (kind, value, frequency)
    let mut drive: HashMap<String, (Quantity, f64, f64)> = HashMap::new();
    for pr in &project.probes {
        let Endpoint::Pin { pin, .. } = &pr.target else {
            continue;
        };
        let node = pin.to_ascii_lowercase();
        // before the bench has run there is nothing measured yet: assume a half duty
        let reading = tel
            .filter(|t| t.time > bb_core::SimTime::ZERO)
            .and_then(|t| t.probes.iter().find(|r| r.id == pr.id));
        match pr.quantity {
            Quantity::Duty => {
                let duty = reading.map(|r| r.value / 100.0).unwrap_or(0.5);
                drive.insert(node, (Quantity::Duty, duty, 490.0));
            }
            Quantity::Logic => {
                let high = reading.map(|r| r.value > 0.5).unwrap_or(false);
                drive.insert(node, (Quantity::Logic, if high { 5.0 } else { 0.0 }, 0.0));
            }
            _ => {}
        }
    }
    if drive.is_empty() {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    // the MCU instance: `xu1 vcc 0 a0 a1 d3 d2 d9 d8 atmega328p params: …`
    let Some(inst) = lines.iter().find(|l| {
        let low = l.to_ascii_lowercase();
        low.starts_with('x') && low.contains(" atmega")
    }) else {
        return text.to_string();
    };
    let toks: Vec<&str> = inst.split_whitespace().collect();
    let Some(si) = toks
        .iter()
        .position(|t| t.to_ascii_lowercase().starts_with("atmega"))
    else {
        return text.to_string();
    };
    let sub = toks[si].to_ascii_lowercase();
    let nodes: Vec<String> = toks[1..si].iter().map(|s| s.to_ascii_lowercase()).collect();
    // the subckt header gives the port names in the same order
    let Some(start) = lines.iter().position(|l| {
        l.to_ascii_lowercase()
            .starts_with(&format!(".subckt {sub} "))
    }) else {
        return text.to_string();
    };
    let Some(end_rel) = lines[start..]
        .iter()
        .position(|l| l.to_ascii_lowercase().starts_with(".ends"))
    else {
        return text.to_string();
    };
    let header = lines[start];
    let ports: Vec<&str> = lines[start]
        .split_whitespace()
        .skip(2)
        .take_while(|t| !t.contains(':') && !t.contains('='))
        .collect();
    let header = if header.contains("params:") || !inst.to_ascii_lowercase().contains("sync=") {
        header.to_string()
    } else {
        format!("{header} params: sync=1u")
    };
    let mut body = vec![
        header.clone(),
        "* outputs replayed from the bench (synced)".to_string(),
    ];
    let gnd = ports.get(1).copied().unwrap_or("0");
    if let Some(vcc) = ports.first() {
        body.push(format!("  rvcc {vcc} {gnd} 1k"));
    }
    for (i, port) in ports.iter().enumerate().skip(2) {
        let node = nodes.get(i).cloned().unwrap_or_default();
        match drive.get(&node) {
            Some((Quantity::Duty, duty, f)) => {
                let per = 1.0 / f.max(1.0);
                let on = (duty.clamp(0.0, 1.0) * per).max(2e-6);
                body.push(format!(
                    "  v{port} {port}i {gnd} PULSE(0 5 0 1u 1u {} {})",
                    bb_spice::expr::fmt_g(on),
                    bb_spice::expr::fmt_g(per)
                ));
                body.push(format!("  r{port} {port}i {port} 25"));
            }
            Some((_, v, _)) => {
                body.push(format!(
                    "  v{port} {port}i {gnd} DC {}",
                    bb_spice::expr::fmt_g(*v)
                ));
                body.push(format!("  r{port} {port}i {port} 25"));
            }
            None => body.push(format!("  r{port} {port} {gnd} 1e9")),
        }
    }
    body.push(lines[start + end_rel].to_string());
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.extend(body);
    out.extend(lines[start + end_rel + 1..].iter().map(|s| s.to_string()));
    out.join("\n") + "\n"
}

/// What an edit of the synced text means for the circuit.
#[derive(Debug, Clone, PartialEq)]
pub enum EditMapping {
    /// Value changes that map onto part properties: (part id, prop key, value).
    Props(Vec<(String, String, f64)>),
    /// The analysis cards changed (configured, enabled, disabled): they become the project's
    /// Pro directives; value changes may come along.
    Cards {
        props: Vec<(String, String, f64)>,
        cards: String,
    },
    /// Nothing changed.
    None,
    /// Something the circuit cannot represent: detach.
    Detach,
}

/// Compares the synced text with an edited version.
pub fn map_edit(project: &Project, synced: &str, edited: &str) -> EditMapping {
    let norm = |s: &str| -> Vec<String> {
        s.lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect()
    };
    let (cards_a, a): (Vec<String>, Vec<String>) =
        norm(synced).into_iter().partition(|l| is_card_line(l));
    let (cards_b, b): (Vec<String>, Vec<String>) =
        norm(edited).into_iter().partition(|l| is_card_line(l));
    let props = match map_values(project, &a, &b) {
        Some(p) => p,
        None => return EditMapping::Detach,
    };
    if cards_a != cards_b {
        // the cards must parse to be stored (a half-typed `.tran` stays in the editor)
        let text = cards_b.join("\n") + "\n";
        let opts = bb_spice::ParseOptions {
            first_line_is_title: false,
            ..Default::default()
        };
        if bb_spice::Netlist::parse_with(&text.replace("*off ", "*"), &opts).is_err() {
            return EditMapping::Detach;
        }
        return EditMapping::Cards { props, cards: text };
    }
    if props.is_empty() {
        EditMapping::None
    } else {
        EditMapping::Props(props)
    }
}

/// Element value edits between two non-card line lists (`None` = not representable).
fn map_values(project: &Project, a: &[String], b: &[String]) -> Option<Vec<(String, String, f64)>> {
    if a.len() != b.len() {
        return None;
    }
    let mut props = Vec::new();
    for (x, y) in a.iter().zip(b) {
        if x == y {
            continue;
        }
        let tx: Vec<&str> = x.split_whitespace().collect();
        let ty: Vec<&str> = y.split_whitespace().collect();
        // same element, same nodes, a different plain value
        if tx.len() < 4 || tx.len() != ty.len() || tx[..3] != ty[..3] || tx[4..] != ty[4..] {
            return None;
        }
        let el = tx[0].to_ascii_lowercase();
        let (kind, key) = match el.chars().next() {
            Some('r') => ('r', "resistance"),
            Some('c') => ('c', "capacitance"),
            Some('l') => ('l', "inductance"),
            _ => return None,
        };
        let v = bb_core::si::parse(ty[3]).ok()?;
        // the part whose element this is: designator `R1` → element `r1`
        let part = project.parts.iter().find(|p| {
            let r = p.reference.to_ascii_lowercase();
            el == r || el == format!("{kind}{r}")
        })?;
        props.push((part.id.clone(), key.to_string(), v));
    }
    Some(props)
}

/// Applies mapped value edits to the document.
pub fn apply(doc: &mut CircuitDoc, props: &[(String, String, f64)]) -> Result<()> {
    let lib = doc.lib.clone();
    for (id, key, v) in props {
        doc.editor
            .set_prop(&lib, id, key, PropValue::Num(*v))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    doc.touched();
    Ok(())
}

/// Stores edited analysis cards as the project's Pro directives (undoable).
pub fn apply_cards(doc: &mut CircuitDoc, cards: &str) {
    doc.editor.set_directives(cards);
    doc.touched();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> CircuitDoc {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/garden-node");
        CircuitDoc::load(&dir, true).unwrap()
    }

    #[test]
    fn synced_netlist_exposes_world_params_and_replays_pwm() {
        let d = doc();
        let t = synced_netlist(&d, None).unwrap();
        assert!(t.contains(".param lux=180"), "{t}");
        assert!(t.contains("pwr(max(lux,"), "{t}");
        assert!(t.contains("PULSE(0 5"), "D9 duty probe replayed: {t}");
        // it runs: .op + the .dc over lux
        let (prepared, lib) = super::super::model::prepare(&t);
        let mut nl = bb_spice::Netlist::parse(&prepared).expect("parses");
        if let Some(l) = lib {
            nl.add_library(l);
        }
        let r = nl.run().expect("runs");
        assert!(!r.runs.is_empty());
    }

    #[test]
    fn value_edits_round_trip_and_others_detach() {
        let d = doc();
        let t = synced_netlist(&d, None).unwrap();
        let edited = t.replace("r1 a1 0 10000", "r1 a1 0 22k");
        assert_eq!(
            map_edit(d.project(), &t, &edited),
            EditMapping::Props(vec![("r1".into(), "resistance".into(), 22_000.0)])
        );
        let detached = t.replace("r1 a1 0 10000", "r1 a1 a0 10000");
        assert_eq!(map_edit(d.project(), &t, &detached), EditMapping::Detach);
        assert_eq!(map_edit(d.project(), &t, &t), EditMapping::None);
    }

    #[test]
    fn card_edits_round_trip_into_the_directives() {
        let mut d = doc();
        let t = synced_netlist(&d, None).unwrap();
        assert!(t.contains(".mc 100"), "{t}");
        // the sidebar disables Monte Carlo and lengthens the transient
        let edited = t
            .replace(".mc 100", "*off .mc 100")
            .replace(".tran 0 0.01 0 1e-6", ".tran 0 0.02 0 1e-6");
        let EditMapping::Cards { props, cards } = map_edit(d.project(), &t, &edited) else {
            panic!("card edits must not detach");
        };
        assert!(props.is_empty());
        assert!(
            cards.contains("*off .mc 100") && cards.contains(".tran 0 0.02"),
            "{cards}"
        );
        apply_cards(&mut d, &cards);
        let again = synced_netlist(&d, None).unwrap();
        assert!(
            again.contains("*off .mc 100"),
            "the disabled card stays: {again}"
        );
        assert!(!again.lines().any(|l| l.starts_with(".mc")), "{again}");
        assert!(again.contains(".tran 0 0.02"), "{again}");
        assert!(d.undo());
        assert!(synced_netlist(&d, None).unwrap().contains("\n.mc 100"));
        // a half-typed card does not get stored
        let typing = t.replace(".tran 0 0.01 0 1e-6", ".tran 0 0.0");
        assert!(matches!(
            map_edit(d.project(), &t, &typing),
            EditMapping::Cards { .. } | EditMapping::Detach
        ));
    }

    #[test]
    fn circuits_without_analyses_get_an_operating_point() {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/night-light");
        let d = CircuitDoc::load(&dir, true).unwrap();
        assert!(d.project().spice.directives.is_empty());
        let t = synced_netlist(&d, None).unwrap();
        assert!(t.lines().any(|l| l.trim() == ".op"), "{t}");
        assert!(t.trim_end().ends_with(".end"), "{t}");
    }
}
