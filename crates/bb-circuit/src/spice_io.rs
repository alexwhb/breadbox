//! `.cir` export (SPICE-8, PRJ-5) and import into a Pro project with an auto-placed schematic
//! (PRJ-4).

use crate::geometry::Placement;
use crate::library::Library;
use crate::netlist::{self, NetlistError, NetlistOptions};
use crate::part::PropValue;
use crate::project::{Mode, NetLabel, PartInstance, Project};
use bb_spice::Netlist;
use bb_spice::netlist::{ElemKind, ParseOptions};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SpiceIoError {
    #[error(transparent)]
    Netlist(#[from] NetlistError),
    #[error("{0}")]
    Parse(String),
}

/// The Pro netlist of a project (behavioural parts as subcircuits, MCUs as `X…ATMEGA328P`,
/// user directives merged), ready for analyses.
pub fn pro_netlist(project: &Project, lib: &Library) -> Result<Netlist, SpiceIoError> {
    let built = netlist::build(project, lib, &NetlistOptions::pro(Some(&project.world)))?;
    let mut nl = built.netlist;
    merge_directives(&mut nl, &project.spice.directives)?;
    Ok(nl)
}

/// Merges `.tran`/`.op`/`.step`/`.mc`/`.meas`/`.options`/`.temp`/`.ic`/`.param`/`.model`
/// cards into a netlist.
pub fn merge_directives(nl: &mut Netlist, directives: &str) -> Result<(), SpiceIoError> {
    if directives.trim().is_empty() {
        return Ok(());
    }
    let opts = ParseOptions {
        first_line_is_title: false,
        ..ParseOptions::default()
    };
    let d =
        Netlist::parse_with(directives, &opts).map_err(|e| SpiceIoError::Parse(e.to_string()))?;
    nl.analyses.extend(d.analyses);
    nl.steps.extend(d.steps);
    if d.mc.is_some() {
        nl.mc = d.mc;
    }
    nl.measures.extend(d.measures);
    nl.options.extend(d.options);
    if !d.temps.is_empty() {
        nl.temps = d.temps;
    }
    nl.ics.extend(d.ics);
    nl.nodesets.extend(d.nodesets);
    nl.top.params.extend(d.top.params);
    nl.top.funcs.extend(d.top.funcs);
    for m in d.top.models {
        nl.top.models.retain(|x| x.name != m.name);
        nl.top.models.push(m);
    }
    for s in d.top.subckts {
        nl.top.subckts.retain(|x| x.name != s.name);
        nl.top.subckts.push(s);
    }
    nl.top.elements.extend(d.top.elements);
    Ok(())
}

/// ngspice-compatible `.cir` text for a project (PRJ-5 "Share → .cir"). MCUs appear as stub
/// subcircuits (pins high-Z) with their firmware noted in a comment.
pub fn export_cir(project: &Project, lib: &Library) -> Result<String, SpiceIoError> {
    let built = netlist::build(project, lib, &NetlistOptions::pro(Some(&project.world)))?;
    let mut nl = built.netlist;
    merge_directives(&mut nl, &project.spice.directives)?;
    // ext() variables frozen at the parts' values (a battery's EMF, a sensor's output), not 0
    let text = nl.to_spice_text_with_ext(&built.ext);
    let notes: String = built
        .warnings
        .iter()
        .filter(|w| w.contains("fw="))
        .map(|w| format!("* breadbox mcu {w}\n"))
        .collect();
    Ok(match text.split_once('\n') {
        Some((title, rest)) => format!("{title}\n{notes}{rest}"),
        None => text,
    })
}

/// The Pro screen's netlist text (design style: `XU1 VCC 0 A0 … ATMEGA328P fw=… sync=1u`).
pub fn pro_display_text(project: &Project, lib: &Library) -> Result<String, SpiceIoError> {
    let mut opts = NetlistOptions::pro(Some(&project.world));
    opts.mcu_params = true;
    let built = netlist::build(project, lib, &opts)?;
    let mut nl = built.netlist;
    merge_directives(&mut nl, &project.spice.directives)?;
    Ok(nl.to_spice_text_with_ext(&built.ext))
}

/// Joins `+` continuation lines and drops comments; keeps line order.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let t = raw.trim_end();
        let tt = t.trim_start();
        if let Some(rest) = tt.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(rest.trim());
            }
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// Imports a SPICE netlist as a Pro project: R/C/L become library resistors/capacitors/
/// inductors, everything else a generic SPICE element; every pin gets a net label with its
/// node name (ground → GND symbols), parts are laid out on a grid. Models, subcircuits,
/// parameters and analyses are kept as the project's directives.
pub fn import_cir(text: &str, name: &str, lib: &Library) -> Result<Project, SpiceIoError> {
    let nl = Netlist::parse(text).map_err(|e| SpiceIoError::Parse(e.to_string()))?;
    let mut project = Project::new(name, Mode::Pro);
    project.description = format!("Imported from SPICE: {}", nl.title);
    // raw element lines (for generic cards) and directive blocks
    let lines = logical_lines(text);
    let mut directives = String::new();
    let mut raw_elements: std::collections::HashMap<String, String> = Default::default();
    let mut depth = 0;
    for (i, l) in lines.iter().enumerate() {
        if i == 0 {
            continue; // title
        }
        let t = l.trim();
        if t.is_empty() || t.starts_with('*') {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        if lower.starts_with(".subckt") {
            depth += 1;
        }
        if depth > 0 || (lower.starts_with('.') && lower != ".end") {
            directives.push_str(t);
            directives.push('\n');
            if lower.starts_with(".ends") {
                depth -= 1;
            }
            continue;
        }
        if let Some(first) = t.split_whitespace().next() {
            raw_elements.insert(first.to_ascii_lowercase(), t.to_string());
        }
    }
    project.spice.directives = directives;

    let mut col = 0;
    let mut row = 0;
    let mut gnd_n = 0;
    for el in &nl.top.elements {
        if matches!(el.kind, ElemKind::Port { .. }) {
            continue;
        }
        let reference = el.name.to_ascii_uppercase();
        let id = project.next_id(&format!("{}_", el.name.to_ascii_lowercase()));
        let num = |e: &Option<bb_spice::expr::Expr>| e.as_ref().and_then(|x| x.as_num());
        let (def_id, props): (&str, Vec<(&str, PropValue)>) = match &el.kind {
            ElemKind::Resistor { value, model: None } if num(value).is_some() => (
                "resistor",
                vec![("resistance", PropValue::Num(num(value).unwrap_or(1e3)))],
            ),
            ElemKind::Capacitor { value, model: None } if num(value).is_some() => (
                "capacitor",
                vec![("capacitance", PropValue::Num(num(value).unwrap_or(1e-6)))],
            ),
            ElemKind::Inductor { value, model: None } if num(value).is_some() => (
                "inductor",
                vec![
                    ("inductance", PropValue::Num(num(value).unwrap_or(1e-3))),
                    ("dcr", PropValue::Num(1e-3)),
                ],
            ),
            _ => {
                let raw = raw_elements
                    .get(&el.name)
                    .cloned()
                    .unwrap_or_else(|| el.name.clone());
                // replace the node tokens with $(1) … $(n)
                let mut toks: Vec<String> = raw.split_whitespace().map(String::from).collect();
                for (k, _) in el.nodes.iter().enumerate() {
                    if k + 1 < toks.len() {
                        toks[k + 1] = format!("$({})", k + 1);
                    }
                }
                (
                    "spice_card",
                    vec![
                        ("card", PropValue::Text(toks.join(" "))),
                        ("nodes", PropValue::Num(el.nodes.len() as f64)),
                    ],
                )
            }
        };
        let Some(def) = lib.get(def_id) else {
            continue;
        };
        let mut inst = PartInstance::new(&id, &reference, def_id);
        for (k, v) in props {
            inst.props.insert(k.into(), v);
        }
        let at = Placement::at(10.0 + col as f64 * 16.0, 10.0 + row as f64 * 12.0);
        inst.schematic = Some(at);
        row += 1;
        if row >= 8 {
            row = 0;
            col += 1;
        }
        // net labels at each pin anchor
        let pin_ids: Vec<String> = if def_id == "spice_card" {
            (1..=el.nodes.len()).map(|i| i.to_string()).collect()
        } else {
            def.pins.iter().map(|p| p.id.clone()).collect()
        };
        for (k, node) in el.nodes.iter().enumerate() {
            let Some(pin) = pin_ids.get(k) else { break };
            let anchor = def
                .schematic
                .pins
                .iter()
                .find(|s| &s.pin == pin)
                .map(|s| at.apply(s.x, s.y));
            let Some((x, y)) = anchor else { continue };
            if node == "0" || node.eq_ignore_ascii_case("gnd") {
                gnd_n += 1;
                let mut g =
                    PartInstance::new(&format!("gnd_{gnd_n}"), &format!("#GND{gnd_n}"), "gnd");
                if let Some(gd) = lib.get("gnd")
                    && let Some(s) = gd.schematic.pins.first()
                {
                    g.schematic = Some(Placement::at(x - s.x, y - s.y));
                }
                project.parts.push(g);
            } else {
                let lid = project.next_id("nl");
                project.schematic.labels.push(NetLabel {
                    id: lid,
                    name: node.to_ascii_uppercase(),
                    x,
                    y,
                    rot: 0.0,
                });
            }
        }
        project.parts.push(inst);
    }
    Ok(project)
}

#[cfg(test)]
mod tests;
