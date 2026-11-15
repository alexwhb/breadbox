//! `.cir` export (SPICE-8, PRJ-5) and import into a Pro project with an auto-placed schematic
//! (PRJ-4); a netlist typed in Pro mapped onto the circuit ([`netlist_to_circuit`], PRO-2).

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
    /// The netlist parses but the circuit cannot hold all of it.
    #[error("{0}")]
    Unrepresentable(String),
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
    if nl.analyses.is_empty() {
        // something to run when the file is opened in ngspice (`-b` runs nothing otherwise)
        nl.analyses
            .push((bb_spice::Analysis::Op, bb_spice::Span::none()));
    }
    // ext() variables frozen at the parts' values (a battery's EMF, a sensor's output), not 0
    let text = nl.to_ngspice_text(&built.ext);
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

/// Imports a SPICE netlist as a Pro project: plain R/C/L (`R1 a b 1k`) become library
/// resistors/capacitors/inductors, V/I sources library sources (their value text kept),
/// everything else a generic SPICE element; every pin gets a
/// net label with its node name (ground → GND symbols), parts are laid out on a grid. Models,
/// subcircuits, parameters and analyses are kept as the project's directives.
pub fn import_cir(text: &str, name: &str, lib: &Library) -> Result<Project, SpiceIoError> {
    let nl = Netlist::parse(text).map_err(|e| SpiceIoError::Parse(e.to_string()))?;
    let mut project = Project::new(name, Mode::Pro);
    project.description = format!("Imported from SPICE: {}", nl.title);
    import_into(text, &nl, &mut project, lib, &|_, _| None);
    Ok(project)
}

/// Is this circuit one a Pro netlist can describe and rebuild (empty, or only parts a netlist
/// import makes: resistors, capacitors, inductors, generic SPICE cards, ground symbols — no
/// boards, sensors or breadboard wiring)?
pub fn is_netlist_circuit(project: &Project) -> bool {
    project.wires.is_empty()
        && project.parts.iter().all(|p| {
            p.bench.is_none()
                && matches!(
                    p.part.as_str(),
                    "resistor"
                        | "capacitor"
                        | "inductor"
                        | "vsource"
                        | "isource"
                        | "spice_card"
                        | "gnd"
                )
        })
}

/// The circuit a netlist typed in Pro describes (`PRO-2`): `old` with its parts, net labels
/// and directives replaced by what [`import_cir`] makes of `text` (everything else — name,
/// world, radios, settings — kept). Parts that are still there (same designator and kind)
/// keep their id and schematic placement, so moving a symbol survives later edits.
///
/// `Parse` when the text does not parse (half typed); `Unrepresentable` when the circuit could
/// not hold all of it (an `.include` the netlister cannot resolve, an element whose nodes
/// would not survive the round trip) — the caller then detaches the netlist.
pub fn netlist_to_circuit(
    text: &str,
    old: &Project,
    lib: &Library,
) -> Result<Project, SpiceIoError> {
    let typed = Netlist::parse(text).map_err(|e| SpiceIoError::Parse(e.to_string()))?;
    let mut p = old.clone();
    p.parts.clear();
    p.wires.clear();
    p.schematic.labels.clear();
    p.schematic.wires.clear();
    p.spice.directives.clear();
    let keep = |reference: &str, def: &str| {
        old.parts
            .iter()
            .find(|q| q.reference.eq_ignore_ascii_case(reference) && q.part == def)
            .and_then(|q| Some((q.id.clone(), q.schematic?)))
    };
    import_into(text, &typed, &mut p, lib, &keep);
    let ids: std::collections::HashSet<&str> = p.parts.iter().map(|x| x.id.as_str()).collect();
    p.probes
        .retain(|pr| pr.target.part().is_none_or(|id| ids.contains(id)));
    check_round_trip(&typed, &p, lib)?;
    Ok(p)
}

/// Every element of `typed` is in the circuit's Pro netlist with the same nodes (a library
/// inductor's DCR resistor is looked through).
fn check_round_trip(typed: &Netlist, p: &Project, lib: &Library) -> Result<(), SpiceIoError> {
    let built = pro_netlist(p, lib).map_err(|e| SpiceIoError::Unrepresentable(e.to_string()))?;
    let norm = |n: &str| {
        let n = n.to_ascii_lowercase();
        if n == "gnd" { "0".to_string() } else { n }
    };
    // `l1 a l1_dcr` + `rl1_dcr l1_dcr b` → `l1 a b`
    let mut alias: std::collections::HashMap<String, String> = Default::default();
    for e in &built.top.elements {
        let name = e.name.to_ascii_lowercase();
        if let Some(l) = name.strip_prefix('r').and_then(|x| x.strip_suffix("_dcr"))
            && e.nodes.len() == 2
            && norm(&e.nodes[0]) == format!("{l}_dcr")
        {
            alias.insert(norm(&e.nodes[0]), norm(&e.nodes[1]));
        }
    }
    for e in &typed.top.elements {
        if matches!(e.kind, ElemKind::Port { .. }) {
            continue;
        }
        let Some(g) = built
            .top
            .elements
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(&e.name))
        else {
            return Err(SpiceIoError::Unrepresentable(format!(
                "{} has no part in the schematic",
                e.name.to_ascii_uppercase()
            )));
        };
        let a: Vec<String> = e.nodes.iter().map(|n| norm(n)).collect();
        let b: Vec<String> = g
            .nodes
            .iter()
            .map(|n| {
                let n = norm(n);
                alias.get(&n).cloned().unwrap_or(n)
            })
            .collect();
        if a != b {
            return Err(SpiceIoError::Unrepresentable(format!(
                "{} would connect to {} instead of {}",
                e.name.to_ascii_uppercase(),
                b.join(" "),
                a.join(" ")
            )));
        }
    }
    Ok(())
}

/// `keep(designator, def)` → an existing part's id and schematic placement to reuse.
type KeepPart<'a> = &'a dyn Fn(&str, &str) -> Option<(String, Placement)>;

/// [`import_cir`] into `project` (parts, labels, directives appended); `keep` reuses parts.
fn import_into(text: &str, nl: &Netlist, project: &mut Project, lib: &Library, keep: KeepPart) {
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
    project.spice.directives.push_str(&directives);

    let mut col = 0;
    let mut row = 0;
    let mut gnd_n = 0;
    for el in &nl.top.elements {
        if matches!(el.kind, ElemKind::Port { .. }) {
            continue;
        }
        let reference = el.name.to_ascii_uppercase();
        let num = |e: &Option<bb_spice::expr::Expr>| e.as_ref().and_then(|x| x.as_num());
        // a library part only for the plain form `R1 a b 1k` (tc1 / IC / m= stay on a card)
        let plain = raw_elements
            .get(&el.name)
            .is_some_and(|l| l.split_whitespace().count() == 4);
        let (def_id, props): (&str, Vec<(&str, PropValue)>) = match &el.kind {
            ElemKind::Resistor { value, model: None } if plain && num(value).is_some() => (
                "resistor",
                vec![("resistance", PropValue::Num(num(value).unwrap_or(1e3)))],
            ),
            ElemKind::Capacitor { value, model: None } if plain && num(value).is_some() => (
                "capacitor",
                vec![("capacitance", PropValue::Num(num(value).unwrap_or(1e-6)))],
            ),
            ElemKind::Inductor { value, model: None } if plain && num(value).is_some() => (
                "inductor",
                vec![
                    ("inductance", PropValue::Num(num(value).unwrap_or(1e-3))),
                    ("dcr", PropValue::Num(1e-3)),
                ],
            ),
            // independent sources: the source symbol, the rest of the line as its value
            // (`DC 0 AC 1`, `PULSE(…)`, `SIN(…)`)
            ElemKind::Vsource(_) | ElemKind::Isource(_) if el.nodes.len() == 2 => {
                let spec: Vec<&str> = raw_elements
                    .get(&el.name)
                    .map(|l| l.split_whitespace().skip(3).collect())
                    .unwrap_or_default();
                let spec = if spec.is_empty() {
                    "DC 0".to_string()
                } else {
                    spec.join(" ")
                };
                let def = if matches!(el.kind, ElemKind::Vsource(_)) {
                    "vsource"
                } else {
                    "isource"
                };
                (def, vec![("spec", PropValue::Text(spec))])
            }
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
        let grid = Placement::at(10.0 + col as f64 * 16.0, 10.0 + row as f64 * 12.0);
        row += 1;
        if row >= 8 {
            row = 0;
            col += 1;
        }
        let (id, at) = match keep(&reference, def_id) {
            Some((id, at)) if project.part(&id).is_none() => (id, at),
            _ => (
                project.next_id(&format!("{}_", el.name.to_ascii_lowercase())),
                grid,
            ),
        };
        let mut inst = PartInstance::new(&id, &reference, def_id);
        for (k, v) in props {
            inst.props.insert(k.into(), v);
        }
        inst.schematic = Some(at);
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
}

#[cfg(test)]
mod tests;
